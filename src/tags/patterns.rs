//! Pattern converters between filenames and tag fields: `filename → tags`
//! (`%artist% - %title%` parsing) and `tags → filename` (`#. T` rendering). Pure
//! string processing — no file or lofty access; the tag editor calls these to
//! bulk-fill tags from filenames and to suggest a rename from the tags.

use crate::core::model::Track;

// ---- filename → tags converter -------------------------------------------

/// A token in a converter pattern like `%artist% - %title%`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatToken {
    Lit(String),
    Field(usize), // index into FIELDS
}

/// Map a `%name%` token to a field index (common mp3tag aliases supported).
pub fn field_index(name: &str) -> Option<usize> {
    match name.to_lowercase().as_str() {
        "title" => Some(0),
        "artist" => Some(1),
        "album" => Some(2),
        "albumartist" | "album_artist" | "album artist" => Some(3),
        "track" | "tracknumber" | "track#" => Some(4),
        "tracktotal" | "totaltracks" => Some(5),
        "disc" | "discnumber" => Some(6),
        "disctotal" => Some(7),
        "year" | "date" => Some(8),
        "genre" => Some(9),
        "composer" => Some(10),
        "comment" => Some(11),
        _ => None,
    }
}

/// Tokenize a converter pattern. Unknown `%name%` tokens are kept as literals.
pub fn parse_pattern(p: &str) -> Vec<PatToken> {
    let mut out = Vec::new();
    let mut rest = p;
    while let Some(start) = rest.find('%') {
        if start > 0 {
            out.push(PatToken::Lit(rest[..start].to_string()));
        }
        rest = &rest[start + 1..];
        if let Some(end) = rest.find('%') {
            let name = &rest[..end];
            match field_index(name) {
                Some(idx) => out.push(PatToken::Field(idx)),
                None => out.push(PatToken::Lit(format!("%{name}%"))),
            }
            rest = &rest[end + 1..];
        } else {
            out.push(PatToken::Lit(format!("%{rest}")));
            rest = "";
        }
    }
    if !rest.is_empty() {
        out.push(PatToken::Lit(rest.to_string()));
    }
    out
}

/// Match a filename stem against the pattern, extracting `(field index, value)`
/// pairs. Returns `None` if a literal delimiter doesn't line up.
pub fn match_filename(stem: &str, tokens: &[PatToken]) -> Option<Vec<(usize, String)>> {
    let mut out = Vec::new();
    let mut rest = stem;
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            PatToken::Lit(s) => {
                rest = rest.strip_prefix(s.as_str())?;
            }
            PatToken::Field(idx) => match tokens.get(i + 1) {
                Some(PatToken::Lit(next)) => {
                    let pos = rest.find(next.as_str())?;
                    out.push((*idx, rest[..pos].trim().to_string()));
                    rest = &rest[pos..];
                }
                _ => {
                    out.push((*idx, rest.trim().to_string()));
                    rest = "";
                }
            },
        }
        i += 1;
    }
    Some(out)
}

// ---- tags → filename ------------------------------------------------------

/// Replace `/ \ : * ? " < > |` (illegal in filenames) with `_`.
fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => '_',
            c => c,
        })
        .collect()
}

/// Build a filename stem (no extension) from a tags→filename pattern. Tokens:
/// `#`=track(2-digit), `T`=title, `A`=album, `AA`=album artist, `AR`=artist,
/// `Y`=year. Everything else is a literal separator. e.g. `#. T` → `01. Title`.
pub fn render_pattern(pattern: &str, t: &Track) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let two: String = chars.iter().skip(i).take(2).collect();
        let (val, len): (Option<String>, usize) = match two.as_str() {
            "AA" => (Some(t.album_artist.to_string()), 2),
            "AR" => (Some(t.artist.to_string()), 2),
            _ => match chars[i] {
                '#' => (
                    Some(if t.track_no > 0 {
                        format!("{:02}", t.track_no)
                    } else {
                        String::new()
                    }),
                    1,
                ),
                'T' => (Some(t.title.clone()), 1),
                'A' => (Some(t.album.to_string()), 1),
                'Y' => (Some(t.year.map(|y| y.to_string()).unwrap_or_default()), 1),
                _ => (None, 1),
            },
        };
        match val {
            Some(v) => out.push_str(&v),
            None => out.push(chars[i]),
        }
        i += len;
    }
    sanitize_filename(out.trim())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{AudioInfo, Codec, TrackId};

    fn make_track() -> Track {
        Track {
            id: TrackId::new(1),
            path: std::path::PathBuf::from("/m/x.mp3"),
            title: "Title".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            album_artist: "AlbumArtist".into(),
            album_id: None,
            artist_id: None,
            track_no: 1,
            disc_no: 1,
            track_total: 0,
            disc_total: 0,
            duration_ms: 0,
            year: Some(2025),
            genre: None,
            composer: String::new(),
            comment: String::new(),
            audio: Some(AudioInfo {
                codec: Codec::Mp3,
                sample_rate: 44100,
                bit_depth: 0,
                channels: 2,
                bitrate_kbps: 320,
            }),
            rating: 0,
            favorite: false,
            play_count: 0,
            added_at: 0,
            last_played: 0,
        }
    }

    #[test]
    fn filename_pattern_extracts_fields() {
        let toks = parse_pattern("%artist% - %title%");
        let got = match_filename("Amr Diab - Tamally Maak", &toks).unwrap();
        assert_eq!(
            got,
            vec![(1, "Amr Diab".to_string()), (0, "Tamally Maak".to_string())]
        );
    }

    #[test]
    fn filename_pattern_leading_number() {
        let toks = parse_pattern("%track% - %title%");
        let got = match_filename("03 - Wave", &toks).unwrap();
        assert_eq!(got, vec![(4, "03".to_string()), (0, "Wave".to_string())]);
        // delimiter not present in the name → no match
        assert!(
            match_filename("no delimiter here", &parse_pattern("%artist% / %title%")).is_none()
        );
    }

    #[test]
    fn renders_filename_from_pattern() {
        let mut tk = make_track();
        tk.track_no = 3;
        tk.title = "Lolak Habibi".into();
        assert_eq!(render_pattern("#. T", &tk), "03. Lolak Habibi");
        tk.album = "A/B".into(); // illegal char sanitized
        assert_eq!(render_pattern("A", &tk), "A_B");
        tk.album_artist = "Tamer".into();
        assert_eq!(render_pattern("AA - T", &tk), "Tamer - Lolak Habibi");
    }
}
