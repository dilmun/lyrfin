//! LRCLIB (lrclib.net) provider — free, no API key. Returns synced (LRC) and
//! plain lyrics by artist / title / album / duration. Community-contributed;
//! strongest for popular English tracks, with growing Arabic coverage.

use serde_json::Value;

use super::LyricsRequest;

const UA: &str = "lyrfin/0.1 ( https://github.com/lyrfin-player )";

/// Exact match first (artist/title/album/duration), then a looser search.
pub fn fetch(agent: &ureq::Agent, req: &LyricsRequest) -> Option<String> {
    get(agent, req).or_else(|| search(agent, req))
}

/// Exact match by artist/title/album/duration.
fn get(agent: &ureq::Agent, req: &LyricsRequest) -> Option<String> {
    let v: Value = agent
        .get("https://lrclib.net/api/get")
        .header("User-Agent", UA)
        .query("artist_name", &req.artist)
        .query("track_name", &req.title)
        .query("album_name", &req.album)
        .query("duration", req.duration_secs.to_string())
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    extract(&v)
}

/// Looser fallback search (no duration/album constraints).
fn search(agent: &ureq::Agent, req: &LyricsRequest) -> Option<String> {
    let v: Value = agent
        .get("https://lrclib.net/api/search")
        .header("User-Agent", UA)
        .query("track_name", &req.title)
        .query("artist_name", &req.artist)
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    v.as_array()?.iter().find_map(extract)
}

/// Prefer synced (LRC) lyrics, fall back to plain.
fn extract(v: &Value) -> Option<String> {
    let field = |k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    field("syncedLyrics").or_else(|| field("plainLyrics"))
}
