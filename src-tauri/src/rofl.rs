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
    pub teams: Vec<TeamSummary>,
    pub ended_in_surrender: bool,
    pub ended_in_early_surrender: bool,
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
    pub riot_tag_line: Option<String>,
    pub puuid: Option<String>,
    pub level: u32,
    pub minions_killed: u32,
    pub neutral_minions_killed: u32,
    pub gold_spent: u32,
    pub physical_damage_to_champions: u32,
    pub magic_damage_to_champions: u32,
    pub true_damage_to_champions: u32,
    pub damage_taken: u32,
    pub damage_self_mitigated: u32,
    pub damage_to_buildings: u32,
    pub damage_to_objectives: u32,
    pub healing: u32,
    pub healing_on_teammates: u32,
    pub shielding_on_teammates: u32,
    pub crowd_control_score: u32,
    pub wards_placed: u32,
    pub wards_killed: u32,
    pub control_wards_bought: u32,
    pub turret_takedowns: u32,
    pub largest_killing_spree: u32,
    pub largest_multi_kill: u32,
    pub double_kills: u32,
    pub triple_kills: u32,
    pub quadra_kills: u32,
    pub penta_kills: u32,
    pub time_spent_dead_seconds: u32,
    pub keystone: Option<String>,
    pub primary_style: Option<String>,
    pub sub_style: Option<String>,
    pub stat_perks: Vec<String>,
    pub trinket: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TeamSummary {
    pub team: String,
    pub win: bool,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub gold_earned: u32,
    pub damage_to_champions: u32,
    pub towers: u32,
    pub inhibitors: u32,
    pub dragons: u32,
    pub elder_dragons: u32,
    pub barons: u32,
    pub heralds: u32,
    pub voidgrubs: u32,
    pub atakhan: u32,
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
    let MatchStats {
        players,
        teams,
        ended_in_surrender,
        ended_in_early_surrender,
    } = match_stats(&metadata, data_dragon_version.as_deref());
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
        teams,
        ended_in_surrender,
        ended_in_early_surrender,
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
    let MatchStats {
        players,
        teams,
        ended_in_surrender,
        ended_in_early_surrender,
    } = match_stats(&metadata, data_dragon_version.as_deref());
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
        teams,
        ended_in_surrender,
        ended_in_early_surrender,
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
        teams: Vec::new(),
        ended_in_surrender: false,
        ended_in_early_surrender: false,
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

struct MatchStats {
    players: Vec<PlayerStats>,
    teams: Vec<TeamSummary>,
    ended_in_surrender: bool,
    ended_in_early_surrender: bool,
}

fn match_stats(metadata: &Value, data_dragon_version: Option<&str>) -> MatchStats {
    let rows = get_string(metadata, &["statsJson", "statsJSON"])
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let players: Vec<_> = rows
        .iter()
        .enumerate()
        .map(|(index, player)| player_from_stats(index, player, data_dragon_version))
        .collect();
    let flag = |key: &str| rows.iter().any(|row| num(row, key) > 0);
    let ended_in_early_surrender = flag("GAME_ENDED_IN_EARLY_SURRENDER");

    MatchStats {
        teams: team_summaries(&rows, &players),
        ended_in_surrender: ended_in_early_surrender || flag("GAME_ENDED_IN_SURRENDER"),
        ended_in_early_surrender,
        players,
    }
}

/// Per-team totals, Blue then Red; teams with no players are omitted.
fn team_summaries(rows: &[Value], players: &[PlayerStats]) -> Vec<TeamSummary> {
    ["Blue", "Red"]
        .iter()
        .filter_map(|team| {
            let members: Vec<_> = rows
                .iter()
                .zip(players)
                .filter(|(_, player)| player.team == *team)
                .collect();
            if members.is_empty() {
                return None;
            }
            let sum = |key: &str| members.iter().map(|(row, _)| num(row, key)).sum();
            Some(TeamSummary {
                team: team.to_string(),
                win: members.iter().any(|(_, player)| player.win),
                kills: members.iter().map(|(_, player)| player.kills).sum(),
                deaths: members.iter().map(|(_, player)| player.deaths).sum(),
                assists: members.iter().map(|(_, player)| player.assists).sum(),
                gold_earned: members.iter().map(|(_, player)| player.gold_earned).sum(),
                damage_to_champions: members
                    .iter()
                    .map(|(_, player)| player.damage_to_champions)
                    .sum(),
                towers: sum("TURRETS_KILLED"),
                inhibitors: sum("BARRACKS_KILLED"),
                dragons: sum("DRAGON_KILLS"),
                elder_dragons: sum("ELDER_DRAGON_KILLS"),
                barons: sum("BARON_KILLS"),
                heralds: sum("RIFT_HERALD_KILLS"),
                voidgrubs: sum("HORDE_KILLS"),
                atakhan: sum("ATAKHAN_KILLS"),
            })
        })
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
    let kills = num(player, "CHAMPIONS_KILLED");
    let deaths = num(player, "NUM_DEATHS");
    let assists = num(player, "ASSISTS");
    let minions_killed = num(player, "MINIONS_KILLED");
    let neutral_minions_killed = num(player, "NEUTRAL_MINIONS_KILLED");
    let id = |key: &str| stat_string(player, key).filter(|value| value != "0");
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
        creep_score: minions_killed + neutral_minions_killed,
        gold_earned: num(player, "GOLD_EARNED"),
        damage_to_champions: num(player, "TOTAL_DAMAGE_DEALT_TO_CHAMPIONS"),
        vision_score: num(player, "VISION_SCORE"),
        items: player_items(player, data_dragon_version),
        summoner_spells: ["SUMMONER_SPELL_1", "SUMMONER_SPELL_2"]
            .iter()
            .filter_map(|key| id(key))
            .collect(),
        perks: ["PERK0", "PERK1", "PERK2", "PERK3", "PERK4", "PERK5"]
            .iter()
            .filter_map(|key| id(key))
            .collect(),
        riot_tag_line: stat_string(player, "RIOT_ID_TAG_LINE"),
        puuid: stat_string(player, "PUUID"),
        level: num(player, "LEVEL"),
        minions_killed,
        neutral_minions_killed,
        gold_spent: num(player, "GOLD_SPENT"),
        physical_damage_to_champions: num(player, "PHYSICAL_DAMAGE_DEALT_TO_CHAMPIONS"),
        magic_damage_to_champions: num(player, "MAGIC_DAMAGE_DEALT_TO_CHAMPIONS"),
        true_damage_to_champions: num(player, "TRUE_DAMAGE_DEALT_TO_CHAMPIONS"),
        damage_taken: num(player, "TOTAL_DAMAGE_TAKEN"),
        damage_self_mitigated: num(player, "TOTAL_DAMAGE_SELF_MITIGATED"),
        damage_to_buildings: num(player, "TOTAL_DAMAGE_DEALT_TO_BUILDINGS"),
        damage_to_objectives: num(player, "TOTAL_DAMAGE_DEALT_TO_OBJECTIVES"),
        healing: num(player, "TOTAL_HEAL"),
        healing_on_teammates: num(player, "TOTAL_HEAL_ON_TEAMMATES"),
        shielding_on_teammates: num(player, "TOTAL_DAMAGE_SHIELDED_ON_TEAMMATES"),
        crowd_control_score: num(player, "TIME_CCING_OTHERS"),
        wards_placed: num(player, "WARD_PLACED"),
        wards_killed: num(player, "WARD_KILLED"),
        control_wards_bought: num(player, "VISION_WARDS_BOUGHT_IN_GAME"),
        turret_takedowns: num(player, "TURRET_TAKEDOWNS"),
        largest_killing_spree: num(player, "LARGEST_KILLING_SPREE"),
        largest_multi_kill: num(player, "LARGEST_MULTI_KILL"),
        double_kills: num(player, "DOUBLE_KILLS"),
        triple_kills: num(player, "TRIPLE_KILLS"),
        quadra_kills: num(player, "QUADRA_KILLS"),
        penta_kills: num(player, "PENTA_KILLS"),
        time_spent_dead_seconds: num(player, "TOTAL_TIME_SPENT_DEAD"),
        keystone: id("KEYSTONE_ID").or_else(|| id("PERK0")),
        primary_style: id("PERK_PRIMARY_STYLE"),
        sub_style: id("PERK_SUB_STYLE"),
        stat_perks: ["STAT_PERK_0", "STAT_PERK_1", "STAT_PERK_2"]
            .iter()
            .filter_map(|key| id(key))
            .collect(),
        trinket: id("ITEM6"),
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

/// Numeric stat; ROFL2 stores numbers as strings. Missing or malformed values read as 0.
fn num(player: &Value, key: &str) -> u32 {
    stat_string(player, key)
        .and_then(|value| {
            value.parse::<u32>().ok().or_else(|| {
                value
                    .parse::<f64>()
                    .ok()
                    .filter(|number| number.is_finite() && *number >= 0.0)
                    .map(|number| number.min(f64::from(u32::MAX)) as u32)
            })
        })
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
    let stem = match file_name.len().checked_sub(5) {
        Some(split)
            if file_name.is_char_boundary(split)
                && file_name[split..].eq_ignore_ascii_case(".rofl") =>
        {
            &file_name[..split]
        }
        _ => file_name,
    };
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

/// Builds a minimal ROFL2 file (magic + version string + JSON trailer) for tests.
#[cfg(test)]
pub(crate) fn rofl2_fixture_bytes(metadata: &Value, version: &str) -> Vec<u8> {
    let raw = metadata.to_string();
    let mut bytes = ROFL2_MAGIC.to_vec();
    bytes.extend_from_slice(b"deadbeef");
    bytes.extend_from_slice(version.as_bytes());
    bytes.extend_from_slice(raw.as_bytes());
    bytes.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    bytes
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
                    "PERK_PRIMARY_STYLE": "8000",
                    "STAT_PERK_0": "5008",
                    "STAT_PERK_1": "0",
                    "PUUID": "abc-123",
                    "LEVEL": "18",
                    "TURRETS_KILLED": "2",
                    "DRAGON_KILLS": "1",
                    "HORDE_KILLS": "3",
                    "WIN": "Win"
                },
                {
                    "RIOT_ID_GAME_NAME": "Ally",
                    "RIOT_ID_TAG_LINE": "",
                    "PUUID": "",
                    "SKIN": "Lux",
                    "TEAM": "100",
                    "CHAMPIONS_KILLED": "1",
                    "GOLD_EARNED": "1000",
                    "TURRETS_KILLED": "1",
                    "BARON_KILLS": "1",
                    "WIN": "Win"
                },
                {
                    "SKIN": "Zed",
                    "TEAM": "200",
                    "CHAMPIONS_KILLED": "7",
                    "NUM_DEATHS": "2",
                    "BARRACKS_KILLED": "1",
                    "GAME_ENDED_IN_SURRENDER": "1",
                    "WIN": "Fail"
                }
            ])).unwrap()
        });
        let raw = metadata.to_string();
        let mut bytes = b"RIOT\x02\x00deadbeef16.13.790.6961".to_vec();
        bytes.extend_from_slice(raw.as_bytes());
        bytes.extend_from_slice(&(raw.len() as u32).to_le_bytes());

        let parsed = read_rofl2_metadata(&bytes).expect("metadata should parse");
        let version = find_ascii_version(&bytes[..128.min(bytes.len())]);
        let stats = match_stats(&parsed, Some("16.13.1"));
        let players = &stats.players;

        assert_eq!(version, Some("16.13.790.6961".to_string()));
        assert_eq!(get_u64(&parsed, &["gameLength"]), Some(123000));
        assert_eq!(players.len(), 3);
        assert_eq!(players[0].name, "Tester#LA2");
        assert_eq!(players[0].champion, "Nasus");
        assert_eq!(players[0].creep_score, 203);
        assert_eq!(players[0].items.len(), 3);
        assert_eq!(players[0].riot_tag_line.as_deref(), Some("LA2"));
        assert_eq!(players[0].puuid.as_deref(), Some("abc-123"));
        assert_eq!(players[0].level, 18);
        assert_eq!(players[0].minions_killed, 200);
        assert_eq!(players[0].keystone.as_deref(), Some("8021"));
        assert_eq!(players[0].primary_style.as_deref(), Some("8000"));
        assert_eq!(players[0].sub_style, None);
        assert_eq!(players[0].stat_perks, vec!["5008"]);
        assert_eq!(players[0].trinket.as_deref(), Some("3340"));
        assert_eq!(players[1].riot_tag_line, None);
        assert_eq!(players[1].puuid, None);
        assert_eq!(players[1].gold_spent, 0);
        assert_eq!(team_summary(players), (5, 7, Some("Blue".to_string())));

        assert!(stats.ended_in_surrender);
        assert!(!stats.ended_in_early_surrender);
        assert_eq!(stats.teams.len(), 2);
        let (blue, red) = (&stats.teams[0], &stats.teams[1]);
        assert_eq!((blue.team.as_str(), blue.win), ("Blue", true));
        assert_eq!((blue.kills, blue.deaths, blue.assists), (5, 5, 6));
        assert_eq!(blue.gold_earned, 11428);
        assert_eq!((blue.towers, blue.dragons, blue.barons), (3, 1, 1));
        assert_eq!((blue.voidgrubs, blue.inhibitors), (3, 0));
        assert_eq!((red.team.as_str(), red.win), ("Red", false));
        assert_eq!((red.kills, red.deaths, red.inhibitors), (7, 2, 1));

        let empty = match_stats(&serde_json::json!({}), None);
        assert!(empty.players.is_empty() && empty.teams.is_empty());
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
        assert_eq!(replay.teams.len(), 2);
        assert_eq!(replay.teams[0].team, "Blue");
        assert_eq!(replay.teams[0].kills, replay.blue_kills);
        assert_eq!(replay.teams[1].kills, replay.red_kills);
        assert!(replay.teams.iter().any(|team| team.win));
        assert!(replay.players.iter().all(|player| player.puuid.is_some()));
        assert!(replay.players.iter().all(|player| player.level > 0));
    }
}
