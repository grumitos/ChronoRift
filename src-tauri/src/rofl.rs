use serde::Serialize;
use serde_json::Value;
use std::{
    error::Error,
    fmt::{self, Display},
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const CLASSIC_MAGIC: &[u8; 6] = b"RIOT\0\0";
const ROFL2_MAGIC: &[u8; 6] = b"RIOT\x02\0";
const CLASSIC_HEADER_LEN: usize = 288;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReplayFormat {
    ClassicRofl,
    Rofl2,
    Unknown,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReplayStatus {
    Parsed,
    Unsupported,
    Invalid,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReplayInspection {
    pub path: String,
    pub file_name: String,
    pub format: ReplayFormat,
    pub status: ReplayStatus,
    pub game_version: Option<String>,
    pub patch: Option<String>,
    pub region: Option<String>,
    pub suggested_locale: Option<String>,
    pub match_id: Option<String>,
    pub game_duration_ms: Option<u64>,
    pub metadata: Value,
    pub notes: Vec<String>,
}

#[derive(Debug)]
pub enum InspectError {
    Io(io::Error),
    InvalidPath,
    InvalidHeader(&'static str),
    InvalidMetadata(serde_json::Error),
}

impl Display for InspectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::InvalidPath => write!(f, "Invalid replay path"),
            Self::InvalidHeader(reason) => write!(f, "Invalid ROFL header: {reason}"),
            Self::InvalidMetadata(err) => write!(f, "Invalid ROFL metadata JSON: {err}"),
        }
    }
}

impl Error for InspectError {}

impl From<io::Error> for InspectError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClassicHeader {
    metadata_offset: u32,
    metadata_length: u32,
    payload_header_offset: u32,
    payload_header_length: u32,
}

pub fn inspect_file(path: impl Into<PathBuf>) -> Result<ReplayInspection, InspectError> {
    let path = path.into();
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(InspectError::InvalidPath)?
        .to_owned();

    let mut file = File::open(&path)?;
    let mut header_bytes = [0; CLASSIC_HEADER_LEN];
    let read = file.read(&mut header_bytes)?;
    if read < CLASSIC_MAGIC.len() {
        return Ok(base_inspection(
            &path,
            file_name,
            ReplayFormat::Unknown,
            ReplayStatus::Invalid,
            "File is too small to be a ROFL replay",
        ));
    }

    if header_bytes[..CLASSIC_MAGIC.len()] == *ROFL2_MAGIC {
        return Ok(base_inspection(
            &path,
            file_name,
            ReplayFormat::Rofl2,
            ReplayStatus::Unsupported,
            "ROFL2 exists, but ChronoRift only parses classic ROFL metadata today",
        ));
    }

    let header = parse_classic_header(&header_bytes)?;
    let metadata = read_json_at(&mut file, header.metadata_offset, header.metadata_length)?;
    let payload_header = read_slice_at(
        &mut file,
        header.payload_header_offset,
        header.payload_header_length,
    )?;
    let payload_match_id = parse_payload_match_id(&payload_header);

    let game_version = get_string(&metadata, &["gameVersion", "GameVersion", "game_version"]);
    let patch = game_version.as_deref().and_then(patch_from_version);
    let region = get_string(
        &metadata,
        &[
            "platformId",
            "PlatformId",
            "platformID",
            "region",
            "Region",
            "gameServerRegion",
        ],
    )
    .map(|value| value.to_ascii_uppercase());
    let suggested_locale = region.as_deref().and_then(suggest_locale_for_region);
    let game_duration_ms = get_u64(
        &metadata,
        &["gameLength", "GameLength", "game_length", "gameDuration"],
    );
    let match_id = payload_match_id
        .or_else(|| get_string(&metadata, &["gameId", "GameId", "matchId", "MatchId"]));

    Ok(ReplayInspection {
        path: path.display().to_string(),
        file_name,
        format: ReplayFormat::ClassicRofl,
        status: ReplayStatus::Parsed,
        game_version,
        patch,
        region,
        suggested_locale,
        match_id,
        game_duration_ms,
        metadata,
        notes: Vec::new(),
    })
}

fn base_inspection(
    path: &Path,
    file_name: String,
    format: ReplayFormat,
    status: ReplayStatus,
    note: impl Into<String>,
) -> ReplayInspection {
    ReplayInspection {
        path: path.display().to_string(),
        file_name,
        format,
        status,
        game_version: None,
        patch: None,
        region: None,
        suggested_locale: None,
        match_id: None,
        game_duration_ms: None,
        metadata: Value::Null,
        notes: vec![note.into()],
    }
}

fn parse_classic_header(bytes: &[u8]) -> Result<ClassicHeader, InspectError> {
    if bytes.len() < CLASSIC_HEADER_LEN {
        return Err(InspectError::InvalidHeader(
            "file is shorter than the classic header",
        ));
    }

    if bytes[..CLASSIC_MAGIC.len()] != *CLASSIC_MAGIC {
        return Err(InspectError::InvalidHeader("missing RIOT classic magic"));
    }

    let metadata_offset = u32_at(bytes, 268);
    let metadata_length = u32_at(bytes, 272);
    let payload_header_offset = u32_at(bytes, 276);
    let payload_header_length = u32_at(bytes, 280);

    if metadata_length == 0 {
        return Err(InspectError::InvalidHeader("metadata length is zero"));
    }

    Ok(ClassicHeader {
        metadata_offset,
        metadata_length,
        payload_header_offset,
        payload_header_length,
    })
}

fn read_json_at(file: &mut File, offset: u32, length: u32) -> Result<Value, InspectError> {
    let bytes = read_slice_at(file, offset, length)?;
    serde_json::from_slice(&bytes).map_err(InspectError::InvalidMetadata)
}

fn read_slice_at(file: &mut File, offset: u32, length: u32) -> Result<Vec<u8>, InspectError> {
    file.seek(SeekFrom::Start(u64::from(offset)))?;
    let mut bytes = vec![0; length as usize];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn parse_payload_match_id(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 8 {
        return None;
    }

    let id = u64::from_le_bytes(bytes[..8].try_into().ok()?);
    (id != 0).then(|| id.to_string())
}

fn get_string(value: &Value, keys: &[&str]) -> Option<String> {
    let object = value.as_object()?;
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(|value| match value {
            Value::String(text) if !text.is_empty() => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        })
}

fn get_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    let object = value.as_object()?;
    keys.iter()
        .find_map(|key| object.get(*key))
        .and_then(|value| match value {
            Value::Number(number) => number.as_u64(),
            Value::String(text) => text.parse().ok(),
            _ => None,
        })
}

fn patch_from_version(version: &str) -> Option<String> {
    let mut parts = version.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    (!major.is_empty() && !minor.is_empty()).then(|| format!("{major}.{minor}"))
}

fn suggest_locale_for_region(region: &str) -> Option<String> {
    let locale = match region {
        "BR1" => "pt_BR",
        "EUN1" | "EUW1" => "en_GB",
        "JP1" => "ja_JP",
        "KR" => "ko_KR",
        "LA1" | "LA2" => "es_MX",
        "NA1" | "OC1" | "PH2" | "SG2" => "en_US",
        "RU" => "ru_RU",
        "TH2" => "th_TH",
        "TR1" => "tr_TR",
        "TW2" => "zh_TW",
        "VN2" => "vi_VN",
        _ => return None,
    };

    Some(locale.to_string())
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("fixed header offset"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_classic_header_offsets() {
        let mut bytes = [0; CLASSIC_HEADER_LEN];
        bytes[..6].copy_from_slice(CLASSIC_MAGIC);
        bytes[268..272].copy_from_slice(&300u32.to_le_bytes());
        bytes[272..276].copy_from_slice(&48u32.to_le_bytes());
        bytes[276..280].copy_from_slice(&400u32.to_le_bytes());
        bytes[280..284].copy_from_slice(&8u32.to_le_bytes());

        let header = parse_classic_header(&bytes).expect("header should parse");

        assert_eq!(header.metadata_offset, 300);
        assert_eq!(header.metadata_length, 48);
        assert_eq!(header.payload_header_offset, 400);
        assert_eq!(header.payload_header_length, 8);
    }

    #[test]
    fn rejects_missing_magic() {
        let bytes = [0; CLASSIC_HEADER_LEN];

        let err = parse_classic_header(&bytes).expect_err("header should fail");

        assert!(err.to_string().contains("missing RIOT classic magic"));
    }

    #[test]
    fn extracts_patch_from_full_version() {
        assert_eq!(
            patch_from_version("14.8.576.8838"),
            Some("14.8".to_string())
        );
        assert_eq!(patch_from_version("14"), None);
    }

    #[test]
    fn extracts_match_id_from_payload_header() {
        assert_eq!(
            parse_payload_match_id(&123456789u64.to_le_bytes()),
            Some("123456789".to_string())
        );
        assert_eq!(parse_payload_match_id(&0u64.to_le_bytes()), None);
    }

    #[test]
    fn suggests_locale_from_region_without_forcing_it() {
        assert_eq!(suggest_locale_for_region("LA1"), Some("es_MX".to_string()));
        assert_eq!(suggest_locale_for_region("KR"), Some("ko_KR".to_string()));
        assert_eq!(suggest_locale_for_region("EUW1"), Some("en_GB".to_string()));
        assert_eq!(suggest_locale_for_region("UNKNOWN"), None);
    }
}
