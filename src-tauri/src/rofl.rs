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
    #[allow(dead_code)]
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
    pub data_dragon_version: Option<String>,
    pub match_id: Option<String>,
    pub game_duration_ms: Option<u64>,
    pub winning_team: Option<String>,
    pub blue_kills: u32,
    pub red_kills: u32,
    pub players: Vec<PlayerStats>,
    pub metadata: Value,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlayerStats {
    pub name: String,
    pub champion: String,
    pub champion_image_url: Option<String>,
    pub team: String,
    pub position: String,
    pub win: bool,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub creep_score: u32,
    pub gold_earned: u32,
    pub damage_to_champions: u32,
    pub vision_score: u32,
    pub items: Vec<PlayerItem>,
    pub summoner_spells: Vec<String>,
    pub perks: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PlayerItem {
    pub slot: u8,
    pub id: String,
    pub image_url: Option<String>,
}

#[derive(Debug)]
pub enum InspectError {
    Io(io::Error),
    InvalidPath,
    InvalidHeader(&'static str),
    InvalidMetadata(serde_json::Error),
    InvalidRofl2MetadataLength,
}

impl Display for InspectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::InvalidPath => write!(f, "Invalid replay path"),
            Self::InvalidHeader(reason) => write!(f, "Invalid ROFL header: {reason}"),
            Self::InvalidMetadata(err) => write!(f, "Invalid ROFL metadata JSON: {err}"),
            Self::InvalidRofl2MetadataLength => write!(f, "Invalid ROFL2 metadata length"),
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
        return inspect_rofl2(path, file_name);
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
    let data_dragon_version = patch.as_deref().map(data_dragon_version_from_patch);
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
    let players = players_from_metadata(&metadata, data_dragon_version.as_deref());
    let (blue_kills, red_kills, winning_team) = team_summary(&players);
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
        data_dragon_version,
        match_id,
        game_duration_ms,
        winning_team,
        blue_kills,
        red_kills,
        players,
        metadata,
        notes: Vec::new(),
    })
}

fn inspect_rofl2(path: PathBuf, file_name: String) -> Result<ReplayInspection, InspectError> {
    let bytes = std::fs::read(&path)?;
    let metadata = read_rofl2_metadata(&bytes)?;
    let game_version = find_ascii_version(&bytes[..bytes.len().min(128)]);
    let patch = game_version.as_deref().and_then(patch_from_version);
    let data_dragon_version = patch.as_deref().map(data_dragon_version_from_patch);
    let (region, file_match_id) = region_and_match_from_file_name(&file_name);
    let suggested_locale = region.as_deref().and_then(suggest_locale_for_region);
    let game_duration_ms = get_u64(
        &metadata,
        &["gameLength", "GameLength", "game_length", "gameDuration"],
    );
    let players = players_from_metadata(&metadata, data_dragon_version.as_deref());
    let (blue_kills, red_kills, winning_team) = team_summary(&players);

    Ok(ReplayInspection {
        path: path.display().to_string(),
        file_name,
        format: ReplayFormat::Rofl2,
        status: ReplayStatus::Parsed,
        game_version,
        patch,
        region,
        suggested_locale,
        data_dragon_version,
        match_id: get_string(&metadata, &["gameId", "GameId", "matchId", "MatchId"])
            .or(file_match_id),
        game_duration_ms,
        winning_team,
        blue_kills,
        red_kills,
        players,
        metadata,
        notes: vec!["ROFL2 metadata read from trailer".to_string()],
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
        data_dragon_version: None,
        match_id: None,
        game_duration_ms: None,
        winning_team: None,
        blue_kills: 0,
        red_kills: 0,
        players: Vec::new(),
        metadata: Value::Null,
        notes: vec![note.into()],
    }
}

fn read_rofl2_metadata(bytes: &[u8]) -> Result<Value, InspectError> {
    if bytes.len() < 4 {
        return Err(InspectError::InvalidRofl2MetadataLength);
    }

    let metadata_length = u32::from_le_bytes(
        bytes[bytes.len() - 4..]
            .try_into()
            .expect("metadata length suffix"),
    ) as usize;
    let metadata_start = bytes
        .len()
        .checked_sub(4 + metadata_length)
        .ok_or(InspectError::InvalidRofl2MetadataLength)?;

    serde_json::from_slice(&bytes[metadata_start..bytes.len() - 4])
        .map_err(InspectError::InvalidMetadata)
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

fn players_from_metadata(metadata: &Value, data_dragon_version: Option<&str>) -> Vec<PlayerStats> {
    let stats = get_string(metadata, &["statsJson", "statsJSON"])
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();

    stats
        .iter()
        .enumerate()
        .map(|(index, player)| player_from_stats(index, player, data_dragon_version))
        .collect()
}

fn player_from_stats(
    index: usize,
    player: &Value,
    data_dragon_version: Option<&str>,
) -> PlayerStats {
    let team = team_name(stat_string(player, "TEAM").as_deref());
    let champion = stat_string(player, "SKIN").unwrap_or_default();
    let name = riot_name(player)
        .or_else(|| stat_string(player, "NAME"))
        .unwrap_or_default();
    let position = stat_string(player, "TEAM_POSITION")
        .or_else(|| stat_string(player, "INDIVIDUAL_POSITION"))
        .unwrap_or_else(|| fallback_position(index).to_string());
    let kills = stat_u32(player, "CHAMPIONS_KILLED");
    let deaths = stat_u32(player, "NUM_DEATHS");
    let assists = stat_u32(player, "ASSISTS");
    let win = stat_string(player, "WIN")
        .map(|value| value.eq_ignore_ascii_case("Win"))
        .unwrap_or(false);

    PlayerStats {
        name,
        champion: champion.clone(),
        champion_image_url: data_dragon_version
            .filter(|_| !champion.is_empty())
            .map(|version| champion_image_url(version, &champion)),
        team,
        position,
        win,
        kills,
        deaths,
        assists,
        creep_score: stat_u32(player, "MINIONS_KILLED")
            + stat_u32(player, "NEUTRAL_MINIONS_KILLED"),
        gold_earned: stat_u32(player, "GOLD_EARNED"),
        damage_to_champions: stat_u32(player, "TOTAL_DAMAGE_DEALT_TO_CHAMPIONS"),
        vision_score: stat_u32(player, "VISION_SCORE"),
        items: player_items(player, data_dragon_version),
        summoner_spells: ["SUMMONER_SPELL_1", "SUMMONER_SPELL_2"]
            .iter()
            .filter_map(|key| stat_string(player, key))
            .filter(|value| value != "0")
            .collect(),
        perks: ["PERK0", "PERK1", "PERK2", "PERK3", "PERK4", "PERK5"]
            .iter()
            .filter_map(|key| stat_string(player, key))
            .filter(|value| value != "0")
            .collect(),
    }
}

fn player_items(player: &Value, data_dragon_version: Option<&str>) -> Vec<PlayerItem> {
    (0..=6)
        .filter_map(|slot| {
            let id = stat_string(player, &format!("ITEM{slot}"))?;
            (id != "0").then(|| PlayerItem {
                slot,
                image_url: data_dragon_version.map(|version| item_image_url(version, &id)),
                id,
            })
        })
        .collect()
}

fn team_summary(players: &[PlayerStats]) -> (u32, u32, Option<String>) {
    let blue_kills = players
        .iter()
        .filter(|player| player.team == "Blue")
        .map(|player| player.kills)
        .sum();
    let red_kills = players
        .iter()
        .filter(|player| player.team == "Red")
        .map(|player| player.kills)
        .sum();
    let winning_team = players
        .iter()
        .find(|player| player.win)
        .map(|player| player.team.clone());

    (blue_kills, red_kills, winning_team)
}

fn stat_string(player: &Value, key: &str) -> Option<String> {
    player.get(key).and_then(|value| match value {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    })
}

fn stat_u32(player: &Value, key: &str) -> u32 {
    stat_string(player, key)
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or(0)
}

fn riot_name(player: &Value) -> Option<String> {
    let name = stat_string(player, "RIOT_ID_GAME_NAME")?;
    let tag = stat_string(player, "RIOT_ID_TAG_LINE")?;
    Some(format!("{name}#{tag}"))
}

fn team_name(value: Option<&str>) -> String {
    match value {
        Some("100") => "Blue".to_string(),
        Some("200") => "Red".to_string(),
        Some(other) => other.to_string(),
        None => "Unknown".to_string(),
    }
}

fn fallback_position(index: usize) -> &'static str {
    match index % 5 {
        0 => "TOP",
        1 => "JUNGLE",
        2 => "MIDDLE",
        3 => "BOTTOM",
        _ => "UTILITY",
    }
}

fn patch_from_version(version: &str) -> Option<String> {
    let mut parts = version.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    (!major.is_empty() && !minor.is_empty()).then(|| format!("{major}.{minor}"))
}

fn data_dragon_version_from_patch(patch: &str) -> String {
    format!("{patch}.1")
}

fn champion_image_url(version: &str, champion: &str) -> String {
    format!("https://ddragon.leagueoflegends.com/cdn/{version}/img/champion/{champion}.png")
}

fn item_image_url(version: &str, item: &str) -> String {
    format!("https://ddragon.leagueoflegends.com/cdn/{version}/img/item/{item}.png")
}

fn find_ascii_version(bytes: &[u8]) -> Option<String> {
    for start in 0..bytes.len() {
        if !bytes[start].is_ascii_digit() {
            continue;
        }

        let end = (start..bytes.len())
            .find(|index| {
                let byte = bytes[*index];
                !(byte.is_ascii_digit() || byte == b'.')
            })
            .unwrap_or(bytes.len());
        let candidate = std::str::from_utf8(&bytes[start..end]).ok()?;
        if candidate.matches('.').count() >= 2 {
            return Some(candidate.to_string());
        }
    }

    None
}

fn region_and_match_from_file_name(file_name: &str) -> (Option<String>, Option<String>) {
    let stem = file_name.strip_suffix(".rofl").unwrap_or(file_name);
    let Some((region, match_id)) = stem.split_once('-') else {
        return (None, None);
    };

    (
        (!region.is_empty()).then(|| region.to_ascii_uppercase()),
        (!match_id.is_empty() && match_id.chars().all(|ch| ch.is_ascii_digit()))
            .then(|| match_id.to_string()),
    )
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

    #[test]
    fn parses_rofl2_trailer_metadata_and_stats() {
        let metadata = serde_json::json!({
            "gameLength": 123000,
            "statsJson": serde_json::to_string(&serde_json::json!([
                {
                    "RIOT_ID_GAME_NAME": "Tester",
                    "RIOT_ID_TAG_LINE": "LA2",
                    "SKIN": "Nasus",
                    "TEAM": "100",
                    "TEAM_POSITION": "TOP",
                    "CHAMPIONS_KILLED": "4",
                    "NUM_DEATHS": "5",
                    "ASSISTS": "6",
                    "MINIONS_KILLED": "200",
                    "NEUTRAL_MINIONS_KILLED": "3",
                    "GOLD_EARNED": "10428",
                    "TOTAL_DAMAGE_DEALT_TO_CHAMPIONS": "20353",
                    "VISION_SCORE": "14",
                    "ITEM0": "3508",
                    "ITEM1": "0",
                    "ITEM2": "3158",
                    "ITEM3": "0",
                    "ITEM4": "0",
                    "ITEM5": "0",
                    "ITEM6": "3340",
                    "SUMMONER_SPELL_1": "6",
                    "SUMMONER_SPELL_2": "12",
                    "PERK0": "8021",
                    "WIN": "Win"
                }
            ])).unwrap()
        });
        let raw = metadata.to_string();
        let mut bytes = b"RIOT\x02\x00deadbeef16.13.790.6961".to_vec();
        bytes.extend_from_slice(raw.as_bytes());
        bytes.extend_from_slice(&(raw.len() as u32).to_le_bytes());

        let parsed = read_rofl2_metadata(&bytes).expect("metadata should parse");
        let version = find_ascii_version(&bytes[..128.min(bytes.len())]);
        let players = players_from_metadata(&parsed, Some("16.13.1"));

        assert_eq!(version, Some("16.13.790.6961".to_string()));
        assert_eq!(get_u64(&parsed, &["gameLength"]), Some(123000));
        assert_eq!(players.len(), 1);
        assert_eq!(players[0].name, "Tester#LA2");
        assert_eq!(players[0].champion, "Nasus");
        assert_eq!(players[0].creep_score, 203);
        assert_eq!(players[0].items.len(), 3);
        assert_eq!(team_summary(&players), (4, 0, Some("Blue".to_string())));
    }

    #[test]
    fn extracts_region_and_match_from_file_name() {
        assert_eq!(
            region_and_match_from_file_name("LA2-1604952545.rofl"),
            (Some("LA2".to_string()), Some("1604952545".to_string()))
        );
    }

    #[test]
    fn parses_optional_real_replay_sample() {
        let Ok(path) = std::env::var("CHRONORIFT_REPLAY_SAMPLE") else {
            return;
        };

        let replay = inspect_file(path).expect("real replay should parse");

        assert_eq!(replay.status, ReplayStatus::Parsed);
        assert!(replay.game_version.is_some());
        assert_eq!(replay.players.len(), 10);
        assert!(replay.game_duration_ms.unwrap_or_default() > 0);
    }
}
