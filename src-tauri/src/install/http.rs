//! HTTP access for the installer: a small [`Fetch`] trait (so resolution is testable with a
//! fake) and the shared reqwest client used for manifests and bundle ranges.

use super::error::InstallError;
use reqwest::{header, StatusCode};
use std::{
    future::Future,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

pub const USER_AGENT: &str = "ChronoRift/0.1";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// A fetched response. Non-2xx responses carry the status and an empty body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Fetched {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Called with `(bytes so far, expected total)` while a body downloads.
pub type OnBytes<'a> = &'a (dyn Fn(u64, Option<u64>) + Sync);

pub trait Fetch: Sync {
    /// GET `url`, reading at most `limit` body bytes (more is an error). Checks `cancel`
    /// between body chunks.
    fn get(
        &self,
        url: &str,
        limit: usize,
        cancel: &AtomicBool,
        on_bytes: OnBytes<'_>,
    ) -> impl Future<Output = Result<Fetched, InstallError>> + Send;
}

pub fn check_cancel(cancel: &AtomicBool) -> Result<(), InstallError> {
    if cancel.load(Ordering::SeqCst) {
        Err(InstallError::Cancelled)
    } else {
        Ok(())
    }
}

/// Shared client (Tauri managed state) with timeouts and a ChronoRift user agent.
#[derive(Clone)]
pub struct HttpClient {
    client: reqwest::Client,
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client }
    }

    /// Fetches `[start, end)` of `url`. Requires a 206 (or a 200 that is exactly the range).
    pub async fn get_range(
        &self,
        url: &str,
        start: u64,
        end: u64,
        cancel: &AtomicBool,
        on_delta: &(dyn Fn(u64) + Sync),
    ) -> Result<Vec<u8>, RangeError> {
        let span = end
            .checked_sub(start)
            .filter(|span| *span > 0)
            .ok_or_else(|| RangeError::fatal(InstallError::Manifest("empty range".into())))?;
        let span_usize = usize::try_from(span)
            .map_err(|_| RangeError::fatal(InstallError::Manifest("range too large".into())))?;
        let last = end - 1;
        let mut response = self
            .client
            .get(url)
            .header(header::RANGE, format!("bytes={start}-{last}"))
            .send()
            .await
            .map_err(|err| RangeError::retry(network(url, err)))?;
        let status = response.status();
        let whole_body_ok = status == StatusCode::OK && start == 0;
        if status != StatusCode::PARTIAL_CONTENT && !whole_body_ok {
            let err = InstallError::Network(format!("{url} returned HTTP {}", status.as_u16()));
            // Riot's CDN sporadically answers 403 for bundles that exist; retry those too.
            let retryable = status == StatusCode::FORBIDDEN
                || status == StatusCode::TOO_MANY_REQUESTS
                || status.is_server_error();
            return Err(RangeError {
                retryable,
                error: err,
            });
        }
        if response.content_length().is_some_and(|len| len != span) {
            return Err(RangeError::retry(InstallError::Network(format!(
                "{url} returned {} bytes for a {span} byte range",
                response.content_length().unwrap_or(0)
            ))));
        }
        let mut body = Vec::with_capacity(span_usize);
        loop {
            check_cancel(cancel).map_err(RangeError::fatal)?;
            let Some(chunk) = response
                .chunk()
                .await
                .map_err(|err| RangeError::retry(network(url, err)))?
            else {
                break;
            };
            if body.len() + chunk.len() > span_usize {
                return Err(RangeError::retry(InstallError::Network(format!(
                    "{url} returned more than the requested {span} bytes"
                ))));
            }
            body.extend_from_slice(&chunk);
            on_delta(chunk.len() as u64);
        }
        if body.len() != span_usize {
            return Err(RangeError::retry(InstallError::Network(format!(
                "{url} returned {} of {span} bytes",
                body.len()
            ))));
        }
        Ok(body)
    }
}

impl Fetch for HttpClient {
    async fn get(
        &self,
        url: &str,
        limit: usize,
        cancel: &AtomicBool,
        on_bytes: OnBytes<'_>,
    ) -> Result<Fetched, InstallError> {
        check_cancel(cancel)?;
        let mut response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|err| network(url, err))?;
        let status = response.status().as_u16();
        if !response.status().is_success() {
            return Ok(Fetched {
                status,
                body: Vec::new(),
            });
        }
        let total = response.content_length();
        if total.is_some_and(|total| total > limit as u64) {
            return Err(too_large(url, limit));
        }
        let mut body = Vec::new();
        loop {
            check_cancel(cancel)?;
            let Some(chunk) = response.chunk().await.map_err(|err| network(url, err))? else {
                break;
            };
            if body.len() + chunk.len() > limit {
                return Err(too_large(url, limit));
            }
            body.extend_from_slice(&chunk);
            on_bytes(body.len() as u64, total);
        }
        Ok(Fetched { status, body })
    }
}

/// A failed range request, and whether trying again may help.
#[derive(Debug)]
pub struct RangeError {
    pub retryable: bool,
    pub error: InstallError,
}

impl RangeError {
    fn retry(error: InstallError) -> Self {
        Self {
            retryable: true,
            error,
        }
    }

    fn fatal(error: InstallError) -> Self {
        Self {
            retryable: false,
            error,
        }
    }
}

fn network(url: &str, err: reqwest::Error) -> InstallError {
    InstallError::Network(format!("{url}: {err}"))
}

fn too_large(url: &str, limit: usize) -> InstallError {
    InstallError::Network(format!("{url} is larger than {limit} bytes"))
}
