//! Browsed-list state + tracklist sort methods on `AppState` (extracted from
//! app/mod.rs). The browsed list (`Browser`) backs the non-Dashboard local views
//! (Split / search): a flat track list shown in the centre instead of the queue.

use super::*;

impl AppState {
    /// Stable-sort track ids by the active sort spec (no-op when sort is off).
    pub(crate) fn sort_ids(&self, mut ids: Vec<TrackId>) -> Vec<TrackId> {
        if !self.sort.is_empty() {
            ids.sort_by(|a, b| self.cmp_tracks(*a, *b));
        }
        ids
    }

    /// Re-sort the currently browsed list in place (after a sort change).
    pub(crate) fn sort_browse(&mut self) {
        let ids = std::mem::take(&mut self.browser.list);
        self.browser.list = self.sort_ids(ids);
    }

    /// Canonical "artist,album,year,track" string for the active sort (explicit
    /// directions only when they differ from a field's default), for persistence.
    pub(crate) fn sort_order_string(&self) -> String {
        self.sort
            .iter()
            .map(|&(f, desc)| {
                if desc == f.default_desc() {
                    f.name().to_string()
                } else {
                    format!("{}:{}", f.name(), if desc { "desc" } else { "asc" })
                }
            })
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Human summary of the active sort, e.g. "artist, album, year↓, track".
    pub fn sort_describe(&self) -> String {
        self.sort
            .iter()
            .map(|&(f, desc)| format!("{}{}", f.name(), if desc { "↓" } else { "↑" }))
            .collect::<Vec<_>>()
            .join(", ")
    }

    // ── Library (#2) 3-column "Miller" browser ─────────────────────────────────

    /// Row counts of the three columns for the current selection: (artists,
    /// albums-of-selected-artist, tracks-of-selected-album). Selections are clamped
    /// so a stale cursor (after a library change) can't read out of bounds.
    pub fn browser_counts(&self) -> (usize, usize, usize) {
        let artists = self.library.artists_sorted();
        let na = artists.len();
        let a = self.browser.artist.min(na.saturating_sub(1));
        let albums = artists
            .get(a)
            .map(|ar| self.library.albums_of(ar.id))
            .unwrap_or_default();
        let nb = albums.len();
        let b = self.browser.album.min(nb.saturating_sub(1));
        let nc = albums
            .get(b)
            .map(|al| self.library.tracks_of(al.id).len())
            .unwrap_or(0);
        (na, nb, nc)
    }

    /// Route a [`Motion`] for the focused 3-column browser: Left/Right switch the
    /// active column, everything else moves within it.
    pub(crate) fn browser_nav(&mut self, m: Motion) {
        match m {
            Motion::Left => self.browser_cols(-1),
            Motion::Right => self.browser_cols(1),
            _ => self.browser_move(m),
        }
    }

    /// Move the selection within the active column. Changing a parent selection
    /// resets the dependent columns (new artist → album+track to 0; new album →
    /// track to 0), so the right-hand columns always reflect the new parent.
    pub(crate) fn browser_move(&mut self, m: Motion) {
        let (na, nb, nc) = self.browser_counts();
        match self.browser.col {
            0 => {
                let old = self.browser.artist;
                self.browser.artist = step(self.browser.artist, m, na);
                if self.browser.artist != old {
                    self.browser.album = 0;
                    self.browser.track = 0;
                }
            }
            1 => {
                let old = self.browser.album;
                self.browser.album = step(self.browser.album, m, nb);
                if self.browser.album != old {
                    self.browser.track = 0;
                }
            }
            _ => self.browser.track = step(self.browser.track, m, nc),
        }
    }

    /// Move the active column left/right (−1 / +1), clamped to 0..=2.
    pub(crate) fn browser_cols(&mut self, delta: i32) {
        self.browser.col = (self.browser.col as i32 + delta).clamp(0, 2) as u8;
    }

    /// Enter/→ on a column: drill into the next column, or play the selected track
    /// once on the TRACKS column.
    pub(crate) fn browser_activate(&mut self) {
        if self.browser.col < 2 {
            self.browser.col += 1;
        } else {
            self.play_browser_track();
        }
    }

    /// Play the track selected in the browser (TRACKS column): load the selected
    /// album's tracks as the queue and start at the selected row.
    pub(crate) fn play_browser_track(&mut self) {
        let (ids, sel) = {
            let artists = self.library.artists_sorted();
            let a = self.browser.artist.min(artists.len().saturating_sub(1));
            let Some(artist) = artists.get(a) else { return };
            let albums = self.library.albums_of(artist.id);
            let b = self.browser.album.min(albums.len().saturating_sub(1));
            let Some(album) = albums.get(b) else { return };
            let tracks = self.library.tracks_of(album.id);
            let sel = self.browser.track.min(tracks.len().saturating_sub(1));
            (tracks.iter().map(|t| t.id).collect::<Vec<_>>(), sel)
        };
        if let Some(id) = ids.get(sel).copied() {
            self.player.queue.items = ids;
            self.player.queue.position = sel;
            self.player.current = Some(id);
            self.play_current();
        }
    }
}

/// Browsed-list state: the flat track list shown in the centre of the
/// non-Dashboard local views (Split / search results) — empty → show the play
/// queue instead. (The Dashboard uses the drill-in `LocalBrowse` model.) Also
/// holds the Library view's 3-column "Miller" browser cursors.
#[derive(Default)]
pub struct Browser {
    /// Tracks currently shown in the centre list (a browsed album/playlist);
    /// empty → show the play queue.
    pub list: Vec<TrackId>,
    /// Title for the browsed list.
    pub title: String,
    /// Library (#2) 3-column browser: the active column — 0 = ARTISTS, 1 = ALBUMS
    /// (of the selected artist), 2 = TRACKS (of the selected album).
    pub col: u8,
    /// Selected row in each column. Dependent columns reset to 0 when a parent
    /// selection changes (a new artist clears album+track; a new album clears track).
    pub artist: usize,
    pub album: usize,
    pub track: usize,
    /// Per-column scroll offsets, kept in sync with the selection during render
    /// (`sticky_off`); `Cell` because the render borrows `&AppState`.
    pub artist_off: std::cell::Cell<usize>,
    pub album_off: std::cell::Cell<usize>,
    pub track_off: std::cell::Cell<usize>,
}
