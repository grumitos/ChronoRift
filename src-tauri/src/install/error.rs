//! Install errors and their IPC shape (`{ code, message, path? }`).

use serde::Serialize;
use std::{
    fmt::{self, Display},
    io,
    path::{Path, PathBuf},
};

#[derive(Debug)]
pub enum InstallError {
    /// No server chosen, or not one of [`super::resolve::REGIONS`].
    InvalidRegion(Option<String>),
    InvalidPatch(String),
    /// Not shaped like a Riot locale (`xx_YY`).
    InvalidLocale(String),
    /// Well-formed, but the manifest does not ship it.
    LocaleUnavailable {
        requested: String,
        available: Vec<String>,
    },
    InvalidDestination,
    ReleaseNotFound {
        version: String,
        notes: Vec<String>,
    },
    Network(String),
    Manifest(String),
    Io {
        source: io::Error,
        path: Option<PathBuf>,
    },
    UnsafePath(String),
    WouldOverwrite(PathBuf),
    NotChronoRiftInstall(PathBuf),
    LocaleMismatch {
        installed: String,
        requested: String,
    },
    MissingExecutable(PathBuf),
    UnknownPlan,
    Cancelled,
    Busy,
}

impl InstallError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidRegion(_) => "invalidRegion",
            Self::InvalidPatch(_) => "invalidPatch",
            Self::InvalidLocale(_) => "invalidLocale",
            Self::LocaleUnavailable { .. } => "localeUnavailable",
            Self::InvalidDestination => "invalidDestination",
            Self::ReleaseNotFound { .. } => "releaseNotFound",
            Self::Network(_) => "network",
            Self::Manifest(_) => "manifest",
            Self::Io { .. } => "io",
            Self::UnsafePath(_) => "unsafePath",
            Self::WouldOverwrite(_) => "wouldOverwrite",
            Self::NotChronoRiftInstall(_) => "notChronoRiftInstall",
            Self::LocaleMismatch { .. } => "localeMismatch",
            Self::MissingExecutable(_) => "missingExecutable",
            Self::UnknownPlan => "unknownPlan",
            Self::Cancelled => "cancelled",
            Self::Busy => "busy",
        }
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Io { path, .. } => path.as_deref(),
            Self::WouldOverwrite(path)
            | Self::NotChronoRiftInstall(path)
            | Self::MissingExecutable(path) => Some(path),
            _ => None,
        }
    }

    /// `map_err` helper that keeps the path an I/O error happened at.
    pub fn io_at(path: &Path) -> impl FnOnce(io::Error) -> Self + '_ {
        move |source| Self::Io {
            source,
            path: Some(path.to_path_buf()),
        }
    }
}

impl Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRegion(None) => write!(f, "Choose a server before installing a client"),
            Self::InvalidRegion(Some(region)) => {
                write!(
                    f,
                    "Unknown server \"{region}\". Choose a server from the list"
                )
            }
            Self::InvalidPatch(patch) => write!(f, "Invalid replay patch or version: {patch}"),
            Self::InvalidLocale(locale) => write!(f, "Invalid Riot locale: {locale}"),
            Self::LocaleUnavailable {
                requested,
                available,
            } => write!(
                f,
                "Locale {requested} is not offered by this client. Available: {}",
                available.join(", ")
            ),
            Self::InvalidDestination => write!(f, "Choose a client install destination first"),
            Self::ReleaseNotFound { version, notes } if notes.is_empty() => {
                write!(f, "No downloadable League client found for {version}")
            }
            Self::ReleaseNotFound { version, notes } => write!(
                f,
                "No downloadable League client found for {version} ({})",
                notes.join("; ")
            ),
            Self::Network(message) => write!(f, "Network error: {message}"),
            Self::Manifest(message) => write!(f, "Invalid Riot manifest: {message}"),
            Self::Io {
                source,
                path: Some(path),
            } => write!(f, "{}: {source}", path.display()),
            Self::Io { source, path: None } => write!(f, "Client install failed: {source}"),
            Self::UnsafePath(path) => write!(f, "Manifest contains an unsafe file path: {path}"),
            Self::WouldOverwrite(path) => write!(
                f,
                "{} already exists. Review repair to resume or fix this install.",
                path.display()
            ),
            Self::NotChronoRiftInstall(path) => write!(
                f,
                "Not a ChronoRift install: {} has no ChronoRift install state",
                path.display()
            ),
            Self::LocaleMismatch {
                installed,
                requested,
            } => write!(
                f,
                "This install is {installed}; repair cannot switch it to {requested}"
            ),
            Self::MissingExecutable(path) => write!(
                f,
                "Install finished but the League executable was not found at {}",
                path.display()
            ),
            Self::UnknownPlan => write!(f, "This install plan expired. Review the plan again"),
            Self::Cancelled => write!(f, "Install cancelled"),
            Self::Busy => write!(f, "Another install is running"),
        }
    }
}

impl std::error::Error for InstallError {}

impl From<io::Error> for InstallError {
    fn from(source: io::Error) -> Self {
        Self::Io { source, path: None }
    }
}

/// What install/repair/plan commands return to the frontend on failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Set for `localeUnavailable`: the locales the manifest offers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub available_locales: Option<Vec<String>>,
}

impl From<InstallError> for CommandError {
    fn from(err: InstallError) -> Self {
        Self {
            code: err.code().to_string(),
            message: err.to_string(),
            path: err.path().map(|path| path.display().to_string()),
            available_locales: match &err {
                InstallError::LocaleUnavailable { available, .. } => Some(available.clone()),
                _ => None,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_code_message_and_optional_path() {
        let err: CommandError = InstallError::WouldOverwrite(PathBuf::from("D:/x/a.wad")).into();
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["code"], "wouldOverwrite");
        assert_eq!(json["path"], "D:/x/a.wad");
        assert!(json["message"].as_str().unwrap().contains("Review repair"));

        let json =
            serde_json::to_value(CommandError::from(InstallError::InvalidRegion(None))).unwrap();
        assert_eq!(json["code"], "invalidRegion");
        assert!(json.get("path").is_none());
        assert!(json["message"]
            .as_str()
            .unwrap()
            .contains("Choose a server"));
        assert!(json.get("availableLocales").is_none());

        let json = serde_json::to_value(CommandError::from(InstallError::LocaleUnavailable {
            requested: "de_DE".into(),
            available: vec!["es_AR".into(), "es_MX".into()],
        }))
        .unwrap();
        assert_eq!(json["code"], "localeUnavailable");
        assert_eq!(
            json["availableLocales"],
            serde_json::json!(["es_AR", "es_MX"])
        );
        assert!(json["message"].as_str().unwrap().contains("es_AR, es_MX"));
    }
}
