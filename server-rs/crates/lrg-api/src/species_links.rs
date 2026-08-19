//! Turns a BioCLIP taxon name into links on the free species databases.
//!
//! The pruned Tree-of-Life head ships names only — no identifiers (see
//! `server-rs/scripts/bioclip_taxa_filter.toml`). So a link to GBIF or
//! iNaturalist can either be a name-based *search* URL, which always works but
//! dumps the user on a results page, or a deep link to the taxon page, which
//! needs an id we do not have. This module buys the id once per distinct
//! taxon from two free, unauthenticated APIs and caches it forever after:
//!
//! * GBIF `/species/match` — authoritative, and already the source of the
//!   common names baked into the head, so its answer agrees with our labels
//!   by construction.
//! * iNaturalist `/v1/taxa` — supplies the taxon page photographers actually
//!   want (photos, range maps, similar species), a localized common name, and
//!   a Wikipedia article URL, which saves guessing at article titles.
//!
//! Everything here is best-effort. A resolver that cannot reach the network
//! returns search URLs for the same providers rather than failing: a link that
//! lands one click away is worth much more than an error, and the plugin
//! writes the result into a read-only catalog field where an error has nowhere
//! to go.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::Url;
use serde::{Deserialize, Serialize};

/// How long a successful resolution stays good.
///
/// Taxonomy does move — genera get split, names get synonymized — but on a
/// scale of years, and a stale deep link still lands on the right organism's
/// page (GBIF and iNaturalist both keep redirects for merged taxa). Long
/// enough that a working catalog never re-fetches.
const POSITIVE_TTL: Duration = Duration::from_secs(180 * 24 * 60 * 60);

/// How long a "no such taxon" answer stays good.
///
/// Much shorter than the positive TTL, because this is the answer that can be
/// wrong in the direction that matters: a name missing from GBIF today may be
/// a name GBIF simply had not ingested yet.
const NEGATIVE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Minimum spacing between outbound calls.
///
/// iNaturalist asks for at most 60 requests/minute and a real User-Agent.
/// Indexing a wildlife catalog walks through its distinct taxa in a burst, so
/// without a gate the very first run is exactly the traffic shape they ask
/// people not to send.
const MIN_CALL_SPACING: Duration = Duration::from_millis(1100);

const HTTP_TIMEOUT: Duration = Duration::from_secs(8);

/// One taxon's links, as returned to the plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaxonLinks {
    /// The name that was queried, echoed back so a batch response is
    /// self-describing.
    pub name: String,
    /// GBIF `usageKey`, absent when GBIF had no match (or was unreachable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gbif_key: Option<u64>,
    /// iNaturalist taxon id, same caveat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inat_id: Option<u64>,
    /// iNaturalist's common name in the requested locale. Better than the
    /// head's baked-in GBIF vernacular, which has no language guarantee.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub common_name: Option<String>,
    /// Provider id → URL. Always populated for every provider: deep links
    /// where an id was found, search URLs otherwise.
    pub urls: HashMap<String, String>,
    /// True when at least one identifier was resolved, i.e. at least one of
    /// the URLs is a deep link rather than a search.
    pub resolved: bool,
}

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    links: TaxonLinks,
    /// Unix seconds; compared against [`POSITIVE_TTL`]/[`NEGATIVE_TTL`].
    fetched_at: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct CacheFile {
    version: u32,
    entries: HashMap<String, CacheEntry>,
}

const CACHE_VERSION: u32 = 1;

pub struct LinkResolver {
    client: reqwest::Client,
    cache: Mutex<HashMap<String, CacheEntry>>,
    /// Set once the on-disk cache has been read; the file is only touched
    /// when someone actually asks for a link.
    loaded: Mutex<bool>,
    /// Gate for [`MIN_CALL_SPACING`]. Held only across a `sleep`, never
    /// across the request itself, so concurrent callers queue instead of
    /// serializing on the slowest response.
    last_call: tokio::sync::Mutex<Option<Instant>>,
    path: PathBuf,
}

impl LinkResolver {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(format!(
                "LrGeniusAI/{} (+https://github.com/LrGenius/LrGeniusAI)",
                lrg_common::version::backend_version()
            ))
            .build()
            .unwrap_or_default();
        Self {
            client,
            cache: Mutex::new(HashMap::new()),
            loaded: Mutex::new(false),
            last_call: tokio::sync::Mutex::new(None),
            path: cache_path(),
        }
    }

    /// Resolve one taxon, using the cache when it can.
    ///
    /// `rank` narrows both APIs — without it a genus query can match a species
    /// whose epithet happens to repeat the genus name. `lang` picks the
    /// Wikipedia host and the iNaturalist common-name locale.
    pub async fn resolve(&self, name: &str, rank: Option<&str>, lang: &str) -> TaxonLinks {
        let name = name.trim();
        if name.is_empty() {
            return TaxonLinks::unresolved("", lang);
        }
        let key = cache_key(name, rank, lang);

        self.ensure_loaded();
        if let Some(hit) = self.lookup(&key) {
            return hit;
        }

        let (links, contacted) = self.fetch(name, rank, lang).await;
        // A network failure leaves both ids empty and must not be cached: the
        // next call, possibly with the network back, has to try again. A
        // genuine "no such name" *is* cached, on the short TTL — that is a
        // real answer, and re-asking for every photo of an unlisted taxon is
        // exactly the traffic the cache exists to prevent.
        if contacted {
            self.store(key, links.clone());
        }
        links
    }

    fn lookup(&self, key: &str) -> Option<TaxonLinks> {
        let cache = self.cache.lock().unwrap();
        let entry = cache.get(key)?;
        let ttl = if entry.links.resolved {
            POSITIVE_TTL
        } else {
            NEGATIVE_TTL
        };
        let age = now_secs().saturating_sub(entry.fetched_at);
        (age < ttl.as_secs()).then(|| entry.links.clone())
    }

    fn store(&self, key: String, links: TaxonLinks) {
        {
            let mut cache = self.cache.lock().unwrap();
            cache.insert(
                key,
                CacheEntry {
                    links,
                    fetched_at: now_secs(),
                },
            );
        }
        self.save();
    }

    fn ensure_loaded(&self) {
        let mut loaded = self.loaded.lock().unwrap();
        if *loaded {
            return;
        }
        *loaded = true;
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return;
        };
        match serde_json::from_str::<CacheFile>(&text) {
            Ok(file) if file.version == CACHE_VERSION => {
                let count = file.entries.len();
                *self.cache.lock().unwrap() = file.entries;
                log::debug!("Loaded {count} cached species links from {:?}", self.path);
            }
            Ok(file) => log::info!(
                "Ignoring species link cache written by version {} (expected {CACHE_VERSION})",
                file.version
            ),
            Err(e) => log::warn!("Could not parse species link cache: {e}"),
        }
    }

    fn save(&self) {
        let file = {
            let cache = self.cache.lock().unwrap();
            CacheFile {
                version: CACHE_VERSION,
                entries: cache
                    .iter()
                    .map(|(k, v)| {
                        (
                            k.clone(),
                            CacheEntry {
                                links: v.links.clone(),
                                fetched_at: v.fetched_at,
                            },
                        )
                    })
                    .collect(),
            }
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match serde_json::to_string(&file).map(|s| std::fs::write(&self.path, s)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => log::warn!("Could not write species link cache: {e}"),
            Err(e) => log::warn!("Could not serialize species link cache: {e}"),
        }
    }

    async fn throttle(&self) {
        let mut last = self.last_call.lock().await;
        if let Some(prev) = *last {
            let elapsed = prev.elapsed();
            if elapsed < MIN_CALL_SPACING {
                tokio::time::sleep(MIN_CALL_SPACING - elapsed).await;
            }
        }
        *last = Some(Instant::now());
    }

    /// Ask both APIs. The flag is "we reached at least one of them", which is
    /// what decides whether the answer is worth caching.
    async fn fetch(&self, name: &str, rank: Option<&str>, lang: &str) -> (TaxonLinks, bool) {
        self.throttle().await;
        let gbif = self.fetch_gbif(name, rank).await;
        let inat = self.fetch_inat(name, rank, lang).await;
        let contacted = gbif.is_ok() || inat.is_ok();

        let mut links = TaxonLinks::unresolved(name, lang);
        if let Ok(Some(key)) = gbif {
            links.gbif_key = Some(key);
            links
                .urls
                .insert("gbif".into(), format!("https://www.gbif.org/species/{key}"));
        }
        if let Ok(Some(taxon)) = inat {
            links.inat_id = Some(taxon.id);
            links.urls.insert(
                "inaturalist".into(),
                format!("https://www.inaturalist.org/taxa/{}", taxon.id),
            );
            links.common_name = taxon.preferred_common_name;
            // iNaturalist's article link is always the English Wikipedia, so
            // it only helps an English reader; everyone else keeps the
            // scientific-name search on their own Wikipedia, which resolves
            // through the redirect every language edition keeps.
            if lang == "en" {
                if let Some(url) = taxon.wikipedia_url {
                    links
                        .urls
                        .insert("wikipedia".into(), url.replacen("http://", "https://", 1));
                }
            }
        }
        links.resolved = links.gbif_key.is_some() || links.inat_id.is_some();
        (links, contacted)
    }

    /// `Err(Offline)` means we never got an answer; `Ok(None)` means the API
    /// answered and has no such taxon.
    async fn fetch_gbif(&self, name: &str, rank: Option<&str>) -> Result<Option<u64>, Offline> {
        let mut params = vec![("name", name.to_string())];
        if let Some(rank) = gbif_rank(rank) {
            params.push(("rank", rank.to_string()));
        }
        let url = Url::parse_with_params("https://api.gbif.org/v1/species/match", &params)
            .map_err(|_| Offline)?;
        let resp = match self.client.get(url).send().await {
            Ok(r) => r,
            Err(e) => {
                log::debug!("GBIF match for {name:?} failed: {e}");
                return Err(Offline);
            }
        };
        let body: GbifMatch = resp.json().await.map_err(|e| {
            log::debug!("GBIF match for {name:?} returned unreadable JSON: {e}");
            Offline
        })?;
        if body.match_type.as_deref() == Some("NONE") {
            return Ok(None);
        }
        // A synonym match echoes the name we asked for; the accepted key is
        // the page a reader should land on.
        Ok(body.accepted_usage_key.or(body.usage_key))
    }

    async fn fetch_inat(
        &self,
        name: &str,
        rank: Option<&str>,
        lang: &str,
    ) -> Result<Option<InatTaxon>, Offline> {
        let mut params = vec![
            ("q", name.to_string()),
            ("per_page", "1".to_string()),
            ("locale", lang.to_string()),
        ];
        if let Some(rank) = rank.filter(|r| !r.is_empty() && *r != "none") {
            params.push(("rank", rank.to_lowercase()));
        }
        let url = Url::parse_with_params("https://api.inaturalist.org/v1/taxa", &params)
            .map_err(|_| Offline)?;
        let resp = match self.client.get(url).send().await {
            Ok(r) => r,
            Err(e) => {
                log::debug!("iNaturalist lookup for {name:?} failed: {e}");
                return Err(Offline);
            }
        };
        let body: InatResponse = resp.json().await.map_err(|e| {
            log::debug!("iNaturalist lookup for {name:?} returned unreadable JSON: {e}");
            Offline
        })?;
        // `q` is a fuzzy search, so the top hit for a name iNaturalist does not
        // have is a *different* organism. Only an exact name match may become
        // a deep link — a wrong species page is worse than a search page.
        Ok(body
            .results
            .into_iter()
            .find(|t| t.name.eq_ignore_ascii_case(name)))
    }
}

impl Default for LinkResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl TaxonLinks {
    /// Search URLs only — what every caller falls back to.
    fn unresolved(name: &str, lang: &str) -> Self {
        TaxonLinks {
            name: name.to_string(),
            gbif_key: None,
            inat_id: None,
            common_name: None,
            urls: search_urls(name, lang),
            resolved: false,
        }
    }
}

/// Name-based URLs for every supported provider.
///
/// Public because the plugin is not the only caller that needs them and
/// because they are what the whole module degrades to when offline.
pub fn search_urls(name: &str, lang: &str) -> HashMap<String, String> {
    let lang = wiki_lang(lang);
    let mut urls = HashMap::new();
    let query = |base: &str, key: &str, value: &str| -> Option<String> {
        Url::parse_with_params(base, &[(key, value)])
            .ok()
            .map(|u| u.to_string())
    };
    let mut insert = |id: &str, url: Option<String>| {
        if let Some(url) = url {
            urls.insert(id.to_string(), url);
        }
    };
    insert(
        "gbif",
        query("https://www.gbif.org/species/search", "q", name),
    );
    insert(
        "inaturalist",
        query("https://www.inaturalist.org/search", "q", name),
    );
    insert(
        "wikipedia",
        query(
            &format!("https://{lang}.wikipedia.org/w/index.php"),
            "search",
            name,
        ),
    );
    insert("wikispecies", wikispecies_url(name));
    insert("eol", query("https://eol.org/search", "q", name));
    insert(
        "col",
        query("https://www.catalogueoflife.org/data/search", "q", name),
    );
    urls
}

/// Wikispecies files scientific names as article titles verbatim, so this one
/// provider needs no lookup to deep-link.
fn wikispecies_url(name: &str) -> Option<String> {
    let mut url = Url::parse("https://species.wikimedia.org/wiki/").ok()?;
    url.path_segments_mut().ok()?.pop().push(name);
    Some(url.to_string())
}

/// Two-letter Wikipedia subdomain, defaulting to English.
///
/// Lightroom hands out tags like `de` or `zh-Hans`; Wikipedia hosts are the
/// bare language, and an unknown one would 404 the whole link.
fn wiki_lang(lang: &str) -> String {
    let base = lang.split(['-', '_']).next().unwrap_or("en");
    if base.len() == 2 && base.chars().all(|c| c.is_ascii_alphabetic()) {
        base.to_ascii_lowercase()
    } else {
        "en".to_string()
    }
}

/// Map our rank labels onto GBIF's enum. `none` and anything unknown drop the
/// parameter entirely, which lets GBIF pick the rank itself.
fn gbif_rank(rank: Option<&str>) -> Option<&'static str> {
    match rank?.to_ascii_lowercase().as_str() {
        "kingdom" => Some("KINGDOM"),
        "phylum" => Some("PHYLUM"),
        "class" => Some("CLASS"),
        "order" => Some("ORDER"),
        "family" => Some("FAMILY"),
        "genus" => Some("GENUS"),
        "species" => Some("SPECIES"),
        _ => None,
    }
}

fn cache_key(name: &str, rank: Option<&str>, lang: &str) -> String {
    format!(
        "{}|{}|{}",
        name.to_lowercase(),
        rank.unwrap_or("").to_lowercase(),
        wiki_lang(lang)
    )
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Alongside the model cache rather than the catalog: the mapping is a
/// property of the world, not of one catalog, so two catalogs on one machine
/// should share the lookups they have already paid for.
fn cache_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join(".cache")
        .join("lrgenius")
        .join("species_links.json")
}

/// Marker for "the request never produced an answer" — see [`LinkResolver::fetch`].
struct Offline;

#[derive(Deserialize)]
struct GbifMatch {
    #[serde(rename = "usageKey")]
    usage_key: Option<u64>,
    #[serde(rename = "acceptedUsageKey")]
    accepted_usage_key: Option<u64>,
    #[serde(rename = "matchType")]
    match_type: Option<String>,
}

#[derive(Deserialize)]
struct InatResponse {
    #[serde(default)]
    results: Vec<InatTaxon>,
}

#[derive(Deserialize)]
struct InatTaxon {
    id: u64,
    name: String,
    preferred_common_name: Option<String>,
    wikipedia_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_urls_cover_every_provider_and_encode() {
        let urls = search_urls("Panthera leo", "en");
        for provider in [
            "gbif",
            "inaturalist",
            "wikipedia",
            "wikispecies",
            "eol",
            "col",
        ] {
            let url = urls.get(provider).unwrap_or_else(|| panic!("{provider}"));
            assert!(url.starts_with("https://"), "{provider}: {url}");
            assert!(!url.contains(' '), "{provider} left a raw space: {url}");
        }
        assert_eq!(
            urls["wikispecies"],
            "https://species.wikimedia.org/wiki/Panthera%20leo"
        );
    }

    #[test]
    fn wikipedia_host_follows_the_language() {
        assert!(search_urls("Bombus", "de")["wikipedia"].starts_with("https://de.wikipedia.org/"));
        // Lightroom hands out region-tagged and junk values; both fall back
        // rather than pointing at a host that does not exist.
        assert!(
            search_urls("Bombus", "zh-Hans")["wikipedia"].starts_with("https://zh.wikipedia.org/")
        );
        assert!(search_urls("Bombus", "")["wikipedia"].starts_with("https://en.wikipedia.org/"));
    }

    #[test]
    fn gbif_rank_maps_known_ranks_only() {
        assert_eq!(gbif_rank(Some("species")), Some("SPECIES"));
        assert_eq!(gbif_rank(Some("Genus")), Some("GENUS"));
        assert_eq!(gbif_rank(Some("none")), None);
        assert_eq!(gbif_rank(None), None);
    }

    #[test]
    fn cache_key_is_case_and_language_normalized() {
        assert_eq!(
            cache_key("Panthera Leo", Some("Species"), "en-US"),
            cache_key("panthera leo", Some("species"), "en")
        );
        assert_ne!(
            cache_key("Panthera leo", Some("species"), "de"),
            cache_key("Panthera leo", Some("species"), "en")
        );
    }

    #[test]
    fn empty_name_never_reaches_the_network() {
        let resolver = LinkResolver::new();
        let links = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(resolver.resolve("  ", Some("species"), "en"));
        assert!(!links.resolved);
        assert!(links.gbif_key.is_none());
    }

    #[test]
    fn stale_positive_entries_are_ignored() {
        let resolver = LinkResolver::new();
        *resolver.loaded.lock().unwrap() = true;
        let mut links = TaxonLinks::unresolved("Panthera leo", "en");
        links.resolved = true;
        links.gbif_key = Some(5219404);
        resolver.cache.lock().unwrap().insert(
            "k".into(),
            CacheEntry {
                links,
                fetched_at: now_secs() - POSITIVE_TTL.as_secs() - 1,
            },
        );
        assert!(resolver.lookup("k").is_none());
    }

    #[test]
    fn fresh_negative_entries_are_served_from_cache() {
        let resolver = LinkResolver::new();
        *resolver.loaded.lock().unwrap() = true;
        resolver.cache.lock().unwrap().insert(
            "k".into(),
            CacheEntry {
                links: TaxonLinks::unresolved("Nonexistent name", "en"),
                fetched_at: now_secs() - 60,
            },
        );
        let hit = resolver.lookup("k").expect("negative entry within TTL");
        assert!(!hit.resolved);
        // Even a negative answer still carries usable search links.
        assert!(hit.urls.contains_key("gbif"));
    }
}
