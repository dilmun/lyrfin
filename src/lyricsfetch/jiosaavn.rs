//! JioSaavn (jiosaavn.com) provider — India's largest streaming catalogue, free
//! and no API key. Fills the Hindi/Bollywood (and other Indian-language) gap that
//! LRCLIB and NetEase miss.
//!
//! Two hops over its unofficial public `api.php`: search for the track, then
//! fetch its lyrics. JioSaavn serves **plain** lyrics only (no timestamps) as an
//! HTML fragment — `<br>`-separated and entity-encoded — which we normalise to
//! plain text; lyrfin then scrolls it by playback progress (and the `,`/`.` sync
//! nudge shifts it via `lyrics_progress`). Any network/JSON hiccup returns
//! `None`, so the provider chain falls through.

use serde_json::Value;

use super::LyricsRequest;

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/120.0 Safari/537.36";
const API: &str = "https://www.jiosaavn.com/api.php";
/// Accept a search hit only if its duration is within this many seconds of the
/// track we're playing — guards against same-title covers / remixes.
const DURATION_TOL_SECS: u64 = 8;

/// Search for the track, then fetch its (plain) lyrics.
pub fn fetch(agent: &ureq::Agent, req: &LyricsRequest) -> Option<String> {
    let id = search_id(agent, req)?;
    lyrics(agent, &id)
}

/// Search `"<artist> <title>"` and pick the best song id that has lyrics.
fn search_id(agent: &ureq::Agent, req: &LyricsRequest) -> Option<String> {
    let query = format!("{} {}", req.artist.trim(), req.title.trim());
    let v: Value = agent
        .get(API)
        .header("User-Agent", UA)
        .query("__call", "search.getResults")
        .query("_format", "json")
        .query("_marker", "0")
        .query("_ctx", "web6dot0")
        .query("api_version", "4")
        .query("q", query.trim())
        .query("n", "5")
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    let results = v.get("results")?.as_array()?;
    pick_id(results, req.duration_secs)
}

/// Among hits that actually carry lyrics, prefer the closest duration match
/// within tolerance, else the first. JioSaavn nests `has_lyrics` / `duration`
/// (seconds, as strings) under `more_info`; the song id is a top-level string.
fn pick_id(results: &[Value], want_secs: u64) -> Option<String> {
    let mut first: Option<String> = None;
    let mut best: Option<(u64, String)> = None; // (secs_diff, id)
    for s in results {
        let mi = s.get("more_info");
        // only tracks that carry lyrics — fetching others returns an error body
        if mi.and_then(|m| m.get("has_lyrics")).and_then(Value::as_str) != Some("true") {
            continue;
        }
        let Some(id) = s.get("id").and_then(Value::as_str) else {
            continue;
        };
        if first.is_none() {
            first = Some(id.to_owned());
        }
        if want_secs == 0 {
            continue;
        }
        let dur = mi
            .and_then(|m| m.get("duration"))
            .and_then(Value::as_str)
            .and_then(|d| d.parse::<u64>().ok());
        if let Some(secs) = dur {
            let diff = secs.abs_diff(want_secs);
            if diff <= DURATION_TOL_SECS && best.as_ref().is_none_or(|(d, _)| diff < *d) {
                best = Some((diff, id.to_owned()));
            }
        }
    }
    best.map(|(_, id)| id).or(first)
}

/// Fetch a song's lyrics and normalise the HTML fragment to plain text.
fn lyrics(agent: &ureq::Agent, id: &str) -> Option<String> {
    let v: Value = agent
        .get(API)
        .header("User-Agent", UA)
        .query("__call", "lyrics.getLyrics")
        .query("lyrics_id", id)
        .query("_format", "json")
        .query("_marker", "0")
        .query("ctx", "web6dot0")
        .query("api_version", "4")
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    let raw = v.get("lyrics").and_then(Value::as_str)?;
    let text = normalise(raw);
    (!text.is_empty()).then_some(text)
}

/// `<br>`-separated, entity-encoded HTML → plain UTF-8 text.
fn normalise(raw: &str) -> String {
    let with_newlines = raw
        .replace("<br/>", "\n")
        .replace("<br />", "\n")
        .replace("<br>", "\n");
    decode_entities(&with_newlines).trim().to_string()
}

/// Decode the handful of HTML entities JioSaavn emits. `&amp;` is decoded last so
/// an already-escaped `&amp;quot;` isn't over-decoded.
fn decode_entities(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn hit(id: &str, has_lyrics: bool, dur: u64) -> Value {
        json!({
            "id": id,
            "more_info": { "has_lyrics": has_lyrics.to_string(), "duration": dur.to_string() },
        })
    }

    #[test]
    fn skips_hits_without_lyrics() {
        let r = [hit("a", false, 200), hit("b", true, 300)];
        // "a" is the top hit but has no lyrics → "b" is chosen
        assert_eq!(pick_id(&r, 0).as_deref(), Some("b"));
    }

    #[test]
    fn picks_closest_duration_among_lyric_hits() {
        let r = [
            hit("a", true, 200),
            hit("b", true, 245),
            hit("c", true, 400),
        ];
        // want 244s → "b" (245, 1s off) within tolerance beats the top hit
        assert_eq!(pick_id(&r, 244).as_deref(), Some("b"));
    }

    #[test]
    fn none_when_no_hit_has_lyrics() {
        let r = [hit("a", false, 200), hit("b", false, 300)];
        assert_eq!(pick_id(&r, 250), None);
    }

    #[test]
    fn normalise_br_and_entities() {
        let raw = "Line one<br>Say &quot;hi&quot;<br/>Tom &amp; Jerry<br><br>";
        assert_eq!(normalise(raw), "Line one\nSay \"hi\"\nTom & Jerry");
    }
}
