//! Library load / rescan plumbing on `AppState` (extracted from app/mod.rs):
//! applying streamed scanner events, handing the scanner a cache snapshot,
//! queuing/draining a rescan request, and swapping in a freshly-scanned library
//! while preserving live playback by path. The scanner itself runs on its own
//! thread (driven from `tui`); this is just the app-side glue.

use super::*;

/// Library load/scan lifecycle state, grouped out of `AppState` (the state behind
/// this module): `restore` is the session pending restore once the library lands,
/// `pending` a queued rescan's roots (drained by the tui loop), and `progress` the
/// active scan's `(done, total?)` — `Some` while a scan is in flight.
#[derive(Default)]
pub struct ScanState {
    pub restore: Option<crate::session::Session>,
    pub pending: Option<Vec<PathBuf>>,
    pub progress: Option<(usize, Option<usize>)>,
}

impl AppState {
    /// Apply a streamed library event from the scanner thread.
    pub fn on_library_event(&mut self, ev: LibraryEvent) {
        match ev {
            LibraryEvent::ScanStarted { .. } => {
                self.scan.progress = Some((0, None));
                self.notify("Scanning library…".into());
            }
            LibraryEvent::Indexed { done, total } => self.scan.progress = Some((done, total)),
            LibraryEvent::Loaded(tracks) => {
                if !tracks.is_empty() {
                    self.set_library(tracks);
                    // refresh the on-disk cache so next launch is instant
                    crate::library::store::LibraryCache::from_library(&self.library)
                        .save(&self.config.dir);
                }
            }
            LibraryEvent::ScanFinished { tracks } => {
                self.scan.progress = None;
                self.notify(if tracks > 0 {
                    format!("Indexed {tracks} tracks")
                } else {
                    "No music found — showing demo library".into()
                });
            }
            LibraryEvent::Error(e) => self.notify(format!("Scan error: {e}")),
            LibraryEvent::TrackAdded(_) => {}
        }
    }

    /// Snapshot of the catalogue (path → track) handed to the scanner so it can
    /// skip re-parsing unchanged files.
    pub fn cache_map(&self) -> std::collections::HashMap<PathBuf, Track> {
        self.library
            .tracks
            .values()
            .map(|t| (t.path.clone(), t.clone()))
            .collect()
    }

    /// Ask the tui loop to (re)scan the current music dirs.
    pub(crate) fn request_rescan(&mut self) {
        self.scan.pending = Some(self.config.music_dirs.clone());
    }

    /// Drained by the tui loop each iteration. Won't start a new scan while one is
    /// in flight — the request stays pending and runs when the current finishes
    /// (prevents overlapping full-disk-walk threads under heavy tag editing).
    pub fn take_rescan(&mut self) -> Option<Vec<PathBuf>> {
        if self.scan.progress.is_some() {
            return None;
        }
        let dirs = self.scan.pending.take();
        if dirs.is_some() {
            self.scan.progress = Some((0, None)); // mark scanning until ScanStarted/Finished
        }
        dirs
    }

    /// Swap in a freshly-scanned library. A rescan re-assigns TrackIds, so a
    /// mid-session rescan (e.g. after a tag edit) remaps the live queue / current /
    /// loaded track by path and keeps playing; an initial load resets to All Tracks
    /// and restores the saved session.
    pub(crate) fn set_library(&mut self, tracks: Vec<Track>) {
        // A rescan re-assigns TrackIds, so capture the LIVE playback state by path
        // (against the old library) before replacing it — a mid-session rescan
        // (e.g. after a tag edit) must not reset the queue or interrupt playback.
        let path_of = |id: TrackId, lib: &Library| lib.track(id).map(|t| t.path.clone());
        let cur_path = self
            .player
            .current
            .and_then(|id| path_of(id, &self.library));
        let loaded_path = self.loaded_track.and_then(|id| path_of(id, &self.library));
        let queue_paths: Vec<PathBuf> = self
            .player
            .queue
            .items
            .iter()
            .filter_map(|&id| path_of(id, &self.library))
            .collect();
        let was_playing = self.player.status == Status::Playing;
        let elapsed = self.player.elapsed;

        let mut lib = Library::from_tracks(tracks);
        UserData::load(&self.config.dir).apply_to(&mut lib);
        self.library = lib;
        self.search.lib_gen += 1; // invalidate the search cache
        self.playlist_name = "All Tracks".into();

        // path → new id, to remap the live playback state across the rescan
        let by_path: std::collections::HashMap<PathBuf, TrackId> = self
            .library
            .tracks
            .values()
            .map(|t| (t.path.clone(), t.id))
            .collect();
        // Did the currently-playing track survive the rescan? A real live track
        // does (a mid-session rescan keeps playing). A *placeholder* current — the
        // demo track seeded when the on-disk cache failed to load, e.g. after a
        // rebuild — does NOT, so we must fall through to restoring the saved
        // session rather than stranding a stale id (which would drop the
        // last-played track on every fresh binary).
        let surviving_current = cur_path.as_ref().and_then(|p| by_path.get(p).copied());

        if let Some(cur) = surviving_current {
            // Mid-session rescan with a live track: remap the current queue /
            // current / loaded track to the new ids by path, and keep playing —
            // the engine still holds the same file, so we never reload or pause it.
            let new_queue: Vec<TrackId> = queue_paths
                .iter()
                .filter_map(|p| by_path.get(p).copied())
                .collect();
            if !new_queue.is_empty() {
                self.player.queue.items = new_queue;
            }
            self.player.current = Some(cur);
            self.player.duration = self
                .library
                .track(cur)
                .map(|t| t.duration())
                .unwrap_or(self.player.duration);
            self.player.elapsed = elapsed;
            self.player.status = if was_playing {
                Status::Playing
            } else {
                Status::Paused
            };
            if let Some(pos) = self.player.queue.items.iter().position(|&x| x == cur) {
                self.player.queue.position = pos;
            }
            self.loaded_track = loaded_path.as_ref().and_then(|p| by_path.get(p).copied());
            self.update_gapless_next(); // queue ids changed → refresh the preload
        } else {
            // Initial load, or the live track vanished (a demo placeholder, or a
            // since-deleted file): reset to All Tracks + restore the saved session.
            // Initial load (nothing playing): reset to All Tracks + restore session.
            let q = self.library.all_tracks_sorted();
            self.player.current = q.first().copied();
            self.player.duration = self
                .player
                .current
                .and_then(|id| self.library.track(id))
                .map(|t| t.duration())
                .unwrap_or_default();
            self.player.queue.items = q;
            self.player.queue.position = 0;
            self.player.elapsed = Duration::ZERO;
            self.player.status = Status::Paused; // playback starts on demand (M4)
            self.selection = 0;
            self.restore_library_state();
            // the picker is set at startup but the current track is only known now
            // (library landed / session restored), so load its cover here — else the
            // now-bar art stays blank until the next track change.
            self.reload_cover();
        }
        // populate the local drill-in browse list now the library is ready —
        // unless `restore_library_state` already loaded the session's section.
        if self.local.items.is_empty() {
            self.local_load_section();
        }
    }
}
