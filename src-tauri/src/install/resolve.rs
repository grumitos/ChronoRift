//! Finds a downloadable `lol-game-client` manifest for a replay build.
//!
//! Order: the community archive (github.com/Morilli/riot-manifests) for the exact build, then
//! other archived builds of the same patch (newest first), then Riot Sieve, which only knows
//! the current patch. A manifest that answers 403/404 is skipped. The result always says
//! where it came from and whether it is the exact build.

use super::{
    error::InstallError,
    http::{check_cancel, Fetch, Fetched},
    progress::{Progress, Stage, Update},
};
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::atomic::AtomicBool,
};

pub const REGIONS: [&str; 17] = [
    "BR1", "EUN1", "EUW1", "JP1", "KR", "LA1", "LA2", "ME1", "NA1", "OC1", "PH2", "RU", "SG2",
    "TH2", "TR1", "TW2", "VN2",
];
/// The game client is the same in every region; these dirs have the most complete history.
const FALLBACK_REGIONS: [&str; 2] = ["NA1", "EUW1"];
const ARCHIVE_RAW: &str = "https://raw.githubusercontent.com/Morilli/riot-manifests/master/LoL";
const ARCHIVE_API: &str = "https://api.github.com/repos/Morilli/riot-manifests/contents/LoL";
const SIEVE: &str = "https://sieve.services.riotcdn.net/api/v1/products/lol/version-sets";
const ARTIFACT: &str = "lol-game-client";
pub const TEXT_LIMIT: usize = 1 << 20;
pub const MANIFEST_LIMIT: usize = 64 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Source {
    Archive,
    Sieve,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveRequest {
    /// Validated server, e.g. `LA2`.
    pub region: String,
    /// `major.minor`, e.g. `16.17`.
    pub patch: String,
    /// Archive-style build, e.g. `16.17.8104348`, when the replay says which build it was.
    pub version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Resolved {
    pub source: Source,
    /// Which region's listing the manifest URL came from.
    pub manifest_region: String,
    /// Archive-style build that was resolved.
    pub version: String,
    pub manifest_url: String,
    pub exact_build: bool,
    pub manifest: Vec<u8>,
}

/// Uppercases and checks the server against [`REGIONS`]. Never defaults.
pub fn normalize_region(region: Option<&str>) -> Result<String, InstallError> {
    let Some(region) = region.map(str::trim).filter(|value| !value.is_empty()) else {
        return Err(InstallError::InvalidRegion(None));
    };
    let upper = region.to_ascii_uppercase();
    if REGIONS.contains(&upper.as_str()) {
        Ok(upper)
    } else {
        Err(InstallError::InvalidRegion(Some(region.to_string())))
    }
}

/// `16.17.810.4348` (replay) or `16.17.8104348+branch…` (Riot) → `16.17.8104348`.
pub fn archive_name(game_version: &str) -> Option<String> {
    let version = game_version.trim().split('+').next()?.trim();
    let parts = version.split('.').collect::<Vec<_>>();
    if parts
        .iter()
        .any(|part| part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return None;
    }
    match parts.as_slice() {
        [major, minor, build] => Some(format!("{major}.{minor}.{build}")),
        [major, minor, a, b] => Some(format!("{major}.{minor}.{a}{b}")),
        _ => None,
    }
}

/// `16.17`, `16.17.1` or a full version → `16.17`.
pub fn normalize_patch(patch: &str) -> Option<String> {
    let mut parts = patch.trim().split('+').next()?.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    let numeric = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    (numeric(major) && numeric(minor)).then(|| format!("{major}.{minor}"))
}

fn build_number(name: &str) -> Option<u64> {
    name.split('.').nth(2)?.parse().ok()
}

fn region_order(region: &str) -> Vec<&str> {
    let mut regions = vec![region];
    for fallback in FALLBACK_REGIONS {
        if !regions.contains(&fallback) {
            regions.push(fallback);
        }
    }
    regions
}

fn archive_file_url(region: &str, name: &str) -> String {
    format!("{ARCHIVE_RAW}/{region}/windows/{ARTIFACT}/{name}.txt")
}

fn archive_listing_url(region: &str) -> String {
    format!("{ARCHIVE_API}/{region}/windows/{ARTIFACT}")
}

fn sieve_url(region: &str) -> String {
    format!("{SIEVE}/{region}?q[platform]=windows")
}

/// Accepts only `https://*.riotcdn.net/**.manifest`; the archive is third-party data.
pub fn valid_manifest_url(text: &str) -> Option<String> {
    let url = Url::parse(text.trim()).ok()?;
    let host = url.host_str()?;
    (url.scheme() == "https"
        && (host == "riotcdn.net" || host.ends_with(".riotcdn.net"))
        && url.path().ends_with(".manifest")
        && url.path().contains("/releases/"))
    .then(|| url.to_string())
}

/// Same-patch archive names, newest build first.
fn listing_candidates(listing: &[u8], patch: &str) -> Result<Vec<String>, InstallError> {
    #[derive(Deserialize)]
    struct Entry {
        name: String,
    }
    let entries: Vec<Entry> = serde_json::from_slice(listing)
        .map_err(|err| InstallError::Network(format!("archive listing: {err}")))?;
    let prefix = format!("{patch}.");
    let mut names = entries
        .into_iter()
        .filter_map(|entry| entry.name.strip_suffix(".txt").map(str::to_string))
        .filter(|name| name.starts_with(&prefix) && archive_name(name).as_deref() == Some(name))
        .collect::<Vec<_>>();
    names.sort_by_key(|name| std::cmp::Reverse(build_number(name).unwrap_or(0)));
    Ok(names)
}

struct Resolver<'a, F: Fetch> {
    fetch: &'a F,
    cancel: &'a AtomicBool,
    progress: &'a dyn Progress,
    tried: HashSet<String>,
    notes: Vec<String>,
}

impl<F: Fetch> Resolver<'_, F> {
    async fn get(&self, url: &str, limit: usize) -> Result<Fetched, InstallError> {
        self.fetch.get(url, limit, self.cancel, &|_, _| {}).await
    }

    /// `Ok(None)` for 404; other failures are errors.
    async fn text(&self, url: &str) -> Result<Option<Vec<u8>>, InstallError> {
        let fetched = self.get(url, TEXT_LIMIT).await?;
        match fetched.status {
            404 => Ok(None),
            _ if fetched.is_success() => Ok(Some(fetched.body)),
            status => Err(InstallError::Network(format!(
                "{url} returned HTTP {status}"
            ))),
        }
    }

    /// Downloads the manifest; `Ok(None)` when Riot no longer serves it.
    async fn manifest(
        &mut self,
        version: &str,
        url: &str,
    ) -> Result<Option<Vec<u8>>, InstallError> {
        if !self.tried.insert(url.to_string()) {
            return Ok(None);
        }
        let progress = self.progress;
        self.progress.report(Update::stage(
            Stage::Manifest,
            format!("Downloading manifest for {version}"),
        ));
        let detail = format!("Downloading manifest for {version}");
        let fetched = self
            .fetch
            .get(url, MANIFEST_LIMIT, self.cancel, &|done, total| {
                progress.report(Update::stage(Stage::Manifest, detail.clone()).bytes(done, total));
            })
            .await?;
        if fetched.is_success() {
            return Ok(Some(fetched.body));
        }
        self.notes.push(format!(
            "{version} manifest returned HTTP {}",
            fetched.status
        ));
        Ok(None)
    }

    /// Reads one archive entry and tries its manifest.
    async fn archive_entry(
        &mut self,
        region: &str,
        name: &str,
    ) -> Result<Option<(String, Vec<u8>)>, InstallError> {
        let Some(text) = self.text(&archive_file_url(region, name)).await? else {
            return Ok(None);
        };
        let Some(url) = valid_manifest_url(&String::from_utf8_lossy(&text)) else {
            self.notes.push(format!(
                "archive entry {region}/{name} is not a Riot manifest URL"
            ));
            return Ok(None);
        };
        Ok(self.manifest(name, &url).await?.map(|bytes| (url, bytes)))
    }

    async fn search_archive(
        &mut self,
        request: &ResolveRequest,
    ) -> Result<Option<Resolved>, InstallError> {
        let regions = region_order(&request.region);
        let found = |region: &str, name: &str, (url, manifest): (String, Vec<u8>)| Resolved {
            source: Source::Archive,
            manifest_region: region.to_string(),
            version: name.to_string(),
            manifest_url: url,
            exact_build: request.version.as_deref() == Some(name),
            manifest,
        };

        if let Some(name) = &request.version {
            for region in &regions {
                self.progress.report(Update::stage(
                    Stage::Resolve,
                    format!("Looking up {name} in the manifest archive ({region})"),
                ));
                if let Some(hit) = self.archive_entry(region, name).await? {
                    return Ok(Some(found(region, name, hit)));
                }
            }
        }

        for region in &regions {
            self.progress.report(Update::stage(
                Stage::Resolve,
                format!("Listing archived {} builds ({region})", request.patch),
            ));
            let Some(listing) = self.text(&archive_listing_url(region)).await? else {
                continue;
            };
            for name in listing_candidates(&listing, &request.patch)? {
                if let Some(hit) = self.archive_entry(region, &name).await? {
                    return Ok(Some(found(region, &name, hit)));
                }
            }
        }
        Ok(None)
    }

    async fn ask_sieve(
        &mut self,
        request: &ResolveRequest,
    ) -> Result<Option<Resolved>, InstallError> {
        self.progress.report(Update::stage(
            Stage::Resolve,
            format!("Asking Riot for the current {} client", request.region),
        ));
        let url = sieve_url(&request.region);
        let Some(body) = self.text(&url).await? else {
            return Ok(None);
        };
        let response: SieveResponse = serde_json::from_slice(&body)
            .map_err(|err| InstallError::Network(format!("Riot Sieve response: {err}")))?;
        let Some((version, url)) = select_sieve_release(
            &response.releases,
            &request.patch,
            request.version.as_deref(),
        ) else {
            self.notes.push(format!(
                "Riot only serves the current patch for {}",
                request.region
            ));
            return Ok(None);
        };
        Ok(self
            .manifest(&version, &url)
            .await?
            .map(|manifest| Resolved {
                source: Source::Sieve,
                manifest_region: request.region.clone(),
                exact_build: request.version.as_deref() == Some(version.as_str()),
                version,
                manifest_url: url,
                manifest,
            }))
    }
}

pub async fn resolve<F: Fetch>(
    fetch: &F,
    request: &ResolveRequest,
    cancel: &AtomicBool,
    progress: &dyn Progress,
) -> Result<Resolved, InstallError> {
    let mut resolver = Resolver {
        fetch,
        cancel,
        progress,
        tried: HashSet::new(),
        notes: Vec::new(),
    };
    match resolver.search_archive(request).await {
        Ok(Some(resolved)) => return Ok(resolved),
        Ok(None) => {}
        Err(InstallError::Cancelled) => return Err(InstallError::Cancelled),
        // The archive is a convenience; if GitHub is unreachable or rate limited, say so
        // and still try Riot.
        Err(err) => resolver.notes.push(format!("archive unavailable: {err}")),
    }
    check_cancel(cancel)?;
    if let Some(resolved) = resolver.ask_sieve(request).await? {
        return Ok(resolved);
    }
    Err(InstallError::ReleaseNotFound {
        version: request
            .version
            .clone()
            .unwrap_or_else(|| request.patch.clone()),
        notes: resolver.notes,
    })
}

#[derive(Debug, Deserialize)]
struct SieveResponse {
    releases: Vec<SieveRelease>,
}

#[derive(Debug, Deserialize)]
struct SieveRelease {
    release: SieveReleaseInfo,
    download: SieveDownload,
}

#[derive(Debug, Deserialize)]
struct SieveReleaseInfo {
    labels: HashMap<String, SieveLabel>,
}

#[derive(Debug, Deserialize)]
struct SieveLabel {
    values: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SieveDownload {
    url: String,
}

/// `(archive-style version, manifest url)`: the exact build if listed, else the newest build
/// of the patch.
fn select_sieve_release(
    releases: &[SieveRelease],
    patch: &str,
    version: Option<&str>,
) -> Option<(String, String)> {
    let label = |release: &SieveRelease, key: &str| {
        release
            .release
            .labels
            .get(key)
            .and_then(|label| label.values.first())
            .cloned()
    };
    let mut candidates = releases
        .iter()
        .filter(|release| label(release, "riot:artifact_type_id").as_deref() == Some(ARTIFACT))
        .filter_map(|release| {
            let version = archive_name(&label(release, "riot:artifact_version_id")?)?;
            let url = valid_manifest_url(&release.download.url)?;
            (normalize_patch(&version).as_deref() == Some(patch)).then_some((version, url))
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|(name, _)| std::cmp::Reverse(build_number(name).unwrap_or(0)));
    if let Some(version) = version {
        if let Some(index) = candidates.iter().position(|(name, _)| name == version) {
            return Some(candidates.swap_remove(index));
        }
    }
    candidates.into_iter().next()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::install::progress::NoProgress;
    use std::sync::Mutex;

    /// Serves canned responses; everything else is a 404. Records requested URLs.
    #[derive(Default)]
    pub(crate) struct FakeFetch {
        pub(crate) responses: HashMap<String, (u16, Vec<u8>)>,
        pub(crate) calls: Mutex<Vec<String>>,
    }

    impl FakeFetch {
        pub(crate) fn with(mut self, url: &str, status: u16, body: &str) -> Self {
            self.responses
                .insert(url.to_string(), (status, body.as_bytes().to_vec()));
            self
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Fetch for FakeFetch {
        async fn get(
            &self,
            url: &str,
            _limit: usize,
            cancel: &AtomicBool,
            _on_bytes: crate::install::http::OnBytes<'_>,
        ) -> Result<Fetched, InstallError> {
            check_cancel(cancel)?;
            self.calls.lock().unwrap().push(url.to_string());
            let (status, body) = self
                .responses
                .get(url)
                .cloned()
                .unwrap_or((404, Vec::new()));
            let body = if (200..300).contains(&status) {
                body
            } else {
                Vec::new()
            };
            Ok(Fetched { status, body })
        }
    }

    fn manifest(id: &str) -> String {
        format!("https://lol.secure.dyn.riotcdn.net/channels/public/releases/{id}.manifest")
    }

    fn request(version: Option<&str>) -> ResolveRequest {
        ResolveRequest {
            region: "LA2".into(),
            patch: "16.17".into(),
            version: version.map(Into::into),
        }
    }

    fn run(fetch: &FakeFetch, request: &ResolveRequest) -> Result<Resolved, InstallError> {
        tauri::async_runtime::block_on(resolve(
            fetch,
            request,
            &AtomicBool::new(false),
            &NoProgress,
        ))
    }

    #[test]
    fn validates_regions_without_defaulting() {
        assert_eq!(normalize_region(Some(" la2 ")).unwrap(), "LA2");
        assert!(matches!(
            normalize_region(None),
            Err(InstallError::InvalidRegion(None))
        ));
        assert!(matches!(
            normalize_region(Some("  ")),
            Err(InstallError::InvalidRegion(None))
        ));
        assert!(matches!(
            normalize_region(Some("PBE1")),
            Err(InstallError::InvalidRegion(Some(_)))
        ));
        assert!(normalize_region(Some("../NA1")).is_err());
        assert_eq!(REGIONS.len(), 17);
    }

    #[test]
    fn maps_replay_versions_to_archive_names() {
        assert_eq!(archive_name("16.17.810.4348").unwrap(), "16.17.8104348");
        assert_eq!(archive_name("16.17.8104348").unwrap(), "16.17.8104348");
        assert_eq!(
            archive_name("16.13.7906961+branch.releases-16-13").unwrap(),
            "16.13.7906961"
        );
        assert_eq!(archive_name("16.17"), None);
        assert_eq!(archive_name("16.17.../x"), None);
        assert_eq!(archive_name("16.17.8104348/../../x"), None);
        assert_eq!(normalize_patch("16.17.810.4348").unwrap(), "16.17");
        assert_eq!(normalize_patch("16.x"), None);
    }

    #[test]
    fn accepts_only_riot_cdn_manifest_urls() {
        assert!(valid_manifest_url(&format!("{}\n", manifest("AB"))).is_some());
        assert!(
            valid_manifest_url("http://lol.secure.dyn.riotcdn.net/releases/A.manifest").is_none()
        );
        assert!(valid_manifest_url("https://evil.example/releases/A.manifest").is_none());
        assert!(
            valid_manifest_url("https://riotcdn.net.evil.example/releases/A.manifest").is_none()
        );
        assert!(valid_manifest_url("https://lol.secure.dyn.riotcdn.net/releases/A.exe").is_none());
    }

    #[test]
    fn exact_archive_build_wins() {
        let fetch = FakeFetch::default()
            .with(
                &archive_file_url("LA2", "16.17.8104348"),
                200,
                &manifest("EXACT"),
            )
            .with(&manifest("EXACT"), 200, "RMAN");
        let resolved = run(&fetch, &request(Some("16.17.8104348"))).unwrap();
        assert_eq!(resolved.source, Source::Archive);
        assert!(resolved.exact_build);
        assert_eq!(resolved.manifest_region, "LA2");
        assert_eq!(resolved.manifest, b"RMAN");
        assert_eq!(fetch.calls().len(), 2, "no listing or sieve call");
    }

    #[test]
    fn falls_back_to_other_region_dirs_for_the_exact_build() {
        let fetch = FakeFetch::default()
            .with(
                &archive_file_url("NA1", "16.17.8104348"),
                200,
                &manifest("NA"),
            )
            .with(&manifest("NA"), 200, "RMAN");
        let resolved = run(&fetch, &request(Some("16.17.8104348"))).unwrap();
        assert!(resolved.exact_build);
        assert_eq!(resolved.manifest_region, "NA1");
    }

    #[test]
    fn forbidden_manifest_falls_back_to_newest_other_build_visibly() {
        let listing = r#"[{"name":"16.16.8049184.txt"},{"name":"16.17.8100000.txt"},
            {"name":"16.17.8104348.txt"},{"name":"16.17.8120000.txt"},{"name":"README"}]"#;
        let fetch = FakeFetch::default()
            .with(
                &archive_file_url("LA2", "16.17.8104348"),
                200,
                &manifest("GONE"),
            )
            .with(&manifest("GONE"), 403, "")
            .with(&archive_listing_url("LA2"), 200, listing)
            .with(
                &archive_file_url("LA2", "16.17.8120000"),
                200,
                &manifest("NEW"),
            )
            .with(&manifest("NEW"), 404, "")
            .with(
                &archive_file_url("LA2", "16.17.8100000"),
                200,
                &manifest("OLD"),
            )
            .with(&manifest("OLD"), 200, "RMAN");
        let resolved = run(&fetch, &request(Some("16.17.8104348"))).unwrap();
        assert_eq!(resolved.version, "16.17.8100000");
        assert!(!resolved.exact_build);
        assert_eq!(resolved.source, Source::Archive);
        let calls = fetch.calls();
        assert_eq!(
            calls.iter().filter(|url| **url == manifest("GONE")).count(),
            1,
            "a manifest URL is tried once"
        );
        assert!(calls.iter().all(|url| !url.contains("sieve")));
        let new = calls.iter().position(|u| *u == manifest("NEW")).unwrap();
        let old = calls.iter().position(|u| *u == manifest("OLD")).unwrap();
        assert!(new < old, "newest build first");
    }

    #[test]
    fn uses_sieve_only_after_the_archive_and_reports_it() {
        let sieve = format!(
            r#"{{"releases":[{{"release":{{"labels":{{
                "riot:artifact_type_id":{{"values":["lol-game-client"]}},
                "riot:artifact_version_id":{{"values":["16.17.8130000+branch.releases-16-17"]}}}}}},
                "download":{{"url":"{}"}}}}]}}"#,
            manifest("SIEVE")
        );
        let fetch = FakeFetch::default()
            .with(&archive_listing_url("LA2"), 500, "")
            .with(&sieve_url("LA2"), 200, &sieve)
            .with(&manifest("SIEVE"), 200, "RMAN");
        let resolved = run(&fetch, &request(Some("16.17.8104348"))).unwrap();
        assert_eq!(resolved.source, Source::Sieve);
        assert!(!resolved.exact_build);
        assert_eq!(resolved.version, "16.17.8130000");
        let calls = fetch.calls();
        assert_eq!(calls.last().unwrap(), &manifest("SIEVE"));
        assert!(calls[0].contains("raw.githubusercontent.com/Morilli"));
    }

    #[test]
    fn reports_release_not_found_with_notes() {
        let fetch = FakeFetch::default().with(&sieve_url("LA2"), 200, r#"{"releases":[]}"#);
        let err = run(&fetch, &request(Some("16.17.8104348"))).unwrap_err();
        assert_eq!(err.code(), "releaseNotFound");
        assert!(err.to_string().contains("current patch"));
    }

    #[test]
    fn cancel_stops_resolution() {
        let fetch = FakeFetch::default();
        let err = tauri::async_runtime::block_on(resolve(
            &fetch,
            &request(None),
            &AtomicBool::new(true),
            &NoProgress,
        ))
        .unwrap_err();
        assert!(matches!(err, InstallError::Cancelled));
    }
}
