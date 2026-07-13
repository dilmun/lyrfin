//! Online tag-search on `AppState` (extracted from app/tags): the tabbed
//! Tag-Edit search popup (iTunes/Deezer/MusicBrainz single + album lookups),
//! source/edition cycling, the per-track diff + masked apply, and the
//! command-palette entry points that kick the searches off.

use super::*;

impl AppState {
    pub fn set_tag_sender(&mut self, tx: crossbeam_channel::Sender<crate::tagsearch::TagRequest>) {
        self.workers.tag = Some(tx);
    }

    /// Open the tag search for the modal's primary (or now-playing) track.
    pub fn open_tag_search(&mut self) {
        self.tags.tab = 1; // Auto Tag tab
        let Some(t) = self.tags_primary().and_then(|id| self.library.track(id)) else {
            self.notify("No track playing".into());
            return;
        };
        let song = t.path.clone();
        let album: Vec<std::path::PathBuf> = match t.album_id {
            Some(a) => self
                .library
                .tracks_of(a)
                .iter()
                .map(|t| t.path.clone())
                .collect(),
            None => vec![song.clone()],
        };
        let label = match (t.title.is_empty(), t.artist.is_empty()) {
            (false, false) => format!("{} · {}", t.title, t.artist),
            (false, true) => t.title.clone(),
            _ => "this track".into(),
        };
        let query = format!("{} {}", t.artist, t.title).trim().to_string();
        let current = crate::tags::EditableTags::from_track(t);
        // local album tracks (path + current tags), in track order
        let album_tracks: Vec<(std::path::PathBuf, crate::tags::EditableTags)> = match t.album_id {
            Some(a) => self
                .library
                .tracks_of(a)
                .iter()
                .map(|t| (t.path.clone(), crate::tags::EditableTags::from_track(t)))
                .collect(),
            None => vec![(song.clone(), current.clone())],
        };
        self.tags.search = Some(TagSearch {
            key: String::new(),
            label,
            seeded: query.clone(),
            query,
            editing: false,
            status: CoverStatus::Searching,
            candidates: Vec::new(),
            sel: 0,
            song,
            album,
            current,
            album_mode: false,
            albums: Vec::new(),
            album_sel: 0,
            track_sel: 0,
            album_tracks,
            pending: None,
            qcaret: 0,
        });
        self.run_tag_search();
    }

    pub fn tag_move(&mut self, m: Motion) {
        let wrap = |i: usize, n: usize| match m {
            Motion::Up => (i + n - 1) % n,
            Motion::Down => (i + 1) % n,
            Motion::Top => 0,
            Motion::Bottom => n - 1,
            _ => i,
        };
        if let Some(ts) = self.tags.search.as_mut().filter(|ts| !ts.editing) {
            if ts.album_mode {
                if !ts.album_tracks.is_empty() {
                    ts.track_sel = wrap(ts.track_sel, ts.album_tracks.len());
                }
            } else if !ts.candidates.is_empty() {
                ts.sel = wrap(ts.sel, ts.candidates.len());
            }
        }
    }

    /// ←/→ in album mode cycles the matched album source.
    pub fn tag_source(&mut self, dir: i32) {
        if let Some(ts) = self.tags.search.as_mut()
            && ts.album_mode
            && !ts.albums.is_empty()
        {
            let n = ts.albums.len() as i32;
            ts.album_sel = (((ts.album_sel as i32 + dir) % n + n) % n) as usize;
            ts.track_sel = 0;
        }
    }

    pub fn tag_input(&mut self, s: String) {
        if let Some(ts) = self.tags.search.as_mut() {
            ts.editing = true;
            ts.qcaret = s.chars().count();
            ts.query = s;
        }
    }

    /// Enter inside the popup: re-search (editing), or *request* the apply for
    /// the active mode (shows a confirm before writing).
    pub fn tag_activate(&mut self) {
        let Some(ts) = self.tags.search.as_ref() else {
            return;
        };
        if ts.editing {
            if let Some(ts) = self.tags.search.as_mut() {
                ts.editing = false;
            }
            self.run_tag_search();
        } else if ts.album_mode {
            self.tag_request(PendingApply::AlbumFull);
        } else {
            self.tag_request(PendingApply::Song);
        }
    }

    /// Stage an apply for confirmation (only if there's a valid target).
    pub fn tag_request(&mut self, kind: PendingApply) {
        if let Some(ts) = self.tags.search.as_mut() {
            let ok = match kind {
                PendingApply::Song | PendingApply::AlbumBasic => {
                    ts.candidates.get(ts.sel).is_some()
                }
                PendingApply::AlbumFull => ts.albums.get(ts.album_sel).is_some(),
            };
            if ok {
                ts.pending = Some(kind);
            }
        }
    }

    /// Confirm + perform the staged tag apply.
    pub fn tag_confirm(&mut self) {
        let Some(kind) = self.tags.search.as_mut().and_then(|ts| ts.pending.take()) else {
            return;
        };
        match kind {
            PendingApply::Song => self.tag_apply(false),
            PendingApply::AlbumBasic => self.tag_apply(true),
            PendingApply::AlbumFull => self.tag_apply_album(),
        }
    }

    /// Apply the selected album source to every matched local track (album mode).
    pub fn tag_apply_album(&mut self) {
        let Some(ts) = self.tags.search.as_mut() else {
            return;
        };
        let Some(src) = ts.albums.get(ts.album_sel) else {
            return;
        };
        let local: Vec<(u16, String)> = ts
            .album_tracks
            .iter()
            .map(|(_, t)| (t.track_no.parse::<u16>().unwrap_or(0), t.title.clone()))
            .collect();
        let assign = crate::tagsearch::match_album(&local, &src.tracks);
        let assignments: Vec<(std::path::PathBuf, crate::tagsearch::TagCandidate)> = ts
            .album_tracks
            .iter()
            .enumerate()
            .filter_map(|(i, (path, _))| assign[i].map(|fi| (path.clone(), src.tracks[fi].clone())))
            .collect();
        if assignments.is_empty() {
            self.notify("No tracks matched".into());
            return;
        }
        let key = ts.key.clone();
        ts.status = CoverStatus::Embedding;
        if let Some(tx) = &self.workers.tag {
            let _ = tx.send(crate::tagsearch::TagRequest::ApplyAlbum { assignments, key });
        }
    }

    /// Write the selected candidate's tags: `album` also pushes album-level
    /// fields (album / album-artist / year / genre) to every album track.
    pub fn tag_apply(&mut self, album: bool) {
        let Some(ts) = self.tags.search.as_mut() else {
            return;
        };
        if ts.editing {
            return;
        }
        let Some(c) = ts.candidates.get(ts.sel) else {
            return;
        };
        let fields = c.clone();
        let song = ts.song.clone();
        let album_paths = if album { ts.album.clone() } else { Vec::new() };
        let key = ts.key.clone();
        ts.status = CoverStatus::Embedding;
        if let Some(tx) = &self.workers.tag {
            let _ = tx.send(crate::tagsearch::TagRequest::Apply {
                fields,
                song,
                album: album_paths,
                key,
            });
        }
    }

    pub fn on_tag_result(&mut self, res: crate::tagsearch::TagResult) {
        use crate::tagsearch::TagResult::*;
        match res {
            Found { key, candidates } => {
                if let Some(ts) = self.tags.search.as_mut()
                    && ts.key == key
                {
                    ts.status = if candidates.is_empty() {
                        CoverStatus::Empty
                    } else {
                        CoverStatus::Results
                    };
                    ts.candidates = candidates;
                    ts.sel = 0;
                }
            }
            AlbumFound { key, albums } => {
                if let Some(ts) = self.tags.search.as_mut()
                    && ts.key == key
                {
                    ts.status = if albums.is_empty() {
                        CoverStatus::Empty
                    } else {
                        CoverStatus::Results
                    };
                    ts.albums = albums;
                    ts.album_sel = 0;
                    ts.track_sel = 0;
                }
            }
            Error { key, msg } => {
                if let Some(ts) = self.tags.search.as_mut()
                    && ts.key == key
                {
                    ts.status = CoverStatus::Error(msg);
                }
            }
            Applied { key, count, msg } => {
                if self.tags.search.as_ref().is_some_and(|ts| ts.key == key) {
                    self.close_tags();
                    if count > 0 {
                        self.request_rescan(); // pick up the new tags
                    }
                    self.notify(msg);
                }
            }
        }
    }

    pub(crate) fn run_tag_search(&mut self) {
        self.workers.tag_seq += 1;
        let key = format!("ts{}", self.workers.tag_seq);
        let Some(ts) = self.tags.search.as_mut() else {
            return;
        };
        ts.key = key.clone();
        ts.status = CoverStatus::Searching;
        ts.candidates.clear();
        ts.sel = 0;
        let query = ts.query.clone();
        // If the user edited the query, treat it as the *artist* to match (no
        // title/year/edition filter) — so "Diana" keeps only Diana… artists, not
        // every song that merely mentions Diana.
        let edited = ts.query.trim() != ts.seeded.trim();
        let (artist, title, year, track_count) = if edited {
            (ts.query.clone(), String::new(), None, 0)
        } else {
            (
                ts.current.artist.clone(),
                ts.current.title.clone(),
                ts.current.year.parse::<u16>().ok(),
                ts.album_tracks.len(),
            )
        };
        if query.is_empty() {
            ts.status = CoverStatus::Empty;
            return;
        }
        if let Some(tx) = &self.workers.tag {
            let _ = tx.send(crate::tagsearch::TagRequest::Search {
                query,
                artist,
                title,
                year,
                track_count,
                key,
            });
        }
    }

    /// Toggle single ⇄ album mode (`s`); fetches the album on first switch.
    pub fn toggle_tag_album(&mut self) {
        let Some(ts) = self.tags.search.as_mut() else {
            return;
        };
        ts.album_mode = !ts.album_mode;
        if ts.album_mode && ts.albums.is_empty() {
            self.run_album_search();
        }
    }

    fn run_album_search(&mut self) {
        self.workers.tag_seq += 1;
        let key = format!("ts{}", self.workers.tag_seq);
        let Some(ts) = self.tags.search.as_mut() else {
            return;
        };
        ts.key = key.clone();
        ts.status = CoverStatus::Searching;
        ts.albums.clear();
        ts.album_sel = 0;
        ts.track_sel = 0;
        let artist = ts.current.artist.clone();
        let album = ts.current.album.clone();
        let track_count = ts.album_tracks.len();
        if album.is_empty() {
            ts.status = CoverStatus::Empty;
            return;
        }
        if let Some(tx) = &self.workers.tag {
            let _ = tx.send(crate::tagsearch::TagRequest::AlbumSearch {
                artist,
                album,
                track_count,
                key,
            });
        }
    }
}
