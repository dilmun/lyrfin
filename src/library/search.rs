//! Fuzzy search + filtering across the library, backed by nucleo-matcher.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::core::model::{Track, TrackId};

#[derive(Debug, Clone)]
pub struct Match {
    pub id: TrackId,
    pub score: u32,
}

/// Rank arbitrary labels against `query` (for the command palette). Returns the
/// indices of matching labels, best first. Empty query → all, in order.
pub fn rank_labels(labels: &[&str], query: &str) -> Vec<usize> {
    let q = query.trim();
    if q.is_empty() {
        return (0..labels.len()).collect();
    }
    let mut matcher = Matcher::new(Config::DEFAULT);
    let pat = Pattern::parse(q, CaseMatching::Ignore, Normalization::Smart);
    let mut buf: Vec<char> = Vec::new();
    let mut out: Vec<(usize, u32)> = Vec::new();
    for (i, l) in labels.iter().enumerate() {
        let utf = Utf32Str::new(l, &mut buf);
        if let Some(score) = pat.score(utf, &mut matcher) {
            out.push((i, score));
        }
    }
    out.sort_by_key(|b| std::cmp::Reverse(b.1));
    out.into_iter().map(|(i, _)| i).collect()
}

/// Rank `tracks` against `query`. Empty query returns everything (score 0).
pub fn search<'a>(tracks: impl Iterator<Item = &'a Track>, query: &str) -> Vec<Match> {
    let q = query.trim();
    if q.is_empty() {
        return tracks.map(|t| Match { id: t.id, score: 0 }).collect();
    }
    let mut matcher = Matcher::new(Config::DEFAULT);
    let pat = Pattern::parse(q, CaseMatching::Ignore, Normalization::Smart);
    let mut buf: Vec<char> = Vec::new();
    let mut out: Vec<Match> = Vec::new();
    for t in tracks {
        let hay = format!("{} {} {}", t.artist, t.album, t.title);
        let utf = Utf32Str::new(&hay, &mut buf);
        if let Some(score) = pat.score(utf, &mut matcher) {
            out.push(Match { id: t.id, score });
        }
    }
    out.sort_by_key(|b| std::cmp::Reverse(b.score));
    out
}
