//! Internet radio via the Radio Browser API (<https://www.radio-browser.info>):
//! a free, no-auth directory of ~50k stations worldwide. Runs on a worker thread
//! (like `tagsearch`) so the UI never blocks on the network; the latest search
//! wins (older ones drop). Country / tag listings power the filter pickers.
//!
//! Only plain streams (mp3/aac/ogg) are surfaced — HLS stations are filtered out
//! since the decoder can't play them — and only stations that last checked OK.

use std::path::PathBuf;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use serde::{Deserialize, Serialize};

mod directory;
use directory::load_directory;

/// Round-robin endpoint over all Radio Browser mirrors.
const BASE: &str = "https://all.api.radio-browser.info";
/// Max stations per result page (live-search fallback).
const LIMIT: usize = 100;
/// Max tags shown in the genre picker (there are thousands; keep the popular).
const TAG_LIMIT: usize = 300;
/// Rough decompressed size of the full directory JSON, for the download bar.
pub const DIRECTORY_EST_BYTES: u64 = 66 * 1024 * 1024;

/// A playable radio station (also persisted as a favorite, hence serde).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Station {
    pub name: String,
    /// Resolved direct stream URL (playlists already followed server-side).
    pub url: String,
    #[serde(default)]
    pub codec: String,
    #[serde(default)]
    pub bitrate: u32,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub countrycode: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub tags: String,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub uuid: String,
    /// How many times listeners tuned in (popularity) and up-voted it.
    #[serde(default)]
    pub clickcount: u32,
    #[serde(default)]
    pub votes: u32,
    /// Recent trend in tune-ins (Radio Browser `clicktrend`): positive = rising,
    /// negative = falling. Drives the Trending section.
    #[serde(default)]
    pub clicktrend: i32,
}

impl Station {
    /// First genre/tag, trimmed (empty string if none).
    pub fn genre(&self) -> &str {
        self.tags.split(',').next().unwrap_or("").trim()
    }

    /// Short "country · genre · bitrate" subtitle for the now-playing line.
    pub fn subtitle(&self) -> String {
        let genre = self.genre();
        let mut parts: Vec<String> = Vec::new();
        if !self.countrycode.is_empty() {
            parts.push(self.countrycode.clone());
        } else if !self.country.is_empty() {
            parts.push(self.country.clone());
        }
        if !genre.is_empty() {
            parts.push(genre.to_string());
        }
        if !self.codec.is_empty() {
            parts.push(self.codec.to_uppercase());
        }
        if self.bitrate > 0 {
            parts.push(format!("{}k", self.bitrate));
        }
        parts.join(" · ")
    }
}

/// One entry in the radio listening history: a station, how many times it was
/// tuned in, and when it was last played (unix seconds). Persisted to
/// `radio_history.json`; drives the Recent + Most Played sidebar sections.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub station: Station,
    #[serde(default)]
    pub play_count: u32,
    #[serde(default)]
    pub last_played: u64,
}

/// A user-created named collection of stations (distinct from the single starred
/// Favorites list). A station may belong to several playlists; stations are stored
/// inline (self-contained value objects). Persisted to `radio_playlists.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Playlist {
    pub id: u32,
    pub name: String,
    #[serde(default)]
    pub stations: Vec<Station>,
}

/// Result ordering for a station search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    Popular, // most-clicked
    Trending, // biggest recent rise (clicktrend)
    Votes,    // most-voted
    Name,     // A→Z
    Bitrate,  // highest bitrate
}

impl Sort {
    /// (API `order` field, `reverse`).
    fn params(self) -> (&'static str, bool) {
        match self {
            Sort::Popular => ("clickcount", true),
            Sort::Trending => ("clicktrend", true),
            Sort::Votes => ("votes", true),
            Sort::Name => ("name", false),
            Sort::Bitrate => ("bitrate", true),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Sort::Popular => "popular",
            Sort::Trending => "trending",
            Sort::Votes => "votes",
            Sort::Name => "name",
            Sort::Bitrate => "bitrate",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Sort::Popular => Sort::Votes,
            Sort::Votes => Sort::Name,
            Sort::Name => Sort::Bitrate,
            Sort::Bitrate => Sort::Popular,
            // Trending is entered via its section, not the `o` cycle — fold it in.
            Sort::Trending => Sort::Popular,
        }
    }
    /// Parse a persisted label back into a `Sort` (defaults to `Popular`).
    pub fn from_label(s: &str) -> Self {
        match s {
            "trending" => Sort::Trending,
            "votes" => Sort::Votes,
            "name" => Sort::Name,
            "bitrate" => Sort::Bitrate,
            _ => Sort::Popular,
        }
    }
}

/// A country in the filter picker (`code` is the ISO-3166-1 alpha-2).
#[derive(Debug, Clone, Default)]
pub struct Country {
    pub name: String,
    pub code: String,
    pub count: u32,
}

/// A genre/tag in the filter picker.
#[derive(Debug, Clone, Default)]
pub struct TagItem {
    pub name: String,
    pub count: u32,
}

/// A search/browse request (`key` lets the UI ignore stale results).
#[derive(Debug, Clone)]
pub enum RadioRequest {
    /// Stations matching the (AND-combined) filters, by name / country / tag.
    Search {
        query: String,
        country: Option<String>, // ISO code
        tag: Option<String>,
        sort: Sort,
        key: String,
    },
    /// The country list (for the picker).
    Countries { key: String },
    /// The popular tags worldwide (genre picker, no country filter).
    Tags { key: String },
    /// Genres actually present in `code`'s stations (genre picker scoped to the
    /// selected country), derived from those stations' tags.
    CountryGenres { code: String, key: String },
    /// Load the full station directory — from the on-disk cache when it's still
    /// within `max_age_secs` (0 = no expiry), else (re)download and re-cache.
    /// `force` ignores the cache entirely (manual refresh).
    LoadDirectory { force: bool, max_age_secs: u64 },
}

/// A worker reply, tagged with the request's `key` (carried for stale-result
/// matching; the current consumer matches on the active request instead).
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum RadioResult {
    Found {
        key: String,
        stations: Vec<Station>,
    },
    Countries {
        key: String,
        items: Vec<Country>,
    },
    Tags {
        key: String,
        items: Vec<TagItem>,
    },
    CountryGenres {
        code: String,
        key: String,
        items: Vec<TagItem>,
    },
    /// The full directory is ready (from cache or a fresh download).
    Directory {
        stations: Vec<Station>,
        from_cache: bool,
    },
    /// Bytes downloaded so far while (re)fetching the directory.
    DirectoryProgress {
        read: u64,
    },
    Error {
        key: String,
        msg: String,
    },
}

/// Raw station row — only the fields we use; everything else is ignored.
#[derive(Deserialize)]
struct RawStation {
    #[serde(default)]
    name: String,
    #[serde(default)]
    url_resolved: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    codec: String,
    #[serde(default)]
    bitrate: u32,
    #[serde(default)]
    country: String,
    #[serde(default)]
    countrycode: String,
    #[serde(default)]
    language: String,
    #[serde(default)]
    tags: String,
    #[serde(default)]
    homepage: String,
    #[serde(default)]
    stationuuid: String,
    #[serde(default)]
    clickcount: u32,
    #[serde(default)]
    votes: u32,
    #[serde(default)]
    clicktrend: i32,
    #[serde(default)]
    hls: u8,
    #[serde(default)]
    lastcheckok: u8,
}

#[derive(Deserialize)]
struct RawCountry {
    #[serde(default)]
    name: String,
    #[serde(default)]
    iso_3166_1: String,
    #[serde(default)]
    stationcount: u32,
}

#[derive(Deserialize)]
struct RawTag {
    #[serde(default)]
    name: String,
    #[serde(default)]
    stationcount: u32,
}

fn clean(rows: Vec<RawStation>) -> Vec<Station> {
    rows.into_iter()
        .filter(|r| r.hls == 0 && r.lastcheckok == 1)
        .filter_map(|r| {
            let url = if !r.url_resolved.is_empty() {
                r.url_resolved
            } else {
                r.url
            };
            if url.is_empty() {
                return None;
            }
            let name = tidy_name(&r.name);
            if name.is_empty() {
                return None; // unnamed junk entries aren't useful
            }
            Some(Station {
                name,
                url,
                codec: r.codec,
                bitrate: r.bitrate,
                country: r.country,
                countrycode: r.countrycode,
                language: r.language,
                tags: r.tags,
                homepage: r.homepage,
                uuid: r.stationuuid,
                clickcount: r.clickcount,
                votes: r.votes,
                clicktrend: r.clicktrend,
            })
        })
        .collect()
}

/// Tidy a directory-supplied station name: collapse runs of whitespace, drop
/// control characters, and trim decorative padding (e.g. "***", ">>", "•••",
/// stray dashes/dots) that crowd many user-submitted names.
fn tidy_name(raw: &str) -> String {
    // collapse whitespace + strip control chars
    let mut s = String::with_capacity(raw.len());
    let mut prev_space = false;
    for ch in raw.trim().chars() {
        if ch.is_control() && !ch.is_whitespace() {
            continue; // drop control chars, but tabs/newlines fold into a space
        }
        if ch.is_whitespace() {
            if !prev_space {
                s.push(' ');
            }
            prev_space = true;
        } else {
            s.push(ch);
            prev_space = false;
        }
    }
    // trim leading/trailing decorative characters
    s.trim_matches(|c: char| "-_*•·~|>=<.+#:/\\ ".contains(c))
        .to_string()
}

/// Build the station-search URL from the active filters (all AND-combined).
fn search_url(query: &str, country: Option<&str>, tag: Option<&str>, sort: Sort) -> String {
    let (order, reverse) = sort.params();
    let mut url = format!(
        "{BASE}/json/stations/search?hidebroken=true&limit={LIMIT}&order={order}&reverse={reverse}"
    );
    let q = query.trim();
    if !q.is_empty() {
        url.push_str(&format!("&name={}", urlencode(q)));
    }
    if let Some(cc) = country.filter(|c| !c.is_empty()) {
        url.push_str(&format!("&countrycode={}", urlencode(cc)));
    }
    if let Some(t) = tag.filter(|t| !t.is_empty()) {
        // `tag` (with the default tagExact=false) is a substring match, so
        // "music" also finds "musica", "pop music", … — unlike `tagList`, which
        // demands an exact tag and returns nothing for most genres.
        url.push_str(&format!("&tag={}", urlencode(t)));
    }
    url
}

/// A short, human-friendly message for a request failure — never the raw URL.
fn err_msg(e: ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(code) => format!("radio directory busy ({code}) — try again"),
        _ => "can't reach the radio directory".into(),
    }
}

/// GET + parse JSON, retrying once (the `all.api` host is round-robin, so a
/// fresh attempt often lands on a healthy mirror after a transient failure).
fn get_json<T: serde::de::DeserializeOwned>(agent: &ureq::Agent, url: &str) -> Result<T, String> {
    let mut last = String::from("request failed");
    for attempt in 0..2 {
        match agent.get(url).call() {
            Ok(mut resp) => {
                return resp
                    .body_mut()
                    .read_json()
                    .map_err(|_| "unexpected response from the radio directory".to_string());
            }
            Err(e) => {
                last = err_msg(e);
                let _ = attempt;
            }
        }
    }
    Err(last)
}

fn fetch_stations(
    agent: &ureq::Agent,
    query: &str,
    country: Option<&str>,
    tag: Option<&str>,
    sort: Sort,
) -> Result<Vec<Station>, String> {
    let rows: Vec<RawStation> = get_json(agent, &search_url(query, country, tag, sort))?;
    Ok(clean(rows))
}

fn fetch_countries(agent: &ureq::Agent) -> Result<Vec<Country>, String> {
    let rows: Vec<RawCountry> = get_json(
        agent,
        &format!("{BASE}/json/countries?order=stationcount&reverse=true&hidebroken=true"),
    )?;
    Ok(rows
        .into_iter()
        .filter(|c| c.stationcount > 0 && !c.name.is_empty() && !c.iso_3166_1.is_empty())
        .map(|c| Country {
            name: c.name,
            code: c.iso_3166_1,
            count: c.stationcount,
        })
        .collect())
}

fn fetch_tags(agent: &ureq::Agent) -> Result<Vec<TagItem>, String> {
    let rows: Vec<RawTag> = get_json(
        agent,
        &format!(
            "{BASE}/json/tags?order=stationcount&reverse=true&hidebroken=true&limit={TAG_LIMIT}"
        ),
    )?;
    Ok(rows
        .into_iter()
        .filter(|t| t.stationcount > 0 && !t.name.trim().is_empty())
        .map(|t| TagItem {
            name: t.name,
            count: t.stationcount,
        })
        .collect())
}

/// How many of a country's (most-popular) stations to scan when deriving its
/// genres. The top stations by clickcount cover the relevant genres well, so a
/// modest cap keeps the download small and the picker quick to populate.
const COUNTRY_SCAN: usize = 500;
/// Cap the per-country genre list (keep the most common).
const COUNTRY_GENRE_LIMIT: usize = 200;

/// Derive the genres that actually appear in `code`'s stations by tallying their
/// tags — so the genre picker, when a country is selected, offers only relevant
/// genres (with country-specific counts) instead of the global tag list.
fn fetch_country_genres(agent: &ureq::Agent, code: &str) -> Result<Vec<TagItem>, String> {
    let url = format!(
        "{BASE}/json/stations/search?hidebroken=true&order=clickcount&reverse=true&countrycode={}&limit={COUNTRY_SCAN}",
        urlencode(code)
    );
    let rows: Vec<RawStation> = get_json(agent, &url)?;
    let mut counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for r in &rows {
        if r.lastcheckok != 1 {
            continue;
        }
        for tag in r.tags.split(',') {
            let t = tag.trim();
            if !t.is_empty() {
                *counts.entry(t.to_string()).or_insert(0) += 1;
            }
        }
    }
    let mut items: Vec<TagItem> = counts
        .into_iter()
        .map(|(name, count)| TagItem { name, count })
        .collect();
    // most common first; ties broken alphabetically for a stable order
    items.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    items.truncate(COUNTRY_GENRE_LIMIT);
    Ok(items)
}

/// Minimal percent-encoding for a query value (spaces + reserved chars).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Spawn the radio worker. `cache_dir` is where the directory snapshot lives.
/// Returns (request sender, result receiver).
pub fn spawn(cache_dir: PathBuf) -> (Sender<RadioRequest>, Receiver<RadioResult>) {
    let (req_tx, req_rx) = unbounded::<RadioRequest>();
    let (res_tx, res_rx) = unbounded::<RadioResult>();
    std::thread::Builder::new()
        .name("lyrfin-radio".into())
        .spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(5)))
                .timeout_recv_body(Some(Duration::from_secs(8))) // fail a hung mirror sooner → retry
                .user_agent(concat!("lyrfin/", env!("CARGO_PKG_VERSION")))
                .build()
                .into();
            let process = |req: RadioRequest| {
                // the directory load streams its own progress + result on res_tx
                if let RadioRequest::LoadDirectory {
                    force,
                    max_age_secs,
                } = req
                {
                    load_directory(&agent, &cache_dir, force, max_age_secs, &res_tx);
                    return;
                }
                let msg = match req {
                    RadioRequest::Search {
                        query,
                        country,
                        tag,
                        sort,
                        key,
                    } => match fetch_stations(
                        &agent,
                        &query,
                        country.as_deref(),
                        tag.as_deref(),
                        sort,
                    ) {
                        Ok(stations) => RadioResult::Found { key, stations },
                        Err(msg) => RadioResult::Error { key, msg },
                    },
                    RadioRequest::Countries { key } => match fetch_countries(&agent) {
                        Ok(items) => RadioResult::Countries { key, items },
                        Err(msg) => RadioResult::Error { key, msg },
                    },
                    RadioRequest::Tags { key } => match fetch_tags(&agent) {
                        Ok(items) => RadioResult::Tags { key, items },
                        Err(msg) => RadioResult::Error { key, msg },
                    },
                    RadioRequest::CountryGenres { code, key } => {
                        match fetch_country_genres(&agent, &code) {
                            Ok(items) => RadioResult::CountryGenres { code, key, items },
                            Err(msg) => RadioResult::Error { key, msg },
                        }
                    }
                    RadioRequest::LoadDirectory { .. } => return, // handled above
                };
                let _ = res_tx.send(msg);
            };
            while let Ok(first) = req_rx.recv() {
                // drain the backlog; only the latest *search* runs (live typing),
                // but every country/tag fetch is processed.
                let mut pending = vec![first];
                loop {
                    match req_rx.try_recv() {
                        Ok(r) => pending.push(r),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
                let last_search = pending
                    .iter()
                    .rposition(|r| matches!(r, RadioRequest::Search { .. }));
                for (i, r) in pending.into_iter().enumerate() {
                    if matches!(r, RadioRequest::Search { .. }) && Some(i) != last_search {
                        continue; // stale search
                    }
                    process(r);
                }
            }
        })
        .expect("spawn radio thread");
    (req_tx, res_rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urlencode_escapes_spaces_and_unicode() {
        assert_eq!(urlencode("jazz fm"), "jazz%20fm");
    }

    #[test]
    fn search_url_and_combines_filters() {
        let u = search_url("mega", Some("EG"), Some("pop"), Sort::Name);
        assert!(u.contains("name=mega"));
        assert!(u.contains("countrycode=EG"));
        assert!(u.contains("tag=pop") && !u.contains("tagList="));
        assert!(u.contains("order=name") && u.contains("reverse=false"));
        // an empty query / filters are simply omitted
        let u2 = search_url("", None, None, Sort::Popular);
        assert!(!u2.contains("name="));
        assert!(!u2.contains("countrycode="));
        assert!(u2.contains("order=clickcount") && u2.contains("reverse=true"));
    }

    #[test]
    fn clean_filters_hls_dead_and_empty_url() {
        let rows = vec![
            RawStation {
                name: "Good".into(),
                url_resolved: "http://x/stream".into(),
                lastcheckok: 1,
                hls: 0,
                bitrate: 128,
                tags: "pop,rock".into(),
                countrycode: "EG".into(),
                ..raw_default()
            },
            RawStation {
                name: "HLS".into(),
                url_resolved: "http://x/hls.m3u8".into(),
                lastcheckok: 1,
                hls: 1,
                ..raw_default()
            },
            RawStation {
                name: "Dead".into(),
                url_resolved: "http://x/dead".into(),
                lastcheckok: 0,
                hls: 0,
                ..raw_default()
            },
        ];
        let out = clean(rows);
        assert_eq!(out.len(), 1, "only the reachable non-HLS station survives");
        assert_eq!(out[0].subtitle(), "EG · pop · 128k");
    }

    #[test]
    fn sort_cycles_through_all() {
        let mut s = Sort::Popular;
        let seen: Vec<&str> = (0..4)
            .map(|_| {
                let l = s.label();
                s = s.next();
                l
            })
            .collect();
        assert_eq!(seen, ["popular", "votes", "name", "bitrate"]);
        assert_eq!(s, Sort::Popular, "wraps back");
    }

    fn raw_default() -> RawStation {
        RawStation {
            name: String::new(),
            url_resolved: String::new(),
            url: String::new(),
            codec: String::new(),
            bitrate: 0,
            country: String::new(),
            countrycode: String::new(),
            language: String::new(),
            tags: String::new(),
            homepage: String::new(),
            stationuuid: String::new(),
            clickcount: 0,
            votes: 0,
            clicktrend: 0,
            hls: 0,
            lastcheckok: 0,
        }
    }

    #[test]
    fn tidy_name_strips_decoration_and_whitespace() {
        assert_eq!(tidy_name("  ***  MEGA   FM  >>> "), "MEGA FM");
        assert_eq!(tidy_name("•• Jazz·Cafe ••"), "Jazz·Cafe");
        assert_eq!(tidy_name("Radio\tOne\n"), "Radio One");
        assert!(tidy_name("---").is_empty(), "all-decoration name → empty");
    }
}
