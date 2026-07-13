//! A small, dependency-free filter query language over tracks. It powers the
//! advanced search box and is built to be reused as the rules engine for smart
//! playlists (M4).
//!
//! Grammar (case-insensitive field/flag names; the `OR` keyword is uppercase):
//! ```text
//!   query   := group ( "OR" group )*       -- OR of groups
//!   group   := term+                        -- implicit AND within a group
//!   term    := "-"? atom                    -- leading "-" negates
//!   atom    := field op value | flag | word
//! ```
//! - **text fields** (`contains`, case-insensitive): `artist album title
//!   albumartist genre composer comment`, op `:` or `=`.
//! - **numeric fields**: `year track disc rating plays duration` (duration in
//!   seconds), ops `: = > < >= <= !=`.
//! - **flags**: `fav` / `favorite`.
//! - **word**: bare term, substring over `artist album title`.
//! - values with spaces can be double-quoted: `artist:"pink floyd"`.
//!
//! A query that is only bare words is *not* "structured" — the caller can keep
//! using fuzzy search for those and reserve this filter for field/OR/negation
//! queries.

use crate::core::model::Track;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextField {
    Artist,
    Album,
    Title,
    AlbumArtist,
    Genre,
    Composer,
    Comment,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumField {
    Year,
    Track,
    Disc,
    Rating,
    Plays,
    Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Gt,
    Lt,
    Ge,
    Le,
}

impl Op {
    fn test(self, a: i64, b: i64) -> bool {
        match self {
            Op::Eq => a == b,
            Op::Ne => a != b,
            Op::Gt => a > b,
            Op::Lt => a < b,
            Op::Ge => a >= b,
            Op::Le => a <= b,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cond {
    Text { field: TextField, needle: String },
    Num { field: NumField, op: Op, value: i64 },
    Fav,
    Word(String),
}

impl Cond {
    fn matches(&self, t: &Track) -> bool {
        match self {
            Cond::Text { field, needle } => contains_ci(text_value(t, *field), needle),
            Cond::Num { field, op, value } => {
                num_value(t, *field).is_some_and(|v| op.test(v, *value))
            }
            Cond::Fav => t.favorite,
            Cond::Word(w) => {
                let hay = format!("{} {} {}", t.artist, t.album, t.title);
                contains_ci(&hay, w)
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    pub neg: bool,
    pub cond: Cond,
}

/// A parsed query: an OR of AND-groups. Empty groups → matches everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub groups: Vec<Vec<Term>>,
}

impl Query {
    pub fn parse(s: &str) -> Query {
        let mut groups: Vec<Vec<Term>> = vec![Vec::new()];
        for tok in tokenize(s) {
            if tok == "OR" {
                groups.push(Vec::new());
                continue;
            }
            if let Some(term) = parse_term(&tok) {
                groups.last_mut().unwrap().push(term);
            }
        }
        groups.retain(|g| !g.is_empty());
        Query { groups }
    }

    /// True when the query uses real structure (a field/flag, OR, or negation) —
    /// i.e. it's more than a bag of plain words and warrants filtering rather
    /// than fuzzy search.
    pub fn is_structured(&self) -> bool {
        self.groups.len() > 1
            || self
                .groups
                .iter()
                .flatten()
                .any(|t| t.neg || !matches!(t.cond, Cond::Word(_)))
    }

    pub fn matches(&self, t: &Track) -> bool {
        if self.groups.is_empty() {
            return true;
        }
        self.groups
            .iter()
            .any(|g| g.iter().all(|term| term.neg ^ term.cond.matches(t)))
    }
}

fn contains_ci(hay: &str, needle: &str) -> bool {
    hay.to_lowercase().contains(&needle.to_lowercase())
}

fn text_value(t: &Track, f: TextField) -> &str {
    match f {
        TextField::Artist => &t.artist,
        TextField::Album => &t.album,
        TextField::Title => &t.title,
        TextField::AlbumArtist => &t.album_artist,
        TextField::Genre => t.genre.as_deref().unwrap_or(""),
        TextField::Composer => &t.composer,
        TextField::Comment => &t.comment,
    }
}

fn num_value(t: &Track, f: NumField) -> Option<i64> {
    match f {
        NumField::Year => t.year.map(|y| y as i64),
        NumField::Track => Some(t.track_no as i64),
        NumField::Disc => Some(t.disc_no as i64),
        NumField::Rating => Some(t.rating as i64),
        NumField::Plays => Some(t.play_count as i64),
        NumField::Duration => Some(t.duration().as_secs() as i64),
    }
}

fn text_field(name: &str) -> Option<TextField> {
    Some(match name {
        "artist" => TextField::Artist,
        "album" => TextField::Album,
        "title" => TextField::Title,
        "albumartist" | "album_artist" | "aa" => TextField::AlbumArtist,
        "genre" => TextField::Genre,
        "composer" => TextField::Composer,
        "comment" => TextField::Comment,
        _ => return None,
    })
}

fn num_field(name: &str) -> Option<NumField> {
    Some(match name {
        "year" | "date" => NumField::Year,
        "track" | "track#" | "tracknumber" => NumField::Track,
        "disc" | "disk" => NumField::Disc,
        "rating" | "stars" => NumField::Rating,
        "plays" | "playcount" | "played" => NumField::Plays,
        "duration" | "length" => NumField::Duration,
        _ => return None,
    })
}

/// Split whitespace, honouring double-quoted spans (quotes are stripped).
fn tokenize(s: &str) -> Vec<String> {
    let mut toks = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    for c in s.chars() {
        match c {
            '"' => in_q = !in_q,
            c if c.is_whitespace() && !in_q => {
                if !cur.is_empty() {
                    toks.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        toks.push(cur);
    }
    toks
}

fn parse_term(tok: &str) -> Option<Term> {
    let (neg, rest) = match tok.strip_prefix('-') {
        Some(r) if !r.is_empty() => (true, r),
        _ => (false, tok),
    };
    if rest.eq_ignore_ascii_case("fav") || rest.eq_ignore_ascii_case("favorite") {
        return Some(Term {
            neg,
            cond: Cond::Fav,
        });
    }
    if let Some(cond) = parse_field(rest) {
        return Some(Term { neg, cond });
    }
    if rest.is_empty() {
        return None;
    }
    Some(Term {
        neg,
        cond: Cond::Word(rest.to_string()),
    })
}

/// Parse a `field op value` atom, or `None` if it isn't one (caller → word).
fn parse_field(s: &str) -> Option<Cond> {
    let opi = s.find([':', '=', '>', '<', '!'])?;
    if opi == 0 {
        return None;
    }
    let name = s[..opi].to_lowercase();
    let (op, val) = split_op(&s[opi..])?;
    if let Some(field) = text_field(&name) {
        if val.is_empty() {
            return None;
        }
        return Some(Cond::Text {
            field,
            needle: val.to_string(),
        });
    }
    if let Some(field) = num_field(&name) {
        let value: i64 = val.trim().parse().ok()?;
        return Some(Cond::Num { field, op, value });
    }
    None
}

fn split_op(s: &str) -> Option<(Op, &str)> {
    for (pat, op) in [(">=", Op::Ge), ("<=", Op::Le), ("!=", Op::Ne)] {
        if let Some(v) = s.strip_prefix(pat) {
            return Some((op, v));
        }
    }
    let (first, rest) = s.split_at(1);
    let op = match first {
        ":" | "=" => Op::Eq,
        ">" => Op::Gt,
        "<" => Op::Lt,
        _ => return None,
    };
    Some((op, rest))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{AudioInfo, Codec, TrackId};

    fn track(artist: &str, album: &str, title: &str) -> Track {
        Track {
            id: TrackId::new(1),
            path: std::path::PathBuf::from("/m/x.mp3"),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            album_artist: artist.into(),
            album_id: None,
            artist_id: None,
            track_no: 3,
            disc_no: 1,
            track_total: 0,
            disc_total: 0,
            duration_ms: 200_000,
            year: Some(2001),
            genre: Some("Rock".into()),
            composer: String::new(),
            comment: String::new(),
            audio: Some(AudioInfo {
                codec: Codec::Mp3,
                sample_rate: 44100,
                bit_depth: 0,
                channels: 2,
                bitrate_kbps: 320,
            }),
            rating: 4,
            favorite: true,
            play_count: 12,
            added_at: 0,
            last_played: 0,
        }
    }

    #[test]
    fn plain_words_are_not_structured() {
        let q = Query::parse("dark side moon");
        assert!(!q.is_structured());
        assert!(q.matches(&track("Pink Floyd", "Dark Side of the Moon", "Time")));
    }

    #[test]
    fn field_text_and_numeric() {
        let t = track("Radiohead", "Kid A", "Idioteque");
        assert!(Query::parse("artist:radio").matches(&t));
        assert!(Query::parse("artist:radio").is_structured());
        assert!(!Query::parse("artist:beatles").matches(&t));
        assert!(Query::parse("year>=2000 year<=2010").matches(&t));
        assert!(!Query::parse("year<2000").matches(&t));
        assert!(Query::parse("rating>=4 plays>10").matches(&t));
        assert!(Query::parse("duration<300").matches(&t));
    }

    #[test]
    fn flags_negation_and_or() {
        let fav = track("A", "B", "C");
        let mut plain = track("A", "B", "C");
        plain.favorite = false;
        plain.artist = "Beatles".into();
        assert!(Query::parse("fav").matches(&fav));
        assert!(!Query::parse("fav").matches(&plain));
        assert!(Query::parse("-fav").matches(&plain));
        // OR group: matches if either side holds
        let q = Query::parse("artist:beatles OR artist:zzz");
        assert!(q.matches(&plain));
        assert!(!q.matches(&fav)); // fav's artist is "A"
    }

    #[test]
    fn quoted_values_keep_spaces() {
        let t = track("Pink Floyd", "The Wall", "Hey You");
        assert!(Query::parse("artist:\"pink floyd\"").matches(&t));
        assert!(!Query::parse("artist:\"pink zeppelin\"").matches(&t));
    }

    #[test]
    fn unknown_field_falls_back_to_word() {
        // "foo:bar" isn't a field → treated as a word over artist/album/title
        let t = track("foo:bar band", "Album", "Song");
        assert!(Query::parse("foo:bar").matches(&t));
        assert!(!Query::parse("foo:bar").is_structured());
    }
}
