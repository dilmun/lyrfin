//! Mouse hit-map + handlers methods on `AppState` (extracted from app/mod.rs).

use super::*;

impl AppState {
    // ---- mouse hit-map ---------------------------------------------------
    /// Register a clickable region (called by the render layer each frame).
    pub fn register_click(&self, rect: Rect, target: MouseTarget) {
        if !self.config.mouse {
            return; // no mouse capture → the hit-map is never read
        }
        self.hit.borrow_mut().push((rect, target));
    }

    /// Resolve the topmost registered region at `(x, y)`. While a modal overlay is
    /// open, only the overlay's own regions (registered at/after `overlay_hits`) are
    /// considered, so a click on the view behind the modal is ignored.
    fn hit_at(&self, x: u16, y: u16) -> Option<(Rect, MouseTarget)> {
        let lo = if self.modal_open() {
            self.overlay_hits.get()
        } else {
            0
        };
        self.hit
            .borrow()
            .iter()
            .enumerate()
            .rev() // last registered = topmost
            .find(|(i, (r, _))| {
                *i >= lo && x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
            })
            .map(|(_, (r, t))| (*r, *t))
    }

    /// Resolve a click at `(x, y)` against the hit map and act. `double` plays
    /// the clicked track (single-click just selects it).
    pub fn handle_click(&mut self, x: u16, y: u16, double: bool) {
        let Some((rect, target)) = self.hit_at(x, y) else {
            return;
        };
        // acting on a target changes state (selection / focus / playback) — request a
        // repaint, since mouse events (unlike keyboard `update`) don't otherwise mark
        // the frame dirty, so the screen would look frozen while nothing animates.
        self.mark_dirty();
        match target {
            MouseTarget::Track(i) => {
                self.focus = Focus::Main;
                // the Dashboard's main list is the drill-in `local.items`; every
                // other local view shows the flat tracklist (`selection`).
                if self.layout == Layout::Dashboard && !self.is_searching() {
                    self.local.sel = i;
                } else {
                    self.selection = i;
                }
                if double {
                    self.activate();
                }
            }
            // Library (#2) columns: click focuses the column + selects; selecting a
            // parent resets the dependent columns; double-click a track plays it.
            MouseTarget::BrowseArtist(i) => {
                self.focus = Focus::Main;
                self.browser.col = 0;
                if self.browser.artist != i {
                    self.browser.artist = i;
                    self.browser.album = 0;
                    self.browser.track = 0;
                }
            }
            MouseTarget::BrowseAlbum(i) => {
                self.focus = Focus::Main;
                self.browser.col = 1;
                if self.browser.album != i {
                    self.browser.album = i;
                    self.browser.track = 0;
                }
            }
            MouseTarget::BrowseTrack(i) => {
                self.focus = Focus::Main;
                self.browser.col = 2;
                self.browser.track = i;
                if double {
                    self.play_browser_track();
                }
            }
            MouseTarget::QueueRow(i) => {
                self.set_focus(Focus::Pane(Panel::Queue));
                if i < self.player.queue.items.len() {
                    self.queue_sel = i;
                    // double-click jumps to that entry and plays (like Enter on
                    // the queue); a single click just selects the row
                    if double && i != self.player.queue.position {
                        self.player.push_history(); // undoable via Previous
                        self.player.queue.position = i;
                        self.player.current = self.player.queue.items.get(i).copied();
                        self.play_current();
                    }
                }
            }
            MouseTarget::SpotifyQueueRow(i) => {
                // focus the QUEUE pane + select; double-click plays from there
                self.focus = crate::app::Focus::Pane(crate::app::Panel::Queue);
                if i < self.spov.sp_queue.len() {
                    self.spotify.queue_sel = i;
                    if double {
                        self.spotify_play(self.spov.sp_queue.clone(), i);
                    }
                }
            }
            MouseTarget::SpotifyItem(i) => {
                // a grid card / browse row: click selects, double-click opens (drill
                // into a container, or play a track) — same as ⏎ on the selection.
                self.focus = Focus::Main;
                if i < self.spotify.items.len() {
                    self.spotify.sel = i;
                    if double {
                        self.spotify_activate();
                    }
                }
            }
            // A carousel ‹/› arrow: select the card it reveals so the sticky viewport
            // scrolls one step in that direction. Scroll-only — never activates, even
            // on a double-click, so repeatedly clicking an arrow just slides the row.
            MouseTarget::GridScroll(i) => {
                self.focus = Focus::Main;
                if self.layout == Layout::Spotify {
                    if i < self.spotify.items.len() {
                        self.spotify.sel = i;
                    }
                } else if i < self.local.items.len() {
                    self.local.sel = i;
                }
            }
            MouseTarget::OpenSpotifyDashboard => {
                match webbrowser::open("https://developer.spotify.com/dashboard") {
                    Ok(_) => self.notify("Opened the Spotify developer dashboard".into()),
                    Err(_) => self.notify(
                        "Couldn't open a browser — visit developer.spotify.com/dashboard".into(),
                    ),
                }
            }
            MouseTarget::OpenSpotifyAuthUrl => {
                // open the FULL auth URL (the panel shows it clipped)
                if let crate::spotify::ConnState::Connecting { url: Some(u) } = &self.spotify.conn {
                    match webbrowser::open(u) {
                        Ok(_) => self.notify("Opening Spotify login in your browser…".into()),
                        Err(_) => self.notify("Couldn't open a browser — copy the URL".into()),
                    }
                }
            }
            MouseTarget::Transport(b) => match b {
                TransportButton::Shuffle => {
                    if self.showing_spotify() {
                        self.spotify_toggle_shuffle();
                    } else {
                        self.player.toggle_shuffle();
                        self.update_gapless_next();
                    }
                }
                TransportButton::Prev => {
                    if self.showing_spotify() {
                        self.spotify_track(-1)
                    } else {
                        self.advance_prev()
                    }
                }
                TransportButton::PlayPause => self.toggle_play(),
                TransportButton::Next => {
                    if self.showing_spotify() {
                        self.spotify_track(1)
                    } else {
                        self.advance_next()
                    }
                }
                TransportButton::Repeat => {
                    if self.showing_spotify() {
                        self.spotify_cycle_repeat();
                    } else {
                        self.player.cycle_repeat();
                        self.update_gapless_next();
                    }
                }
            },
            MouseTarget::Seek => {
                if rect.width > 0 {
                    let frac = x.saturating_sub(rect.x) as f32 / rect.width as f32;
                    if self.showing_spotify() {
                        self.spotify_seek_to_fraction(frac);
                    } else {
                        self.seek_to_fraction(frac);
                    }
                }
            }
            MouseTarget::RadioGoLive => self.radio_go_live(),
            MouseTarget::Volume => self.volume_from_x(rect, x),
            MouseTarget::Tree(i) => {
                // a sidebar section row: focus the sidebar, select + load that
                // section (like Enter / Move on the flat section list).
                self.focus = Focus::Sidebar;
                if let Some(&sec) = LocalSection::ALL.get(i) {
                    self.local.section = sec;
                    self.local_load_section();
                }
            }
            MouseTarget::SpotifySection(i) => {
                // the Spotify sidebar's section row — select + load it
                self.focus = Focus::Sidebar;
                if let Some(&s) = crate::spotify::api::Section::ALL.get(i) {
                    self.spotify.section = s;
                    self.spotify_load_section();
                }
            }
            MouseTarget::SettingRow(i) => {
                self.settings.sel = i;
                if double {
                    self.settings_activate();
                }
            }
            MouseTarget::RadioRow(i) => {
                if i < self.radio_view_list().len() {
                    self.radio.sel = i;
                    if double {
                        let st = self.radio_view_list()[i].clone();
                        self.play_station(st);
                    }
                }
            }
            MouseTarget::RadioPick(i) => {
                if let Some(p) = &mut self.radio.picker {
                    p.sel = i;
                }
                if double {
                    self.radio_apply_picker();
                }
            }
            MouseTarget::RadioChip(c) => match c {
                0 => {
                    self.radio.fav_view = false;
                    self.radio.editing = true;
                }
                1 => self.radio_open_picker(PickerKind::Country),
                2 => self.radio_open_picker(PickerKind::Genre),
                3 => self.radio_cycle_sort(),
                4 => self.radio_toggle_favorites(),
                _ => {}
            },
            MouseTarget::PaletteRow(pos) => {
                if let Some(p) = self.palette.as_mut() {
                    p.sel = pos;
                }
                if double {
                    self.palette_activate();
                }
            }
            MouseTarget::OverlayTab(i) => self.set_overlay_tab(i),
            MouseTarget::Scroll(b) => match b {
                ScrollBox::Tracklist => self.focus = Focus::Main,
                ScrollBox::Tree => self.focus = Focus::Sidebar,
                ScrollBox::Queue => self.set_focus(Focus::Pane(Panel::Queue)),
                ScrollBox::Artist => {
                    self.focus = Focus::Pane(Panel::Artist);
                    // double-click opens the artist's page (like ⏎ on the pane) —
                    // the Spotify view's pane opens the Spotify artist, others local
                    if double {
                        self.open_artist_page();
                    }
                }
                ScrollBox::Lyrics => {
                    self.focus = Focus::Pane(Panel::Lyrics);
                    // double-click cycles the lyrics format (plain → karaoke → …)
                    if double {
                        self.cycle_lyrics_format();
                    }
                }
                ScrollBox::Settings => {}
                ScrollBox::Radio => {}
                ScrollBox::SpotifyQueue => {
                    self.focus = crate::app::Focus::Pane(crate::app::Panel::Queue);
                }
                // clicking empty space in a Library column focuses + activates it
                ScrollBox::BrowseArtists => {
                    self.focus = Focus::Main;
                    self.browser.col = 0;
                }
                ScrollBox::BrowseAlbums => {
                    self.focus = Focus::Main;
                    self.browser.col = 1;
                }
                ScrollBox::BrowseTracks => {
                    self.focus = Focus::Main;
                    self.browser.col = 2;
                }
            },
        }
    }

    /// Touchpad/wheel scroll over the Main browse content. On a cover-art grid or
    /// release-carousel the two axes map to 2-D card navigation — two-finger
    /// left/right steps a card horizontally (within a row / carousel), up/down moves
    /// vertically (between rows / carousels) — matching the physical gesture and the
    /// keyboard's `h`/`l`/`j`/`k`. On a plain list, only up/down step the selection
    /// (horizontal scroll is a no-op — there's nothing to scroll sideways).
    fn scroll_main(&mut self, m: Motion) {
        if self.grid_nav_active() {
            self.grid_touch_scroll(m);
        } else if matches!(m, Motion::Up | Motion::Down) {
            self.move_selection(m);
        }
    }

    /// Touchpad two-finger scroll over a cover grid / carousel. A physical swipe
    /// fires *many* discrete scroll events, so stepping one card per event runs away
    /// (the "too fast / not smooth" feel). Instead accumulate signed event counts and
    /// commit one card only every `H_STEP` / `V_STEP` events.
    ///
    /// The horizontal move is **locked to the current row/carousel** (see
    /// `grid_scroll_current`), so a sideways gesture never leaps to another row when
    /// it reaches the end. Each committed horizontal step also clears the vertical
    /// accumulator, so the stray vertical jitter of a mostly-sideways swipe can't
    /// build up a row change — only a deliberate up/down gesture (few horizontals)
    /// lets `v` reach its threshold. That vertical gesture is what "frees" the row.
    fn grid_touch_scroll(&mut self, m: Motion) {
        // events per one card/row step — the user's "Touchpad scroll speed" setting
        // (Fast = 1 → every event; Slow = 3 → heavily throttled).
        let step = self.config.touchpad_speed.step_events();
        // "Lock grid scroll to row": when on, a sideways swipe clamps to the current
        // row/carousel; when off, horizontal wraps onto the next row (free 2-D).
        let locked = self.config.grid_scroll_lock;
        match m {
            Motion::Left | Motion::Right => {
                let dir = if matches!(m, Motion::Left) { -1 } else { 1 };
                // reversing mid-gesture restarts the count so it responds at once
                if self.grid_scroll.h != 0 && self.grid_scroll.h.signum() != dir {
                    self.grid_scroll.h = 0;
                }
                self.grid_scroll.h += dir;
                if self.grid_scroll.h.abs() >= step {
                    self.grid_scroll.h = 0;
                    self.grid_scroll.v = 0; // a real horizontal step cancels sideways jitter
                    if locked {
                        self.grid_scroll_current(dir, 0);
                    } else {
                        self.grid_move_current(dir, 0);
                    }
                }
            }
            Motion::Up | Motion::Down => {
                let dir = if matches!(m, Motion::Up) { -1 } else { 1 };
                if self.grid_scroll.v != 0 && self.grid_scroll.v.signum() != dir {
                    self.grid_scroll.v = 0;
                }
                self.grid_scroll.v += dir;
                if self.grid_scroll.v.abs() >= step {
                    self.grid_scroll.v = 0;
                    self.grid_scroll.h = 0;
                    self.grid_move_current(0, dir); // free move: change rows/carousels
                }
            }
            _ => {}
        }
    }

    /// Wheel scroll: move/adjust whichever box is under the pointer (never the
    /// focused box globally). Does nothing if the pointer is over empty chrome.
    pub fn handle_scroll(&mut self, x: u16, y: u16, m: Motion) {
        let Some((_, target)) = self.hit_at(x, y) else {
            return;
        };
        // Two-finger horizontal (left/right) scroll only drives the Main browse grid/
        // carousel; over any other box (a list, the volume/seek bar, the sidebar) a
        // sideways gesture does nothing rather than misfiring as a vertical step.
        if matches!(m, Motion::Left | Motion::Right)
            && !matches!(
                target,
                MouseTarget::Track(_)
                    | MouseTarget::SpotifyItem(_)
                    | MouseTarget::Scroll(ScrollBox::Tracklist)
            )
        {
            return;
        }
        self.mark_dirty(); // the scrolled list moved → repaint (see handle_click)
        match target {
            MouseTarget::Track(_) | MouseTarget::Scroll(ScrollBox::Tracklist) => {
                self.focus = Focus::Main;
                self.scroll_main(m);
            }
            MouseTarget::Tree(_) | MouseTarget::Scroll(ScrollBox::Tree) => {
                self.focus = Focus::Sidebar;
                self.move_selection(m);
            }
            // wheel over the Spotify sidebar steps + loads the section (like j/k)
            MouseTarget::SpotifySection(_) => {
                self.focus = Focus::Sidebar;
                self.move_selection(m);
            }
            MouseTarget::QueueRow(_) | MouseTarget::Scroll(ScrollBox::Queue) => {
                self.set_focus(Focus::Pane(Panel::Queue));
                self.move_selection(m);
            }
            MouseTarget::SettingRow(_) | MouseTarget::Scroll(ScrollBox::Settings) => {
                self.move_selection(m);
            }
            MouseTarget::RadioRow(_)
            | MouseTarget::RadioPick(_)
            | MouseTarget::Scroll(ScrollBox::Radio) => {
                // move_selection routes to the open picker or the station list
                self.move_selection(m);
            }
            MouseTarget::SpotifyQueueRow(_) | MouseTarget::Scroll(ScrollBox::SpotifyQueue) => {
                self.focus = crate::app::Focus::Pane(crate::app::Panel::Queue);
                self.move_selection(m); // routes to the queue cursor in the Spotify view
            }
            // wheel over the Spotify main list moves its selection (like Track)
            MouseTarget::SpotifyItem(_) => {
                self.focus = Focus::Main;
                self.scroll_main(m);
            }
            MouseTarget::Scroll(ScrollBox::Artist) => {
                self.focus = Focus::Pane(Panel::Artist);
                let cur = self.scroll.artist.get();
                let next = match m {
                    Motion::Up => cur.saturating_sub(2),
                    Motion::Down => (cur + 2).min(self.scroll.artist_max.get()),
                    _ => cur,
                };
                self.scroll.artist.set(next);
            }
            MouseTarget::Scroll(ScrollBox::Lyrics) => {
                self.focus = Focus::Pane(Panel::Lyrics);
                // wheeling takes the pane out of playback auto-follow (until the
                // track changes) so it can be read at the user's pace
                let cur = self.scroll.lyrics.get();
                let next = match m {
                    Motion::Up => cur.saturating_sub(2),
                    Motion::Down => (cur + 2).min(self.scroll.lyrics_max.get()),
                    _ => cur,
                };
                self.scroll.lyrics.set(next);
                self.scroll.lyrics_manual.set(true);
            }
            // wheel over the command palette browses its list (selection follows)
            MouseTarget::PaletteRow(_) => {
                let n = self.palette_matches().len();
                if let Some(p) = self.palette.as_mut() {
                    p.sel = match m {
                        Motion::Up => p.sel.saturating_sub(1),
                        Motion::Down => (p.sel + 1).min(n.saturating_sub(1)),
                        _ => p.sel,
                    };
                }
            }
            MouseTarget::Volume => {
                let d = if matches!(m, Motion::Up) { 4 } else { -4 };
                self.player.adjust_volume(d);
                self.engine
                    .send(AudioCommand::SetVolume(self.player.volume));
            }
            MouseTarget::Seek => {
                let d = if matches!(m, Motion::Up) { 5 } else { -5 };
                if self.showing_spotify() {
                    self.spotify_seek(d);
                } else {
                    self.seek_relative(d);
                }
            }
            // wheel over a Library column: focus it + move within it
            MouseTarget::BrowseArtist(_) | MouseTarget::Scroll(ScrollBox::BrowseArtists) => {
                self.focus = Focus::Main;
                self.browser.col = 0;
                self.move_selection(m);
            }
            MouseTarget::BrowseAlbum(_) | MouseTarget::Scroll(ScrollBox::BrowseAlbums) => {
                self.focus = Focus::Main;
                self.browser.col = 1;
                self.move_selection(m);
            }
            MouseTarget::BrowseTrack(_) | MouseTarget::Scroll(ScrollBox::BrowseTracks) => {
                self.focus = Focus::Main;
                self.browser.col = 2;
                self.move_selection(m);
            }
            _ => {}
        }
    }

    /// Set the volume from a click/drag y-position within a vertical bar `rect`
    /// (top = 100%, bottom = 0%).
    /// Set volume from a click/drag x-position within a horizontal bar `rect`
    /// (left = 0%, right = 100%).
    pub(crate) fn volume_from_x(&mut self, rect: Rect, x: u16) {
        let w = rect.width.max(1);
        let xc = x.clamp(rect.x, rect.x + w - 1);
        let frac = if w > 1 {
            (xc - rect.x) as f32 / (w - 1) as f32
        } else {
            1.0
        };
        let vol = (frac.clamp(0.0, 1.0) * 100.0).round() as u8;
        self.player.volume = vol;
        self.engine.send(AudioCommand::SetVolume(vol));
    }

    /// Left-drag: scrub the progress bar or the volume bar (whichever it's over).
    pub fn handle_drag(&mut self, x: u16, y: u16) {
        if self.modal_open() {
            return; // a modal owns input — don't scrub the chrome behind it
        }
        let found = self
            .hit
            .borrow()
            .iter()
            .rev()
            .find(|(r, t)| {
                matches!(t, MouseTarget::Seek | MouseTarget::Volume)
                    && x >= r.x
                    && x < r.x + r.width
                    && y >= r.y
                    && y < r.y + r.height
            })
            .map(|(r, t)| (*r, *t));
        if found.is_some() {
            self.mark_dirty(); // scrubbing moves the bar → repaint (see handle_click)
        }
        match found {
            Some((rect, MouseTarget::Seek)) if rect.width > 0 => {
                let frac = x.saturating_sub(rect.x) as f32 / rect.width as f32;
                if self.showing_spotify() {
                    self.spotify_seek_to_fraction(frac);
                } else {
                    self.seek_to_fraction(frac);
                }
            }
            Some((rect, MouseTarget::Volume)) => self.volume_from_x(rect, x),
            _ => {}
        }
    }
}
