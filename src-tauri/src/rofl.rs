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
/// Los metadatos reales ocupan ~120 KiB; un valor mayor indica un archivo dañado.
const MAX_METADATA_BYTES: u64 = 64 * 1024 * 1024;
/// El id de la partida ocupa los primeros 8 bytes de la cabecera de la carga útil clásica.
const MATCH_ID_BYTES: u32 = 8;
const POSITIONS: [&str; 5] = ["TOP", "JUNGLE", "MIDDLE", "BOTTOM", "UTILITY"];

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
    pub teams: Vec<TeamSummary>,
    pub metadata: Value,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlayerStats {
    pub name: String,
    /// Riot ID sin la etiqueta; en repeticiones viejas, el nombre de invocador.
    pub game_name: String,
    pub tag_line: String,
    pub champion: String,
    pub champion_image_url: Option<String>,
    pub team: String,
    /// `TOP`, `JUNGLE`, `MIDDLE`, `BOTTOM`, `UTILITY` o vacío (ARAM y otros modos).
    pub position: String,
    pub win: bool,
    pub level: u32,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub creep_score: u32,
    pub gold_earned: u32,
    pub damage_to_champions: u32,
    pub physical_damage_to_champions: u32,
    pub magic_damage_to_champions: u32,
    pub true_damage_to_champions: u32,
    pub damage_taken: u32,
    pub damage_self_mitigated: u32,
    pub heal_shield_on_teammates: u32,
    pub time_dead_seconds: u32,
    pub vision_score: u32,
    pub turret_kills: u32,
    pub dragon_kills: u32,
    pub baron_kills: u32,
    pub herald_kills: u32,
    pub voidgrub_kills: u32,
    pub items: Vec<PlayerItem>,
    pub summoner_spells: Vec<String>,
    pub perks: Vec<String>,
    pub keystone: Option<String>,
    pub primary_style: Option<String>,
    pub sub_style: Option<String>,
}

/// Totales de un equipo al final de la partida.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TeamSummary {
    pub team: String,
    pub win: bool,
    pub kills: u32,
    pub gold_earned: u32,
    pub damage_to_champions: u32,
    pub towers: u32,
    pub inhibitors: u32,
    pub dragons: u32,
    pub barons: u32,
    pub heralds: u32,
    pub voidgrubs: u32,
    pub atakhan: u32,
}

/// Objeto de un jugador. `slot` 0–5 es el inventario, 6 el abalorio y 7 el objeto de la misión
/// de rol (`ROLE_BOUND_ITEM`, desde las misiones de rol de 2026): las botas del tirador, el
/// Centinela de Control del soporte o la recompensa de la misión en las demás posiciones.
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
            Self::InvalidPath => write!(f, "Ruta de repetición no válida"),
            Self::InvalidHeader(reason) => write!(f, "Cabecera ROFL no válida: {reason}"),
            Self::InvalidMetadata(err) => write!(f, "Metadatos ROFL no válidos: {err}"),
            Self::InvalidRofl2MetadataLength => {
                write!(f, "Longitud de metadatos ROFL2 no válida")
            }
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
    let file_len = file.metadata()?.len();
    let mut header_buffer = [0; CLASSIC_HEADER_LEN];
    let read = read_up_to(&mut file, &mut header_buffer)?;
    if read < CLASSIC_MAGIC.len() {
        return Ok(base_inspection(
            &path,
            file_name,
            ReplayFormat::Unknown,
            ReplayStatus::Invalid,
            "Archivo demasiado pequeño para ser una repetición ROFL",
        ));
    }
    let header_bytes = &header_buffer[..read];

    if header_bytes[..ROFL2_MAGIC.len()] == *ROFL2_MAGIC {
        return inspect_rofl2(path, file_name, &mut file, file_len, header_bytes);
    }

    let header = parse_classic_header(header_bytes)?;
    let metadata = read_json_at(
        &mut file,
        file_len,
        header.metadata_offset,
        header.metadata_length,
    )?;
    // De la cabecera de la carga útil solo se usa el id de la partida.
    let payload_header = read_slice_at(
        &mut file,
        file_len,
        header.payload_header_offset,
        header.payload_header_length,
        MATCH_ID_BYTES,
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
    let (players, teams) = match_from_metadata(&metadata, data_dragon_version.as_deref());
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
        teams,
        metadata,
        notes: Vec::new(),
    })
}

/// Lee solo la cabecera y el bloque final de metadatos; la carga útil (15–20 MiB) no se toca.
fn inspect_rofl2(
    path: PathBuf,
    file_name: String,
    file: &mut File,
    file_len: u64,
    head: &[u8],
) -> Result<ReplayInspection, InspectError> {
    let metadata = read_rofl2_metadata(file, file_len)?;
    let game_version = find_ascii_version(&head[..head.len().min(128)]);
    let patch = game_version.as_deref().and_then(patch_from_version);
    let data_dragon_version = patch.as_deref().map(data_dragon_version_from_patch);
    let (region, file_match_id) = region_and_match_from_file_name(&file_name);
    let suggested_locale = region.as_deref().and_then(suggest_locale_for_region);
    let game_duration_ms = get_u64(
        &metadata,
        &["gameLength", "GameLength", "game_length", "gameDuration"],
    );
    let (players, teams) = match_from_metadata(&metadata, data_dragon_version.as_deref());
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
        teams,
        metadata,
        notes: vec!["Metadatos ROFL2 leídos del bloque final".to_string()],
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
        teams: Vec::new(),
        metadata: Value::Null,
        notes: vec![note.into()],
    }
}

fn read_rofl2_metadata<R: Read + Seek>(reader: &mut R, len: u64) -> Result<Value, InspectError> {
    if len < 4 {
        return Err(InspectError::InvalidRofl2MetadataLength);
    }
    reader.seek(SeekFrom::Start(len - 4))?;
    let mut suffix = [0; 4];
    reader.read_exact(&mut suffix)?;
    let metadata_length = u64::from(u32::from_le_bytes(suffix));
    if metadata_length > MAX_METADATA_BYTES {
        return Err(InspectError::InvalidRofl2MetadataLength);
    }
    let metadata_start = len
        .checked_sub(4 + metadata_length)
        .ok_or(InspectError::InvalidRofl2MetadataLength)?;

    reader.seek(SeekFrom::Start(metadata_start))?;
    let mut bytes = vec![0; metadata_length as usize];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(InspectError::InvalidMetadata)
}

/// Como `read`, pero insiste hasta llenar el búfer o llegar al final del archivo.
fn read_up_to<R: Read>(reader: &mut R, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

fn parse_classic_header(bytes: &[u8]) -> Result<ClassicHeader, InspectError> {
    if bytes.len() < CLASSIC_HEADER_LEN {
        return Err(InspectError::InvalidHeader(
            "el archivo es más corto que la cabecera clásica",
        ));
    }

    if bytes[..CLASSIC_MAGIC.len()] != *CLASSIC_MAGIC {
        return Err(InspectError::InvalidHeader("falta la firma RIOT clásica"));
    }

    let metadata_offset = u32_at(bytes, 268);
    let metadata_length = u32_at(bytes, 272);
    let payload_header_offset = u32_at(bytes, 276);
    let payload_header_length = u32_at(bytes, 280);

    if metadata_length == 0 {
        return Err(InspectError::InvalidHeader("metadatos vacíos"));
    }
    // Mismo tope que en ROFL2: un archivo enorme no justifica reservar gigas para el JSON.
    if u64::from(metadata_length) > MAX_METADATA_BYTES {
        return Err(InspectError::InvalidHeader("metadatos demasiado grandes"));
    }

    Ok(ClassicHeader {
        metadata_offset,
        metadata_length,
        payload_header_offset,
        payload_header_length,
    })
}

fn read_json_at(
    file: &mut File,
    file_len: u64,
    offset: u32,
    length: u32,
) -> Result<Value, InspectError> {
    let bytes = read_slice_at(file, file_len, offset, length, length)?;
    serde_json::from_slice(&bytes).map_err(InspectError::InvalidMetadata)
}

/// Lee los primeros `keep` bytes de una sección declarada en la cabecera, después de comprobar
/// que la sección entera cabe en el archivo: nunca reserva más de lo que se usa.
fn read_slice_at(
    file: &mut File,
    file_len: u64,
    offset: u32,
    length: u32,
    keep: u32,
) -> Result<Vec<u8>, InspectError> {
    if u64::from(offset) + u64::from(length) > file_len {
        return Err(InspectError::InvalidHeader("sección fuera del archivo"));
    }
    file.seek(SeekFrom::Start(u64::from(offset)))?;
    let mut bytes = vec![0; length.min(keep) as usize];
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

fn match_from_metadata(
    metadata: &Value,
    data_dragon_version: Option<&str>,
) -> (Vec<PlayerStats>, Vec<TeamSummary>) {
    let stats = get_string(metadata, &["statsJson", "statsJSON"])
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let players = stats
        .iter()
        .map(|player| player_from_stats(player, data_dragon_version))
        .collect::<Vec<_>>();
    let teams = team_summaries(&stats, &players);
    (players, teams)
}

fn player_from_stats(player: &Value, data_dragon_version: Option<&str>) -> PlayerStats {
    let team = team_name(stat_string(player, "TEAM").as_deref());
    let champion = stat_string(player, "SKIN").unwrap_or_default();
    let game_name = stat_string(player, "RIOT_ID_GAME_NAME")
        .or_else(|| stat_string(player, "NAME"))
        .unwrap_or_default();
    let tag_line = stat_string(player, "RIOT_ID_TAG_LINE").unwrap_or_default();
    let name = riot_name(player)
        .or_else(|| stat_string(player, "NAME"))
        .unwrap_or_default();
    // Solo posiciones de la Grieta; en ARAM y otros modos queda vacía en lugar de inventarla.
    let position = ["TEAM_POSITION", "INDIVIDUAL_POSITION"]
        .iter()
        .filter_map(|key| stat_string(player, key))
        .find(|value| POSITIONS.contains(&value.as_str()))
        .unwrap_or_default();
    let win = stat_string(player, "WIN")
        .map(|value| value.eq_ignore_ascii_case("Win"))
        .unwrap_or(false);
    let non_zero = |key: &str| stat_string(player, key).filter(|value| value != "0");

    PlayerStats {
        name,
        game_name,
        tag_line,
        champion: champion.clone(),
        champion_image_url: data_dragon_version
            .filter(|_| !champion.is_empty())
            .map(|version| champion_image_url(version, &champion)),
        team,
        position,
        win,
        level: stat_u32(player, "LEVEL"),
        kills: stat_u32(player, "CHAMPIONS_KILLED"),
        deaths: stat_u32(player, "NUM_DEATHS"),
        assists: stat_u32(player, "ASSISTS"),
        creep_score: stat_u32(player, "MINIONS_KILLED")
            .saturating_add(stat_u32(player, "NEUTRAL_MINIONS_KILLED")),
        gold_earned: stat_u32(player, "GOLD_EARNED"),
        damage_to_champions: stat_u32(player, "TOTAL_DAMAGE_DEALT_TO_CHAMPIONS"),
        physical_damage_to_champions: stat_u32(player, "PHYSICAL_DAMAGE_DEALT_TO_CHAMPIONS"),
        magic_damage_to_champions: stat_u32(player, "MAGIC_DAMAGE_DEALT_TO_CHAMPIONS"),
        true_damage_to_champions: stat_u32(player, "TRUE_DAMAGE_DEALT_TO_CHAMPIONS"),
        damage_taken: stat_u32(player, "TOTAL_DAMAGE_TAKEN"),
        damage_self_mitigated: stat_u32(player, "TOTAL_DAMAGE_SELF_MITIGATED"),
        heal_shield_on_teammates: stat_u32(player, "TOTAL_HEAL_ON_TEAMMATES")
            .saturating_add(stat_u32(player, "TOTAL_DAMAGE_SHIELDED_ON_TEAMMATES")),
        time_dead_seconds: stat_u32(player, "TOTAL_TIME_SPENT_DEAD"),
        vision_score: stat_u32(player, "VISION_SCORE"),
        turret_kills: stat_u32(player, "TURRETS_KILLED"),
        dragon_kills: stat_u32(player, "DRAGON_KILLS"),
        baron_kills: stat_u32(player, "BARON_KILLS"),
        herald_kills: stat_u32(player, "RIFT_HERALD_KILLS"),
        voidgrub_kills: stat_u32(player, "HORDE_KILLS"),
        items: player_items(player, data_dragon_version),
        summoner_spells: ["SUMMONER_SPELL_1", "SUMMONER_SPELL_2"]
            .iter()
            .filter_map(|key| non_zero(key))
            .collect(),
        perks: ["PERK0", "PERK1", "PERK2", "PERK3", "PERK4", "PERK5"]
            .iter()
            .filter_map(|key| non_zero(key))
            .collect(),
        keystone: non_zero("PERK0"),
        primary_style: non_zero("PERK_PRIMARY_STYLE"),
        sub_style: non_zero("PERK_SUB_STYLE"),
    }
}

/// Totales por equipo. Las torres e inhibidores destruidos salen de lo que perdió el rival
/// (`FRIENDLY_*_LOST`, que incluye lo derribado por súbditos); sin esas claves se suman los
/// derribos de los jugadores.
fn team_summaries(stats: &[Value], players: &[PlayerStats]) -> Vec<TeamSummary> {
    let mut teams = Vec::new();
    for (team, opponent) in [("Blue", "Red"), ("Red", "Blue")] {
        let members = stats
            .iter()
            .zip(players)
            .filter(|(_, player)| player.team == team)
            .collect::<Vec<_>>();
        if members.is_empty() {
            continue;
        }
        let lost_by_opponent = |key: &str| {
            stats
                .iter()
                .zip(players)
                .filter(|(_, player)| player.team == opponent)
                .filter_map(|(raw, _)| stat_string(raw, key)?.parse::<u32>().ok())
                .max()
        };
        let sum = |value: fn(&Value, &PlayerStats) -> u32| {
            saturating_sum(members.iter().map(|(raw, player)| value(raw, player)))
        };
        teams.push(TeamSummary {
            team: team.to_string(),
            win: members.iter().any(|(_, player)| player.win),
            kills: sum(|_, player| player.kills),
            gold_earned: sum(|_, player| player.gold_earned),
            damage_to_champions: sum(|_, player| player.damage_to_champions),
            towers: lost_by_opponent("FRIENDLY_TURRET_LOST")
                .unwrap_or_else(|| sum(|_, player| player.turret_kills)),
            inhibitors: lost_by_opponent("FRIENDLY_DAMPEN_LOST")
                .unwrap_or_else(|| sum(|raw, _| stat_u32(raw, "BARRACKS_KILLED"))),
            dragons: sum(|_, player| player.dragon_kills),
            barons: sum(|_, player| player.baron_kills),
            heralds: sum(|_, player| player.herald_kills),
            voidgrubs: sum(|_, player| player.voidgrub_kills),
            atakhan: sum(|raw, _| stat_u32(raw, "ATAKHAN_KILLS")),
        });
    }
    teams
}

fn player_items(player: &Value, data_dragon_version: Option<&str>) -> Vec<PlayerItem> {
    (0..=7)
        .filter_map(|slot| {
            let key = match slot {
                7 => "ROLE_BOUND_ITEM".to_string(),
                _ => format!("ITEM{slot}"),
            };
            let id = stat_string(player, &key)?;
            (id != "0").then(|| PlayerItem {
                slot,
                image_url: data_dragon_version.map(|version| item_image_url(version, &id)),
                id,
            })
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
    let (blue_kills, red_kills) = (kills("Blue"), kills("Red"));
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

/// Suma sin desbordar: un archivo dañado puede traer valores cercanos a `u32::MAX`.
fn saturating_sum(values: impl Iterator<Item = u32>) -> u32 {
    values.fold(0, u32::saturating_add)
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
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(file_name);
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

/// Utilidades de prueba compartidas con `library`: repeticiones sintéticas con el formato real.
#[cfg(test)]
pub(crate) mod test_support {
    use serde_json::{json, Value};
    use std::path::Path;

    /// Escribe un ROFL2 como los de Riot: firma, versión con prefijo de longitud, carga útil
    /// opaca y metadatos JSON al final seguidos de su longitud.
    pub(crate) fn write_rofl2(path: &Path, version: &str, metadata: &Value) {
        let mut bytes = b"RIOT\x02\x00".to_vec();
        bytes.extend_from_slice(&[0x37, 0x14, 0xCB, 0x24, 0x56, 0x62, 0x63, 0xE2]);
        bytes.push(version.len() as u8);
        bytes.extend_from_slice(version.as_bytes());
        bytes.extend_from_slice(&[0x01, 0, 0, 0, 0x02, 0, 0, 0]);
        bytes.extend_from_slice(&[0xA5; 512]);
        let raw = metadata.to_string();
        bytes.extend_from_slice(raw.as_bytes());
        bytes.extend_from_slice(&(raw.len() as u32).to_le_bytes());
        std::fs::write(path, bytes).unwrap();
    }

    /// Metadatos con el esquema del bloque final real (`statsJson` es texto JSON).
    pub(crate) fn metadata(game_length: u64, players: &[Value]) -> Value {
        json!({
            "gameLength": game_length,
            "lastGameChunkId": 92,
            "lastKeyFrameId": 45,
            "statsJson": Value::Array(players.to_vec()).to_string(),
        })
    }

    /// Estadísticas de un jugador con las claves y los valores de texto que usa Riot.
    pub(crate) fn player(
        riot_id: (&str, &str),
        champion: &str,
        team: &str,
        position: &str,
        kda: [u32; 3],
        win: bool,
    ) -> Value {
        let result = if win { "Win" } else { "Fail" };
        // Lo perdido por cada equipo se repite en todos sus jugadores, como en Riot.
        let (turrets_lost, inhibitors_lost) = if team == "100" {
            ("3", "0")
        } else {
            ("7", "2")
        };
        let mut stats = json!({
            "RIOT_ID_GAME_NAME": riot_id.0,
            "RIOT_ID_TAG_LINE": riot_id.1,
            "NAME": "",
            "SKIN": champion,
            "TEAM": team,
            "TEAM_POSITION": position,
            "INDIVIDUAL_POSITION": "JUNGLE",
            "CHAMPIONS_KILLED": kda[0].to_string(),
            "NUM_DEATHS": kda[1].to_string(),
            "ASSISTS": kda[2].to_string(),
            "MINIONS_KILLED": "200",
            "NEUTRAL_MINIONS_KILLED": "3",
            "GOLD_EARNED": "10428",
            "TOTAL_DAMAGE_DEALT_TO_CHAMPIONS": "20353",
            "VISION_SCORE": "14",
            "LEVEL": "18",
            "ITEM0": "3508",
            "ITEM1": "0",
            "ITEM2": "3158",
            "ITEM3": "0",
            "ITEM4": "0",
            "ITEM5": "0",
            "ITEM6": "3340",
            "SUMMONER_SPELL_1": "4",
            "SUMMONER_SPELL_2": "14",
            "PERK0": "8005",
            "PERK1": "9111",
            "PERK2": "9105",
            "PERK3": "8299",
            "PERK4": "8473",
            "PERK5": "8451",
            "WIN": result,
        });
        // En dos partes: un solo `json!` con todas las claves supera el límite de recursión.
        let extended = json!({
            "PHYSICAL_DAMAGE_DEALT_TO_CHAMPIONS": "15000",
            "MAGIC_DAMAGE_DEALT_TO_CHAMPIONS": "4000",
            "TRUE_DAMAGE_DEALT_TO_CHAMPIONS": "1353",
            "TOTAL_DAMAGE_TAKEN": "30100",
            "TOTAL_DAMAGE_SELF_MITIGATED": "25200",
            "TOTAL_HEAL_ON_TEAMMATES": "100",
            "TOTAL_DAMAGE_SHIELDED_ON_TEAMMATES": "50",
            "TOTAL_TIME_SPENT_DEAD": "240",
            "TURRETS_KILLED": "1",
            "DRAGON_KILLS": "1",
            "BARON_KILLS": "0",
            "RIFT_HERALD_KILLS": "0",
            "HORDE_KILLS": "3",
            "ATAKHAN_KILLS": "0",
            "FRIENDLY_TURRET_LOST": turrets_lost,
            "FRIENDLY_DAMPEN_LOST": inhibitors_lost,
            "PERK_PRIMARY_STYLE": "8000",
            "PERK_SUB_STYLE": "8400",
        });
        if let (Some(base), Value::Object(extra)) = (stats.as_object_mut(), extended) {
            base.extend(extra);
        }
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{metadata, player, write_rofl2};
    use super::*;
    use serde_json::json;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_directory(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("chronorift-{label}-{unique}"));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn inspects_a_rofl2_replay_end_to_end() {
        let root = temp_directory("rofl2");
        // Extensión en mayúsculas: la región y la partida se leen igual del nombre.
        let path = root.join("la2-1604952545.ROFL");
        let mut players = [
            player(
                ("PaulPNS", "2406"),
                "Urgot",
                "100",
                "TOP",
                [7, 12, 15],
                true,
            ),
            player(
                ("H0ЯR0ЯDЯФNΞ", "8878"),
                "MonkeyKing",
                "100",
                "",
                [23, 7, 8],
                true,
            ),
            player(
                ("Juanie7", "LAS"),
                "Nautilus",
                "200",
                "UTILITY",
                [2, 12, 26],
                false,
            ),
        ];
        // Como en ARAM: sin posición de la Grieta no se inventa una.
        players[2]["TEAM_POSITION"] = json!("");
        players[2]["INDIVIDUAL_POSITION"] = json!("Invalid");
        // Misión de rol: Urgot la tiene, Wukong no trae la clave y Nautilus la trae vacía ("0").
        players[0]["ROLE_BOUND_ITEM"] = json!("3020");
        players[2]["ROLE_BOUND_ITEM"] = json!("0");
        write_rofl2(&path, "16.17.810.4348", &metadata(2_698_573, &players));

        let replay = inspect_file(&path).unwrap();

        assert_eq!(replay.format, ReplayFormat::Rofl2);
        assert_eq!(replay.status, ReplayStatus::Parsed);
        assert_eq!(replay.game_version.as_deref(), Some("16.17.810.4348"));
        assert_eq!(replay.patch.as_deref(), Some("16.17"));
        assert_eq!(replay.data_dragon_version.as_deref(), Some("16.17.1"));
        assert_eq!(replay.region.as_deref(), Some("LA2"));
        assert_eq!(replay.match_id.as_deref(), Some("1604952545"));
        assert_eq!(replay.suggested_locale.as_deref(), Some("es_MX"));
        assert_eq!(replay.game_duration_ms, Some(2_698_573));
        assert_eq!(replay.metadata["lastGameChunkId"], 92);
        assert_eq!((replay.blue_kills, replay.red_kills), (30, 2));
        assert_eq!(replay.winning_team.as_deref(), Some("Blue"));

        let [urgot, wukong, nautilus] = replay.players.as_slice() else {
            panic!("expected three players");
        };
        assert_eq!(urgot.name, "PaulPNS#2406");
        assert_eq!((urgot.kills, urgot.deaths, urgot.assists), (7, 12, 15));
        assert_eq!(urgot.creep_score, 203);
        assert_eq!(
            (
                urgot.gold_earned,
                urgot.damage_to_champions,
                urgot.vision_score
            ),
            (10_428, 20_353, 14)
        );
        assert_eq!(
            urgot
                .items
                .iter()
                .map(|item| (item.slot, item.id.as_str()))
                .collect::<Vec<_>>(),
            [(0, "3508"), (2, "3158"), (6, "3340"), (7, "3020")]
        );
        assert_eq!(
            urgot.items[0].image_url.as_deref(),
            Some("https://ddragon.leagueoflegends.com/cdn/16.17.1/img/item/3508.png")
        );
        assert_eq!(
            urgot.champion_image_url.as_deref(),
            Some("https://ddragon.leagueoflegends.com/cdn/16.17.1/img/champion/Urgot.png")
        );
        assert_eq!(urgot.summoner_spells, ["4", "14"]);
        assert_eq!(urgot.perks.len(), 6);
        assert_eq!(
            (
                urgot.game_name.as_str(),
                urgot.tag_line.as_str(),
                urgot.level
            ),
            ("PaulPNS", "2406", 18)
        );
        assert_eq!(
            (
                urgot.physical_damage_to_champions,
                urgot.magic_damage_to_champions,
                urgot.true_damage_to_champions
            ),
            (15_000, 4_000, 1_353)
        );
        assert_eq!(
            (
                urgot.damage_taken,
                urgot.damage_self_mitigated,
                urgot.heal_shield_on_teammates,
                urgot.time_dead_seconds
            ),
            (30_100, 25_200, 150, 240)
        );
        assert_eq!(
            (
                urgot.keystone.as_deref(),
                urgot.primary_style.as_deref(),
                urgot.sub_style.as_deref()
            ),
            (Some("8005"), Some("8000"), Some("8400"))
        );
        assert_eq!(wukong.name, "H0ЯR0ЯDЯФNΞ#8878");
        assert_eq!(
            wukong.position, "JUNGLE",
            "an empty TEAM_POSITION falls back to INDIVIDUAL_POSITION"
        );
        assert_eq!((nautilus.team.as_str(), nautilus.win), ("Red", false));
        assert_eq!(nautilus.position, "");
        for player in [wukong, nautilus] {
            assert!(
                player.items.iter().all(|item| item.slot != 7),
                "no role item without a ROLE_BOUND_ITEM value"
            );
        }

        // Torres e inhibidores destruidos = lo que perdió el rival.
        let [blue, red] = replay.teams.as_slice() else {
            panic!("expected two teams");
        };
        assert_eq!(
            (
                blue.team.as_str(),
                blue.win,
                blue.kills,
                blue.towers,
                blue.inhibitors
            ),
            ("Blue", true, 30, 7, 2)
        );
        assert_eq!(
            (
                red.team.as_str(),
                red.win,
                red.kills,
                red.towers,
                red.inhibitors
            ),
            ("Red", false, 2, 3, 0)
        );
        assert_eq!(
            (blue.gold_earned, blue.dragons, blue.voidgrubs, red.dragons),
            (20_856, 2, 6, 1)
        );

        // Un archivo dañado con valores al borde de `u32` satura las sumas en lugar de desbordar.
        players[0]["MINIONS_KILLED"] = json!(u32::MAX.to_string());
        players[1]["CHAMPIONS_KILLED"] = json!(u32::MAX.to_string());
        write_rofl2(&path, "16.17.810.4348", &metadata(2_698_573, &players));
        let saturated = inspect_file(&path).unwrap();
        assert_eq!(saturated.players[0].creep_score, u32::MAX);
        assert_eq!(
            (saturated.blue_kills, saturated.teams[0].kills),
            (u32::MAX, u32::MAX)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inspects_classic_replays_and_rejects_corrupt_or_foreign_files() {
        let root = temp_directory("classic");
        let classic_metadata = json!({
            "gameVersion": "10.10.320.3039",
            "platformId": "euw1",
            "gameLength": 1_800_000,
            "statsJson": json!([
                player(("Classic", "EUW"), "Ahri", "100", "MIDDLE", [5, 1, 9], true)
            ])
            .to_string(),
        })
        .to_string();
        let metadata_offset = CLASSIC_HEADER_LEN as u32;
        let payload_offset = metadata_offset + classic_metadata.len() as u32;
        let mut bytes = vec![0; CLASSIC_HEADER_LEN];
        bytes[..6].copy_from_slice(CLASSIC_MAGIC);
        bytes[268..272].copy_from_slice(&metadata_offset.to_le_bytes());
        bytes[272..276].copy_from_slice(&(classic_metadata.len() as u32).to_le_bytes());
        bytes[276..280].copy_from_slice(&payload_offset.to_le_bytes());
        bytes[280..284].copy_from_slice(&8_u32.to_le_bytes());
        bytes.extend_from_slice(classic_metadata.as_bytes());
        bytes.extend_from_slice(&4_567_890_123_u64.to_le_bytes());
        let classic = root.join("classic.rofl");
        fs::write(&classic, &bytes).unwrap();

        let replay = inspect_file(&classic).unwrap();
        assert_eq!(replay.format, ReplayFormat::ClassicRofl);
        assert_eq!(replay.game_version.as_deref(), Some("10.10.320.3039"));
        assert_eq!(replay.patch.as_deref(), Some("10.10"));
        assert_eq!(replay.region.as_deref(), Some("EUW1"));
        assert_eq!(replay.suggested_locale.as_deref(), Some("en_GB"));
        assert_eq!(replay.match_id.as_deref(), Some("4567890123"));
        assert_eq!(replay.game_duration_ms, Some(1_800_000));
        assert_eq!(replay.players.len(), 1);

        // Una longitud enorme o fuera del archivo es un error, no una reserva de 4 GiB.
        for length in [u32::MAX, bytes.len() as u32] {
            bytes[272..276].copy_from_slice(&length.to_le_bytes());
            fs::write(&classic, &bytes).unwrap();
            assert!(
                matches!(inspect_file(&classic), Err(InspectError::InvalidHeader(_))),
                "{length}"
            );
        }

        let tiny = root.join("tiny.rofl");
        fs::write(&tiny, b"bad").unwrap();
        let tiny = inspect_file(&tiny).unwrap();
        assert_eq!(tiny.format, ReplayFormat::Unknown);
        assert_eq!(tiny.status, ReplayStatus::Invalid);

        let foreign = root.join("foreign.rofl");
        fs::write(&foreign, vec![0x42; 400]).unwrap();
        assert!(matches!(
            inspect_file(&foreign),
            Err(InspectError::InvalidHeader(_))
        ));

        let broken_trailer = root.join("LA2-1.rofl");
        let mut rofl2 = b"RIOT\x02\x00".to_vec();
        rofl2.extend_from_slice(&[0; 64]);
        rofl2.extend_from_slice(&u32::MAX.to_le_bytes());
        fs::write(&broken_trailer, rofl2).unwrap();
        assert!(matches!(
            inspect_file(&broken_trailer),
            Err(InspectError::InvalidRofl2MetadataLength)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    /// Con CHRONORIFT_REPLAY_DIRECTORY: contrasta cada repetición real con una lectura
    /// independiente del bloque final y comprueba que inspeccionar no modifica la carpeta.
    #[test]
    fn real_replays_match_an_independent_read_and_stay_untouched() {
        let Ok(directory) = std::env::var("CHRONORIFT_REPLAY_DIRECTORY") else {
            eprintln!("skipped: set CHRONORIFT_REPLAY_DIRECTORY to a folder with real .rofl files");
            return;
        };
        let directory = PathBuf::from(directory);
        let before = snapshot(&directory);

        let (library, _) =
            crate::library::scan_folder(&directory).expect("replay folder should scan");
        assert!(library.failures.is_empty(), "{:?}", library.failures);
        assert!(
            !library.entries.is_empty(),
            "no .rofl files in {}",
            directory.display()
        );

        for entry in &library.entries {
            let path = PathBuf::from(&entry.path);
            let context = path.display().to_string();
            let replay = inspect_file(&path).unwrap_or_else(|error| panic!("{context}: {error}"));
            let bytes = fs::read(&path).unwrap();
            if bytes[..6] != *ROFL2_MAGIC {
                continue;
            }
            // Lectura independiente: versión con prefijo de longitud en el byte 14 y JSON final.
            let version_len = usize::from(bytes[14]);
            let version = std::str::from_utf8(&bytes[15..15 + version_len]).unwrap();
            let trailer_len =
                u32::from_le_bytes(bytes[bytes.len() - 4..].try_into().unwrap()) as usize;
            let trailer: Value =
                serde_json::from_slice(&bytes[bytes.len() - 4 - trailer_len..bytes.len() - 4])
                    .unwrap();
            let stats: Vec<Value> =
                serde_json::from_str(trailer["statsJson"].as_str().unwrap()).unwrap();
            let stem = entry.file_name.trim_end_matches(".rofl");
            let (region, match_id) = stem.split_once('-').unwrap();

            assert_eq!(replay.game_version.as_deref(), Some(version), "{context}");
            assert_eq!(replay.patch, entry.patch, "{context}");
            assert_eq!(replay.region.as_deref(), Some(region), "{context}");
            assert_eq!(replay.match_id.as_deref(), Some(match_id), "{context}");
            assert_eq!(
                replay.game_duration_ms,
                trailer["gameLength"].as_u64(),
                "{context}"
            );
            assert_eq!(entry.game_duration_ms, replay.game_duration_ms, "{context}");
            assert_eq!(replay.players.len(), stats.len(), "{context}");
            assert_eq!(entry.player_count, stats.len(), "{context}");

            let mut kills = (0, 0);
            let mut winners = Vec::new();
            for (player, raw) in replay.players.iter().zip(&stats) {
                let text = |key: &str| {
                    raw.get(key)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                let number = |key: &str| text(key).parse::<u32>().unwrap_or_default();
                let list = |keys: &[String]| {
                    keys.iter()
                        .map(|key| text(key))
                        .filter(|value| !value.is_empty() && value != "0")
                        .collect::<Vec<_>>()
                };
                let team = match text("TEAM").as_str() {
                    "100" => "Blue".to_string(),
                    "200" => "Red".to_string(),
                    other => other.to_string(),
                };
                let item_keys = (0..=6)
                    .map(|slot| format!("ITEM{slot}"))
                    .chain(["ROLE_BOUND_ITEM".to_string()])
                    .collect::<Vec<_>>();
                let perk_keys = (0..6).map(|slot| format!("PERK{slot}")).collect::<Vec<_>>();
                let spell_keys = [
                    "SUMMONER_SPELL_1".to_string(),
                    "SUMMONER_SPELL_2".to_string(),
                ];

                assert_eq!(
                    player.name,
                    format!("{}#{}", text("RIOT_ID_GAME_NAME"), text("RIOT_ID_TAG_LINE")),
                    "{context}"
                );
                assert_eq!(player.champion, text("SKIN"), "{context}");
                assert_eq!(player.team, team, "{context}");
                assert_eq!(
                    (player.kills, player.deaths, player.assists),
                    (
                        number("CHAMPIONS_KILLED"),
                        number("NUM_DEATHS"),
                        number("ASSISTS")
                    ),
                    "{context}"
                );
                assert_eq!(
                    player.creep_score,
                    number("MINIONS_KILLED") + number("NEUTRAL_MINIONS_KILLED"),
                    "{context}"
                );
                assert_eq!(
                    (
                        player.gold_earned,
                        player.damage_to_champions,
                        player.vision_score
                    ),
                    (
                        number("GOLD_EARNED"),
                        number("TOTAL_DAMAGE_DEALT_TO_CHAMPIONS"),
                        number("VISION_SCORE")
                    ),
                    "{context}"
                );
                assert_eq!(
                    player
                        .items
                        .iter()
                        .map(|item| item.id.clone())
                        .collect::<Vec<_>>(),
                    list(&item_keys),
                    "{context}"
                );
                assert_eq!(player.summoner_spells, list(&spell_keys), "{context}");
                assert_eq!(player.perks, list(&perk_keys), "{context}");
                assert_eq!(player.win, text("WIN") == "Win", "{context}");
                assert_eq!(
                    (
                        player.game_name.as_str(),
                        player.tag_line.as_str(),
                        player.level
                    ),
                    (
                        text("RIOT_ID_GAME_NAME").as_str(),
                        text("RIOT_ID_TAG_LINE").as_str(),
                        number("LEVEL")
                    ),
                    "{context}"
                );
                assert_eq!(
                    [
                        player.physical_damage_to_champions,
                        player.magic_damage_to_champions,
                        player.true_damage_to_champions,
                        player.damage_taken,
                        player.damage_self_mitigated,
                        player.time_dead_seconds,
                    ],
                    [
                        number("PHYSICAL_DAMAGE_DEALT_TO_CHAMPIONS"),
                        number("MAGIC_DAMAGE_DEALT_TO_CHAMPIONS"),
                        number("TRUE_DAMAGE_DEALT_TO_CHAMPIONS"),
                        number("TOTAL_DAMAGE_TAKEN"),
                        number("TOTAL_DAMAGE_SELF_MITIGATED"),
                        number("TOTAL_TIME_SPENT_DEAD"),
                    ],
                    "{context}"
                );
                assert_eq!(
                    (
                        player.keystone.clone().unwrap_or_default(),
                        player.sub_style.clone().unwrap_or_default()
                    ),
                    (text("PERK0"), text("PERK_SUB_STYLE")),
                    "{context}"
                );

                match player.team.as_str() {
                    "Blue" => kills.0 += player.kills,
                    "Red" => kills.1 += player.kills,
                    _ => {}
                }
                if player.win {
                    winners.push(player.team.clone());
                }
            }
            assert_eq!((replay.blue_kills, replay.red_kills), kills, "{context}");
            assert_eq!((entry.blue_kills, entry.red_kills), kills, "{context}");
            assert_eq!(replay.winning_team, winners.first().cloned(), "{context}");
            assert_eq!(entry.players.len(), stats.len(), "{context}");
            for team in &replay.teams {
                let opponent = if team.team == "Blue" { "200" } else { "100" };
                let lost = |key: &str| {
                    stats
                        .iter()
                        .filter(|raw| raw["TEAM"] == opponent)
                        .filter_map(|raw| raw[key].as_str()?.parse::<u32>().ok())
                        .max()
                        .unwrap_or_default()
                };
                assert_eq!(
                    (team.towers, team.inhibitors),
                    (lost("FRIENDLY_TURRET_LOST"), lost("FRIENDLY_DAMPEN_LOST")),
                    "{context}"
                );
            }
        }

        assert_eq!(
            snapshot(&directory),
            before,
            "inspection must not modify the replay folder"
        );
    }

    /// Nombre, tamaño y fecha de modificación de cada entrada de la carpeta.
    fn snapshot(directory: &Path) -> Vec<(String, u64, Option<SystemTime>)> {
        let mut entries = fs::read_dir(directory)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| {
                let info = entry.metadata().unwrap();
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    info.len(),
                    info.modified().ok(),
                )
            })
            .collect::<Vec<_>>();
        entries.sort();
        entries
    }
}
