//! Prefix-token inverted index: `token → roaring bitmap of track ids`.
//!
//! Plain-word search is fuzzy (nucleo) and would otherwise scan every track on
//! each keystroke. This index narrows that to a small candidate set first: each
//! query word is matched as a *prefix* of an indexed token, the per-word hits are
//! AND-combined into a bitmap, and only those tracks are handed to nucleo for
//! ranking. When the index finds nothing (a typo, or a mid-word/scattered query
//! that prefix matching can't express) the caller falls back to a full fuzzy
//! scan, so nothing fuzzy would have matched is silently dropped.
//!
//! The index is keyed on `AppState::lib_gen`, so it's rebuilt only when the track
//! set or tags change — never per keystroke (a query never mutates the library).

use std::collections::BTreeMap;

use roaring::RoaringBitmap;

use crate::core::model::Track;

#[derive(Default)]
pub struct SearchIndex {
    /// The library generation this index reflects; `None` until first built.
    built_for: Option<u64>,
    /// token → ids of tracks whose artist/album/title contains that token.
    /// A `BTreeMap` (not `HashMap`) so a query word can range-scan every token
    /// that shares its prefix in `O(log n + hits)`.
    map: BTreeMap<Box<str>, RoaringBitmap>,
}

impl SearchIndex {
    /// Whether the index already reflects library generation `generation`.
    pub fn is_fresh(&self, generation: u64) -> bool {
        self.built_for == Some(generation)
    }

    /// Rebuild from the current tracks, tagging it as `generation`. Indexes the
    /// same fields the fuzzy haystack uses (artist + album + title) so a
    /// candidate is never something nucleo would then reject for the field.
    pub fn rebuild<'a>(&mut self, tracks: impl Iterator<Item = &'a Track>, generation: u64) {
        self.map.clear();
        for t in tracks {
            let id = t.id.get();
            index_text(&mut self.map, &t.artist, id);
            index_text(&mut self.map, &t.album, id);
            index_text(&mut self.map, &t.title, id);
        }
        self.built_for = Some(generation);
    }

    /// Candidate track ids for `query`: tracks where **every** query word is a
    /// prefix of some indexed token (bitmap AND of the per-word prefix unions).
    ///
    /// `None` means the query has no usable word (caller → full fuzzy scan); an
    /// empty bitmap means words were present but matched nothing (also a signal
    /// to fall back, since fuzzy may still match mid-word).
    pub fn candidates(&self, query: &str) -> Option<RoaringBitmap> {
        let mut acc: Option<RoaringBitmap> = None;
        for word in tokens(query) {
            let hits = self.prefix_union(&word);
            acc = Some(match acc {
                None => hits,
                Some(mut a) => {
                    a &= &hits;
                    a
                }
            });
            if acc.as_ref().is_some_and(RoaringBitmap::is_empty) {
                break; // the AND can only shrink — no point continuing
            }
        }
        acc
    }

    /// Union of the posting lists of every indexed token that starts with `prefix`.
    fn prefix_union(&self, prefix: &str) -> RoaringBitmap {
        use std::ops::Bound::{Included, Unbounded};
        let mut out = RoaringBitmap::new();
        // Walk tokens in sorted order from `prefix` onward, stopping at the first
        // that no longer shares it (all such tokens are contiguous in the BTree).
        let bounds: (std::ops::Bound<&str>, std::ops::Bound<&str>) = (Included(prefix), Unbounded);
        for (tok, bits) in self.map.range::<str, _>(bounds) {
            if !tok.starts_with(prefix) {
                break;
            }
            out |= bits;
        }
        out
    }
}

/// Split text into lowercased alphanumeric tokens (any non-alphanumeric char is a
/// delimiter). Unicode-aware, so non-Latin scripts tokenise sensibly.
fn tokens(s: &str) -> impl Iterator<Item = String> + '_ {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
}

fn index_text(map: &mut BTreeMap<Box<str>, RoaringBitmap>, text: &str, id: u32) {
    for w in tokens(text) {
        map.entry(w.into_boxed_str()).or_default().insert(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{Track, TrackId};

    fn track(id: u32, artist: &str, album: &str, title: &str) -> Track {
        Track {
            id: TrackId::new(id),
            path: std::path::PathBuf::from(format!("/{id}.mp3")),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            album_artist: artist.into(),
            album_id: None,
            artist_id: None,
            track_no: 0,
            disc_no: 0,
            track_total: 0,
            disc_total: 0,
            duration_ms: 0,
            year: None,
            genre: None,
            composer: String::new(),
            comment: String::new(),
            audio: None,
            rating: 0,
            favorite: false,
            play_count: 0,
            added_at: 0,
            last_played: 0,
        }
    }

    fn ids(b: &RoaringBitmap) -> Vec<u32> {
        b.iter().collect()
    }

    fn demo() -> SearchIndex {
        let tracks = [
            track(1, "Adele", "30", "Hello"),
            track(2, "Adele", "25", "Hello"),
            track(3, "Adele", "21", "Rolling in the Deep"),
            track(4, "Radiohead", "Kid A", "Everything in Its Right Place"),
        ];
        let mut idx = SearchIndex::default();
        idx.rebuild(tracks.iter(), 7);
        idx
    }

    #[test]
    fn freshness_tracks_generation() {
        let idx = demo();
        assert!(idx.is_fresh(7));
        assert!(!idx.is_fresh(8));
        assert!(!SearchIndex::default().is_fresh(0));
    }

    #[test]
    fn prefix_matches_partial_tokens() {
        let idx = demo();
        // "ade" is a prefix of the "adele" token → all three Adele tracks
        assert_eq!(ids(&idx.candidates("ade").unwrap()), vec![1, 2, 3]);
        // full token still works
        assert_eq!(ids(&idx.candidates("radiohead").unwrap()), vec![4]);
    }

    #[test]
    fn multiple_words_are_anded() {
        let idx = demo();
        // "adele" ∩ "hello" → only the two tracks titled Hello
        assert_eq!(ids(&idx.candidates("adele hello").unwrap()), vec![1, 2]);
        // word order doesn't matter
        assert_eq!(ids(&idx.candidates("hello ad").unwrap()), vec![1, 2]);
    }

    #[test]
    fn cross_field_tokens_match() {
        let idx = demo();
        // "rolling" (title) ∩ "21" (album) → track 3
        assert_eq!(ids(&idx.candidates("21 rolling").unwrap()), vec![3]);
    }

    #[test]
    fn no_match_yields_empty_for_fallback() {
        let idx = demo();
        // a typo no token starts with → empty bitmap → caller falls back to fuzzy
        assert!(idx.candidates("zztop").unwrap().is_empty());
        // present-but-incompatible AND → empty
        assert!(idx.candidates("adele radiohead").unwrap().is_empty());
    }

    #[test]
    fn empty_query_has_no_words() {
        let idx = demo();
        assert!(idx.candidates("   ").is_none());
        assert!(idx.candidates("").is_none());
    }
}
