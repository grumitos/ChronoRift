use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    fmt::{self, Display},
    fs::{self, File},
    io::Write,
    path::PathBuf,
};
use tauri::{AppHandle, Emitter};

const PROGRESS_EVENT: &str = "download-progress";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadArtifactRequest {
    pub url: String,
    pub destination_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub destination_path: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub percent: Option<f64>,
    pub done: bool,
}

#[derive(Debug)]
pub enum DownloadError {
    InvalidUrl(String),
    UnsupportedScheme(String),
    MissingParent(PathBuf),
    CreateDirectory(std::io::Error),
    CreateFile(std::io::Error),
    Request(reqwest::Error),
    Write(std::io::Error),
    Emit(tauri::Error),
}

impl Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUrl(url) => write!(f, "Invalid download URL: {url}"),
            Self::UnsupportedScheme(scheme) => {
                write!(f, "Unsupported download URL scheme: {scheme}")
            }
            Self::MissingParent(path) => {
                write!(
                    f,
                    "Download destination has no parent folder: {}",
                    path.display()
                )
            }
            Self::CreateDirectory(err) => write!(f, "Could not create destination folder: {err}"),
            Self::CreateFile(err) => write!(f, "Could not create destination file: {err}"),
            Self::Request(err) => write!(f, "Download request failed: {err}"),
            Self::Write(err) => write!(f, "Could not write download: {err}"),
            Self::Emit(err) => write!(f, "Could not publish download progress: {err}"),
        }
    }
}

impl Error for DownloadError {}

pub async fn download_artifact(
    app: AppHandle,
    request: DownloadArtifactRequest,
) -> Result<(), DownloadError> {
    let url = parse_url(&request.url)?;
    let destination = PathBuf::from(&request.destination_path);
    let parent = destination
        .parent()
        .ok_or_else(|| DownloadError::MissingParent(destination.clone()))?;

    fs::create_dir_all(parent).map_err(DownloadError::CreateDirectory)?;

    let mut response = reqwest::get(url)
        .await
        .map_err(DownloadError::Request)?
        .error_for_status()
        .map_err(DownloadError::Request)?;
    let total_bytes = response.content_length();
    let mut file = File::create(&destination).map_err(DownloadError::CreateFile)?;
    let mut downloaded_bytes = 0;

    emit_progress(&app, &destination, 0, total_bytes, false)?;

    while let Some(chunk) = response.chunk().await.map_err(DownloadError::Request)? {
        file.write_all(&chunk).map_err(DownloadError::Write)?;
        downloaded_bytes += chunk.len() as u64;
        emit_progress(&app, &destination, downloaded_bytes, total_bytes, false)?;
    }

    file.flush().map_err(DownloadError::Write)?;
    emit_progress(&app, &destination, downloaded_bytes, total_bytes, true)
}

fn parse_url(url: &str) -> Result<Url, DownloadError> {
    let parsed = Url::parse(url).map_err(|_| DownloadError::InvalidUrl(url.to_owned()))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        scheme => Err(DownloadError::UnsupportedScheme(scheme.to_owned())),
    }
}

fn emit_progress(
    app: &AppHandle,
    destination: &PathBuf,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    done: bool,
) -> Result<(), DownloadError> {
    let percent = total_bytes
        .filter(|total| *total > 0)
        .map(|total| downloaded_bytes as f64 / total as f64 * 100.0);

    app.emit(
        PROGRESS_EVENT,
        DownloadProgress {
            destination_path: destination.display().to_string(),
            downloaded_bytes,
            total_bytes,
            percent,
            done,
        },
    )
    .map_err(DownloadError::Emit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_http_urls() {
        assert!(parse_url("https://example.com/file").is_ok());
        assert!(parse_url("http://example.com/file").is_ok());
        assert!(parse_url("file:///tmp/file").is_err());
        assert!(parse_url("not a url").is_err());
    }
}
