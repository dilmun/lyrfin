//! Resolve a Spotify podcast episode to a playable **public MP3**.
//!
//! Spotify won't grant podcast decryption keys to third-party (librespot)
//! clients, so Spotify-hosted episode audio can't be decoded (see the memory note
//! *librespot-episode-playback*). But almost every podcast is *also* distributed
//! via a public RSS feed, where the audio is a plain, DRM-free MP3. This worker
//! finds that MP3:
//!
//! 1. Apple's iTunes Search API (free, no key) maps the **show name** → its RSS
//!    `feedUrl` (cached per show).
//! 2. The feed is fetched + parsed; the **episode title** is matched to an item,
//!    whose `<enclosure>` URL is the MP3.
//!
//! The app then streams that URL through its own engine. Runs off-thread so the
//! UI never blocks; latest-wins coalesced (only the current episode matters).

use std::collections::HashMap;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};
use serde_json::Value;

const UA: &str = "lyrfin/0.1 ( https://github.com/lyrfin-player )";
/// Don't read an unbounded RSS feed into memory (some are large).
const MAX_FEED_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct PodcastRequest {
    /// The show's name — looked up against the iTunes podcast directory.
    pub show: String,
    /// The episode's title — matched within the resolved feed.
    pub title: String,
    /// The episode uri the app assigned; echoed back so it can match the current
    /// now-playing item and ignore stale results.
    pub key: String,
}

#[derive(Debug, Clone)]
pub struct PodcastResult {
    pub key: String,
    /// A directly-streamable MP3 URL, or `None` if no public match was found.
    pub url: Option<String>,
}

/// Spawn the podcast resolver; returns (request sender, result receiver).
pub fn spawn() -> (Sender<PodcastRequest>, Receiver<PodcastResult>) {
    let (req_tx, req_rx) = unbounded::<PodcastRequest>();
    let (res_tx, res_rx) = unbounded::<PodcastResult>();
    std::thread::Builder::new()
        .name("lyrfin-podcast".into())
        .spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(6)))
                .timeout_recv_body(Some(Duration::from_secs(12)))
                .build()
                .into();
            // show name (normalised) → RSS feed url; feeds don't move within a run
            let mut feed_cache: HashMap<String, String> = HashMap::new();
            while let Ok(req) = req_rx.recv() {
                // coalesce: only the latest episode the user asked for matters
                let mut req = req;
                while let Ok(newer) = req_rx.try_recv() {
                    req = newer;
                }
                let url = resolve(&agent, &req, &mut feed_cache);
                let _ = res_tx.send(PodcastResult { key: req.key, url });
            }
        })
        .expect("spawn podcast thread");
    (req_tx, res_rx)
}

/// Find the episode's public MP3: show → feed (cached) → match the title → MP3.
fn resolve(
    agent: &ureq::Agent,
    req: &PodcastRequest,
    cache: &mut HashMap<String, String>,
) -> Option<String> {
    if req.show.trim().is_empty() || req.title.trim().is_empty() {
        return None;
    }
    // a cached feed for this show first; otherwise try the directory's top matches
    let mut feeds: Vec<String> = Vec::new();
    if let Some(f) = cache.get(&norm(&req.show)) {
        feeds.push(f.clone());
    }
    feeds.extend(itunes_feeds(agent, &req.show));
    feeds.dedup();

    for feed in feeds {
        if let Some(url) = match_episode(agent, &feed, &req.title) {
            cache.insert(norm(&req.show), feed); // remember the feed that worked
            return Some(url);
        }
    }
    None
}

/// RSS feed URLs for a show name, best matches first (iTunes ranks by relevance).
fn itunes_feeds(agent: &ureq::Agent, show: &str) -> Vec<String> {
    let Ok(mut resp) = agent
        .get("https://itunes.apple.com/search")
        .header("User-Agent", UA)
        .query("media", "podcast")
        .query("limit", "5")
        .query("term", show)
        .call()
    else {
        return Vec::new();
    };
    let Ok(v) = resp.body_mut().read_json::<Value>() else {
        return Vec::new();
    };
    let Some(results) = v.get("results").and_then(Value::as_array) else {
        return Vec::new();
    };
    // prefer feeds whose collection name matches the show, but keep the rest as
    // fallbacks (a show can have several feeds; only the right one has the episode)
    let want = norm(show);
    let mut exact = Vec::new();
    let mut rest = Vec::new();
    for r in results {
        let Some(feed) = r.get("feedUrl").and_then(Value::as_str) else {
            continue;
        };
        let name = r
            .get("collectionName")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if title_match(&want, &norm(name)) {
            exact.push(feed.to_string());
        } else {
            rest.push(feed.to_string());
        }
    }
    exact.extend(rest);
    exact
}

/// Fetch + parse a feed and return the MP3 enclosure URL of the item matching
/// `title`.
fn match_episode(agent: &ureq::Agent, feed: &str, title: &str) -> Option<String> {
    let resp = agent.get(feed).header("User-Agent", UA).call().ok()?;
    let mut buf = Vec::new();
    use std::io::Read as _;
    resp.into_body()
        .into_reader()
        .take(MAX_FEED_BYTES)
        .read_to_end(&mut buf)
        .ok()?;
    let channel = rss::Channel::read_from(&buf[..]).ok()?;
    let want = norm(title);

    // prefer an exact title match; fall back to a containment match (Spotify
    // occasionally tweaks a title vs the feed)
    let mut fallback: Option<String> = None;
    for item in channel.items() {
        let (Some(t), Some(enc)) = (item.title(), item.enclosure()) else {
            continue;
        };
        let url = enc.url();
        if url.is_empty() {
            continue;
        }
        let it = norm(t);
        if it == want {
            return Some(url.to_string());
        }
        if fallback.is_none() && title_match(&want, &it) {
            fallback = Some(url.to_string());
        }
    }
    fallback
}

/// Normalise a title/name for comparison: lowercase, keep alphanumerics + spaces,
/// collapse whitespace. Keeps episode numbers (e.g. "#2516" → "2516").
fn norm(s: &str) -> String {
    let cleaned: String = s
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect();
    cleaned.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether two already-normalised strings match: equal, or one contains the other
/// (guarded by a minimum length so short titles don't over-match).
fn title_match(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    a == b || (a.len() >= 6 && b.len() >= 6 && (a.contains(b) || b.contains(a)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_titles() {
        assert_eq!(norm("#2516 - Rowan Jacobsen!"), "2516 rowan jacobsen");
        assert_eq!(norm("  Mixed   CASE  "), "mixed case");
    }

    #[test]
    fn matches_exact_and_contained_titles() {
        assert!(title_match(
            &norm("#664 - Crooners Welcome"),
            &norm("#664 - Crooners Welcome")
        ));
        // feed prepends the show name → containment still matches
        assert!(title_match(
            &norm("Gavin de Becker on fear"),
            &norm("DOAC: Gavin de Becker on fear")
        ));
        // unrelated titles don't match
        assert!(!title_match(
            &norm("Episode about cats"),
            &norm("Totally different show")
        ));
        // too-short tokens don't over-match via containment
        assert!(!title_match(&norm("a b"), &norm("a b c d e f")));
    }
}
