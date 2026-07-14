//! Persistence of user data (ratings, favorites, play counts, recently played)
//! keyed by file path. Stored as JSON in the config dir. The track catalogue
//! itself is rebuilt by the scanner each run; only user-authored data persists.
//!
//! (M3 uses JSON for portability + zero native deps. The store interface is
//! deliberately small so it can be swapped for SQLite later without touching
//! callers.)

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::library::Library;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UserData {
    #[serde(default)]
    entries: HashMap<String, TrackUserData>,
    #[serde(default)]
    recently_played: Vec<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct TrackUserData {
    #[serde(default)]
    rating: u8,
    #[serde(default)]
    favorite: bool,
    #[serde(default)]
    play_count: u32,
    #[serde(default)]
    last_played: u32,
}

impl UserData {
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(dir))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Overlay persisted ratings/favorites/plays onto a freshly-scanned library.
    pub fn apply_to(&self, lib: &mut Library) {
        let ids: Vec<_> = lib.tracks.keys().copied().collect();
        for id in ids {
            if let Some(t) = lib.tracks.get_mut(&id)
                && let Some(u) = self.entries.get(&t.path.to_string_lossy().into_owned())
            {
                t.rating = u.rating.min(5);
                t.favorite = u.favorite;
                t.play_count = u.play_count;
                t.last_played = u.last_played;
            }
        }
        lib.favorites = lib
            .tracks
            .values()
            .filter(|t| t.favorite)
            .map(|t| t.id)
            .collect();
        // restore the recently-played order (resolve saved paths → ids)
        let by_path: HashMap<String, crate::core::model::TrackId> = lib
            .tracks
            .values()
            .map(|t| (t.path.to_string_lossy().into_owned(), t.id))
            .collect();
        // resolve saved paths → ids, dropping any duplicates kept from older
        // builds (the list is move-to-top deduped going forward).
        let mut seen = std::collections::HashSet::new();
        lib.recently_played = self
            .recently_played
            .iter()
            .filter_map(|p| by_path.get(p).copied())
            .filter(|id| seen.insert(*id))
            .collect();
        lib.recompute_views();
    }

    /// Snapshot current user data from the library, ready to save.
    pub fn capture(lib: &Library) -> Self {
        let mut entries = HashMap::new();
        for t in lib.tracks.values() {
            if t.rating > 0 || t.favorite || t.play_count > 0 {
                entries.insert(
                    t.path.to_string_lossy().into_owned(),
                    TrackUserData {
                        rating: t.rating,
                        favorite: t.favorite,
                        play_count: t.play_count,
                        last_played: t.last_played,
                    },
                );
            }
        }
        let recently_played = lib
            .recently_played
            .iter()
            .filter_map(|id| lib.tracks.get(id))
            .map(|t| t.path.to_string_lossy().into_owned())
            .collect();
        UserData {
            entries,
            recently_played,
        }
    }

    pub fn save(&self, dir: &Path) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(Self::path(dir), json);
        }
    }

    fn path(dir: &Path) -> PathBuf {
        dir.join("store.json")
    }
}

/// A saved search ("bookmark") — a named query string for quick-jump. Stored as
/// plain strings, so it's robust across rescans.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub name: String,
    pub query: String,
}

/// Persisted bookmarks (`bookmarks.json`).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct BookmarkStore {
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
}

impl BookmarkStore {
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join("bookmarks.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(bookmarks: &[Bookmark], dir: &Path) {
        let store = BookmarkStore {
            bookmarks: bookmarks.to_vec(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&store) {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(dir.join("bookmarks.json"), json);
        }
    }
}

/// Persisted internet-radio favorites (`radio_favorites.json`): the user's
/// starred stations, loaded on start and rewritten whenever the list changes.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RadioFavorites {
    #[serde(default)]
    pub stations: Vec<crate::radio::Station>,
}

impl RadioFavorites {
    pub fn load(dir: &Path) -> Vec<crate::radio::Station> {
        std::fs::read_to_string(dir.join("radio_favorites.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Self>(&t).ok())
            .map(|s| s.stations)
            .unwrap_or_default()
    }

    pub fn save(stations: &[crate::radio::Station], dir: &Path) {
        let store = RadioFavorites {
            stations: stations.to_vec(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&store) {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(dir.join("radio_favorites.json"), json);
        }
    }
}

/// Persisted internet-radio listening history (`radio_history.json`): every station
/// tuned in, with its play count + last-played time. Rewritten on each tune-in and
/// bounded to [`RadioHistory::CAP`] entries; drives the Recent + Most Played sections.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RadioHistory {
    #[serde(default)]
    pub entries: Vec<crate::radio::HistoryEntry>,
}

impl RadioHistory {
    /// Keep at most this many stations in the history (newest-played retained).
    pub const CAP: usize = 200;

    pub fn load(dir: &Path) -> Vec<crate::radio::HistoryEntry> {
        std::fs::read_to_string(dir.join("radio_history.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Self>(&t).ok())
            .map(|s| s.entries)
            .unwrap_or_default()
    }

    pub fn save(entries: &[crate::radio::HistoryEntry], dir: &Path) {
        let store = RadioHistory {
            entries: entries.to_vec(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&store) {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(dir.join("radio_history.json"), json);
        }
    }
}

/// Persisted listening history: unix timestamps of plays (`history.json`),
/// oldest → newest. Drives the stats heatmap / streaks.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct HistoryStore {
    #[serde(default)]
    pub plays: Vec<u64>,
}

impl HistoryStore {
    /// Keep at most this many events (newest retained) — bounds the file size.
    pub const CAP: usize = 20_000;

    pub fn load(dir: &Path) -> Vec<u64> {
        let mut plays = std::fs::read_to_string(dir.join("history.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<HistoryStore>(&t).ok())
            .map(|s| s.plays)
            .unwrap_or_default();
        // drop corrupt/implausible timestamps (must be after 2001-09-09) so the
        // streak math (day - 1) can't underflow on a garbage file.
        plays.retain(|&t| t >= 1_000_000_000);
        plays
    }

    pub fn save(plays: &[u64], dir: &Path) {
        let tail = plays.len().saturating_sub(Self::CAP);
        let store = HistoryStore {
            plays: plays[tail..].to_vec(),
        };
        if let Ok(json) = serde_json::to_string(&store) {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(dir.join("history.json"), json);
        }
    }
}

/// User playlists, persisted by track **file path** (so they survive a rescan).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PlaylistStore {
    #[serde(default)]
    playlists: Vec<PlaylistEntry>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PlaylistEntry {
    name: String,
    #[serde(default)]
    tracks: Vec<String>,
    /// Smart playlist rule (see `crate::query`); `None` for normal playlists.
    #[serde(default)]
    query: Option<String>,
    /// Sidebar folder grouping, if any.
    #[serde(default)]
    folder: Option<String>,
}

impl PlaylistStore {
    pub fn load(dir: &Path) -> Self {
        std::fs::read_to_string(dir.join("playlists.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Snapshot the library's current playlists (track ids → paths).
    pub fn from_library(lib: &Library) -> Self {
        let playlists = lib
            .playlists
            .values()
            .map(|p| PlaylistEntry {
                name: p.name.clone(),
                tracks: p
                    .track_ids
                    .iter()
                    .filter_map(|id| lib.tracks.get(id))
                    .map(|t| t.path.to_string_lossy().into_owned())
                    .collect(),
                query: p.query.clone(),
                folder: p.folder.clone(),
            })
            .collect();
        PlaylistStore { playlists }
    }

    /// Add the persisted playlists to the library (resolving paths → ids),
    /// skipping any whose name already exists.
    pub fn apply_to(&self, lib: &mut Library) {
        let by_path: HashMap<String, crate::core::model::TrackId> = lib
            .tracks
            .values()
            .map(|t| (t.path.to_string_lossy().into_owned(), t.id))
            .collect();
        let existing: std::collections::HashSet<String> =
            lib.playlists.values().map(|p| p.name.clone()).collect();
        for entry in &self.playlists {
            if existing.contains(&entry.name) {
                continue;
            }
            let id = if let Some(q) = &entry.query {
                lib.create_smart_playlist(entry.name.clone(), q.clone())
            } else {
                let id = lib.create_playlist(entry.name.clone());
                for path in &entry.tracks {
                    if let Some(&tid) = by_path.get(path) {
                        lib.add_to_playlist(id, tid);
                    }
                }
                id
            };
            if entry.folder.is_some() {
                lib.set_playlist_folder(id, entry.folder.clone());
            }
        }
    }

    pub fn save(&self, dir: &Path) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::create_dir_all(dir);
            let _ = std::fs::write(dir.join("playlists.json"), json);
        }
    }
}

/// On-disk cache of the scanned catalogue so the library loads instantly on
/// launch instead of re-scanning every time. A background sync then reconciles
/// it with the filesystem (new / changed / removed files). Change detection uses
/// each track's `added_at` (the file mtime captured at scan time).
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LibraryCache {
    #[serde(default)]
    pub tracks: Vec<crate::core::model::Track>,
}

impl LibraryCache {
    /// Compact binary format (bincode) over a buffered reader/writer — ~10-50x
    /// faster to parse and ~3x smaller than the old pretty JSON, so a large
    /// library starts near-instantly. A version/format mismatch just fails to
    /// load and the scanner rebuilds it.
    pub fn load(dir: &Path) -> Self {
        std::fs::File::open(dir.join("library.bin"))
            .ok()
            .and_then(|f| {
                let mut r = std::io::BufReader::new(f);
                // bincode 3 serde-compat reader; a format mismatch → Err → rebuild
                bincode::serde::decode_from_std_read(&mut r, bincode::config::standard()).ok()
            })
            .unwrap_or_default()
    }

    pub fn from_library(lib: &Library) -> Self {
        LibraryCache {
            tracks: lib.tracks.values().cloned().collect(),
        }
    }

    pub fn save(&self, dir: &Path) {
        let _ = std::fs::create_dir_all(dir);
        if let Ok(f) = std::fs::File::create(dir.join("library.bin")) {
            let mut w = std::io::BufWriter::new(f);
            let _ =
                bincode::serde::encode_into_std_write(self, &mut w, bincode::config::standard());
        }
    }
}
