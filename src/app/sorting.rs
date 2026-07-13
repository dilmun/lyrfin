//! Tracklist sorting: the `SortField` key, parsing a sort spec
//! (`artist,year:asc,-title`), and the multi-key stable comparison used to order
//! a browsed list. Pure domain logic over `Track` (no UI / IO).

use super::*;

/// A tracklist sort key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortField {
    Artist,
    AlbumArtist,
    Album,
    Year,
    Track,
    Disc,
    Title,
    Genre,
    Duration,
    Rating,
    Plays,
    Added,
}

impl SortField {
    fn from_name(s: &str) -> Option<Self> {
        Some(match s.trim().to_lowercase().as_str() {
            "artist" => Self::Artist,
            "albumartist" | "album_artist" | "aa" => Self::AlbumArtist,
            "album" => Self::Album,
            "year" | "date" => Self::Year,
            "track" | "track#" | "trackno" | "tracknumber" => Self::Track,
            "disc" | "disk" => Self::Disc,
            "title" | "name" => Self::Title,
            "genre" => Self::Genre,
            "duration" | "length" | "time" => Self::Duration,
            "rating" | "stars" => Self::Rating,
            "plays" | "playcount" | "played" => Self::Plays,
            "added" | "date_added" | "recent" => Self::Added,
            _ => return None,
        })
    }
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Artist => "artist",
            Self::AlbumArtist => "albumartist",
            Self::Album => "album",
            Self::Year => "year",
            Self::Track => "track",
            Self::Disc => "disc",
            Self::Title => "title",
            Self::Genre => "genre",
            Self::Duration => "duration",
            Self::Rating => "rating",
            Self::Plays => "plays",
            Self::Added => "added",
        }
    }
    /// Fields where "latest/highest first" is the natural default direction.
    pub(super) fn default_desc(self) -> bool {
        matches!(self, Self::Year | Self::Added | Self::Plays | Self::Rating)
    }
}

/// Parse a sort spec like `artist,album,year,track` into `(field, descending)`
/// keys. A field may carry an explicit direction (`year:asc`, `-title`); else it
/// uses the field's natural default (year/added/plays/rating → descending).
pub fn parse_sort(spec: &str) -> Vec<(SortField, bool)> {
    spec.split(',')
        .filter_map(|tok| {
            let tok = tok.trim();
            let (name, dir) = if let Some(n) = tok.strip_prefix('-') {
                (n, Some(true))
            } else if let Some((n, d)) = tok.split_once(':') {
                let dir = match d.trim().to_lowercase().as_str() {
                    "desc" | "d" => Some(true),
                    "asc" | "a" => Some(false),
                    _ => None,
                };
                (n, dir)
            } else {
                (tok, None)
            };
            let f = SortField::from_name(name)?;
            Some((f, dir.unwrap_or_else(|| f.default_desc())))
        })
        .collect()
}

/// Compare two tracks on a single sort field (ascending; case-insensitive text).
fn cmp_field(f: SortField, x: &Track, y: &Track) -> std::cmp::Ordering {
    let s = |a: &str, b: &str| a.to_lowercase().cmp(&b.to_lowercase());
    match f {
        SortField::Artist => s(&x.artist, &y.artist),
        SortField::AlbumArtist => s(&x.album_artist, &y.album_artist),
        SortField::Album => s(&x.album, &y.album),
        SortField::Title => s(&x.title, &y.title),
        SortField::Genre => s(
            x.genre.as_deref().unwrap_or(""),
            y.genre.as_deref().unwrap_or(""),
        ),
        SortField::Year => x.year.unwrap_or(0).cmp(&y.year.unwrap_or(0)),
        SortField::Track => x.track_no.cmp(&y.track_no),
        SortField::Disc => x.disc_no.cmp(&y.disc_no),
        SortField::Duration => x.duration().cmp(&y.duration()),
        SortField::Rating => x.rating.cmp(&y.rating),
        SortField::Plays => x.play_count.cmp(&y.play_count),
        SortField::Added => x.added_at.cmp(&y.added_at),
    }
}

impl AppState {
    /// Multi-key stable comparison of two tracks by the active `sort` spec (each
    /// key honoring its descending flag); `Equal` when every key ties.
    pub(super) fn cmp_tracks(&self, a: TrackId, b: TrackId) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self.library.track(a), self.library.track(b)) {
            (Some(x), Some(y)) => {
                for &(field, desc) in &self.sort {
                    let o = cmp_field(field, x, y);
                    let o = if desc { o.reverse() } else { o };
                    if o != Ordering::Equal {
                        return o;
                    }
                }
                Ordering::Equal
            }
            _ => Ordering::Equal,
        }
    }
}
