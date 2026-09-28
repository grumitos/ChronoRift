use crate::client::ClientInstallResult;
use serde::Serialize;
use std::{
    cmp::Ordering,
    fs,
    path::{Path, PathBuf},
};

const METADATA_DIR: &str = ".chronorift";
const METADATA_FILE: &str = "install.json";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct InstalledClient {
    pub patch: String,
    pub resolved_version: String,
    pub region: String,
    pub locale: String,
    pub destination_path: String,
    pub executable_path: String,
    pub executable_exists: bool,
}

/// Reads `<root>/.chronorift/install.json` and `<root>/*/.chronorift/install.json`.
/// Read-only; unreadable or corrupt entries are skipped, and a missing root yields no clients.
pub fn find_installed_clients(root: impl AsRef<Path>) -> Vec<InstalledClient> {
    let root = root.as_ref();
    if !root.is_dir() {
        return Vec::new();
    }

    let mut candidates = vec![root.to_path_buf()];
    if let Ok(entries) = fs::read_dir(root) {
        candidates.extend(
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .filter(|path| path.file_name().is_some_and(|name| name != METADATA_DIR)),
        );
    }

    let mut clients: Vec<_> = candidates
        .iter()
        .filter_map(|dir| read_install_metadata(dir))
        .collect();
    clients.sort_by(|a, b| {
        compare_versions(&b.patch, &a.patch)
            .then_with(|| compare_versions(&b.resolved_version, &a.resolved_version))
            .then_with(|| a.destination_path.cmp(&b.destination_path))
    });
    clients
}

fn read_install_metadata(dir: &Path) -> Option<InstalledClient> {
    let path: PathBuf = dir.join(METADATA_DIR).join(METADATA_FILE);
    let bytes = fs::read(path).ok()?;
    let result: ClientInstallResult = serde_json::from_slice(&bytes).ok()?;
    let executable_exists = Path::new(&result.executable_path).is_file();

    Some(InstalledClient {
        patch: result.patch,
        resolved_version: result.resolved_version,
        region: result.region,
        locale: result.locale,
        destination_path: result.destination_path,
        executable_path: result.executable_path,
        executable_exists,
    })
}

/// Compares dotted versions numerically segment by segment (14.10 > 14.9).
fn compare_versions(a: &str, b: &str) -> Ordering {
    let mut left = a.split('.');
    let mut right = b.split('.');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (Some(x), Some(y)) => {
                let ordering = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    _ => x.cmp(y),
                };
                if ordering != Ordering::Equal {
                    return ordering;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "chronorift-clients-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_metadata(dir: &Path, patch: &str, with_exe: bool) {
        let exe = dir.join("Game").join("League of Legends.exe");
        if with_exe {
            fs::create_dir_all(exe.parent().unwrap()).unwrap();
            fs::write(&exe, b"").unwrap();
        }
        let result = ClientInstallResult {
            patch: patch.to_string(),
            resolved_version: format!("{patch}.100.1000"),
            region: "LA2".to_string(),
            locale: "es_MX".to_string(),
            destination_path: dir.display().to_string(),
            executable_path: exe.display().to_string(),
            manifest_url: "https://example.invalid/manifest".to_string(),
            file_count: 1,
            download_bytes: 2,
            install_bytes: 3,
        };
        let meta = dir.join(METADATA_DIR);
        fs::create_dir_all(&meta).unwrap();
        fs::write(
            meta.join(METADATA_FILE),
            serde_json::to_vec_pretty(&result).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn discovers_clients_at_root_and_one_level_down() {
        let root = temp_dir("discover");
        write_metadata(&root, "14.2", false);
        write_metadata(&root.join("14.9"), "14.9", true);
        write_metadata(&root.join("14.10"), "14.10", false);
        write_metadata(&root.join("a").join("deep"), "15.1", true);
        let corrupt = root.join("corrupt").join(METADATA_DIR);
        fs::create_dir_all(&corrupt).unwrap();
        fs::write(corrupt.join(METADATA_FILE), b"{ nope").unwrap();
        fs::create_dir_all(root.join("empty")).unwrap();

        let clients = find_installed_clients(&root);
        fs::remove_dir_all(&root).unwrap();

        let patches: Vec<_> = clients.iter().map(|c| c.patch.as_str()).collect();
        assert_eq!(patches, vec!["14.10", "14.9", "14.2"]);
        assert!(!clients[0].executable_exists);
        assert!(clients[1].executable_exists);
        assert_eq!(clients[1].locale, "es_MX");
        assert_eq!(clients[1].resolved_version, "14.9.100.1000");

        let json = serde_json::to_value(&clients[1]).unwrap();
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "destinationPath",
                "executableExists",
                "executablePath",
                "locale",
                "patch",
                "region",
                "resolvedVersion",
            ]
        );
    }

    #[test]
    fn missing_root_yields_no_clients() {
        let root = temp_dir("missing");
        fs::remove_dir_all(&root).unwrap();

        assert!(find_installed_clients(&root).is_empty());
    }

    #[test]
    fn compares_versions_numerically() {
        assert_eq!(compare_versions("14.10", "14.9"), Ordering::Greater);
        assert_eq!(compare_versions("15.1", "14.24"), Ordering::Greater);
        assert_eq!(compare_versions("14.9", "14.9"), Ordering::Equal);
        assert_eq!(compare_versions("14.9.1", "14.9"), Ordering::Greater);
    }
}
