//! RMAN manifests: a panic-free wrapper around `cdragon-rman` plus locale file selection.
//!
//! `cdragon-rman` 0.2 indexes and unwraps freely while reading tables, so everything is read
//! once inside `catch_unwind` into owned data; lookups afterwards are checked. RMAN 2.1 has
//! the same table layout as 2.0 (the crate only accepts 2.0), so the version byte is patched.
//!
//! File flags are a bitmask where flag id `n` is bit `n - 1` (the crate's `FileFlagSet::iter`
//! uses bit `n`, which is off by one).

use super::error::InstallError;
use crate::locale::normalize_locale;
use cdragon_rman::{FileFlagEntry, FileFlagSet, Rman};
use std::{
    collections::HashMap,
    io::Read,
    panic::{self, AssertUnwindSafe},
};

const HEADER_LEN: usize = 28;
/// Decompressed body cap; real game-client bodies are well under 100 MB.
const MAX_BODY: u32 = 512 << 20;
const WINDOWS_FLAG: &str = "windows";
const PLATFORM_MARKERS: [&str; 2] = ["windows", "macos"];

type FileFilter = Box<dyn Fn(&ManifestFile) -> bool>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkRef {
    pub id: u64,
    pub bundle_id: u64,
    pub bundle_offset: u64,
    pub bundle_size: u32,
    pub target_size: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChunkLocation {
    bundle_id: u64,
    bundle_offset: u64,
    bundle_size: u32,
    target_size: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flag {
    pub id: u8,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestFile {
    /// `/`-separated path relative to the game folder.
    pub path: String,
    pub size: u64,
    /// Raw flag bitmask (bit `id - 1` set for each flag), `None` when the file has none.
    pub flag_mask: Option<u64>,
    pub chunk_ids: Vec<u64>,
    pub is_link: bool,
}

#[derive(Debug, Clone)]
pub struct Manifest {
    pub flags: Vec<Flag>,
    pub files: Vec<ManifestFile>,
    chunks: HashMap<u64, ChunkLocation>,
}

/// A file to install, with its chunks in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    pub path: String,
    pub size: u64,
    pub chunks: Vec<ChunkRef>,
}

fn manifest_error(message: impl Into<String>) -> InstallError {
    InstallError::Manifest(message.into())
}

/// The bit a flag id occupies in a file's flag mask.
pub fn flag_bit(id: u8) -> u64 {
    match id {
        1..=64 => 1u64 << (id - 1),
        _ => 0,
    }
}

/// Reads the private mask of a crate `FileFlagSet` by probing each bit through its public
/// iterator (which tests bit `entry.id`).
fn decode_mask(set: &FileFlagSet) -> u64 {
    let probes = (0u8..64)
        .map(|bit| FileFlagEntry { id: bit, flag: "" })
        .collect::<Vec<_>>();
    probes.iter().fold(0, |mask, probe| {
        if set.iter(std::iter::once(probe)).next().is_some() {
            mask | (1u64 << probe.id)
        } else {
            mask
        }
    })
}

struct RawDir {
    id: u64,
    parent: Option<u64>,
    name: String,
}

struct RawFile {
    name: String,
    directory: Option<u64>,
    size: u32,
    mask: Option<u64>,
    chunk_ids: Vec<u64>,
    is_link: bool,
}

struct RawTables {
    flags: Vec<Flag>,
    dirs: Vec<RawDir>,
    chunks: HashMap<u64, ChunkLocation>,
    files: Vec<RawFile>,
}

pub fn parse(bytes: &[u8]) -> Result<Manifest, InstallError> {
    if bytes.len() < HEADER_LEN || &bytes[..4] != b"RMAN" {
        return Err(manifest_error("not an RMAN file"));
    }
    let version = (bytes[4], bytes[5]);
    if !matches!(version, (2, 0) | (2, 1)) {
        return Err(manifest_error(format!(
            "unsupported RMAN version {}.{}",
            version.0, version.1
        )));
    }
    let u32_at =
        |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    let (offset, compressed_len, body_len) = (u32_at(8), u32_at(12), u32_at(24));
    let body_end = u64::from(offset) + u64::from(compressed_len);
    if (offset as usize) < HEADER_LEN || body_end > bytes.len() as u64 {
        return Err(manifest_error("truncated RMAN body"));
    }
    if body_len > MAX_BODY {
        return Err(manifest_error("RMAN body is too large"));
    }

    // 2.1 → 2.0 for the crate's version check; the layout is unchanged.
    let header = [b'R', b'M', b'A', b'N', 2, 0];
    let reader = header.as_slice().chain(&bytes[6..]);
    let tables = panic::catch_unwind(AssertUnwindSafe(|| read_tables(reader)))
        .map_err(|_| manifest_error("malformed RMAN tables"))??;
    build_manifest(tables)
}

/// Everything that touches the crate's panicking readers.
fn read_tables(reader: impl Read) -> Result<RawTables, InstallError> {
    let rman = Rman::read(reader).map_err(|err| manifest_error(err.to_string()))?;
    let flags = rman
        .iter_flags()
        .map(|flag| Flag {
            id: flag.id,
            name: flag.flag.to_string(),
        })
        .collect();
    let dirs = rman
        .iter_directories()
        .map(|dir| RawDir {
            id: dir.id,
            parent: dir.parent_id,
            name: dir.name.to_string(),
        })
        .collect();

    let mut chunks = HashMap::new();
    for bundle in rman.iter_bundles() {
        // The crate's running offset is u32 and can overflow; recompute it as u64.
        let mut offset = 0u64;
        for chunk in bundle.iter_chunks() {
            chunks.entry(chunk.id).or_insert(ChunkLocation {
                bundle_id: bundle.id,
                bundle_offset: offset,
                bundle_size: chunk.bundle_size,
                target_size: chunk.target_size,
            });
            offset = offset
                .checked_add(u64::from(chunk.bundle_size))
                .ok_or_else(|| manifest_error("bundle offset overflow"))?;
        }
    }

    let files = rman
        .iter_files()
        .map(|file| RawFile {
            name: file.name.to_string(),
            directory: file.directory_id,
            size: file.filesize,
            mask: file.flags.as_ref().map(decode_mask),
            chunk_ids: file.iter_chunks().collect(),
            is_link: file.link.is_some(),
        })
        .collect();
    Ok(RawTables {
        flags,
        dirs,
        chunks,
        files,
    })
}

fn build_manifest(tables: RawTables) -> Result<Manifest, InstallError> {
    let dirs = tables
        .dirs
        .iter()
        .map(|dir| (dir.id, dir))
        .collect::<HashMap<_, _>>();
    let dir_path = |id: u64| -> Result<Vec<&str>, InstallError> {
        let mut parts = Vec::new();
        let mut next = Some(id);
        while let Some(id) = next {
            if parts.len() > dirs.len() {
                return Err(manifest_error("directory cycle"));
            }
            let dir = dirs
                .get(&id)
                .ok_or_else(|| manifest_error(format!("unknown directory id {id:016X}")))?;
            parts.push(dir.name.as_str());
            next = dir.parent.filter(|parent| *parent != id);
        }
        parts.reverse();
        Ok(parts)
    };

    let files = tables
        .files
        .into_iter()
        .map(|file| {
            let mut parts = match file.directory {
                Some(id) => dir_path(id)?,
                None => Vec::new(),
            };
            parts.push(&file.name);
            let path = parts
                .into_iter()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("/");
            Ok(ManifestFile {
                path,
                size: u64::from(file.size),
                flag_mask: file.mask,
                chunk_ids: file.chunk_ids,
                is_link: file.is_link,
            })
        })
        .collect::<Result<Vec<_>, InstallError>>()?;

    Ok(Manifest {
        flags: tables.flags,
        files,
        chunks: tables.chunks,
    })
}

impl Manifest {
    /// Locale flags present in the manifest, sorted, e.g. `["ar_AE", …, "zh_TW"]`.
    pub fn locales(&self) -> Vec<String> {
        let mut locales = self
            .flags
            .iter()
            .filter_map(|flag| normalize_locale(&flag.name).filter(|locale| *locale == flag.name))
            .collect::<Vec<_>>();
        locales.sort();
        locales.dedup();
        locales
    }

    fn uses_flags(&self) -> bool {
        !self.flags.is_empty()
            && self
                .files
                .iter()
                .any(|file| file.flag_mask.unwrap_or(0) != 0)
    }

    /// Chunk locations for one file, checked against the file size.
    fn planned(&self, file: &ManifestFile) -> Result<PlannedFile, InstallError> {
        let mut total = 0u64;
        let chunks = file
            .chunk_ids
            .iter()
            .map(|id| {
                let location = self.chunks.get(id).ok_or_else(|| {
                    manifest_error(format!("{} uses unknown chunk {id:016X}", file.path))
                })?;
                total += u64::from(location.target_size);
                Ok(ChunkRef {
                    id: *id,
                    bundle_id: location.bundle_id,
                    bundle_offset: location.bundle_offset,
                    bundle_size: location.bundle_size,
                    target_size: location.target_size,
                })
            })
            .collect::<Result<Vec<_>, InstallError>>()?;
        if total != file.size {
            return Err(manifest_error(format!(
                "{} chunks add up to {total} bytes, expected {}",
                file.path, file.size
            )));
        }
        Ok(PlannedFile {
            path: file.path.clone(),
            size: file.size,
            chunks,
        })
    }

    /// Neutral + Windows + `locale` files. Uses file flags; the path heuristic is only a
    /// fallback for manifests without flags. A locale the manifest does not offer is an
    /// error, never substituted.
    pub fn select(&self, locale: &str) -> Result<Selection, InstallError> {
        let requested = normalize_locale(locale)
            .ok_or_else(|| InstallError::InvalidLocale(locale.to_string()))?;
        let locales = self.locales();
        if !locales.is_empty() && !locales.contains(&requested) {
            return Err(InstallError::LocaleUnavailable {
                requested,
                available: locales,
            });
        }

        let (keep, localized): (FileFilter, FileFilter) = if self.uses_flags() {
            let mut locale_bits = 0;
            let mut platform_bits = 0;
            for flag in &self.flags {
                if locales.contains(&flag.name) {
                    locale_bits |= flag_bit(flag.id);
                } else {
                    platform_bits |= flag_bit(flag.id);
                }
            }
            let bit_named = |name: &str| {
                self.flags
                    .iter()
                    .filter(|flag| flag.name == name)
                    .fold(0, |bits, flag| bits | flag_bit(flag.id))
            };
            let wanted = bit_named(&requested);
            let windows = bit_named(WINDOWS_FLAG);
            (
                Box::new(move |file| {
                    let mask = file.flag_mask.unwrap_or(0);
                    (mask & locale_bits == 0 || mask & wanted != 0)
                        && (mask & platform_bits == 0 || mask & windows != 0)
                }),
                Box::new(move |file| file.flag_mask.unwrap_or(0) & locale_bits != 0),
            )
        } else {
            let allowed = allowed_markers(&requested);
            let known = locales.clone();
            let all = locales.clone();
            (
                Box::new(move |file| path_matches_locale(&file.path, &allowed, &known)),
                Box::new(move |file| all.iter().any(|locale| has_marker(&file.path, locale))),
            )
        };

        let selected = self
            .files
            .iter()
            .filter(|file| !file.is_link && keep(file))
            .collect::<Vec<_>>();
        let locale_assets = asset_locale(
            &requested,
            &locales,
            selected
                .iter()
                .filter(|file| localized(file))
                .map(|file| file.path.as_str()),
        );
        Ok(Selection {
            files: selected
                .into_iter()
                .map(|file| self.planned(file))
                .collect::<Result<_, _>>()?,
            locale_assets,
            available_locales: locales,
        })
    }
}

/// The selected files for one locale.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    pub files: Vec<PlannedFile>,
    /// The locale whose asset files are installed (`es_AR` → `es_MX` when Riot ships the
    /// Mexican files for both); the requested locale when nothing says otherwise.
    pub locale_assets: String,
    pub available_locales: Vec<String>,
}

/// Picks the locale named most often (as a `.xx_YY.` / `/xx_YY/` marker) by the localized
/// files that were selected.
fn asset_locale<'a>(
    requested: &str,
    locales: &[String],
    localized_paths: impl Iterator<Item = &'a str>,
) -> String {
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for path in localized_paths {
        for locale in locales {
            if has_marker(path, locale) {
                *counts.entry(locale.as_str()).or_default() += 1;
            }
        }
    }
    let requested_count = counts.get(requested).copied().unwrap_or(0);
    counts
        .into_iter()
        .filter(|(_, count)| *count > requested_count)
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        .map_or_else(|| requested.to_string(), |(locale, _)| locale.to_string())
}

/// Path heuristic: drop files named for any other locale, and non-Windows platform files.
fn path_matches_locale(path: &str, allowed: &[String], locales: &[String]) -> bool {
    if locales
        .iter()
        .any(|known| !allowed.contains(known) && has_marker(path, known))
    {
        return false;
    }
    let has_platform = PLATFORM_MARKERS
        .iter()
        .any(|platform| has_marker(path, platform));
    !has_platform || has_marker(path, WINDOWS_FLAG)
}

fn has_marker(path: &str, marker: &str) -> bool {
    path.contains(&format!(".{marker}."))
        || path.contains(&format!("/{marker}/"))
        || path.ends_with(&format!(".{marker}"))
}

/// File-name markers a locale may use: its own, plus the locale it shares assets with
/// (for the path heuristic only; flags make this explicit).
fn allowed_markers(locale: &str) -> Vec<String> {
    let shared = match locale {
        "en_AU" | "en_GB" | "en_PH" | "en_SG" | "id_ID" | "zh_MY" => "en_US",
        "es_AR" => "es_MX",
        other => other,
    };
    let mut allowed = vec![locale.to_string()];
    if shared != locale {
        allowed.push(shared.to_string());
    }
    allowed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn flags(names: &[&str]) -> Vec<Flag> {
        names
            .iter()
            .enumerate()
            .map(|(index, name)| Flag {
                id: index as u8 + 1,
                name: name.to_string(),
            })
            .collect()
    }

    fn file(path: &str, flag_ids: &[u8], chunk: u64) -> ManifestFile {
        ManifestFile {
            path: path.to_string(),
            size: 10,
            flag_mask: (!flag_ids.is_empty())
                .then(|| flag_ids.iter().fold(0, |mask, id| mask | flag_bit(*id))),
            chunk_ids: vec![chunk],
            is_link: false,
        }
    }

    fn manifest(files: Vec<ManifestFile>) -> Manifest {
        let chunks = files
            .iter()
            .flat_map(|file| file.chunk_ids.clone())
            .map(|id| {
                (
                    id,
                    ChunkLocation {
                        bundle_id: 1,
                        bundle_offset: id * 10,
                        bundle_size: 5,
                        target_size: 10,
                    },
                )
            })
            .collect();
        Manifest {
            flags: flags(&["windows", "macos", "es_AR", "es_MX", "fr_FR", "en_GB"]),
            files,
            chunks,
        }
    }

    fn paths(files: &[PlannedFile]) -> Vec<&str> {
        files.iter().map(|file| file.path.as_str()).collect()
    }

    #[test]
    fn flag_id_n_is_bit_n_minus_one() {
        assert_eq!(flag_bit(1), 1);
        assert_eq!(flag_bit(15), 1 << 14);
        assert_eq!(flag_bit(64), 1 << 63);
        assert_eq!(flag_bit(0), 0);
        assert_eq!(flag_bit(65), 0);
    }

    #[test]
    fn selects_by_flags_not_names() {
        let manifest = manifest(vec![
            file("League of Legends.exe", &[], 1),
            file("DATA/Bootstrap.windows.wad.client", &[1], 2),
            file("DATA/Bootstrap.macos.wad.client", &[2], 3),
            // Misleading names: flags decide.
            file("DATA/Aatrox.fr_FR.wad.client", &[3, 4], 4),
            file("DATA/Aatrox.es_MX.wad.client", &[5], 5),
            file("DATA/Aatrox.en_GB.wad.client", &[6], 6),
        ]);
        let selection = manifest.select("es_ar").unwrap();
        // The only es_AR-flagged file is named fr_FR, so that is where its assets come from.
        assert_eq!(selection.locale_assets, "fr_FR");
        let selected = selection.files;
        assert_eq!(
            paths(&selected),
            vec![
                "League of Legends.exe",
                "DATA/Bootstrap.windows.wad.client",
                "DATA/Aatrox.fr_FR.wad.client",
            ]
        );
        assert_eq!(selected[0].chunks[0].bundle_offset, 10);
    }

    #[test]
    fn unknown_locale_lists_available_ones() {
        let err = manifest(vec![file("a", &[], 1)])
            .select("de_DE")
            .unwrap_err();
        assert_eq!(err.code(), "localeUnavailable");
        assert!(err.to_string().contains("en_GB, es_AR, es_MX, fr_FR"));
        let err = manifest(vec![file("a", &[], 1)])
            .select("german")
            .unwrap_err();
        assert_eq!(err.code(), "invalidLocale");
    }

    #[test]
    fn reports_shared_asset_locale_and_sorted_locales() {
        let manifest = manifest(vec![
            file("League of Legends.exe", &[], 1),
            file("DATA/Aatrox.es_MX.wad.client", &[3, 4], 2),
            file("DATA/Ahri.es_MX.wad.client", &[3, 4], 3),
            file("DATA/Aatrox.fr_FR.wad.client", &[5], 4),
        ]);
        let es_ar = manifest.select("es_AR").unwrap();
        let es_mx = manifest.select("es_MX").unwrap();
        let fr_fr = manifest.select("fr_FR").unwrap();
        assert_eq!(es_ar.locale_assets, "es_MX");
        assert_eq!(es_mx.locale_assets, "es_MX");
        assert_eq!(fr_fr.locale_assets, "fr_FR");
        assert_eq!(es_ar.files, es_mx.files);
        assert_eq!(
            es_ar.available_locales,
            vec!["en_GB", "es_AR", "es_MX", "fr_FR"]
        );
        // No localized files at all: nothing says otherwise.
        assert_eq!(manifest.select("en_GB").unwrap().locale_assets, "en_GB");
    }

    #[test]
    fn falls_back_to_path_markers_without_flags() {
        let mut manifest = manifest(vec![
            file("League of Legends.exe", &[], 1),
            file("DATA/Bootstrap.windows.wad.client", &[], 2),
            file("DATA/Bootstrap.macos.wad.client", &[], 3),
            file("DATA/Aatrox.es_MX.wad.client", &[], 4),
            file("DATA/Aatrox.fr_FR.wad.client", &[], 5),
        ]);
        manifest.flags.retain(|flag| flag.name != "en_GB");
        let selection = manifest.select("es_AR").unwrap();
        assert_eq!(selection.locale_assets, "es_MX");
        assert_eq!(
            paths(&selection.files),
            vec![
                "League of Legends.exe",
                "DATA/Bootstrap.windows.wad.client",
                "DATA/Aatrox.es_MX.wad.client",
            ]
        );
    }

    #[test]
    fn missing_chunk_or_size_mismatch_is_an_error() {
        let mut broken = manifest(vec![file("a", &[], 1)]);
        broken.chunks.clear();
        assert_eq!(broken.select("es_MX").unwrap_err().code(), "manifest");

        let mut short = manifest(vec![file("a", &[], 1)]);
        short.files[0].size = 11;
        assert_eq!(short.select("es_MX").unwrap_err().code(), "manifest");
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        assert!(parse(b"").is_err());
        assert!(parse(b"NOPE").is_err());
        let mut header = b"RMAN\x03\x00".to_vec();
        header.resize(HEADER_LEN, 0);
        assert!(parse(&header).unwrap_err().to_string().contains("3.0"));

        // Valid header and zstd body, garbage tables: the crate panics inside, we return Err.
        for body in [
            vec![0xFFu8; 64],
            vec![0u8; 3],
            (0..=255u8).collect::<Vec<_>>(),
        ] {
            let compressed = zstd::encode_all(body.as_slice(), 0).unwrap();
            let mut bytes = b"RMAN\x02\x01".to_vec();
            bytes.extend_from_slice(&(1u16 << 9).to_le_bytes());
            bytes.extend_from_slice(&(HEADER_LEN as u32).to_le_bytes());
            bytes.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&7u64.to_le_bytes());
            bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&compressed);
            assert_eq!(parse(&bytes).unwrap_err().code(), "manifest");

            bytes.truncate(bytes.len() - 1);
            assert!(parse(&bytes).unwrap_err().to_string().contains("truncated"));
        }
    }

    /// `CHRONORIFT_MANIFEST_SAMPLE=/path/to/16.17.manifest cargo test -- --ignored`
    #[test]
    #[ignore = "needs CHRONORIFT_MANIFEST_SAMPLE pointing at a real game-client manifest"]
    fn decodes_flags_of_a_real_manifest() {
        let path = std::env::var("CHRONORIFT_MANIFEST_SAMPLE")
            .expect("set CHRONORIFT_MANIFEST_SAMPLE to a real lol-game-client manifest");
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(
            &bytes[..6],
            b"RMAN\x02\x01",
            "sample should be an RMAN 2.1 manifest"
        );
        let manifest = parse(&bytes).unwrap();
        assert!(manifest.flags.contains(&Flag {
            id: 1,
            name: "windows".into()
        }));

        let selection = manifest.select("es_MX").unwrap();
        assert_eq!(selection.locale_assets, "es_MX");
        assert_eq!(selection.available_locales.len(), 28);
        assert!(selection.available_locales.windows(2).all(|w| w[0] < w[1]));
        let es_mx = selection.files;
        let by_path = |path: &str| es_mx.iter().find(|file| file.path == path);
        assert!(by_path("League of Legends.exe").is_some());
        assert!(es_mx.iter().any(|file| file.path.contains(".es_MX.")));
        assert!(!es_mx.iter().any(|file| file.path.contains(".fr_FR.")));
        assert!(!es_mx.iter().any(|file| file.path.contains(".macos.")));

        // Flag selection agrees with the path-heuristic fallback on this manifest.
        let locales = manifest.locales();
        let heuristic = |locale: &str| {
            manifest
                .files
                .iter()
                .filter(|file| {
                    !file.is_link
                        && path_matches_locale(&file.path, &allowed_markers(locale), &locales)
                })
                .map(|file| file.path.clone())
                .collect::<HashSet<_>>()
        };
        let names =
            |files: &[PlannedFile]| files.iter().map(|f| f.path.clone()).collect::<HashSet<_>>();
        for locale in ["es_MX", "es_AR", "en_GB", "en_US", "ko_KR"] {
            let flagged = names(&manifest.select(locale).unwrap().files);
            let by_path = heuristic(locale);
            assert_eq!(
                flagged.symmetric_difference(&by_path).collect::<Vec<_>>(),
                Vec::<&String>::new(),
                "{locale}"
            );
        }

        // es_AR adds its own UI/Global files on top of the Mexican assets.
        let es_ar = manifest.select("es_AR").unwrap();
        assert_eq!(es_ar.locale_assets, "es_MX");
        let es_ar_paths = names(&es_ar.files);
        assert!(es_ar_paths.contains("DATA/FINAL/UI.es_AR.wad.client"));
        assert!(es_ar_paths.contains("DATA/FINAL/Champions/Aatrox.es_MX.wad.client"));
        assert!(!names(&es_mx).contains("DATA/FINAL/UI.es_AR.wad.client"));
        let en_gb = manifest.select("en_GB").unwrap();
        assert_eq!(en_gb.locale_assets, "en_US");
        assert!(en_gb.files.iter().any(|file| file.path.contains(".en_US.")));
        assert!(en_gb.files.iter().any(|file| file.path.contains(".en_GB.")));
        assert_eq!(manifest.select("ko_KR").unwrap().locale_assets, "ko_KR");
        assert_eq!(
            manifest.select("xx_XX").unwrap_err().code(),
            "localeUnavailable"
        );
        println!(
            "{} files, {} chunk refs",
            es_mx.len(),
            es_mx.iter().map(|file| file.chunks.len()).sum::<usize>()
        );
    }
}
