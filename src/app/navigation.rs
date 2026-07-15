//! Cursor navigation & activation on `AppState` (extracted from app/mod.rs): the
//! single dispatcher that routes a Motion (j/k, ctrl-n/p, page, top/bottom) to
//! whichever list/overlay is focused, and the Enter/Activate dispatcher. Each
//! arm just moves a cursor or calls into the owning subsystem — no rendering.

use super::*;

/// Step a fixed ring of sections, returning the newly-selected entry when the
/// motion actually moved off the current one (else `None`). The two browser
/// sources — Spotify's `Section` and the local `LocalSection` — share this
/// stepping math; only the ring and the field/loader they apply it to differ,
/// which keeps the per-source navigation a thin instance over one model.
fn step_ring<S: Copy + PartialEq>(all: &[S], cur: S, m: Motion) -> Option<S> {
    let i = all.iter().position(|s| *s == cur).unwrap_or(0);
    let next = step(i, m, all.len());
    (next != i).then(|| all[next])
}

impl AppState {
    /// Universal item navigation (ctrl-n/ctrl-p): move within whatever list or
    /// menu is focused — the command palette, the open Tag Edit tab, or (default)
    /// the focused list/overlay handled by `move_selection`.
    pub(super) fn nav(&mut self, m: Motion) {
        if self.palette.is_some() {
            let n = self.palette_matches().len();
            if let Some(p) = &mut self.palette {
                p.sel = step(p.sel, m, n);
            }
        } else if self.tags.edit.is_some() && self.tags.tab == 0 {
            self.tag_edit_move(m);
        } else if self.tags.search.is_some() && self.tags.tab == 1 {
            self.tag_move(m);
        } else if self.tags.cover.is_some() && self.tags.tab == 2 {
            self.cover_move(m);
        } else {
            self.move_selection(m);
        }
    }

    /// Move the cursor of whichever list/overlay currently owns movement — the
    /// stats/add/settings overlays first, then the per-view list (Radio stations,
    /// Spotify sidebar/list/pane, the library columns, or the focused pane).
    pub(super) fn move_selection(&mut self, m: Motion) {
        // The Info overlay captures movement to scroll its active tab (over-scroll
        // is clamped at render time against the real content height).
        if self.info.is_some() {
            self.info_scroll(m);
            return;
        }
        // The "add to playlist" picker captures movement.
        if !self.input.add_targets.is_empty() {
            let n = self.library.playlists.len() + 1; // +1 for the "New playlist" row
            self.input.add_sel = step(self.input.add_sel, m, n);
            return;
        }
        // The Spotify playlist picker captures movement (its own cursor).
        if self.spotify.pl_modal.is_some() {
            self.spotify_playlist_move(m);
            return;
        }
        // Settings popup overlay captures movement first (it's a group detail).
        if self.settings.popup.is_some() {
            let n = self.settings_group_items().len();
            self.settings.sel = step(self.settings.sel, m, n);
            return;
        }
        // Settings overlay: a group tab is always active, so movement walks its rows.
        if self.settings.overlay {
            let n = self.settings_group_items().len();
            self.settings.sel = step(self.settings.sel, m, n);
            return;
        }
        // The radio "add to playlist" picker captures movement (+1 = New playlist row)
        // — but only in the Radio view, so a modal left open never steals j/k from a
        // local/Spotify list after switching views.
        if self.layout == Layout::Radio && self.radio.pl.adding.is_some() {
            let n = self.radio.playlists.len() + 1;
            self.radio.pl.add_sel = step(self.radio.pl.add_sel, m, n);
            return;
        }
        // In the Radio view, movement walks the open picker, the section sidebar
        // (when it has focus), the flat playlist list, else the station list.
        if self.layout == Layout::Radio {
            if self.radio.picker.is_some() {
                let n = self.radio_picker_match_count();
                if let Some(p) = &mut self.radio.picker {
                    p.sel = step(p.sel, m, n);
                }
            } else if self.focus == Focus::Sidebar {
                if let Some(s) = step_ring(&crate::app::RadioSection::ALL, self.radio.section, m) {
                    self.radio.section = s;
                    self.radio.sel = 0;
                    self.radio.pl.open = None; // leaving the section drops any drill
                }
            } else if self.radio.section == crate::app::RadioSection::Playlists
                && self.radio.pl.open.is_none()
            {
                let n = self.radio.playlists.len();
                self.radio.pl.sel = step(self.radio.pl.sel, m, n);
            } else {
                let n = self.radio_view_list().len();
                self.radio.sel = step(self.radio.sel, m, n);
            }
            return;
        }
        // Every browser view (Spotify + the local Dashboard/Split/Compact) shares
        // ONE focus dispatch; only the backing cursor differs, so each arm calls a
        // small per-source helper. Radio (handled above) keeps its own single-list
        // model.
        match self.focus {
            Focus::Sidebar => self.browse_move_section(m),
            Focus::Pane(Panel::Queue) => self.browse_move_queue(m),
            Focus::Pane(Panel::Artist) => self.scroll_artist_pane(m),
            // Lyrics pane auto-follows playback until the user scrolls it by hand.
            Focus::Pane(Panel::Lyrics) => self.scroll_lyrics_pane(m),
            // other panes have no scrollable text; search mode: j/k is a no-op.
            Focus::Pane(_) | Focus::Search => {}
            Focus::Main => self.browse_move_main(m),
        }
    }

    /// Whether the Main content is a cover-art grid or release-carousel region using
    /// 2-D navigation right now — so directional input moves cards in a plane
    /// (horizontal within a row/carousel, vertical between rows/carousels) rather
    /// than stepping a flat list. The single source of truth shared by the keymap
    /// (`h`/`l`/`j`/`k`) and touchpad scroll (`handle_scroll`), so both agree on when
    /// a view is a grid.
    pub(crate) fn grid_nav_active(&self) -> bool {
        if self.focus != Focus::Main {
            return false;
        }
        match self.layout {
            Layout::Spotify => {
                self.spotify_grid_active()
                    || self.spotify_browse_grid_active()
                    || self.spotify_podcast_grid_active()
                    || self.spotify_sectioned_active()
                    || self
                        .spotify_carousels_from()
                        .is_some_and(|from| self.spotify.sel >= from)
            }
            Layout::Dashboard => {
                !self.is_searching()
                    && (self.local_grid_active()
                        || self
                            .artist_releases_from()
                            .is_some_and(|from| self.local.sel >= from))
            }
            _ => false,
        }
    }

    /// Move (and live-load) the focused section sidebar — Spotify's `Section`
    /// ring or the local `LocalSection` ring. The stepping math is shared
    /// (`step`); only the section list + load call are source-specific.
    fn browse_move_section(&mut self, m: Motion) {
        if self.layout == Layout::Spotify {
            if let Some(s) = step_ring(&crate::spotify::api::Section::ALL, self.spotify.section, m)
            {
                self.spotify.section = s;
                self.spotify_load_section();
            }
        } else if let Some(s) = step_ring(&crate::app::LocalSection::ALL, self.local.section, m) {
            self.local.section = s;
            self.local_load_section();
        }
    }

    /// Move the focused Queue pane's selection — the Spotify queue mirror or the
    /// local play queue.
    fn browse_move_queue(&mut self, m: Motion) {
        if self.layout == Layout::Spotify {
            let n = self.spov.sp_queue.len();
            self.spotify.queue_sel = step(self.spotify.queue_sel, m, n);
        } else {
            let n = self.queue_len();
            self.queue_sel = step(self.queue_sel, m, n);
        }
    }

    /// Move the focused Main list's selection. Spotify and the local drill-in
    /// list each step their own cursor (the local Dashboard skips non-selectable
    /// group headers on the artist page); the flat local tracklist views
    /// (Split/Compact, or while searching) step the shared display selection.
    fn browse_move_main(&mut self, m: Motion) {
        // the Library view's Main content is the 3-column browser: h/l switch the
        // active column, j/k move within it.
        if self.layout == Layout::LibraryFocus {
            self.browser_nav(m);
            return;
        }
        if self.layout == Layout::Spotify {
            let n = self.spotify.items.len();
            self.spotify.sel = step(self.spotify.sel, m, n);
            return;
        }
        if self.layout != Layout::Dashboard || self.is_searching() {
            self.selection = step(self.selection, m, self.display_len());
            return;
        }
        let n = self.local.items.len();
        let mut sel = step(self.local.sel, m, n);
        let fwd = matches!(m, Motion::Down | Motion::PageDown | Motion::Top);
        // step past non-selectable group headers (the artist page)
        let mut guard = 0;
        while self
            .local
            .items
            .get(sel)
            .is_some_and(|i| !i.is_selectable())
            && guard < n
        {
            sel = if fwd {
                (sel + 1).min(n.saturating_sub(1))
            } else {
                sel.saturating_sub(1)
            };
            guard += 1;
        }
        self.local.sel = sel;
    }

    /// Scroll the (shared) artist info pane's text region. One pane is visible at a
    /// time, so the local and Spotify artist panes share `scroll.artist`.
    fn scroll_artist_pane(&mut self, m: Motion) {
        let max = self.scroll.artist_max.get();
        let cur = self.scroll.artist.get();
        let next = match m {
            Motion::Up => cur.saturating_sub(1),
            Motion::Down => cur + 1,
            Motion::PageUp => cur.saturating_sub(5),
            Motion::PageDown => cur + 5,
            Motion::Top => 0,
            Motion::Bottom => max,
            _ => cur,
        };
        self.scroll.artist.set(next.min(max));
    }

    /// Scroll the lyrics/notes pane by hand, taking it out of playback auto-follow
    /// (`lyrics_manual`) until the track changes. Clamped to the renderer's last max;
    /// `Top`/`Bottom` also re-anchor. Shared by the local and Spotify lyrics panes.
    fn scroll_lyrics_pane(&mut self, m: Motion) {
        let max = self.scroll.lyrics_max.get();
        let cur = self.scroll.lyrics.get();
        let next = match m {
            Motion::Up => cur.saturating_sub(1),
            Motion::Down => cur + 1,
            Motion::PageUp => cur.saturating_sub(5),
            Motion::PageDown => cur + 5,
            Motion::Top => 0,
            Motion::Bottom => max,
            _ => cur,
        };
        self.scroll.lyrics.set(next.min(max));
        self.scroll.lyrics_manual.set(true);
    }

    /// Dispatch Enter/Activate to the focused pane / browser column.
    pub(super) fn activate(&mut self) {
        if !self.input.add_targets.is_empty() {
            self.confirm_add_to_playlist();
            return;
        }
        // Spotify playlist modals capture Enter (unfollow confirm → picker/name).
        if self.spotify.pl_confirm_delete.is_some() {
            self.spotify_confirm_delete_playlist();
            return;
        }
        if self.spotify.pl_modal.is_some() {
            self.spotify_playlist_confirm();
            return;
        }
        // the settings popup (or the full Settings overlay) takes Enter
        if self.settings.popup.is_some() || self.settings.overlay {
            self.settings_activate();
            return;
        }
        match self.focus {
            // the section sidebar (Dashboard only) → load that section's list
            Focus::Sidebar => self.local_activate(),
            Focus::Pane(Panel::Queue) => {
                let target = self.queue_sel;
                if target < self.player.queue.items.len() && target != self.player.queue.position {
                    self.player.push_history(); // jumping is undoable via Previous
                    self.player.queue.position = target;
                    self.player.current = self.player.queue.items.get(target).copied();
                    self.play_current();
                }
            }
            // Artist pane: ⏎ opens the now-playing artist's full page (Spotify or
            // local, depending on the view).
            Focus::Pane(Panel::Artist) => self.open_artist_page(),
            // Lyrics pane scrolls (no select); Search mode has no Enter action.
            Focus::Pane(_) | Focus::Search => {}
            // Library: Enter drills into the next column, or plays on the TRACKS column.
            Focus::Main if self.layout == Layout::LibraryFocus => self.browser_activate(),
            // Main: the Dashboard drill-in (container → drill, track → play); other
            // local views keep the flat tracklist activate.
            Focus::Main if self.layout == Layout::Dashboard && !self.is_searching() => {
                self.local_activate()
            }
            Focus::Main => self.activate_selection(),
        }
    }
}
