//! Repair classification: compares planned files with what is on disk. Read-only.

use super::{error::InstallError, manifest::PlannedFile, store::safe_join};
use std::{fs, io, path::Path};

/// Paths listed per category in a repair plan.
pub const PATH_LIST_CAP: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileCondition {
    Ok,
    Missing,
    WrongSize,
}

/// Classifies each file under `game_path`. Anything present that is not a regular file of
/// exactly the manifest size is `WrongSize`.
pub fn check_files(
    game_path: &Path,
    files: &[PlannedFile],
) -> Result<Vec<FileCondition>, InstallError> {
    files
        .iter()
        .map(|file| {
            let target = safe_join(game_path, &file.path)?;
            Ok(match fs::metadata(&target) {
                Ok(meta) if meta.is_file() && meta.len() == file.size => FileCondition::Ok,
                Ok(_) => FileCondition::WrongSize,
                Err(err) if err.kind() == io::ErrorKind::NotFound => FileCondition::Missing,
                Err(_) => FileCondition::WrongSize,
            })
        })
        .collect()
}

#[derive(Debug, Default)]
pub struct RepairSelection<'a> {
    pub ok: usize,
    pub missing: Vec<&'a PlannedFile>,
    pub wrong_size: Vec<&'a PlannedFile>,
}

impl<'a> RepairSelection<'a> {
    pub fn new(files: &'a [PlannedFile], conditions: &[FileCondition]) -> Self {
        let mut selection = Self::default();
        for (file, condition) in files.iter().zip(conditions) {
            match condition {
                FileCondition::Ok => selection.ok += 1,
                FileCondition::Missing => selection.missing.push(file),
                FileCondition::WrongSize => selection.wrong_size.push(file),
            }
        }
        selection
    }

    /// Files to fetch: missing ones, plus wrong-size ones only when replacement is allowed.
    pub fn to_fetch(&self, replace_wrong_size: bool) -> Vec<PlannedFile> {
        let mut files = self.missing.clone();
        if replace_wrong_size {
            files.extend(self.wrong_size.iter().copied());
        }
        files.into_iter().cloned().collect()
    }
}

pub fn capped_paths(files: &[&PlannedFile]) -> Vec<String> {
    files
        .iter()
        .take(PATH_LIST_CAP)
        .map(|file| file.path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::store::tests::temp_root;

    fn file(path: &str, size: u64) -> PlannedFile {
        PlannedFile {
            path: path.into(),
            size,
            chunks: Vec::new(),
        }
    }

    #[test]
    fn classifies_ok_missing_and_wrong_size() {
        let root = temp_root("repair-classify");
        fs::create_dir_all(root.join("DATA")).unwrap();
        fs::write(root.join("League of Legends.exe"), b"abcd").unwrap();
        fs::write(root.join("DATA").join("a.wad"), b"ab").unwrap();
        fs::create_dir_all(root.join("dir.wad")).unwrap();

        let files = [
            file("League of Legends.exe", 4),
            file("DATA/a.wad", 5),
            file("DATA/b.wad", 1),
            file("dir.wad", 0),
        ];
        let conditions = check_files(&root, &files).unwrap();
        let unsafe_path = check_files(&root, &[file("../escape", 1)]);
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(
            conditions,
            vec![
                FileCondition::Ok,
                FileCondition::WrongSize,
                FileCondition::Missing,
                FileCondition::WrongSize,
            ]
        );
        assert_eq!(unsafe_path.unwrap_err().code(), "unsafePath");
    }

    #[test]
    fn wrong_size_files_are_fetched_only_when_allowed() {
        let files = vec![file("a", 1), file("b", 2), file("c", 3)];
        let conditions = [
            FileCondition::Ok,
            FileCondition::Missing,
            FileCondition::WrongSize,
        ];
        let selection = RepairSelection::new(&files, &conditions);
        let names = |files: Vec<PlannedFile>| files.into_iter().map(|f| f.path).collect::<Vec<_>>();

        assert_eq!(selection.ok, 1);
        assert_eq!(names(selection.to_fetch(false)), vec!["b"]);
        assert_eq!(names(selection.to_fetch(true)), vec!["b", "c"]);
        assert_eq!(capped_paths(&selection.wrong_size), vec!["c"]);
    }
}
