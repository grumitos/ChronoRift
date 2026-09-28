use serde::Serialize;
use serde_json::Value;
use std::{
    error::Error,
    fmt::{self, Display},
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

const MAGIC_LEN: usize = 6;
const CLASSIC_MAGIC: &[u8; MAGIC_LEN] = b"RIOT\0\0";
const ROFL2_MAGIC: &[u8; MAGIC_LEN] = b"RIOT\x02\0";
const CLASSIC_HEADER_LEN: usize = 288;
/// ROFL2 header: magic(6) + 8 unknown bytes, then a u8 length and the game version.
const ROFL2_VERSION_LEN_OFFSET: usize = MAGIC_LEN + 8;
/// ROFL2 ends with the metadata JSON followed by its u32 little-endian length.
const ROFL2_TRAILER_LEN: u64 = 4;
/// Upper bound for any metadata block read from an untrusted replay.
const MAX_METADATA_LEN: u64 = 16 * 1024 * 1024;
/// Only the leading u64 match id is read from the classic payload header.
const PAYLOAD_MATCH_ID_LEN: u64 = 8;
const STATS_JSON_KEYS: [&str; 2] = ["statsJson", "statsJSON"];
/// Replaces the raw `statsJson` string in `metadata` sent to the UI.
pub const STATS_PLACEHOLDER: &str = "<parsed into players>";

/// Riot platform ids ChronoRift recognizes in metadata and file names.
const KNOWN_PLATFORMS: [&str; 17] = [
    "BR1", "EUN1", "EUW1", "JP1", "KR", "LA1", "LA2", "ME1", "NA1", "OC1", "PH2", "RU", "SG2",
    "TH2", "TR1", "TW2", "VN2",
];

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ReplayFormat {
    ClassicRofl,
    Rofl2,
    Unknown,
}

/// `Invalid`: not a readable replay (too small, no RIOT magic, broken header).
/// `Unsupported`: recognized ROFL2 file whose trailer metadata could not be read.
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
    /// The replay's "major.minor" patch. The frontend resolves the concrete
    /// DataDragon release (e.g. "14.8" -> "14.8.1") from Riot's versions list.
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
    /// Replay metadata JSON; `statsJson` is replaced by [`STATS_PLACEHOLDER`].
    pub metadata: Value,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlayerStats {
    pub name: String,
    pub champion: String,
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
}

/// Only I/O failures and unusable paths are errors. A file that was read but is
/// not a usable replay comes back as `Ok` with `Invalid`/`Unsupported` and a note.
#[derive(Debug)]
pub enum InspectError {
    Io(io::Error),
    InvalidPath,
}

impl Display for InspectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
            Self::InvalidPath => write!(f, "Invalid replay path"),
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

/// Identity of the file being inspected, shared by every result.
struct Source<'a> {
    path: &'a Path,
    file_name: &'a str,
}

impl Source<'_> {
    fn empty(&self, format: ReplayFormat, status: ReplayStatus, note: String) -> ReplayInspection {
        ReplayInspection {
            path: self.path.display().to_string(),
            file_name: self.file_name.to_string(),
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
            notes: vec![note],
        }
    }
}

pub fn inspect_file(path: impl Into<PathBuf>) -> Result<ReplayInspection, InspectError> {
    let path = path.into();
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(InspectError::InvalidPath)?
        .to_owned();
    let mut file = File::open(&path)?;
    let file_len = file.metadata()?.len();
    let source = Source {
        path: &path,
        file_name: &file_name,
    };
    inspect_reader(&source, &mut file, file_len)
}

fn inspect_reader<R: Read + Seek>(
    source: &Source,
    reader: &mut R,
    file_len: u64,
) -> Result<ReplayInspection, InspectError> {
    let mut head = Vec::with_capacity(CLASSIC_HEADER_LEN);
    reader
        .by_ref()
        .take(CLASSIC_HEADER_LEN as u64)
        .read_to_end(&mut head)?;

    if head.len() < MAGIC_LEN {
        return Ok(source.empty(
            ReplayFormat::Unknown,
            ReplayStatus::Invalid,
            "File is too small to be a ROFL replay".to_string(),
        ));
    }
    if head[..MAGIC_LEN] == *ROFL2_MAGIC {
        return inspect_rofl2(source, reader, file_len, &head);
    }
    if head[..MAGIC_LEN] == *CLASSIC_MAGIC {
        return inspect_classic(source, reader, file_len, &head);
    }
    Ok(source.empty(
        ReplayFormat::Unknown,
        ReplayStatus::Invalid,
        "File does not start with the RIOT replay signature".to_string(),
    ))
}

fn inspect_classic<R: Read + Seek>(
    source: &Source,
    reader: &mut R,
    file_len: u64,
    head: &[u8],
) -> Result<ReplayInspection, InspectError> {
    let invalid =
        |note: String| Ok(source.empty(ReplayFormat::ClassicRofl, ReplayStatus::Invalid, note));
    let header = match parse_classic_header(head) {
        Ok(header) => header,
        Err(reason) => return invalid(format!("Invalid ROFL header: {reason}")),
    };
    let (offset, length) = (
        u64::from(header.metadata_offset),
        u64::from(header.metadata_length),
    );
    if let Err(reason) = check_range(offset, length, file_len, MAX_METADATA_LEN) {
        return invalid(format!("Metadata block {reason}"));
    }
    let metadata = match serde_json::from_slice::<Value>(&read_range(reader, offset, length)?) {
        Ok(value) if value.is_object() => value,
        _ => return invalid("Metadata block is not a JSON object".to_string()),
    };

    let (payload_offset, payload_length) = (
        u64::from(header.payload_header_offset),
        u64::from(header.payload_header_length),
    );
    let payload_match_id = match check_range(payload_offset, payload_length, file_len, u64::MAX) {
        Ok(()) => parse_payload_match_id(&read_range(
            reader,
            payload_offset,
            payload_length.min(PAYLOAD_MATCH_ID_LEN),
        )?),
        Err(_) => None,
    };

    let (file_region, file_match_id) = region_and_match_from_file_name(source.file_name);
    let game_version = get_string(&metadata, &["gameVersion", "GameVersion", "game_version"]);
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
    .and_then(|value| known_platform(&value))
    .or(file_region);
    let match_id = payload_match_id
        .or_else(|| get_string(&metadata, &["gameId", "GameId", "matchId", "MatchId"]))
        .or(file_match_id);

    Ok(parsed_inspection(
        source,
        ReplayFormat::ClassicRofl,
        metadata,
        game_version,
        region,
        match_id,
        Vec::new(),
    ))
}

fn inspect_rofl2<R: Read + Seek>(
    source: &Source,
    reader: &mut R,
    file_len: u64,
    head: &[u8],
) -> Result<ReplayInspection, InspectError> {
    let (header_version, version_note) = match rofl2_header_version(head) {
        Some(version) => (Some(version), None),
        None => (
            find_ascii_version(&head[..head.len().min(128)]),
            Some("ROFL2 version field missing; version guessed from header bytes".to_string()),
        ),
    };
    let (region, file_match_id) = region_and_match_from_file_name(source.file_name);

    let metadata = match read_rofl2_trailer(reader, file_len)? {
        Ok(metadata) => metadata,
        Err(reason) => {
            let mut inspection = source.empty(
                ReplayFormat::Rofl2,
                ReplayStatus::Unsupported,
                format!("ROFL2 trailer metadata unreadable: {reason}"),
            );
            inspection.patch = header_version.as_deref().and_then(patch_from_version);
            inspection.data_dragon_version = inspection.patch.clone();
            inspection.game_version = header_version;
            inspection.suggested_locale = region.as_deref().and_then(suggest_locale_for_region);
            inspection.region = region;
            inspection.match_id = file_match_id;
            return Ok(inspection);
        }
    };

    let match_id =
        get_string(&metadata, &["gameId", "GameId", "matchId", "MatchId"]).or(file_match_id);
    let mut notes = vec!["ROFL2 metadata read from trailer".to_string()];
    notes.extend(version_note);
    Ok(parsed_inspection(
        source,
        ReplayFormat::Rofl2,
        metadata,
        header_version,
        region,
        match_id,
        notes,
    ))
}

fn parsed_inspection(
    source: &Source,
    format: ReplayFormat,
    mut metadata: Value,
    game_version: Option<String>,
    region: Option<String>,
    match_id: Option<String>,
    notes: Vec<String>,
) -> ReplayInspection {
    let patch = game_version.as_deref().and_then(patch_from_version);
    let game_duration_ms = get_u64(
        &metadata,
        &["gameLength", "GameLength", "game_length", "gameDuration"],
    );
    let MatchStats {
        players,
        teams,
        ended_in_surrender,
        ended_in_early_surrender,
    } = match_stats(&metadata);
    strip_stats_json(&mut metadata);
    let (blue_kills, red_kills, winning_team) = team_summary(&players);

    ReplayInspection {
        path: source.path.display().to_string(),
        file_name: source.file_name.to_string(),
        format,
        status: ReplayStatus::Parsed,
        game_version,
        data_dragon_version: patch.clone(),
        patch,
        suggested_locale: region.as_deref().and_then(suggest_locale_for_region),
        region,
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
        notes,
    }
}

/// Reads the ROFL2 trailer (JSON followed by its u32 LE length) without
/// touching the rest of the file. The inner `Err` explains a malformed trailer.
fn read_rofl2_trailer<R: Read + Seek>(
    reader: &mut R,
    file_len: u64,
) -> io::Result<Result<Value, String>> {
    let body_len = file_len.saturating_sub(MAGIC_LEN as u64);
    if body_len < ROFL2_TRAILER_LEN {
        return Ok(Err("file ends before the trailer length".to_string()));
    }
    let length_at = file_len - ROFL2_TRAILER_LEN;
    let mut length_bytes = [0; ROFL2_TRAILER_LEN as usize];
    reader.seek(SeekFrom::Start(length_at))?;
    reader.read_exact(&mut length_bytes)?;
    let length = u64::from(u32::from_le_bytes(length_bytes));
    if length > body_len - ROFL2_TRAILER_LEN {
        return Ok(Err(format!(
            "trailer claims {length} bytes, more than the file holds"
        )));
    }
    let start = length_at - length;
    if let Err(reason) = check_range(start, length, file_len, MAX_METADATA_LEN) {
        return Ok(Err(format!("metadata {reason}")));
    }
    Ok(
        match serde_json::from_slice::<Value>(&read_range(reader, start, length)?) {
            Ok(value) if value.is_object() => Ok(value),
            Ok(_) => Err("metadata is not a JSON object".to_string()),
            Err(err) => Err(format!("metadata is not JSON ({err})")),
        },
    )
}

/// The length-prefixed version string after the ROFL2 magic, e.g. "16.17.810.4348".
fn rofl2_header_version(head: &[u8]) -> Option<String> {
    let length = usize::from(*head.get(ROFL2_VERSION_LEN_OFFSET)?);
    let start = ROFL2_VERSION_LEN_OFFSET + 1;
    let bytes = head.get(start..start + length)?;
    let text = std::str::from_utf8(bytes).ok()?;
    is_version_string(text).then(|| text.to_string())
}

fn is_version_string(text: &str) -> bool {
    text.starts_with(|ch: char| ch.is_ascii_digit())
        && text.contains('.')
        && text.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
}

fn parse_classic_header(bytes: &[u8]) -> Result<ClassicHeader, &'static str> {
    if bytes.len() < CLASSIC_HEADER_LEN {
        return Err("file is shorter than the classic header");
    }
    if bytes[..MAGIC_LEN] != *CLASSIC_MAGIC {
        return Err("missing RIOT classic magic");
    }

    let header = ClassicHeader {
        metadata_offset: u32_at(bytes, 268),
        metadata_length: u32_at(bytes, 272),
        payload_header_offset: u32_at(bytes, 276),
        payload_header_length: u32_at(bytes, 280),
    };
    if header.metadata_length == 0 {
        return Err("metadata length is zero");
    }
    Ok(header)
}

/// Validates an untrusted `(offset, length)` pair before anything is allocated.
fn check_range(offset: u64, length: u64, file_len: u64, cap: u64) -> Result<(), String> {
    if length == 0 {
        return Err("is empty".to_string());
    }
    if length > cap {
        return Err(format!("claims {length} bytes, over the {cap} byte limit"));
    }
    match offset.checked_add(length) {
        Some(end) if end <= file_len => Ok(()),
        _ => Err(format!(
            "at {offset}+{length} extends past the end of the file ({file_len} bytes)"
        )),
    }
}

/// Reads a range that already passed [`check_range`] (so `length` is bounded).
fn read_range<R: Read + Seek>(reader: &mut R, offset: u64, length: u64) -> io::Result<Vec<u8>> {
    let length = usize::try_from(length)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "range too large"))?;
    reader.seek(SeekFrom::Start(offset))?;
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn parse_payload_match_id(bytes: &[u8]) -> Option<String> {
    let id = u64::from_le_bytes(bytes.get(..8)?.try_into().ok()?);
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

fn strip_stats_json(metadata: &mut Value) {
    let Some(object) = metadata.as_object_mut() else {
        return;
    };
    for key in STATS_JSON_KEYS {
        if let Some(value @ Value::String(_)) = object.get_mut(key) {
            *value = Value::String(STATS_PLACEHOLDER.to_string());
        }
    }
}

struct MatchStats {
    players: Vec<PlayerStats>,
    teams: Vec<TeamSummary>,
    ended_in_surrender: bool,
    ended_in_early_surrender: bool,
}

fn match_stats(metadata: &Value) -> MatchStats {
    let rows = STATS_JSON_KEYS
        .iter()
        .find_map(|key| metadata.get(*key).and_then(Value::as_str))
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| match value {
            Value::Array(rows) => Some(rows),
            _ => None,
        })
        .unwrap_or_default();
    let players: Vec<_> = rows
        .iter()
        .enumerate()
        .map(|(index, player)| player_from_stats(index, player))
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

fn saturating_sum(values: impl Iterator<Item = u32>) -> u32 {
    values.fold(0, u32::saturating_add)
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
            let sum = |key: &str| saturating_sum(members.iter().map(|(row, _)| num(row, key)));
            let total = |field: fn(&PlayerStats) -> u32| {
                saturating_sum(members.iter().map(|(_, player)| field(player)))
            };
            Some(TeamSummary {
                team: team.to_string(),
                win: members.iter().any(|(_, player)| player.win),
                kills: total(|player| player.kills),
                deaths: total(|player| player.deaths),
                assists: total(|player| player.assists),
                gold_earned: total(|player| player.gold_earned),
                damage_to_champions: total(|player| player.damage_to_champions),
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

fn player_from_stats(index: usize, player: &Value) -> PlayerStats {
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
        champion,
        team,
        position,
        win,
        kills,
        deaths,
        assists,
        creep_score: minions_killed.saturating_add(neutral_minions_killed),
        gold_earned: num(player, "GOLD_EARNED"),
        damage_to_champions: num(player, "TOTAL_DAMAGE_DEALT_TO_CHAMPIONS"),
        vision_score: num(player, "VISION_SCORE"),
        items: player_items(player),
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

fn player_items(player: &Value) -> Vec<PlayerItem> {
    (0..=6)
        .filter_map(|slot| {
            let id = stat_string(player, &format!("ITEM{slot}"))?;
            (id != "0").then_some(PlayerItem { slot, id })
        })
        .collect()
}

fn team_summary(players: &[PlayerStats]) -> (u32, u32, Option<String>) {
    let kills = |team: &str| {
        saturating_sum(
            players
                .iter()
                .filter(|player| player.team == team)
                .map(|player| player.kills),
        )
    };
    let winning_team = players
        .iter()
        .find(|player| player.win)
        .map(|player| player.team.clone());

    (kills("Blue"), kills("Red"), winning_team)
}

fn stat_string(player: &Value, key: &str) -> Option<String> {
    player.get(key).and_then(|value| match value {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    })
}

/// Numeric stat; ROFL2 stores numbers as strings. Missing, negative or
/// malformed values read as 0; values above `u32::MAX` clamp.
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

/// Fallback for ROFL2 headers without a valid length-prefixed version field:
/// the first run of digits and dots with at least two dots.
fn find_ascii_version(bytes: &[u8]) -> Option<String> {
    bytes
        .split(|byte| !(byte.is_ascii_digit() || *byte == b'.'))
        .filter_map(|run| std::str::from_utf8(run).ok())
        .find(|run| is_version_string(run) && run.matches('.').count() >= 2)
        .map(str::to_string)
}

/// Uppercases `value` and returns it only if it is a known Riot platform id.
fn known_platform(value: &str) -> Option<String> {
    let upper = value.trim().to_ascii_uppercase();
    KNOWN_PLATFORMS.contains(&upper.as_str()).then_some(upper)
}

/// Parses `<anything>-<PLATFORM>-<digits>.rofl` or `<PLATFORM>-<digits>.rofl`
/// (case-insensitive). Returns `(None, None)` unless the platform is known.
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
    let Some((rest, digits)) = stem.rsplit_once('-') else {
        return (None, None);
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return (None, None);
    }
    let platform = rest.rsplit_once('-').map_or(rest, |(_, platform)| platform);
    match known_platform(platform) {
        Some(region) => (Some(region), Some(digits.to_string())),
        None => (None, None),
    }
}

fn suggest_locale_for_region(region: &str) -> Option<String> {
    let locale = match region {
        "BR1" => "pt_BR",
        "EUN1" | "EUW1" => "en_GB",
        "JP1" => "ja_JP",
        "KR" => "ko_KR",
        "LA1" => "es_MX",
        "LA2" => "es_AR",
        "ME1" => "ar_AE",
        "NA1" => "en_US",
        "OC1" => "en_AU",
        "PH2" => "en_PH",
        "RU" => "ru_RU",
        "SG2" => "en_SG",
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

/// Builds a minimal ROFL2 file (magic + 8 bytes + length-prefixed version +
/// JSON trailer) for tests.
#[cfg(test)]
pub(crate) fn rofl2_fixture_bytes(metadata: &Value, version: &str) -> Vec<u8> {
    let raw = metadata.to_string();
    let mut bytes = ROFL2_MAGIC.to_vec();
    bytes.extend_from_slice(b"deadbeef");
    bytes.push(u8::try_from(version.len()).expect("short version"));
    bytes.extend_from_slice(version.as_bytes());
    bytes.extend_from_slice(b"payload");
    bytes.extend_from_slice(raw.as_bytes());
    bytes.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    bytes
}

/// Path of the real replay used by `#[ignore]` tests.
#[cfg(test)]
pub(crate) fn replay_sample_path() -> PathBuf {
    PathBuf::from(
        std::env::var("CHRONORIFT_REPLAY_SAMPLE")
            .expect("set CHRONORIFT_REPLAY_SAMPLE to a real .rofl file to run this ignored test"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn inspect_bytes(file_name: &str, bytes: &[u8]) -> ReplayInspection {
        let path = PathBuf::from(file_name);
        let source = Source {
            path: &path,
            file_name,
        };
        inspect_reader(&source, &mut Cursor::new(bytes), bytes.len() as u64)
            .expect("in-memory inspection never fails on I/O")
    }

    fn classic_header(metadata: (u32, u32), payload: (u32, u32)) -> Vec<u8> {
        let mut bytes = vec![0; CLASSIC_HEADER_LEN];
        bytes[..6].copy_from_slice(CLASSIC_MAGIC);
        bytes[268..272].copy_from_slice(&metadata.0.to_le_bytes());
        bytes[272..276].copy_from_slice(&metadata.1.to_le_bytes());
        bytes[276..280].copy_from_slice(&payload.0.to_le_bytes());
        bytes[280..284].copy_from_slice(&payload.1.to_le_bytes());
        bytes
    }

    #[test]
    fn parses_classic_header_offsets() {
        let bytes = classic_header((300, 48), (400, 8));

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

        assert!(err.contains("missing RIOT classic magic"));
    }

    #[test]
    fn parses_classic_replay_with_metadata_and_payload_id() {
        let metadata = serde_json::json!({
            "gameVersion": "9.3.263.4538",
            "gameLength": 1_500_000,
            "statsJson": serde_json::json!([
                { "SKIN": "Ahri", "TEAM": "100", "CHAMPIONS_KILLED": 3, "WIN": "Win" }
            ]).to_string(),
        })
        .to_string();
        let meta_offset = CLASSIC_HEADER_LEN as u32;
        let payload_offset = meta_offset + metadata.len() as u32;
        let mut bytes = classic_header((meta_offset, metadata.len() as u32), (payload_offset, 34));
        bytes.extend_from_slice(metadata.as_bytes());
        bytes.extend_from_slice(&987_654u64.to_le_bytes());
        bytes.extend_from_slice(&[0; 26]);

        let replay = inspect_bytes("replay-EUW1-1.rofl", &bytes);

        assert_eq!(replay.status, ReplayStatus::Parsed);
        assert_eq!(replay.format, ReplayFormat::ClassicRofl);
        assert_eq!(replay.game_version.as_deref(), Some("9.3.263.4538"));
        assert_eq!(replay.patch.as_deref(), Some("9.3"));
        assert_eq!(replay.data_dragon_version.as_deref(), Some("9.3"));
        assert_eq!(replay.region.as_deref(), Some("EUW1"));
        assert_eq!(replay.suggested_locale.as_deref(), Some("en_GB"));
        assert_eq!(replay.match_id.as_deref(), Some("987654"));
        assert_eq!(replay.players.len(), 1);
        assert_eq!(replay.metadata["statsJson"], STATS_PLACEHOLDER);
        assert_eq!(replay.metadata["gameLength"], 1_500_000);
    }

    #[test]
    fn classic_lengths_are_bounded_before_allocating() {
        // Metadata claims 4 GB: must be rejected from the header alone.
        let huge = classic_header((CLASSIC_HEADER_LEN as u32, u32::MAX), (0, 0));
        let replay = inspect_bytes("huge.rofl", &huge);
        assert_eq!(replay.status, ReplayStatus::Invalid);
        assert_eq!(replay.format, ReplayFormat::ClassicRofl);
        assert!(replay.notes[0].contains("limit"), "{:?}", replay.notes);

        // Under the cap but past the end of the file.
        let past_end = classic_header((u32::MAX - 10, 1024), (0, 0));
        let replay = inspect_bytes("past.rofl", &past_end);
        assert_eq!(replay.status, ReplayStatus::Invalid);
        assert!(
            replay.notes[0].contains("past the end"),
            "{:?}",
            replay.notes
        );

        // Payload header pointing outside the file is ignored, not fatal.
        let metadata = br#"{"gameVersion":"9.1.1.1"}"#;
        let mut bytes = classic_header(
            (CLASSIC_HEADER_LEN as u32, metadata.len() as u32),
            (u32::MAX, u32::MAX),
        );
        bytes.extend_from_slice(metadata);
        let replay = inspect_bytes("ok.rofl", &bytes);
        assert_eq!(replay.status, ReplayStatus::Parsed);
        assert_eq!(replay.match_id, None);
    }

    #[test]
    fn garbage_and_tiny_files_are_invalid_with_a_reason() {
        let tiny = inspect_bytes("tiny.rofl", b"RIO");
        let garbage = inspect_bytes("garbage.rofl", &[0u8; 400]);
        let short_classic = inspect_bytes("short.rofl", b"RIOT\0\0abc");
        let bad_json = {
            let mut bytes = classic_header((CLASSIC_HEADER_LEN as u32, 4), (0, 0));
            bytes.extend_from_slice(b"nope");
            inspect_bytes("bad.rofl", &bytes)
        };

        for replay in [&tiny, &garbage, &short_classic, &bad_json] {
            assert_eq!(replay.status, ReplayStatus::Invalid);
            assert_eq!(replay.notes.len(), 1);
            assert!(!replay.notes[0].is_empty());
        }
        assert_eq!(tiny.format, ReplayFormat::Unknown);
        assert_eq!(garbage.format, ReplayFormat::Unknown);
        assert!(garbage.notes[0].contains("RIOT"));
        assert_eq!(short_classic.format, ReplayFormat::ClassicRofl);
        assert!(bad_json.notes[0].contains("JSON"));
    }

    #[test]
    fn missing_file_is_an_io_error() {
        let err = inspect_file(std::env::temp_dir().join("chronorift-missing-replay.rofl"))
            .expect_err("missing file should fail");
        assert!(matches!(err, InspectError::Io(_)));
    }

    #[test]
    fn reads_rofl2_trailer_without_reading_the_body() {
        let metadata = serde_json::json!({ "gameLength": 5, "lastGameChunkId": 3 });
        let bytes = rofl2_fixture_bytes(&metadata, "16.17.810.4348");

        let value = read_rofl2_trailer(&mut Cursor::new(&bytes), bytes.len() as u64)
            .unwrap()
            .expect("trailer should parse");
        assert_eq!(value, metadata);

        // Trailer length larger than the file: unsupported, not a panic.
        let mut broken = bytes.clone();
        let at = broken.len() - 4;
        broken[at..].copy_from_slice(&u32::MAX.to_le_bytes());
        let reason = read_rofl2_trailer(&mut Cursor::new(&broken), broken.len() as u64)
            .unwrap()
            .expect_err("oversized trailer");
        assert!(reason.contains("more than the file holds"));

        let replay = inspect_bytes("x-LA2-77.rofl", &broken);
        assert_eq!(replay.status, ReplayStatus::Unsupported);
        assert_eq!(replay.format, ReplayFormat::Rofl2);
        assert_eq!(replay.game_version.as_deref(), Some("16.17.810.4348"));
        assert_eq!(replay.region.as_deref(), Some("LA2"));
        assert_eq!(replay.match_id.as_deref(), Some("77"));
        assert!(replay.notes[0].contains("trailer"));

        // Magic only, no room for a trailer.
        let magic_only = inspect_bytes("m.rofl", ROFL2_MAGIC);
        assert_eq!(magic_only.status, ReplayStatus::Unsupported);
    }

    #[test]
    fn reads_rofl2_version_from_length_prefixed_field() {
        let mut head = ROFL2_MAGIC.to_vec();
        head.extend_from_slice(&[0x37, 0x14, 0xcb, 0x24, 0x56, 0x62, 0x63, 0xe2]);
        head.push(14);
        head.extend_from_slice(b"16.17.810.4348");
        head.extend_from_slice(&[1, 0, 0, 0, 2]);
        assert_eq!(
            rofl2_header_version(&head),
            Some("16.17.810.4348".to_string())
        );

        // Length points past the buffer or at non-version bytes: no field.
        let mut truncated = head[..ROFL2_VERSION_LEN_OFFSET].to_vec();
        truncated.push(200);
        assert_eq!(rofl2_header_version(&truncated), None);
        assert_eq!(rofl2_header_version(ROFL2_MAGIC), None);

        // Fallback digit-run scan still finds a version in older layouts.
        assert_eq!(
            find_ascii_version(b"RIOT\x02\x00deadbeef16.13.790.6961{"),
            Some("16.13.790.6961".to_string())
        );
        assert_eq!(find_ascii_version(b"1.2 and 3"), None);
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
        assert_eq!(parse_payload_match_id(&[1, 2, 3]), None);
    }

    #[test]
    fn suggests_locale_from_region_without_forcing_it() {
        assert_eq!(suggest_locale_for_region("LA1"), Some("es_MX".to_string()));
        assert_eq!(suggest_locale_for_region("LA2"), Some("es_AR".to_string()));
        assert_eq!(suggest_locale_for_region("OC1"), Some("en_AU".to_string()));
        assert_eq!(suggest_locale_for_region("PH2"), Some("en_PH".to_string()));
        assert_eq!(suggest_locale_for_region("SG2"), Some("en_SG".to_string()));
        assert_eq!(suggest_locale_for_region("ME1"), Some("ar_AE".to_string()));
        assert_eq!(suggest_locale_for_region("KR"), Some("ko_KR".to_string()));
        assert_eq!(suggest_locale_for_region("EUW1"), Some("en_GB".to_string()));
        assert_eq!(suggest_locale_for_region("UNKNOWN"), None);
        for platform in KNOWN_PLATFORMS {
            assert!(
                suggest_locale_for_region(platform).is_some(),
                "{platform} has no locale"
            );
        }
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
        let bytes = rofl2_fixture_bytes(&metadata, "16.13.790.6961");

        let replay = inspect_bytes("LA2-1604952545.rofl", &bytes);
        let players = &replay.players;

        assert_eq!(replay.status, ReplayStatus::Parsed);
        assert_eq!(replay.game_version.as_deref(), Some("16.13.790.6961"));
        assert_eq!(replay.data_dragon_version.as_deref(), Some("16.13"));
        assert_eq!(replay.game_duration_ms, Some(123000));
        assert_eq!(replay.metadata["statsJson"], STATS_PLACEHOLDER);
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
        assert_eq!((replay.blue_kills, replay.red_kills), (5, 7));
        assert_eq!(replay.winning_team.as_deref(), Some("Blue"));

        assert!(replay.ended_in_surrender);
        assert!(!replay.ended_in_early_surrender);
        assert_eq!(replay.teams.len(), 2);
        let (blue, red) = (&replay.teams[0], &replay.teams[1]);
        assert_eq!((blue.team.as_str(), blue.win), ("Blue", true));
        assert_eq!((blue.kills, blue.deaths, blue.assists), (5, 5, 6));
        assert_eq!(blue.gold_earned, 11428);
        assert_eq!((blue.towers, blue.dragons, blue.barons), (3, 1, 1));
        assert_eq!((blue.voidgrubs, blue.inhibitors), (3, 0));
        assert_eq!((red.team.as_str(), red.win), ("Red", false));
        assert_eq!((red.kills, red.deaths, red.inhibitors), (7, 2, 1));

        let empty = match_stats(&serde_json::json!({}));
        assert!(empty.players.is_empty() && empty.teams.is_empty());
    }

    #[test]
    fn huge_stat_values_saturate_instead_of_panicking() {
        let row = |team: &str| {
            serde_json::json!({
                "TEAM": team,
                "CHAMPIONS_KILLED": "4294967295",
                "GOLD_EARNED": "1e30",
                "MINIONS_KILLED": "4294967295",
                "NEUTRAL_MINIONS_KILLED": "10",
                "TURRETS_KILLED": "-5",
                "DRAGON_KILLS": "4294967295",
            })
        };
        let metadata = serde_json::json!({
            "statsJson": serde_json::json!([row("100"), row("100"), row("200")]).to_string()
        });

        let stats = match_stats(&metadata);

        assert_eq!(stats.players[0].creep_score, u32::MAX);
        assert_eq!(stats.players[0].gold_earned, u32::MAX);
        assert_eq!(stats.teams[0].kills, u32::MAX);
        assert_eq!(stats.teams[0].dragons, u32::MAX);
        assert_eq!(stats.teams[0].towers, 0);
        assert_eq!(team_summary(&stats.players).0, u32::MAX);
    }

    #[test]
    fn extracts_region_and_match_from_file_name() {
        let parse = region_and_match_from_file_name;
        let both = |region: &str, id: &str| (Some(region.to_string()), Some(id.to_string()));
        assert_eq!(parse("LA2-1604952545.rofl"), both("LA2", "1604952545"));
        assert_eq!(
            parse("046a1a6c-LA2-1621247134.rofl"),
            both("LA2", "1621247134")
        );
        assert_eq!(parse("euw1-42.ROFL"), both("EUW1", "42"));
        assert_eq!(parse("my-game-kr-7.rofl"), both("KR", "7"));
        assert_eq!(parse("NA1-222"), both("NA1", "222"));
        assert_eq!(parse("046a1a6c-1621247134.rofl"), (None, None));
        assert_eq!(parse("XX9-123.rofl"), (None, None));
        assert_eq!(parse("LA2-12a.rofl"), (None, None));
        assert_eq!(parse("LA2-.rofl"), (None, None));
        assert_eq!(parse("LA2.rofl"), (None, None));
        assert_eq!(parse("-1.rofl"), (None, None));
        assert_eq!(parse("LA2-1621247134 (1).rofl"), (None, None));
        assert_eq!(parse("ñ.rofl"), (None, None));
        assert_eq!(parse(""), (None, None));
    }

    #[test]
    fn metadata_platform_must_be_known() {
        assert_eq!(known_platform("la2"), Some("LA2".to_string()));
        assert_eq!(known_platform("PBE1"), None);
        assert_eq!(known_platform(""), None);
    }

    #[test]
    #[ignore = "needs CHRONORIFT_REPLAY_SAMPLE"]
    fn parses_real_replay_sample() {
        let replay = inspect_file(replay_sample_path()).expect("real replay should parse");

        assert_eq!(replay.status, ReplayStatus::Parsed);
        assert_eq!(replay.format, ReplayFormat::Rofl2);
        assert_eq!(replay.game_version.as_deref(), Some("16.17.810.4348"));
        assert_eq!(replay.patch.as_deref(), Some("16.17"));
        assert_eq!(replay.data_dragon_version.as_deref(), Some("16.17"));
        assert_eq!(replay.region.as_deref(), Some("LA2"));
        assert_eq!(replay.match_id.as_deref(), Some("1621247134"));
        assert_eq!(replay.suggested_locale.as_deref(), Some("es_AR"));
        assert_eq!(replay.players.len(), 10);
        assert!(replay.game_duration_ms.unwrap_or_default() > 0);
        assert_eq!(replay.teams.len(), 2);
        assert_eq!(replay.teams[0].team, "Blue");
        assert_eq!(replay.teams[0].kills, replay.blue_kills);
        assert_eq!(replay.teams[1].kills, replay.red_kills);
        assert!(replay.teams.iter().any(|team| team.win));
        assert!(replay.players.iter().all(|player| player.puuid.is_some()));
        assert!(replay.players.iter().all(|player| player.level > 0));
        assert_eq!(replay.metadata["statsJson"], STATS_PLACEHOLDER);
        assert!(serde_json::to_string(&replay).unwrap().len() < 64 * 1024);
    }
}
