//! Finds ChronoRift-installed clients under a clients root. Read-only.

use super::store::{self, METADATA_DIR};
use serde::Serialize;
use std::{cmp::Ordering, fs, path::Path};

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
    /// True when the executable is not `<destinationPath>/Game/League of Legends.exe`
    /// (v1 installs put the files directly in the destination).
    pub legacy_layout: bool,
}

/// Reads `<root>/.chronorift/install.json` and `<root>/*/.chronorift/install.json` (v1 or
/// v2). Unreadable or corrupt entries are skipped, and a missing root yields no clients.
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
        .filter_map(|dir| store::read_record(dir).ok().flatten())
        .map(|stored| InstalledClient {
            executable_exists: stored.executable_path.is_file(),
            legacy_layout: !in_game_folder(&stored.root, &stored.executable_path),
            patch: stored.record.release.patch,
            resolved_version: stored.record.release.resolved_version,
            region: stored.record.release.region,
            locale: stored.record.release.locale,
            destination_path: stored.root.display().to_string(),
            executable_path: stored.executable_path.display().to_string(),
        })
        .collect();
    clients.sort_by(|a, b| {
        compare_versions(&b.patch, &a.patch)
            .then_with(|| compare_versions(&b.resolved_version, &a.resolved_version))
            .then_with(|| a.destination_path.cmp(&b.destination_path))
    });
    clients
}

/// `executable` sits directly in a `Game` folder (any case) that is directly in `root`.
fn in_game_folder(root: &Path, executable: &Path) -> bool {
    let Some(folder) = executable.parent() else {
        return false;
    };
    let named_game = folder
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(store::GAME_DIR));
    named_game && folder.parent() == Some(root)
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
    use crate::install::store::{
        tests::{release, temp_root},
        write_record, InstallRecord, GAME_DIR, GAME_EXE, RECORD_FILE, STORE_VERSION,
    };
    use std::path::PathBuf;

    fn write_v2(dir: &Path, patch: &str, with_exe: bool) -> PathBuf {
        let exe = dir.join(GAME_DIR).join(GAME_EXE);
        if with_exe {
            fs::create_dir_all(exe.parent().unwrap()).unwrap();
            fs::write(&exe, b"").unwrap();
        }
        let mut release = release();
        release.patch = patch.into();
        release.resolved_version = format!("{patch}.1000");
        write_record(
            dir,
            &InstallRecord {
                version: STORE_VERSION,
                release,
                game_dir: GAME_DIR.into(),
                file_count: 1,
                download_bytes: 2,
                install_bytes: 3,
                installed_at_ms: 4,
            },
        )
        .unwrap();
        exe
    }

    fn write_v1(dir: &Path, patch: &str) {
        let meta = dir.join(METADATA_DIR);
        fs::create_dir_all(&meta).unwrap();
        fs::write(
            meta.join(RECORD_FILE),
            serde_json::json!({
                "patch": patch, "resolvedVersion": format!("{patch}.100.1000"),
                "region": "LA2", "locale": "es_MX",
                "destinationPath": dir.display().to_string(),
                "executablePath": dir.join(GAME_EXE).display().to_string(),
                "manifestUrl": "https://example.invalid/releases/A.manifest",
                "fileCount": 1, "downloadBytes": 2, "installBytes": 3
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn discovers_v1_and_v2_clients_at_root_and_one_level_down() {
        let root = temp_root("clients");
        write_v1(&root, "14.2");
        let exe = write_v2(&root.join("14.9"), "14.9", true);
        write_v2(&root.join("14.10"), "14.10", false);
        write_v2(&root.join("a").join("deep"), "15.1", true);
        let corrupt = root.join("corrupt").join(METADATA_DIR);
        fs::create_dir_all(&corrupt).unwrap();
        fs::write(corrupt.join(RECORD_FILE), b"{ nope").unwrap();
        fs::create_dir_all(root.join("empty")).unwrap();

        let clients = find_installed_clients(&root);
        fs::remove_dir_all(&root).unwrap();

        let patches: Vec<_> = clients.iter().map(|c| c.patch.as_str()).collect();
        assert_eq!(patches, vec!["14.10", "14.9", "14.2"]);
        assert!(!clients[0].executable_exists);
        assert!(clients[1].executable_exists);
        assert_eq!(clients[1].executable_path, exe.display().to_string());
        assert_eq!(
            clients[1].destination_path,
            root.join("14.9").display().to_string()
        );
        assert_eq!(clients[2].resolved_version, "14.2.100.1000");
        assert!(!clients[0].legacy_layout);
        assert!(!clients[1].legacy_layout);
        assert!(
            clients[2].legacy_layout,
            "v1 files sit directly in the root"
        );

        let json = serde_json::to_value(&clients[1]).unwrap();
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "destinationPath",
                "executableExists",
                "executablePath",
                "legacyLayout",
                "locale",
                "patch",
                "region",
                "resolvedVersion",
            ]
        );
    }

    #[test]
    fn missing_root_yields_no_clients() {
        let root = temp_root("clients-missing");
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
