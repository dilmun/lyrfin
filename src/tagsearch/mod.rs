//! Online tag/metadata search via **iTunes** (`entity=song`) + **Deezer** — both
//! keyless, good Arabic *and* English coverage. Album mode also queries
//! **MusicBrainz**, whose release DB carries the Deluxe / regional editions the
//! other two often lack. A `Search` returns candidate tag sets to preview; an
//! `Apply` writes the chosen tags into the song (all fields) and, optionally, the
//! album-level fields into every album track. Runs on a worker thread so the UI
//! never blocks.

use std::path::PathBuf;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};
use serde_json::Value;

mod sources;
use sources::*;

use crate::tags::EditableTags;

const UA: &str = "lyrfin/0.1 ( https://github.com/lyrfin-player )";
const MAX_RESULTS: usize = 10;

/// One candidate tag set found online.
#[derive(Clone, Default)]
pub struct TagCandidate {
    pub source: &'static str,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub year: Option<u16>,
    pub genre: Option<String>,
    pub track_no: Option<u16>,
    pub track_total: Option<u16>,
    pub disc_no: Option<u16>,
}

/// A matched album from one source, with its full (track-ordered) tracklist.
/// `album`/`artist` carry the matched names for display once the album-pick UI
/// surfaces them.
#[allow(dead_code)]
#[derive(Clone)]
pub struct AlbumMatch {
    pub source: &'static str,
    pub album: String,
    pub artist: String,
    pub tracks: Vec<TagCandidate>,
}

pub enum TagRequest {
    Search {
        query: String,
        /// The current track's artist/title/year + local album track count —
        /// results must match the artist (exact or name-prefix variant), are
        /// ordered by title similarity, and the matching album-total / year win
        /// ties (so the right edition wins).
        artist: String,
        title: String,
        year: Option<u16>,
        track_count: usize,
        key: String,
    },
    /// Fetch the whole album (full tracklist) from each source. `track_count` is
    /// the local album's track count — editions closest to it rank first (so a
    /// Deluxe local copy matches the Deluxe release, not the regular one).
    AlbumSearch {
        artist: String,
        album: String,
        track_count: usize,
        key: String,
    },
    Apply {
        fields: TagCandidate,
        song: PathBuf,
        /// Other album tracks to receive the album-level fields (empty = song only).
        album: Vec<PathBuf>,
        key: String,
    },
    /// Write per-track fetched tags to each matched local file.
    ApplyAlbum {
        assignments: Vec<(PathBuf, TagCandidate)>,
        key: String,
    },
}

pub enum TagResult {
    Found {
        key: String,
        candidates: Vec<TagCandidate>,
    },
    AlbumFound {
        key: String,
        albums: Vec<AlbumMatch>,
    },
    /// Reserved error path for the tag-search worker; not emitted yet.
    #[allow(dead_code)]
    Error { key: String, msg: String },
    Applied {
        key: String,
        count: usize,
        msg: String,
    },
}

/// Spawn the tag worker; returns (request sender, result receiver).
pub fn spawn() -> (Sender<TagRequest>, Receiver<TagResult>) {
    let (req_tx, req_rx) = unbounded::<TagRequest>();
    let (res_tx, res_rx) = unbounded::<TagResult>();
    std::thread::Builder::new()
        .name("lyrfin-tags".into())
        .spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(6)))
                .timeout_recv_body(Some(Duration::from_secs(10)))
                .build()
                .into();
            let process = |req: TagRequest| match req {
                TagRequest::Search {
                    query,
                    artist,
                    title,
                    year,
                    track_count,
                    key,
                } => {
                    let mut out = Vec::new();
                    itunes(&agent, &query, &mut out);
                    deezer(&agent, &query, &mut out);
                    rank_and_filter(&mut out, &artist, &title, year, track_count);
                    out.truncate(MAX_RESULTS);
                    let _ = res_tx.send(TagResult::Found {
                        key,
                        candidates: out,
                    });
                }
                TagRequest::AlbumSearch {
                    artist,
                    album,
                    track_count,
                    key,
                } => {
                    let mut albums = Vec::new();
                    itunes_album(&agent, &artist, &album, track_count, &mut albums);
                    deezer_album(&agent, &artist, &album, track_count, &mut albums);
                    // MusicBrainz carries the Deluxe/regional editions the other
                    // two often lack (slower: rate-limited).
                    musicbrainz_album(&agent, &artist, &album, track_count, &mut albums);
                    rank_editions(&mut albums, track_count);
                    let _ = res_tx.send(TagResult::AlbumFound { key, albums });
                }
                TagRequest::Apply {
                    fields,
                    song,
                    album,
                    key,
                } => {
                    let mut count = 0usize;
                    let mut last_err = None;
                    match apply_one(&song, &fields, true) {
                        Ok(()) => count += 1,
                        Err(e) => last_err = Some(e),
                    }
                    for p in &album {
                        if p == &song {
                            continue;
                        }
                        match apply_one(p, &fields, false) {
                            Ok(()) => count += 1,
                            Err(e) => last_err = Some(e),
                        }
                    }
                    let msg = match last_err {
                        Some(e) if count == 0 => format!("Tag write failed: {e}"),
                        _ => format!("Applied tags to {count} track(s)"),
                    };
                    let _ = res_tx.send(TagResult::Applied { key, count, msg });
                }
                TagRequest::ApplyAlbum { assignments, key } => {
                    let mut count = 0usize;
                    let mut last_err = None;
                    for (path, fields) in &assignments {
                        match apply_one(path, fields, true) {
                            Ok(()) => count += 1,
                            Err(e) => last_err = Some(e),
                        }
                    }
                    let msg = match last_err {
                        Some(e) if count == 0 => format!("Tag write failed: {e}"),
                        _ => format!("Applied album tags to {count} track(s)"),
                    };
                    let _ = res_tx.send(TagResult::Applied { key, count, msg });
                }
            };
            // Coalesce: process every Apply, but collapse a burst of queued
            // searches to the latest (older queries are stale).
            while let Ok(req) = req_rx.recv() {
                let mut pending = vec![req];
                while let Ok(more) = req_rx.try_recv() {
                    pending.push(more);
                }
                let is_search = |r: &TagRequest| {
                    matches!(
                        r,
                        TagRequest::Search { .. } | TagRequest::AlbumSearch { .. }
                    )
                };
                let last_search = pending.iter().rposition(is_search);
                for (i, r) in pending.into_iter().enumerate() {
                    if is_search(&r) && Some(i) != last_search {
                        continue; // stale search
                    }
                    process(r);
                }
            }
        })
        .expect("spawn tags thread");
    (req_tx, res_rx)
}

/// Normalised key for an *exact* name compare: lowercase, alphanumerics only.
/// Ignores case / spacing / punctuation, and keeps non-Latin scripts (Arabic),
/// so "Adele" == "adele" but "Adele" != "Jason Adele AAAA".
fn norm(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// Ordered, lowercased name tokens (for prefix comparison).
fn name_tokens(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether two artist names refer to the same artist, tolerating a trailing
/// surname/variant: one name's token list must be a whole-token *prefix* of the
/// other's. So "Assala" matches "Assala Nasri" (services credit her both ways),
/// but "Adele" still does NOT match "Jason Adele AAAA" (adele isn't the 1st token).
pub(crate) fn artist_match(a: &str, b: &str) -> bool {
    let (ta, tb) = (name_tokens(a), name_tokens(b));
    if ta.is_empty() || tb.is_empty() {
        return false;
    }
    let (short, long) = if ta.len() <= tb.len() {
        (&ta, &tb)
    } else {
        (&tb, &ta)
    };
    short.iter().zip(long).all(|(x, y)| x == y)
}

/// Word set for fuzzy title comparison.
fn tokens(s: &str) -> std::collections::HashSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_string)
        .collect()
}

/// The album's *core* name — everything before an edition suffix like
/// "(Deluxe Edition)", "[Remastered]", ": Deluxe" or " - Deluxe" — normalised.
/// So "Hello (Deluxe Edition)" and "Hello" resolve to the same album.
fn album_core(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut end = lower.len();
    for pat in ["(", "[", ":"] {
        if let Some(i) = lower.find(pat) {
            end = end.min(i);
        }
    }
    if let Some(i) = lower.find(" - ") {
        end = end.min(i);
    }
    norm(&lower[..end])
}

/// Whether a candidate album title is the same album as `local` (ignoring an
/// edition suffix). Empty local name matches anything (artist filter still applies).
pub(crate) fn album_matches(cand: &str, local: &str) -> bool {
    let lc = album_core(local);
    lc.is_empty() || album_core(cand) == lc
}

/// Jaccard token similarity (intersection / union) in 0.0..=1.0.
pub(crate) fn title_sim(a: &str, b: &str) -> f32 {
    let (ta, tb) = (tokens(a), tokens(b));
    if ta.is_empty() || tb.is_empty() {
        return 0.0;
    }
    let inter = ta.intersection(&tb).count() as f32;
    let union = ta.union(&tb).count() as f32;
    inter / union
}

/// Minimum title similarity to keep a candidate — drops same-artist songs whose
/// title doesn't actually match (e.g. searching Adele "I Miss You" must not list
/// "Someone Like You" / "When We Were Young").
const TITLE_MATCH: f32 = 0.4;

/// Tighten the candidate list: (1) require a matching artist ([`artist_match`] —
/// exact, or a name-prefix variant), (2) require the title to actually match
/// (fuzzy, above [`TITLE_MATCH`]), then order by
/// matching album track total (so a Deluxe local copy picks the Deluxe edition's
/// song = right track total), then matching year, then title. `count` = the local
/// album's track count (0 = unknown). With no artist/title that filter is skipped.
fn rank_and_filter(
    out: &mut Vec<TagCandidate>,
    artist: &str,
    title: &str,
    year: Option<u16>,
    count: usize,
) {
    if !artist.trim().is_empty() {
        out.retain(|c| artist_match(&c.artist, artist));
    }
    if !title.trim().is_empty() {
        out.retain(|c| title_sim(&c.title, title) >= TITLE_MATCH);
    }
    let score = |c: &TagCandidate| {
        let total_match = count > 0 && c.track_total == Some(count as u16);
        let year_match = matches!((year, c.year), (Some(a), Some(b)) if a == b);
        (if total_match { 50.0 } else { 0.0 })
            + (if year_match { 20.0 } else { 0.0 })
            + title_sim(&c.title, title)
    };
    out.sort_by(|a, b| {
        score(b)
            .partial_cmp(&score(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// Write a candidate's tags to one file. `full` writes every field; otherwise
/// only the album-level fields (album / album-artist / year / genre).
fn apply_one(path: &std::path::Path, c: &TagCandidate, full: bool) -> Result<(), String> {
    let mut e = EditableTags::default();
    let mut dirty = [false; 13];
    let mut set = |i: usize, slot: &mut String, val: String| {
        if !val.is_empty() {
            *slot = val;
            dirty[i] = true;
        }
    };
    // album-level fields (always written)
    set(2, &mut e.album, c.album.clone());
    let aa = if !c.album_artist.is_empty() {
        c.album_artist.clone()
    } else {
        c.artist.clone()
    };
    set(3, &mut e.album_artist, aa);
    if let Some(y) = c.year {
        set(8, &mut e.year, y.to_string());
    }
    if let Some(g) = &c.genre {
        set(9, &mut e.genre, g.clone());
    }
    // track-level fields (only on the song itself)
    if full {
        set(0, &mut e.title, c.title.clone());
        set(1, &mut e.artist, c.artist.clone());
        if let Some(t) = c.track_no {
            set(4, &mut e.track_no, t.to_string());
        }
        if let Some(t) = c.track_total {
            set(5, &mut e.track_total, t.to_string());
        }
        if let Some(d) = c.disc_no {
            set(6, &mut e.disc_no, d.to_string());
        }
    }
    crate::tags::write_tags(path, &e, &dirty)
}

/// Order album editions so the one closest to the local track `count` is first
/// (e.g. a 16-track Deluxe local copy picks the 16-track release, not the 11).
fn rank_editions(albums: &mut [AlbumMatch], count: usize) {
    if count > 0 {
        albums.sort_by_key(|a| a.tracks.len().abs_diff(count));
    }
}

/// Map each local track `(track_no, title)` to the index of its best fetched
/// track (or `None`). Match by track number first, then title similarity; every
/// fetched track is used at most once.
pub fn match_album(local: &[(u16, String)], fetched: &[TagCandidate]) -> Vec<Option<usize>> {
    let mut used = vec![false; fetched.len()];
    let mut out = vec![None; local.len()];
    for (li, (tno, _)) in local.iter().enumerate() {
        if *tno == 0 {
            continue;
        }
        for (fi, f) in fetched.iter().enumerate() {
            if !used[fi] && f.track_no == Some(*tno) {
                out[li] = Some(fi);
                used[fi] = true;
                break;
            }
        }
    }
    for (li, (_, title)) in local.iter().enumerate() {
        if out[li].is_some() {
            continue;
        }
        let mut best: Option<(usize, f32)> = None;
        for (fi, f) in fetched.iter().enumerate() {
            if used[fi] {
                continue;
            }
            let s = title_sim(&f.title, title);
            if s >= TITLE_MATCH && best.map(|b| s > b.1).unwrap_or(true) {
                best = Some((fi, s));
            }
        }
        if let Some((fi, _)) = best {
            out[li] = Some(fi);
            used[fi] = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(artist: &str, title: &str, year: Option<u16>) -> TagCandidate {
        TagCandidate {
            source: "x",
            artist: artist.into(),
            title: title.into(),
            year,
            ..Default::default()
        }
    }

    #[test]
    fn artist_mid_token_dropped() {
        // "Adele" must not match "Jason Adele AAAA" (adele isn't the first token)
        let mut v = vec![
            mk("Jason Adele AAAA", "Hello", None),
            mk("adele", "Hello", None), // case-insensitive match kept
        ];
        rank_and_filter(&mut v, "Adele", "Hello", None, 0);
        assert_eq!(v.len(), 1, "only the matching artist survives");
        assert_eq!(v[0].artist, "adele");
    }

    #[test]
    fn artist_prefix_variant_matches() {
        // services credit her "Assala"; the local tag is "Assala Nasri" — match
        assert!(artist_match("Assala", "Assala Nasri"));
        assert!(artist_match("Assala Nasri", "Assala"));
        assert!(!artist_match("Adele", "Jason Adele AAAA"));
        assert!(!artist_match("Prince Royce", "Prince Charming")); // 2nd token differs
        let mut v = vec![
            mk("Assala", "Aktar", None),
            mk("Other Singer", "Aktar", None),
        ];
        rank_and_filter(&mut v, "Assala Nasri", "Aktar", None, 0);
        assert_eq!(v.len(), 1);
        assert_eq!(
            v[0].artist, "Assala",
            "credited 'Assala' kept for 'Assala Nasri'"
        );
    }

    #[test]
    fn same_artist_wrong_title_dropped() {
        // Adele "I Miss You" must not list her other (different-title) songs
        let mut v = vec![
            mk("Adele", "I Miss You", None),
            mk("Adele", "When We Were Young", None),
            mk("Adele", "I Miss You", None),
            mk("Adele", "Someone Like You", None),
        ];
        rank_and_filter(&mut v, "Adele", "I Miss You", None, 0);
        assert_eq!(v.len(), 2, "only the title-matching songs survive");
        assert!(v.iter().all(|c| c.title == "I Miss You"));
    }

    #[test]
    fn title_variant_kept() {
        let mut v = vec![
            mk("Adele", "I Miss You (Live)", None),
            mk("Adele", "Hometown Glory", None),
        ];
        rank_and_filter(&mut v, "Adele", "I Miss You", None, 0);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].title, "I Miss You (Live)", "close title variant kept");
    }

    #[test]
    fn year_wins_ties_then_title() {
        let mut v = vec![
            mk("Adele", "Hello", Some(1999)),        // wrong year
            mk("Adele", "Hello (Live)", Some(2015)), // right year, looser title
            mk("Adele", "Hello", Some(2015)),        // right year + exact title
        ];
        rank_and_filter(&mut v, "Adele", "Hello", Some(2015), 0);
        assert_eq!(v[0].year, Some(2015));
        assert_eq!(v[0].title, "Hello", "matching year + best title first");
    }

    #[test]
    fn album_core_strips_edition_suffix() {
        assert_eq!(album_core("Hello (Deluxe Edition)"), album_core("Hello"));
        assert_eq!(album_core("25 [Remastered]"), album_core("25"));
        assert_eq!(album_core("30: Deluxe"), album_core("30"));
        assert_eq!(album_core("Hello - Deluxe Edition"), album_core("Hello"));
        assert!(album_matches("Hello (Deluxe Edition)", "Hello"));
        assert!(!album_matches("Goodbye", "Hello"));
    }

    #[test]
    fn edition_closest_to_local_count_wins() {
        let album = |n: usize| AlbumMatch {
            source: "x",
            album: "Hello".into(),
            artist: "Adele".into(),
            tracks: (0..n).map(|_| mk("Adele", "t", None)).collect(),
        };
        // local copy is the 16-track deluxe; regular(11) is fetched first
        let mut albums = vec![album(11), album(16), album(13)];
        rank_editions(&mut albums, 16);
        assert_eq!(albums[0].tracks.len(), 16, "deluxe (matching count) first");
    }

    #[test]
    fn album_matches_by_track_then_title() {
        let mut t1 = mk("A", "Neon Rain", None);
        t1.track_no = Some(2);
        let mut t2 = mk("A", "Midnight Protocol", None);
        t2.track_no = Some(1);
        let t3 = mk("A", "Afterglow", None); // no track number
        let fetched = vec![t1, t2, t3];
        let local = vec![
            (1u16, "midnight protocol".to_string()),
            (2u16, "neon rain".to_string()),
            (0u16, "afterglow".to_string()),
        ];
        let m = match_album(&local, &fetched);
        assert_eq!(m[0], Some(1), "local #1 → fetched track 1");
        assert_eq!(m[1], Some(0), "local #2 → fetched track 2");
        assert_eq!(m[2], Some(2), "no-number local → title match");
    }

    #[test]
    fn edited_query_filters_to_artist() {
        // user edits the query to "Diana" → only Diana… artists, no song titled Diana
        let mut v = vec![
            mk("Paul Anka", "Diana", None),
            mk("Michael Jackson", "Dirty Diana", None),
            mk("Diana Haddad", "Ya Bashar", None),
        ];
        rank_and_filter(&mut v, "Diana", "", None, 0); // edited: artist=query, no title
        assert_eq!(v.len(), 1, "only the Diana… artist survives");
        assert_eq!(v[0].artist, "Diana Haddad");
    }

    #[test]
    fn empty_artist_keeps_all() {
        let mut v = vec![mk("A", "t", None), mk("B", "t", None)];
        rank_and_filter(&mut v, "", "", None, 0);
        assert_eq!(v.len(), 2, "no artist to match → keep everything");
    }
}
