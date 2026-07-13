//! Music library: scanning, the in-memory index, persistence, and search.
//!
//! The scanner runs on a worker thread and streams [`LibraryEvent`]s as it
//! discovers tracks, so a large collection populates the UI progressively
//! instead of blocking startup.

pub mod index;
pub mod scanner;
pub mod search;
pub mod store;

use std::collections::HashMap;

use crate::core::model::{Album, AlbumId, Artist, ArtistId, Playlist, PlaylistId, Track, TrackId};

/// Events sent scanner/store → UI.
/// Scanner → UI events. A few variants (ScanStarted/TrackAdded/Error) are part
/// of the protocol but not emitted by the current incremental scanner.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum LibraryEvent {
    ScanStarted {
        roots: usize,
    },
    Indexed {
        done: usize,
        total: Option<usize>,
    },
    TrackAdded(TrackId),
    /// Full batch of scanned tracks (index is built on the UI thread).
    Loaded(Vec<Track>),
    ScanFinished {
        tracks: usize,
    },
    Error(String),
}

/// Contiguous track storage: a dense `Vec<Track>` for cache-friendly iteration
/// plus an id → index map for O(1) lookup. Replaces a `HashMap<TrackId, Track>`,
/// which stored 296-byte structs with ~1.15× slack and pointer-chased every
/// access. Exposes the small subset of the old map API the codebase uses.
#[derive(Debug, Default)]
pub struct TrackStore {
    items: Vec<Track>,
    index: HashMap<TrackId, usize>,
}

impl TrackStore {
    pub fn get(&self, id: &TrackId) -> Option<&Track> {
        self.index.get(id).map(|&i| &self.items[i])
    }
    pub fn get_mut(&mut self, id: &TrackId) -> Option<&mut Track> {
        let i = *self.index.get(id)?;
        self.items.get_mut(i)
    }
    pub fn values(&self) -> std::slice::Iter<'_, Track> {
        self.items.iter()
    }
    pub fn keys(&self) -> std::collections::hash_map::Keys<'_, TrackId, usize> {
        self.index.keys()
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    /// Pairs with `len` (clippy::len_without_is_empty); not called directly yet.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn insert(&mut self, id: TrackId, track: Track) {
        if let Some(&i) = self.index.get(&id) {
            self.items[i] = track;
        } else {
            self.index.insert(id, self.items.len());
            self.items.push(track);
        }
    }
}

/// The in-memory catalogue. Queries here return ids that the UI resolves to rows.
#[derive(Debug, Default)]
pub struct Library {
    pub tracks: TrackStore,
    pub albums: HashMap<AlbumId, Album>,
    pub artists: HashMap<ArtistId, Artist>,
    pub playlists: HashMap<PlaylistId, Playlist>,
    pub recently_played: Vec<TrackId>,
    pub recently_added: Vec<TrackId>,
    pub most_played: Vec<TrackId>,
    pub favorites: Vec<TrackId>,
    /// Likely duplicates (same artist+title appearing more than once).
    pub duplicates: Vec<TrackId>,
    /// Tracks missing core tags (Unknown artist/album or empty title).
    pub untagged: Vec<TrackId>,
    /// Cached facet data for the sidebar (computed once in `from_tracks`, never
    /// per-frame): sorted genre / year lists + their per-value track counts.
    pub genres: Vec<String>,
    pub years: Vec<u16>,
    pub genre_counts: HashMap<String, usize>,
    pub year_counts: HashMap<u16, usize>,
}

impl Library {
    /// Build an indexed library from a flat list of scanned tracks: assign
    /// album/artist ids by grouping, and populate the lookup maps.
    pub fn from_tracks(mut tracks: Vec<Track>) -> Library {
        let mut lib = Library::default();
        let mut artist_ids: HashMap<String, ArtistId> = HashMap::new();
        let mut album_ids: HashMap<(String, String), AlbumId> = HashMap::new();
        let mut next_artist = 1u32;
        let mut next_album = 1u32;

        // Pass 0: intern shared strings so every track that shares an artist /
        // album / genre points at one Arc instead of owning a duplicate String.
        // Also resolve the empty-AlbumArtist fallback here.
        {
            let mut pool: HashMap<Box<str>, crate::core::model::IStr> = HashMap::new();
            let mut intern = |s: &crate::core::model::IStr| -> crate::core::model::IStr {
                match pool.get(s.as_str()) {
                    Some(v) => v.clone(),
                    None => {
                        let v = s.clone();
                        pool.insert(s.as_str().into(), v.clone());
                        v
                    }
                }
            };
            for t in &mut tracks {
                t.artist = intern(&t.artist);
                t.album = intern(&t.album);
                if t.album_artist.trim().is_empty() {
                    t.album_artist = t.artist.clone();
                } else {
                    t.album_artist = intern(&t.album_artist);
                }
                if let Some(g) = t.genre.clone() {
                    t.genre = Some(intern(&g));
                }
            }
        }

        for t in &mut tracks {
            // Group by AlbumArtist (compilations / featured tracks stay under one
            // artist); the empty-AlbumArtist fallback was resolved in pass 0.
            let aid = *artist_ids
                .entry(t.album_artist.to_string())
                .or_insert_with(|| {
                    let id = ArtistId::new(next_artist);
                    next_artist += 1;
                    lib.artists.insert(
                        id,
                        Artist {
                            id,
                            name: t.album_artist.to_string(),
                            album_ids: vec![],
                        },
                    );
                    id
                });
            let alid = *album_ids
                .entry((t.album_artist.to_string(), t.album.to_string()))
                .or_insert_with(|| {
                    let id = AlbumId::new(next_album);
                    next_album += 1;
                    lib.albums.insert(
                        id,
                        Album {
                            id,
                            title: t.album.to_string(),
                            artist: t.album_artist.to_string(),
                            artist_id: Some(aid),
                            year: t.year,
                            genre: t.genre.as_ref().map(|g| g.to_string()),
                            track_ids: vec![],
                            cover_path: None,
                        },
                    );
                    if let Some(ar) = lib.artists.get_mut(&aid) {
                        ar.album_ids.push(id);
                    }
                    id
                });
            t.album_id = Some(alid);
            t.artist_id = Some(aid);
            if let Some(al) = lib.albums.get_mut(&alid) {
                al.track_ids.push(t.id);
            }
        }

        for t in tracks {
            if t.favorite {
                lib.favorites.push(t.id);
            }
            lib.tracks.insert(t.id, t);
        }
        lib.recompute_views();
        lib
    }

    /// Rebuild derived ordering (recently added). Call after edits.
    pub fn recompute_views(&mut self) {
        let mut ids: Vec<(TrackId, u32)> =
            self.tracks.values().map(|t| (t.id, t.added_at)).collect();
        ids.sort_by_key(|b| std::cmp::Reverse(b.1));
        self.recently_added = ids.into_iter().map(|(id, _)| id).collect();

        // most played: only tracks actually played, highest count first
        let mut played: Vec<&Track> = self.tracks.values().filter(|t| t.play_count > 0).collect();
        played.sort_by_key(|b| std::cmp::Reverse(b.play_count));
        self.most_played = played.into_iter().map(|t| t.id).collect();

        // duplicates: same artist+title appearing more than once
        let mut by_key: HashMap<(String, String), Vec<TrackId>> = HashMap::new();
        for t in self.tracks.values() {
            by_key
                .entry((t.artist.to_lowercase(), t.title.to_lowercase()))
                .or_default()
                .push(t.id);
        }
        let mut dups: Vec<&Track> = by_key
            .values()
            .filter(|g| g.len() > 1)
            .flatten()
            .filter_map(|id| self.tracks.get(id))
            .collect();
        dups.sort_by(|a, b| {
            a.artist
                .to_lowercase()
                .cmp(&b.artist.to_lowercase())
                .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        self.duplicates = dups.into_iter().map(|t| t.id).collect();

        // untagged: missing core tags (scanner's Unknown fallbacks / empty title)
        let mut bad: Vec<&Track> = self
            .tracks
            .values()
            .filter(|t| {
                t.title.trim().is_empty()
                    || t.artist == "Unknown Artist"
                    || t.album == "Unknown Album"
            })
            .collect();
        bad.sort_by_key(|a| a.title.to_lowercase());
        self.untagged = bad.into_iter().map(|t| t.id).collect();

        // facets (sidebar): sorted genre / year lists + per-value counts. Cached
        // here so the per-frame sidebar render never re-scans the whole library.
        let mut genre_counts: HashMap<String, usize> = HashMap::new();
        let mut year_counts: HashMap<u16, usize> = HashMap::new();
        let mut genre_disp: HashMap<String, String> = HashMap::new(); // lower → shown
        for t in self.tracks.values() {
            if let Some(g) = t.genre.as_ref().map(|g| g.trim()).filter(|g| !g.is_empty()) {
                let key = g.to_lowercase();
                *genre_counts.entry(key.clone()).or_default() += 1;
                genre_disp.entry(key).or_insert_with(|| g.to_string());
            }
            if let Some(y) = t.year.filter(|&y| y > 0) {
                *year_counts.entry(y).or_default() += 1;
            }
        }
        let mut genres: Vec<String> = genre_disp.into_values().collect();
        genres.sort_by_key(|a| a.to_lowercase());
        let mut years: Vec<u16> = year_counts.keys().copied().collect();
        years.sort_unstable_by(|a, b| b.cmp(a)); // newest first
        self.genres = genres;
        self.years = years;
        self.genre_counts = genre_counts;
        self.year_counts = year_counts;
    }

    /// All track ids sorted by artist / album / disc / track (stable browse order).
    pub fn all_tracks_sorted(&self) -> Vec<TrackId> {
        let mut v: Vec<&Track> = self.tracks.values().collect();
        v.sort_by(|a, b| {
            a.album_artist
                .to_lowercase()
                .cmp(&b.album_artist.to_lowercase())
                .then(a.album.to_lowercase().cmp(&b.album.to_lowercase()))
                .then(a.disc_no.cmp(&b.disc_no))
                .then(a.track_no.cmp(&b.track_no))
        });
        v.into_iter().map(|t| t.id).collect()
    }

    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.get(&id)
    }

    pub fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.get_mut(&id)
    }

    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// Artists sorted for the browser's left column.
    pub fn artists_sorted(&self) -> Vec<&Artist> {
        let mut v: Vec<&Artist> = self.artists.values().collect();
        v.sort_by_key(|a| a.name.to_lowercase());
        v
    }

    pub fn albums_of(&self, artist: ArtistId) -> Vec<&Album> {
        self.artists
            .get(&artist)
            .map(|a| {
                a.album_ids
                    .iter()
                    .filter_map(|id| self.albums.get(id))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// All of an artist's tracks across their albums, ordered by album year then
    /// disc/track number.
    pub fn tracks_of_artist(&self, artist: ArtistId) -> Vec<TrackId> {
        let mut albums = self.albums_of(artist);
        albums.sort_by_key(|a| (a.year.unwrap_or(0), a.title.to_lowercase()));
        albums
            .iter()
            .flat_map(|al| self.tracks_of(al.id).into_iter().map(|t| t.id))
            .collect()
    }

    pub fn tracks_of(&self, album: AlbumId) -> Vec<&Track> {
        self.albums
            .get(&album)
            .map(|al| {
                let mut t: Vec<&Track> = al
                    .track_ids
                    .iter()
                    .filter_map(|id| self.tracks.get(id))
                    .collect();
                t.sort_by_key(|t| (t.disc_no, t.track_no));
                t
            })
            .unwrap_or_default()
    }

    /// Albums of an artist, newest release first (for the library tree).
    pub fn albums_of_by_year(&self, artist: ArtistId) -> Vec<&Album> {
        let mut v = self.albums_of(artist);
        v.sort_by(|a, b| {
            b.year
                .unwrap_or(0)
                .cmp(&a.year.unwrap_or(0))
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        v
    }

    // ---- facets (genre / year) ------------------------------------------
    /// Tracks whose genre matches `genre` (case-insensitive), in browse order.
    pub fn tracks_with_genre(&self, genre: &str) -> Vec<TrackId> {
        let want = genre.trim();
        self.all_tracks_sorted()
            .into_iter()
            .filter(|id| {
                self.tracks
                    .get(id)
                    .and_then(|t| t.genre.as_ref())
                    .is_some_and(|g| g.trim().eq_ignore_ascii_case(want))
            })
            .collect()
    }

    // ---- playlists -------------------------------------------------------
    pub fn playlists_sorted(&self) -> Vec<&Playlist> {
        let mut v: Vec<&Playlist> = self.playlists.values().collect();
        v.sort_by_key(|a| a.name.to_lowercase());
        v
    }

    /// All albums, sorted by album-artist ▸ year ▸ title (the order the Albums
    /// browse section lists them in).
    pub fn albums_sorted(&self) -> Vec<&Album> {
        let mut v: Vec<&Album> = self.albums.values().collect();
        v.sort_by(|a, b| {
            a.artist
                .to_lowercase()
                .cmp(&b.artist.to_lowercase())
                .then(a.year.cmp(&b.year))
                .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        v
    }

    pub fn create_playlist(&mut self, name: String) -> PlaylistId {
        let id = PlaylistId::new(self.playlists.keys().map(|p| p.get()).max().unwrap_or(0) + 1);
        self.playlists.insert(
            id,
            Playlist {
                id,
                name,
                track_ids: Vec::new(),
                query: None,
                folder: None,
            },
        );
        id
    }

    /// Create a smart (dynamic) playlist whose members are computed from `query`.
    pub fn create_smart_playlist(&mut self, name: String, query: String) -> PlaylistId {
        let id = self.create_playlist(name);
        if let Some(p) = self.playlists.get_mut(&id) {
            p.query = Some(query);
        }
        id
    }

    /// True if the playlist is rule-based (its tracks come from a query).
    /// Part of the playlist API; not yet called from a view.
    #[allow(dead_code)]
    pub fn is_smart_playlist(&self, id: PlaylistId) -> bool {
        self.playlists.get(&id).is_some_and(|p| p.query.is_some())
    }

    /// Assign a playlist to a folder (`None` = ungroup).
    pub fn set_playlist_folder(&mut self, id: PlaylistId, folder: Option<String>) {
        if let Some(p) = self.playlists.get_mut(&id) {
            p.folder = folder.filter(|f| !f.trim().is_empty());
        }
    }

    pub fn delete_playlist(&mut self, id: PlaylistId) {
        self.playlists.remove(&id);
    }

    pub fn rename_playlist(&mut self, id: PlaylistId, name: String) {
        if let Some(p) = self.playlists.get_mut(&id) {
            p.name = name;
        }
    }

    pub fn add_to_playlist(&mut self, id: PlaylistId, track: TrackId) {
        if let Some(p) = self.playlists.get_mut(&id)
            && p.query.is_none() // smart playlists are rule-based; can't add to them
            && !p.track_ids.contains(&track)
        {
            p.track_ids.push(track);
        }
    }

    /// Remove a track from a normal playlist (smart playlists are rule-based, so
    /// their membership can't be edited by hand). No-op if absent.
    pub fn remove_from_playlist(&mut self, id: PlaylistId, track: TrackId) {
        if let Some(p) = self.playlists.get_mut(&id)
            && p.query.is_none()
        {
            p.track_ids.retain(|&t| t != track);
        }
    }

    /// Tracks of a playlist: a smart playlist evaluates its query live (in browse
    /// order); a normal playlist returns its stored track ids.
    pub fn playlist_tracks(&self, id: PlaylistId) -> Vec<TrackId> {
        let Some(p) = self.playlists.get(&id) else {
            return Vec::new();
        };
        match &p.query {
            Some(q) => {
                let query = crate::query::Query::parse(q);
                self.all_tracks_sorted()
                    .into_iter()
                    .filter(|tid| self.tracks.get(tid).is_some_and(|t| query.matches(t)))
                    .collect()
            }
            None => p.track_ids.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::Track;

    fn track(artist: &str, album_artist: &str, album: &str) -> Track {
        Track {
            id: TrackId::new(1),
            path: std::path::PathBuf::from(format!("/{artist}/{album}")),
            title: "t".into(),
            artist: artist.into(),
            album: album.into(),
            album_artist: album_artist.into(),
            album_id: None,
            artist_id: None,
            track_no: 1,
            disc_no: 1,
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

    #[test]
    fn groups_by_album_artist_not_track_artist() {
        // different track artists, same album_artist → one artist node
        let lib = Library::from_tracks(vec![
            track("Amr Diab", "Amr Diab", "Greatest Hits"),
            track("Sherine ft. Amr", "Amr Diab", "Greatest Hits"),
        ]);
        let names: Vec<String> = lib.artists.values().map(|a| a.name.clone()).collect();
        assert!(names.contains(&"Amr Diab".to_string()));
        assert!(!names.contains(&"Sherine ft. Amr".to_string()));
        let amr = lib.artists.values().find(|a| a.name == "Amr Diab").unwrap();
        assert_eq!(amr.album_ids.len(), 1);
    }

    #[test]
    fn empty_album_artist_falls_back_to_track_artist() {
        let lib = Library::from_tracks(vec![track("Solo Singer", "", "Album")]);
        let names: Vec<String> = lib.artists.values().map(|a| a.name.clone()).collect();
        assert!(names.contains(&"Solo Singer".to_string()));
    }
}
