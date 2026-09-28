//! Progress reporting. The engine talks to [`Progress`]; the Tauri impl throttles events.

use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter};

pub const PROGRESS_EVENT: &str = "download-progress";
const MIN_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Resolve,
    Manifest,
    Plan,
    Download,
    Repair,
    Complete,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    pub destination_path: String,
    pub stage: Stage,
    pub detail: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub percent: Option<f64>,
    pub files_done: u64,
    pub files_total: u64,
    pub done: bool,
}

/// One progress update, without the destination (the reporter knows it).
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub stage: Stage,
    pub detail: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub files_done: u64,
    pub files_total: u64,
    pub done: bool,
}

impl Update {
    pub fn stage(stage: Stage, detail: impl Into<String>) -> Self {
        Self {
            stage,
            detail: detail.into(),
            downloaded_bytes: 0,
            total_bytes: None,
            files_done: 0,
            files_total: 0,
            done: false,
        }
    }

    pub fn bytes(mut self, downloaded: u64, total: Option<u64>) -> Self {
        self.downloaded_bytes = downloaded;
        self.total_bytes = total;
        self
    }

    pub fn files(mut self, done: u64, total: u64) -> Self {
        self.files_done = done;
        self.files_total = total;
        self
    }

    pub fn finished(mut self) -> Self {
        self.done = true;
        self
    }
}

pub trait Progress: Send + Sync {
    fn report(&self, update: Update);
}

/// Emits `download-progress` at most ~10 times per second. Stage changes and final
/// (`done`) events are always emitted.
pub struct TauriProgress {
    app: AppHandle,
    destination: PathBuf,
    throttle: Throttle,
}

impl TauriProgress {
    pub fn new(app: AppHandle, destination: &Path) -> Self {
        Self {
            app,
            destination: destination.to_path_buf(),
            throttle: Throttle::default(),
        }
    }
}

impl Progress for TauriProgress {
    fn report(&self, update: Update) {
        if !self.throttle.allow(&update, Instant::now()) {
            return;
        }
        let percent = update
            .total_bytes
            .filter(|total| *total > 0)
            .map(|total| (update.downloaded_bytes as f64 / total as f64 * 100.0).min(100.0));
        // Progress is best effort; a closed window must not fail the install.
        let _ = self.app.emit(
            PROGRESS_EVENT,
            ProgressEvent {
                destination_path: self.destination.display().to_string(),
                stage: update.stage,
                detail: update.detail,
                downloaded_bytes: update.downloaded_bytes,
                total_bytes: update.total_bytes,
                percent,
                files_done: update.files_done,
                files_total: update.files_total,
                done: update.done,
            },
        );
    }
}

#[derive(Default)]
struct Throttle(Mutex<Option<(Stage, Instant)>>);

impl Throttle {
    fn allow(&self, update: &Update, now: Instant) -> bool {
        let Ok(mut last) = self.0.lock() else {
            return true;
        };
        let allowed = update.done
            || match *last {
                Some((stage, at)) => {
                    stage != update.stage || now.duration_since(at) >= MIN_INTERVAL
                }
                None => true,
            };
        if allowed {
            *last = Some((update.stage, now));
        }
        allowed
    }
}

/// Discards updates.
#[cfg(test)]
pub struct NoProgress;

#[cfg(test)]
impl Progress for NoProgress {
    fn report(&self, _update: Update) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttles_same_stage_but_passes_stage_changes_and_final_events() {
        let throttle = Throttle::default();
        let start = Instant::now();
        let download = Update::stage(Stage::Download, "a");
        assert!(throttle.allow(&download, start));
        assert!(!throttle.allow(&download, start + Duration::from_millis(10)));
        assert!(throttle.allow(&download, start + Duration::from_millis(120)));
        assert!(throttle.allow(
            &Update::stage(Stage::Complete, "done"),
            start + Duration::from_millis(121)
        ));
        assert!(throttle.allow(
            &Update::stage(Stage::Complete, "done").finished(),
            start + Duration::from_millis(122)
        ));
    }

    #[test]
    fn event_is_camel_case_with_stage_names() {
        let json = serde_json::to_value(ProgressEvent {
            destination_path: "D:/c".into(),
            stage: Stage::Cancelled,
            detail: String::new(),
            downloaded_bytes: 1,
            total_bytes: Some(2),
            percent: Some(50.0),
            files_done: 0,
            files_total: 3,
            done: true,
        })
        .unwrap();
        assert_eq!(json["stage"], "cancelled");
        assert_eq!(json["filesTotal"], 3);
        assert_eq!(json["destinationPath"], "D:/c");
    }
}
