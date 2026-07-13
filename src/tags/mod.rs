//! Tag editing: an editable view of a track's standard tags, plus write-back to
//! the file via lofty. Writes modify the file's existing primary tag in place,
//! so embedded cover art, lyrics, and unknown frames are preserved. Rating /
//! favorite / play-count are app-side and are never written to files.

use std::path::Path;

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::TaggedFile;
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::Tag;
use lofty::tag::items::Timestamp;

use crate::core::model::Track;

mod patterns;
mod replaygain;

pub use patterns::{match_filename, parse_pattern, render_pattern};
pub use replaygain::read_replaygain;

/// Field labels, in display/edit order. Indices line up with `get`/`set`.
pub const FIELDS: [&str; 13] = [
    "Title",
    "Artist",
    "Album",
    "Album Artist",
    "Track #",
    "Track Total",
    "Disc #",
    "Disc Total",
    "Year",
    "Genre",
    "Composer",
    "Comment",
    "Lyrics",
];

/// Numeric fields (validated to digits on input). Indices into `FIELDS`.
pub fn is_numeric(i: usize) -> bool {
    matches!(i, 4..=8)
}

/// An editable snapshot of a track's standard tags. Numbers are kept as strings
/// while editing and parsed on write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditableTags {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub track_no: String,
    pub track_total: String,
    pub disc_no: String,
    pub disc_total: String,
    pub year: String,
    pub genre: String,
    pub composer: String,
    pub comment: String,
    pub lyrics: String,
}

impl EditableTags {
    pub fn from_track(t: &Track) -> Self {
        let num = |n: u16| if n == 0 { String::new() } else { n.to_string() };
        EditableTags {
            title: t.title.clone(),
            artist: t.artist.to_string(),
            album: t.album.to_string(),
            album_artist: t.album_artist.to_string(),
            track_no: num(t.track_no),
            track_total: num(t.track_total),
            disc_no: num(t.disc_no),
            disc_total: num(t.disc_total),
            year: t.year.map(|y| y.to_string()).unwrap_or_default(),
            genre: t.genre.as_ref().map(|g| g.to_string()).unwrap_or_default(),
            composer: t.composer.clone(),
            comment: t.comment.clone(),
            lyrics: String::new(), // read from the file lazily (not in Track)
        }
    }

    pub fn get(&self, i: usize) -> &str {
        match i {
            0 => &self.title,
            1 => &self.artist,
            2 => &self.album,
            3 => &self.album_artist,
            4 => &self.track_no,
            5 => &self.track_total,
            6 => &self.disc_no,
            7 => &self.disc_total,
            8 => &self.year,
            9 => &self.genre,
            10 => &self.composer,
            11 => &self.comment,
            12 => &self.lyrics,
            _ => "",
        }
    }

    pub fn set(&mut self, i: usize, v: String) {
        match i {
            0 => self.title = v,
            1 => self.artist = v,
            2 => self.album = v,
            3 => self.album_artist = v,
            4 => self.track_no = v,
            5 => self.track_total = v,
            6 => self.disc_no = v,
            7 => self.disc_total = v,
            8 => self.year = v,
            9 => self.genre = v,
            10 => self.composer = v,
            11 => self.comment = v,
            12 => self.lyrics = v,
            _ => {}
        }
    }

    /// Apply only the `dirty` fields onto an in-memory `Track` (numbers parsed).
    pub fn apply_to(&self, t: &mut Track, dirty: &[bool]) {
        let d = |i: usize| dirty.get(i).copied().unwrap_or(false);
        let pn = |s: &str| s.trim().parse::<u16>().ok();
        if d(0) {
            t.title = self.title.trim().to_string();
        }
        if d(1) {
            t.artist = self.artist.trim().into();
        }
        if d(2) {
            t.album = self.album.trim().into();
        }
        if d(3) {
            t.album_artist = self.album_artist.trim().into();
        }
        if d(4) {
            t.track_no = pn(&self.track_no).unwrap_or(0);
        }
        if d(5) {
            t.track_total = pn(&self.track_total).unwrap_or(0);
        }
        if d(6) {
            t.disc_no = pn(&self.disc_no).unwrap_or(0);
        }
        if d(7) {
            t.disc_total = pn(&self.disc_total).unwrap_or(0);
        }
        if d(8) {
            t.year = pn(&self.year);
        }
        if d(9) {
            let g = self.genre.trim();
            t.genre = (!g.is_empty()).then(|| g.into());
        }
        if d(10) {
            t.composer = self.composer.trim().to_string();
        }
        if d(11) {
            t.comment = self.comment.trim().to_string();
        }
    }
}

/// Some MP3s have junk bytes (usually zero padding) between the ID3v2 tag and
/// the first audio frame. lofty can read them but its MP3 writer bails with
/// `UnknownFormat`. This rewrites the file as tag-then-audio (both byte-identical,
/// only the junk removed) so the tag becomes editable. No-op for well-formed
/// files and non-MP3s.
fn normalize_mp3_padding(path: &Path) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    let mut hdr = [0u8; 10];
    if f.read_exact(&mut hdr).is_err() || &hdr[0..3] != b"ID3" {
        return Ok(()); // not ID3v2 (FLAC/M4A/no tag) — leave it alone
    }
    let s = &hdr[6..10];
    if s.iter().any(|b| b & 0x80 != 0) {
        return Ok(()); // size not synchsafe — don't risk it
    }
    let tag_end = 10
        + (((s[0] as usize) << 21)
            | ((s[1] as usize) << 14)
            | ((s[2] as usize) << 7)
            | (s[3] as usize));

    // a real MPEG frame sync right after the tag → already well-formed
    let mut peek = [0u8; 2];
    if f.seek(SeekFrom::Start(tag_end as u64)).is_err() || f.read_exact(&mut peek).is_err() {
        return Ok(());
    }
    if peek[0] == 0xFF && (peek[1] & 0xE0) == 0xE0 {
        return Ok(());
    }

    // junk between tag and audio: find the first frame sync and strip the gap
    let data = std::fs::read(path)?;
    let Some(audio) = (tag_end..data.len().saturating_sub(1))
        .find(|&i| data[i] == 0xFF && (data[i + 1] & 0xE0) == 0xE0)
    else {
        return Ok(()); // no audio sync found — don't touch
    };
    if audio <= tag_end {
        return Ok(());
    }
    let mut out = Vec::with_capacity(data.len() - (audio - tag_end));
    out.extend_from_slice(&data[..tag_end]);
    out.extend_from_slice(&data[audio..]);
    let tmp = path.with_extension("lyrfin-tmp");
    std::fs::write(&tmp, &out)?;
    std::fs::rename(&tmp, path)?; // atomic replace
    Ok(())
}

/// Write only the `dirty` fields back to `path`, preserving every other frame
/// (cover/lyrics/unknown + untouched tags) by modifying the existing primary tag
/// in place. `dirty` is indexed like `FIELDS`.
pub fn write_tags(path: &Path, e: &EditableTags, dirty: &[bool]) -> Result<(), String> {
    // best-effort: clean junk padding so lofty's MP3 writer can save it
    let _ = normalize_mp3_padding(path);
    // First try a normal write (full fidelity — keeps any month/day). If a
    // malformed legacy date frame makes lofty reject the timestamp, retry with
    // implicit date conversion off and consolidate the dates (see below).
    match write_attempt(path, e, dirty, true) {
        Err(msg) if is_timestamp_err(&msg) => write_attempt(path, e, dirty, false),
        other => other,
    }
}

/// Whether a lofty error is the malformed-timestamp rejection we recover from.
fn is_timestamp_err(msg: &str) -> bool {
    msg.to_ascii_lowercase().contains("timestamp")
}

/// Read a file's tags. `implicit` mirrors lofty's default conversions (e.g.
/// combining ID3v2.3 TYER/TDAT/TIME into one TDRC); turning it off avoids
/// building an invalid TDRC from a garbage TDAT, leaving [`sanitize_dates`] to
/// repair the resulting date frame before the save.
fn read_tagged(path: &Path, implicit: bool) -> Result<TaggedFile, String> {
    if implicit {
        lofty::read_from_path(path).map_err(|x| x.to_string())
    } else {
        Probe::open(path)
            .map_err(|x| x.to_string())?
            .options(ParseOptions::new().implicit_conversions(false))
            .read()
            .map_err(|x| x.to_string())
    }
}

fn write_attempt(
    path: &Path,
    e: &EditableTags,
    dirty: &[bool],
    implicit: bool,
) -> Result<(), String> {
    let mut tagged = read_tagged(path, implicit)?;
    if tagged.primary_tag_mut().is_none() {
        let tt = tagged.primary_tag_type();
        tagged.insert_tag(Tag::new(tt));
    }
    let tag = tagged.primary_tag_mut().ok_or("no writable tag")?;

    for i in 0..FIELDS.len() {
        if dirty.get(i).copied().unwrap_or(false) {
            apply_field(tag, i, e.get(i));
        }
    }

    // repair a malformed date frame so lofty doesn't refuse the whole save. lofty
    // 0.24 consolidates the old ID3v2.3 split-date frames (TYER/TDAT/…) into a single
    // RecordingDate on read, so the previous raw-frame cleanup (fix_split_dates) is
    // obsolete — its raw frames aren't reachable through the generic Tag anymore.
    sanitize_dates(tag);
    tagged
        .save_to_path(path, WriteOptions::default())
        .map_err(|x| x.to_string())
}

/// Embed `data` as the front-cover picture on `path`, replacing any existing
/// front cover. The MIME type is sniffed from the bytes (JPEG/PNG). Every other
/// frame/tag is preserved.
pub fn embed_cover(path: &Path, data: &[u8]) -> Result<(), String> {
    use lofty::picture::{MimeType, Picture, PictureType};
    if data.len() < 4 {
        return Err("empty image".into());
    }
    let mime = if data.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        MimeType::Png
    } else {
        MimeType::Jpeg // default; iTunes/Deezer serve JPEG
    };
    let _ = normalize_mp3_padding(path);
    let embed = |implicit: bool| -> Result<(), String> {
        let mut tagged = read_tagged(path, implicit)?;
        if tagged.primary_tag_mut().is_none() {
            let tt = tagged.primary_tag_type();
            tagged.insert_tag(Tag::new(tt));
        }
        let tag = tagged.primary_tag_mut().ok_or("no writable tag")?;
        // lofty 0.24 replaced `Picture::new_unchecked` with a builder (`unchecked`
        // skips the MIME/data validation, matching the old call).
        let pic = Picture::unchecked(data.to_vec())
            .pic_type(PictureType::CoverFront)
            .mime_type(mime.clone())
            .description("Front Cover")
            .build();
        tag.remove_picture_type(PictureType::CoverFront);
        tag.push_picture(pic);
        sanitize_dates(tag); // don't let a bad date frame block the cover write
        tagged
            .save_to_path(path, WriteOptions::default())
            .map_err(|x| x.to_string())
    };
    match embed(true) {
        Err(msg) if is_timestamp_err(&msg) => embed(false),
        other => other,
    }
}

/// Set field `i` (text or numeric) on a tag, or remove it when blank/zero.
fn apply_field(tag: &mut Tag, i: usize, v: &str) {
    match i {
        0 => set_text(tag, ItemKey::TrackTitle, v),
        1 => set_text(tag, ItemKey::TrackArtist, v),
        2 => set_text(tag, ItemKey::AlbumTitle, v),
        3 => set_text(tag, ItemKey::AlbumArtist, v),
        4 => set_num(tag, v, |t, n| t.set_track(n), |t| t.remove_track()),
        5 => set_num(
            tag,
            v,
            |t, n| t.set_track_total(n),
            |t| t.remove_track_total(),
        ),
        6 => set_num(tag, v, |t, n| t.set_disk(n), |t| t.remove_disk()),
        7 => set_num(
            tag,
            v,
            |t, n| t.set_disk_total(n),
            |t| t.remove_disk_total(),
        ),
        8 => set_num(
            tag,
            v,
            |t, n| {
                t.set_date(year_ts(n));
            },
            |t| t.remove_date(),
        ),
        9 => set_text(tag, ItemKey::Genre, v),
        10 => set_text(tag, ItemKey::Composer, v),
        11 => set_text(tag, ItemKey::Comment, v),
        12 => set_text(tag, ItemKey::Lyrics, v),
        _ => {}
    }
}

/// Per-segment ceilings lofty enforces on a timestamp: year, month, day, hour,
/// minute, second (mirrors `Timestamp::verify`).
const TS_LIMITS: [u32; 6] = [9999, 12, 31, 23, 59, 59];

/// Whether every numeric segment of a date string is within lofty's limits. A
/// non-numeric or oversized segment (e.g. month 13, hour 25) makes it invalid.
fn timestamp_ok(s: &str) -> bool {
    s.split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .take(6)
        .enumerate()
        .all(|(i, p)| p.parse::<u32>().map(|v| v <= TS_LIMITS[i]).unwrap_or(false))
}

/// Rebuild a date string from its longest leading run of in-range segments,
/// stopping at the first that overflows. `None` if not even a valid year remains.
fn sanitize_timestamp(s: &str) -> Option<String> {
    const SEP: [char; 5] = ['-', '-', 'T', ':', ':'];
    let mut kept: Vec<u32> = Vec::new();
    for (i, p) in s
        .split(|c: char| !c.is_ascii_digit())
        .filter(|p| !p.is_empty())
        .take(6)
        .enumerate()
    {
        match p.parse::<u32>() {
            Ok(v) if v <= TS_LIMITS[i] => kept.push(v),
            _ => break,
        }
    }
    let (year, rest) = kept.split_first()?;
    let mut out = format!("{year:04}");
    for (i, v) in rest.iter().enumerate() {
        out.push(SEP[i]);
        out.push_str(&format!("{v:02}"));
    }
    Some(out)
}

/// Repair malformed date frames so an unrelated edit can still save. Some files
/// carry a recording/release timestamp with an out-of-range segment; lofty
/// re-validates timestamps on write and refuses the *entire* save, so editing
/// any field on such a file fails (see the "invalid timestamp" error). Reduce
/// each offending date item to its longest valid prefix (often just the year),
/// or drop it. Valid dates are left untouched.
fn sanitize_dates(tag: &mut Tag) {
    for key in [
        ItemKey::RecordingDate,
        ItemKey::ReleaseDate,
        ItemKey::OriginalReleaseDate,
    ] {
        let Some(s) = tag.get_string(key).map(str::to_string) else {
            continue;
        };
        if timestamp_ok(&s) {
            continue; // already valid — leave it alone
        }
        match sanitize_timestamp(&s) {
            Some(clean) => {
                tag.insert_text(key, clean);
            }
            None => tag.remove_key(key),
        }
    }
}

/// Embedded lyrics text (USLT / LYRICS tag), or empty if none.
pub fn read_lyrics(path: &Path) -> String {
    lofty::read_from_path(path)
        .ok()
        .and_then(|t| {
            t.primary_tag()
                .or_else(|| t.first_tag())
                .and_then(|tag| tag.get_string(ItemKey::Lyrics).map(str::to_string))
        })
        .unwrap_or_default()
}

fn set_text(tag: &mut Tag, key: ItemKey, v: &str) {
    let v = v.trim();
    if v.is_empty() {
        tag.remove_key(key);
    } else {
        tag.insert_text(key, v.to_string());
    }
}

/// A year-only [`Timestamp`] — lofty 0.24 stores the year through the date field
/// (`set_date`) rather than the removed `set_year` accessor.
fn year_ts(year: u32) -> Timestamp {
    Timestamp {
        year: year as u16,
        month: None,
        day: None,
        hour: None,
        minute: None,
        second: None,
    }
}

fn set_num(tag: &mut Tag, v: &str, set: impl Fn(&mut Tag, u32), remove: impl Fn(&mut Tag)) {
    match v.trim().parse::<u32>() {
        Ok(n) if n > 0 => set(tag, n),
        _ => remove(tag),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_set_cover_every_field() {
        let mut t = EditableTags::default();
        for i in 0..FIELDS.len() {
            t.set(i, format!("v{i}"));
        }
        for i in 0..FIELDS.len() {
            assert_eq!(t.get(i), format!("v{i}"));
        }
    }

    #[test]
    fn numeric_field_classification() {
        assert!(is_numeric(4)); // Track #
        assert!(is_numeric(8)); // Year
        assert!(!is_numeric(0)); // Title
        assert!(!is_numeric(9)); // Genre
        assert_eq!(FIELDS[12], "Lyrics");
        assert!(!is_numeric(12)); // Lyrics is free text
    }

    #[test]
    fn strips_junk_padding_between_tag_and_audio() {
        // ID3v2.4 header (size 0 → 10-byte tag) + 4 junk bytes + a frame sync
        let mut data = Vec::new();
        data.extend_from_slice(b"ID3");
        data.extend_from_slice(&[4, 0, 0]); // ver, flags
        data.extend_from_slice(&[0, 0, 0, 0]); // synchsafe size 0
        data.extend_from_slice(&[0, 0, 0, 0]); // junk padding
        data.extend_from_slice(&[0xFF, 0xFB, 0x90, 0x00, 1, 2, 3]); // "audio"
        let path = std::env::temp_dir().join("lyrfin_pad_test.mp3");
        std::fs::write(&path, &data).unwrap();

        normalize_mp3_padding(&path).unwrap();
        let after = std::fs::read(&path).unwrap();
        assert_eq!(&after[..3], b"ID3");
        assert_eq!(after[10], 0xFF, "audio now immediately follows the tag");
        assert_eq!(after.len(), 17, "4 junk bytes removed");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn numeric_input_keeps_value() {
        // numeric fields are stored verbatim; non-digits are filtered at input
        // time (see app::tag_edit_set_field), parsing happens in apply_to.
        let mut t = EditableTags::default();
        t.set(4, "7".into());
        t.set(8, "1999".into());
        assert_eq!(t.get(4), "7");
        assert_eq!(t.get(8), "1999");
    }

    #[test]
    fn timestamp_validity_matches_loftys_limits() {
        assert!(timestamp_ok("2018"));
        assert!(timestamp_ok("2018-10-15"));
        assert!(timestamp_ok("2018-10-15T23:59:59"));
        assert!(timestamp_ok("9999"));
        assert!(!timestamp_ok("2018-13-01")); // month 13
        assert!(!timestamp_ok("2018-10-32")); // day 32
        assert!(!timestamp_ok("2018-10-15T25:00")); // hour 25
        assert!(!timestamp_ok("10000")); // year > 9999
    }

    #[test]
    fn sanitize_keeps_longest_valid_prefix() {
        // truncate at the first out-of-range segment
        assert_eq!(sanitize_timestamp("2018-13-01").as_deref(), Some("2018"));
        assert_eq!(sanitize_timestamp("2018-10-32").as_deref(), Some("2018-10"));
        assert_eq!(
            sanitize_timestamp("2018-10-15T25:00:00").as_deref(),
            Some("2018-10-15")
        );
        // already-valid dates round-trip (normalized zero-padding)
        assert_eq!(
            sanitize_timestamp("2018-1-5").as_deref(),
            Some("2018-01-05")
        );
        // no salvageable year → drop it
        assert_eq!(sanitize_timestamp("99999"), None);
        assert_eq!(sanitize_timestamp("garbage"), None);
    }
}
