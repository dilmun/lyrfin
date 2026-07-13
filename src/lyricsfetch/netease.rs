//! NetEase Cloud Music (music.163.com) provider — the largest
//! Chinese/Japanese/Korean catalogue (incl. K-pop), free and no API key.
//!
//! Two hops over its (unofficial, `Referer`-gated) public API: search for the
//! track, then fetch its synced LRC. NetEase serves the lyrics and their
//! translation as two LRC tracks sharing timestamps. Its translation is **always
//! Chinese**, so we only interleave it — letting [`crate::lyrics::Lyrics::parse`]
//! pair each original line with its translation (its "two lines at the same
//! timestamp" rule) — when the user's translation target is Chinese; otherwise we
//! return the original alone and lyrfin's own translator renders the configured
//! language. Any network/JSON hiccup returns `None`, so the chain falls through.

use serde_json::Value;

use super::LyricsRequest;

/// NetEase blocks non-browser clients; a browser UA + site `Referer` is enough.
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/120.0 Safari/537.36";
const REFERER: &str = "https://music.163.com/";
/// Accept a search hit only if its duration is within this many seconds of the
/// track we're playing — guards against same-title covers / remixes.
const DURATION_TOL_SECS: u64 = 8;

/// Search for the track, then fetch its LRC — bilingual only when the user reads
/// Chinese, since NetEase's translation track is always Chinese.
pub fn fetch(agent: &ureq::Agent, req: &LyricsRequest) -> Option<String> {
    let id = search_id(agent, req)?;
    lyric(agent, id, req.translate_to.starts_with("zh"))
}

/// Search `"<artist> <title>"` and pick the best-matching song id.
fn search_id(agent: &ureq::Agent, req: &LyricsRequest) -> Option<u64> {
    let query = format!("{} {}", req.artist.trim(), req.title.trim());
    let v: Value = agent
        .get("https://music.163.com/api/search/get")
        .header("User-Agent", UA)
        .header("Referer", REFERER)
        .query("s", query.trim())
        .query("type", "1") // 1 = songs
        .query("limit", "5")
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    let songs = v.get("result")?.get("songs")?.as_array()?;
    pick_id(songs, req.duration_secs)
}

/// Prefer the closest duration match within tolerance, else the top (most
/// relevant) hit. NetEase durations are in milliseconds.
fn pick_id(songs: &[Value], want_secs: u64) -> Option<u64> {
    let mut top: Option<u64> = None;
    let mut best: Option<(u64, u64)> = None; // (secs_diff, id)
    for s in songs {
        let Some(id) = s.get("id").and_then(Value::as_u64) else {
            continue;
        };
        top.get_or_insert(id);
        if want_secs == 0 {
            continue;
        }
        let Some(ms) = s.get("duration").and_then(Value::as_u64) else {
            continue;
        };
        let diff = (ms / 1000).abs_diff(want_secs);
        if diff <= DURATION_TOL_SECS && best.is_none_or(|(d, _)| diff < d) {
            best = Some((diff, id));
        }
    }
    best.map(|(_, id)| id).or(top)
}

/// Fetch a song's LRC, merging the (Chinese) translation track only when
/// `merge_trans` and one exists.
fn lyric(agent: &ureq::Agent, id: u64, merge_trans: bool) -> Option<String> {
    let v: Value = agent
        .get("https://music.163.com/api/song/lyric")
        .header("User-Agent", UA)
        .header("Referer", REFERER)
        .query("id", id.to_string())
        .query("lv", "1") // original lyric
        .query("tv", "-1") // translated lyric (if any)
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    let lrc = lyric_field(&v, "lrc")?;
    Some(combine(lrc, lyric_field(&v, "tlyric"), merge_trans))
}

/// Interleave the original with its (Chinese) translation when `merge_trans` and
/// a translation exists — same timestamps, so the parser pairs each line. Else
/// the original alone, leaving translation to lyrfin's own translator.
fn combine(lrc: String, tlyric: Option<String>, merge_trans: bool) -> String {
    match tlyric.filter(|_| merge_trans) {
        Some(trans) => format!("{lrc}\n{trans}"),
        None => lrc,
    }
}

/// Non-empty `<field>.lyric` text, e.g. `lrc.lyric` / `tlyric.lyric`.
fn lyric_field(v: &Value, field: &str) -> Option<String> {
    v.get(field)?
        .get("lyric")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn song(id: u64, dur_ms: u64) -> Value {
        json!({ "id": id, "duration": dur_ms })
    }

    #[test]
    fn picks_closest_duration_within_tolerance() {
        let songs = [song(1, 200_000), song(2, 183_000), song(3, 240_000)];
        // want 184s → song 2 (183s, 1s off) beats the top hit (song 1, 200s)
        assert_eq!(pick_id(&songs, 184), Some(2));
    }

    #[test]
    fn falls_back_to_top_hit_when_none_within_tolerance() {
        let songs = [song(1, 200_000), song(2, 300_000)];
        // want 100s → nothing within tolerance → most-relevant (first) hit
        assert_eq!(pick_id(&songs, 100), Some(1));
    }

    #[test]
    fn takes_top_hit_when_duration_unknown() {
        let songs = [song(7, 200_000), song(8, 201_000)];
        assert_eq!(pick_id(&songs, 0), Some(7));
    }

    #[test]
    fn none_when_no_songs() {
        assert_eq!(pick_id(&[], 180), None);
    }

    #[test]
    fn combine_merges_translation_only_when_wanted() {
        // user reads Chinese → interleave original + translation
        assert_eq!(combine("a".into(), Some("b".into()), true), "a\nb");
        // user reads another language → original only; lyrfin translates on top
        assert_eq!(combine("a".into(), Some("b".into()), false), "a");
        // no translation track → original regardless
        assert_eq!(combine("a".into(), None, true), "a");
    }

    #[test]
    fn lyric_field_extracts_nonempty_and_trims() {
        let v = json!({ "lrc": { "lyric": "  [00:01.00]hi\n" }, "tlyric": { "lyric": "" } });
        assert_eq!(lyric_field(&v, "lrc").as_deref(), Some("[00:01.00]hi"));
        assert_eq!(lyric_field(&v, "tlyric"), None); // empty → None
        assert_eq!(lyric_field(&v, "missing"), None);
    }
}
