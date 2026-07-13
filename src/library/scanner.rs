//! Filesystem scanner: walk roots, read tags with lofty, emit tracks. Runs on a
//! worker thread and streams progress so a large library populates the UI as it
//! goes (never blocking startup).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crossbeam_channel::Sender;
use lofty::prelude::*;
use walkdir::WalkDir;

use crate::core::model::{AudioInfo, Codec, Track, TrackId};
use crate::library::LibraryEvent;

/// Audio extensions we index.
pub const SUPPORTED_EXTS: &[&str] = &[
    "flac", "mp3", "m4a", "aac", "ogg", "opus", "wav", "aiff", "aif", "wv",
];

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SUPPORTED_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Spawn a background scan of `roots`, streaming events on `tx`. `cache` holds
/// the previously-scanned tracks (path → Track); files whose mtime is unchanged
/// are reused from it instead of re-parsing their tags, so a sync of an unchanged
/// library does almost no work. Files missing from the new walk are dropped.
pub fn spawn(roots: Vec<PathBuf>, cache: HashMap<PathBuf, Track>, tx: Sender<LibraryEvent>) {
    std::thread::Builder::new()
        .name("lyrfin-scan".into())
        .spawn(move || scan_into(roots, cache, tx))
        .expect("spawn scanner");
}

fn scan_into(roots: Vec<PathBuf>, cache: HashMap<PathBuf, Track>, tx: Sender<LibraryEvent>) {
    let _ = tx.send(LibraryEvent::ScanStarted { roots: roots.len() });

    // collect candidate files first (cheap) for a progress denominator
    let mut files: Vec<PathBuf> = Vec::new();
    for root in &roots {
        for entry in WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() && is_supported(entry.path()) {
                files.push(entry.path().to_path_buf());
            }
        }
    }
    let total = files.len();

    // Read tags in parallel across a few scoped threads — this phase is I/O +
    // parse bound (lofty), so it scales well. Order is preserved per chunk; ids
    // are assigned densely afterward. No rayon: std::thread::scope suffices.
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8);
    let chunk = total.div_ceil(threads).max(1);
    let done = std::sync::atomic::AtomicUsize::new(0);
    let read_one = |path: &PathBuf| -> Option<Track> {
        let mtime = file_mtime(path);
        // reuse the cached track when the file hasn't changed since last scan
        match cache.get(path) {
            Some(c) if c.added_at == mtime as u32 => Some(c.clone()),
            _ => read_tags(path),
        }
    };
    let chunks: Vec<Vec<Option<Track>>> = std::thread::scope(|s| {
        let handles: Vec<_> = files
            .chunks(chunk)
            .map(|slice| {
                let tx = tx.clone();
                let done = &done;
                let read_one = &read_one;
                s.spawn(move || {
                    let mut out = Vec::with_capacity(slice.len());
                    for path in slice {
                        out.push(read_one(path));
                        let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                        if d.is_multiple_of(64) {
                            let _ = tx.send(LibraryEvent::Indexed {
                                done: d,
                                total: Some(total),
                            });
                        }
                    }
                    out
                })
            })
            .collect();
        // a panicked worker contributes nothing rather than aborting the scan
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect()
    });

    // flatten in walk order + assign dense sequential ids
    let mut tracks = Vec::with_capacity(total);
    for chunk_res in chunks {
        for mut t in chunk_res.into_iter().flatten() {
            t.id = TrackId::new(tracks.len() as u32 + 1);
            tracks.push(t);
        }
    }

    let n = tracks.len();
    let _ = tx.send(LibraryEvent::Loaded(tracks));
    let _ = tx.send(LibraryEvent::ScanFinished { tracks: n });
}

fn codec_from_ext(path: &Path) -> Codec {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("flac") => Codec::Flac,
        Some("m4a") => Codec::Alac,
        Some("mp3") => Codec::Mp3,
        Some("aac") => Codec::Aac,
        Some("ogg") => Codec::OggVorbis,
        Some("opus") => Codec::Opus,
        Some("wav") => Codec::Wav,
        _ => Codec::Other,
    }
}

/// Parse one file's tags into a `Track` (id assigned by the caller).
fn read_tags(path: &Path) -> Option<Track> {
    let tagged = lofty::read_from_path(path).ok()?;
    let props = tagged.properties();
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());

    let stem = || {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown")
            .to_string()
    };
    let get = |f: &dyn Fn(&lofty::tag::Tag) -> Option<String>| tag.and_then(f);

    let title = get(&|t| t.title().map(|s| s.to_string())).unwrap_or_else(stem);
    let artist =
        get(&|t| t.artist().map(|s| s.to_string())).unwrap_or_else(|| "Unknown Artist".into());
    let album =
        get(&|t| t.album().map(|s| s.to_string())).unwrap_or_else(|| "Unknown Album".into());
    let album_artist = tag
        .and_then(|t| t.get_string(ItemKey::AlbumArtist).map(|s| s.to_string()))
        .unwrap_or_else(|| artist.clone());
    let track_no = tag.and_then(|t| t.track()).unwrap_or(0) as u16;
    let disc_no = tag.and_then(|t| t.disk()).unwrap_or(1) as u16;
    let track_total = tag.and_then(|t| t.track_total()).unwrap_or(0) as u16;
    let disc_total = tag.and_then(|t| t.disk_total()).unwrap_or(0) as u16;
    let year = tag.and_then(|t| t.date()).map(|ts| ts.year);
    let genre = get(&|t| t.genre().map(|s| s.to_string()));
    let composer = tag
        .and_then(|t| t.get_string(ItemKey::Composer).map(|s| s.to_string()))
        .unwrap_or_default();
    let comment = get(&|t| t.comment().map(|s| s.to_string())).unwrap_or_default();

    let audio = AudioInfo {
        codec: codec_from_ext(path),
        sample_rate: props.sample_rate().unwrap_or(0),
        bit_depth: props.bit_depth().unwrap_or(0),
        channels: props.channels().unwrap_or(2),
        bitrate_kbps: props.audio_bitrate().unwrap_or(0),
    };

    Some(Track {
        id: TrackId::new(1), // assigned by scan_into after collection
        path: path.to_path_buf(),
        title,
        artist: artist.into(),
        album: album.into(),
        album_artist: album_artist.into(),
        album_id: None,
        artist_id: None,
        track_no,
        disc_no,
        track_total,
        disc_total,
        duration_ms: props.duration().as_millis() as u32,
        year,
        genre: genre.map(Into::into),
        composer,
        comment,
        audio: Some(audio),
        rating: 0,
        favorite: false,
        play_count: 0,
        added_at: file_mtime(path) as u32,
        last_played: 0,
    })
}

fn file_mtime(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
