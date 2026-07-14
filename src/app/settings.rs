//! Settings overlay + popups methods on `AppState` (extracted from app/mod.rs).

use super::*;

impl AppState {
    /// The master list of every settings row, in [`SETTINGS_GROUPS`] order — the
    /// single source of truth for both settings surfaces. The global overlay's
    /// active tab is this list filtered by group ([`Self::group_rows`] /
    /// [`Self::settings_group_items`]); the per-view `;` popup curates the same
    /// rows through the same helper ([`Self::popup_tab_defs`]). Define a setting
    /// here once and it reaches both.
    ///
    /// A few rows are view-contextual (music dirs and key bindings are
    /// variable-length; the panel rows follow the current `layout.panels()`; the
    /// grid `View` row and the Spotify log-out row appear only when meaningful).
    pub fn settings_items(&self) -> Vec<Setting> {
        use Setting::*;
        // General — app-wide behaviour
        let mut v = vec![
            IconSet,
            Mouse,
            NextHint,
            OsMediaControls,
            TouchpadScroll,
            GridScrollLock,
            OverlaySize,
            ReducedMotion,
            Fps,
            RadioRefresh,
            ArabicShaping,
        ];

        // Panes — the current view's movable panels (show / dock / size), then the
        // pane stacking orientation. Panels are per-view, so this mirrors the view
        // the overlay was opened over (same rows the `;` popup's Panes tab shows).
        for &p in self.layout.panels() {
            v.push(PanelShow(p));
            if self.layout.panel_movable(p) {
                v.push(PanelDock(p));
                v.push(p.size_setting());
            }
        }
        if self.layout.panels().len() > 1 {
            v.push(PanesLayout);
        }

        // Grid — the cover-art grid. `View` (list⇄grid) is per-section runtime
        // state, so it shows only on a grid-capable section; shape + size are global.
        if self.grid_capable_section() {
            v.push(GridList);
        }
        v.push(GridShape);
        v.push(GridSize);

        // Tracklist — the rows⇄columns layout switch leads the columns it governs
        v.push(TrackColumns);
        v.push(ColIndex);
        v.push(ColArtist);
        v.push(ColAlbumArtist);
        v.push(ColAlbum);
        v.push(ColYear);
        v.push(ColGenre);
        v.push(ColComposer);
        v.push(ColFormat);
        v.push(ColBitrate);
        v.push(ColRating);
        v.push(ColTime);
        v.push(ColPlays);
        v.push(ColComment);

        // Audio — local playback transitions
        v.push(Gapless);
        v.push(Crossfade);
        v.push(SilenceSkip);
        v.push(ReplayGain);
        v.push(RadioDvr);

        // Visualizer — the playback-bar spectrum + big-view peak caps
        v.push(PlayerViz);
        v.push(PlayerVizMode);
        v.push(PeakCaps);

        // Lyrics
        v.push(LyricsAlign);
        v.push(LyricsGap);
        v.push(LyricsGradient);
        v.push(LyricsColor);
        v.push(LyricsKaraoke);
        v.push(LyricsDual);
        v.push(LyricsTranslate);
        v.push(LyricsTeleprompter);

        // Theme — follow the OS light/dark setting (revealing the two per-appearance
        // pickers) or pick one palette; then album-art tinting. Only the rows relevant
        // to the current mode are shown, so it's always clear which theme is active.
        v.push(ThemeFollowSystem);
        if self.config.theme_follows_system {
            v.push(LightTheme);
            v.push(DarkTheme);
        } else {
            v.push(Theme);
        }
        v.push(AlbumArt);
        v.push(DynamicAccent);

        // Spotify — connection + streaming, global so the account can be managed
        // from any view. The log-out/reset row appears only with a live session or
        // a cached token (nothing to reset otherwise).
        v.push(SpotifyClientId);
        v.push(SpotifyBitrate);
        v.push(SpotifyShowAccount);
        v.push(SpotifyReauth);
        if matches!(
            self.spotify.conn,
            crate::spotify::ConnState::Connected { .. }
        ) || self.spotify.tokens.is_some()
        {
            v.push(SpotifyLogout);
        }

        // Library — the on-disk music folders (variable) + rescan
        for i in 0..self.config.music_dirs.len() {
            v.push(MusicDir(i));
        }
        v.push(AddDir);
        v.push(Rescan);

        // Keys — configurable key bindings
        for i in 0..crate::keymap::configurable_actions().len() {
            v.push(Keybind(i));
        }

        v
    }

    /// The global Settings overlay's visible group tabs: every [`SETTINGS_GROUPS`]
    /// entry the current view actually has rows for. Only "Panes" is ever dropped
    /// (Radio and Concert host no movable panels), so the overlay never shows a
    /// blank tab. `settings.group` indexes into this list, not the raw const.
    pub fn settings_tabs(&self) -> Vec<&'static str> {
        let present: std::collections::HashSet<&'static str> =
            self.settings_items().iter().map(|s| s.group()).collect();
        SETTINGS_GROUPS
            .iter()
            .copied()
            .filter(|g| present.contains(g))
            .collect()
    }

    /// The name of the currently-active tab: the `;` popup's tab if open, else the
    /// global overlay's group tab.
    pub fn settings_active_group(&self) -> Option<&'static str> {
        if self.settings.popup.is_some() {
            let names = self.popup_tab_names();
            let i = self
                .settings
                .popup
                .unwrap_or(0)
                .min(names.len().saturating_sub(1));
            return names.get(i).copied();
        }
        let tabs = self.settings_tabs();
        self.settings.group.and_then(|g| tabs.get(g).copied())
    }

    /// Settings rows currently shown: the active `;` popup tab's rows, or — in the
    /// global overlay — the active group tab's rows.
    pub fn settings_group_items(&self) -> Vec<Setting> {
        if self.settings.popup.is_some() {
            let tabs = self.popup_tab_defs();
            let i = self
                .settings
                .popup
                .unwrap_or(0)
                .min(tabs.len().saturating_sub(1));
            return tabs
                .into_iter()
                .nth(i)
                .map(|(_, rows)| rows)
                .unwrap_or_default();
        }
        match self.settings_active_group() {
            Some(g) => self
                .settings_items()
                .into_iter()
                .filter(|s| s.group() == g)
                .collect(),
            None => Vec::new(),
        }
    }

    /// All settings in group `g`, in `settings_items` order.
    fn group_rows(&self, g: &str) -> Vec<Setting> {
        self.settings_items()
            .into_iter()
            .filter(|s| s.group() == g)
            .collect()
    }

    /// The `;` popup's **Tracklist** tab: the "Tracklist" group rows (the rows⇄columns
    /// layout switch leads the per-column toggles it governs) curated to what the
    /// current view's source can populate — every column for the local library, but
    /// only the metadata subset Spotify carries (#, Artist, Album, Year, Time).
    /// Offering a column the source can't fill would mislead. Mirrors the render-side
    /// curation in `components::spotify_tracks`.
    fn columns_tab(&self) -> Vec<Setting> {
        use Setting::*;
        let rows = self.group_rows("Tracklist"); // TrackColumns, then every column
        if self.layout == Layout::Spotify {
            rows.into_iter()
                .filter(|s| {
                    matches!(
                        s,
                        TrackColumns | ColIndex | ColArtist | ColAlbum | ColYear | ColTime
                    )
                })
                .collect()
        } else {
            rows
        }
    }

    /// Whether the current browse section can show the cover grid — so the grid⇄list
    /// "View" row is meaningful: Spotify Albums/Artists/Playlists, local
    /// Albums/Artists. Mirrors `spotify_grid_active`/`local_grid_active`'s section gate.
    fn grid_capable_section(&self) -> bool {
        use crate::spotify::api::Section as Sp;
        if self.layout == Layout::Spotify {
            matches!(
                self.spotify.section,
                Sp::Albums | Sp::Artists | Sp::Playlists
            )
        } else {
            matches!(
                self.local.section,
                crate::app::LocalSection::Albums | crate::app::LocalSection::Artists
            )
        }
    }

    /// The `;` popup's tab names (for the tab bar).
    pub fn popup_tab_names(&self) -> Vec<&'static str> {
        self.popup_tab_defs().into_iter().map(|(n, _)| n).collect()
    }

    /// The largest row count among the current view's `;` popup tabs. The popup is
    /// sized to this (rather than the active tab) so switching tabs never resizes
    /// or re-centers the card — the frame stays put and the content scrolls inside.
    pub fn popup_max_rows(&self) -> usize {
        self.popup_tab_defs()
            .iter()
            .map(|(_, rows)| rows.len())
            .max()
            .unwrap_or(0)
    }

    /// Every setting the current view's `;` popup exposes, across all its tabs.
    #[cfg(test)]
    pub fn popup_all_settings(&self) -> Vec<Setting> {
        self.popup_tab_defs()
            .into_iter()
            .flat_map(|(_, v)| v)
            .collect()
    }

    /// The per-view `;` popup's curated tabs — a subset of [`SETTINGS_GROUPS`]
    /// relevant to the current view, drawing the same rows as the global overlay
    /// through [`Self::group_rows`], so every tab name here is a real group name
    /// and a setting lives in exactly one place. Layout: Panes (the movable side
    /// panels + orientation) → the view's content tabs (Dashboard/Spotify split
    /// Grid + Tracklist; Lyrics view → Lyrics; Concert → Theme; the rest → General)
    /// → Audio (local playback transitions) → Visualizer → Library → Spotify. The
    /// set-once tabs (Library, Spotify connection) sit last.
    fn popup_tab_defs(&self) -> Vec<(&'static str, Vec<Setting>)> {
        use Setting::*;
        let mut tabs: Vec<(&'static str, Vec<Setting>)> = Vec::new();

        // Panes — show / dock / size per movable panel + stacking orientation.
        // Skipped where the view hosts no panels (Radio / Concert).
        let panes = self.group_rows("Panes");
        if !panes.is_empty() {
            tabs.push(("Panes", panes));
        }

        // Content — what the view's main area shows, named per view
        match self.layout {
            // grid display vs the track table are distinct concerns → own tabs.
            // (Spotify's Tracklist is curated to the columns it can populate.)
            Layout::Dashboard | Layout::Spotify => {
                tabs.push(("Grid", self.group_rows("Grid")));
                tabs.push(("Tracklist", self.columns_tab()));
            }
            Layout::LyricsFocus => tabs.push(("Lyrics", self.group_rows("Lyrics"))),
            Layout::Concert => tabs.push(("Theme", self.group_rows("Theme"))),
            Layout::FullPlayer => {} // its content is the visualizer → Visualizer tab
            _ => tabs.push(("General", self.group_rows("General"))),
        }

        // Audio — local playback transitions (gapless / crossfade / silence-skip /
        // ReplayGain). Local-library views only; N/A to Spotify (librespot decodes
        // its own stream) and to live radio (no track seams).
        if matches!(
            self.layout,
            Layout::FullPlayer
                | Layout::LyricsFocus
                | Layout::Dashboard
                | Layout::LibraryFocus
                | Layout::Concert
        ) {
            tabs.push(("Audio", self.group_rows("Audio")));
        }

        // Visualizer — the now-bar (mini) spectrum, in every view. Peak caps only
        // where the big Now-Playing visualizer shows them, so the tab is built here
        // rather than from the full "Visualizer" group (which the global overlay uses).
        let mut viz = vec![PlayerViz, PlayerVizMode];
        if self.layout == Layout::FullPlayer {
            viz.push(PeakCaps);
        }
        tabs.push(("Visualizer", viz));

        // Library — the on-disk music folders + rescan, for the local-library views.
        // Set-once, so it sits late; first-run onboarding sends new users here.
        if matches!(self.layout, Layout::Dashboard | Layout::LibraryFocus) {
            tabs.push(("Library", self.group_rows("Library")));
        }

        // Spotify — the connection setup (client id / quality / re-auth / logout),
        // on the Spotify view. Rarely changed, so it sits last.
        if self.layout == Layout::Spotify {
            tabs.push(("Spotify", self.group_rows("Spotify")));
        }

        tabs
    }

    pub(crate) fn settings_item(&self) -> Option<Setting> {
        let items = self.settings_group_items();
        items
            .get(self.settings.sel.min(items.len().saturating_sub(1)))
            .copied()
    }

    /// Open the full Settings overlay at its first tab (General). The groups
    /// ([`Self::settings_tabs`]) are horizontal tabs (Tab / ⇧Tab switch); `group`
    /// is the active-tab index into that list.
    pub fn open_settings(&mut self) {
        self.settings.overlay = true;
        self.settings.group = Some(0);
        self.settings.sel = 0;
        self.settings.off.set(0);
    }

    /// Open the Settings overlay directly at the named group tab (the palette's `→`
    /// reveal, and tests / deep links). Falls back to the first tab if unknown.
    pub fn open_settings_group(&mut self, group: &str) {
        self.settings.overlay = true;
        self.settings.group = Some(
            self.settings_tabs()
                .iter()
                .position(|g| *g == group)
                .unwrap_or(0),
        );
        self.settings.sel = 0;
        self.settings.off.set(0);
    }

    /// Switch the active tabbed overlay to tab `i` (a tab click). Routes to the
    /// Info overlay's tabs if it's open, else the global Settings group tabs.
    pub fn set_overlay_tab(&mut self, i: usize) {
        if let Some(info) = &mut self.info {
            if let Some(&tab) = crate::app::InfoTab::ALL.get(i) {
                info.tab = tab;
            }
        } else if self.tags_open() {
            self.tags_tab_to(i as u8);
        } else if self.settings.overlay {
            let n = self.settings_tabs().len();
            self.settings.group = Some(i.min(n.saturating_sub(1)));
            self.settings.sel = 0;
            self.settings.off.set(0);
        } else if self.settings.popup.is_some() {
            let n = self.popup_tab_names().len();
            self.settings.popup = Some(i.min(n.saturating_sub(1)));
            self.settings.sel = 0;
            self.settings.off.set(0);
        }
    }

    /// Del / ^d on the selected settings row — currently removes a music dir.
    pub(crate) fn settings_remove(&mut self) {
        if self.settings.popup.is_none() && !self.settings.overlay {
            return;
        }
        if let Some(Setting::MusicDir(i)) = self.settings_item()
            && i < self.config.music_dirs.len()
        {
            let removed = self.config.music_dirs.remove(i);
            self.config.save();
            self.request_rescan();
            self.notify(format!("Removed {}", removed.display()));
            self.settings.sel = self
                .settings
                .sel
                .min(self.settings_group_items().len().saturating_sub(1));
        }
    }

    /// Enter / space on the selected settings row. Both the global overlay (a
    /// group tab) and the `;` popup always have an active group, so Enter always
    /// acts on the selected row.
    pub(crate) fn settings_activate(&mut self) {
        if let Some(item) = self.settings_item() {
            self.activate_setting(item);
        }
    }

    /// Enter/Space on a specific settings row: cycle / toggle / open its prompt. The
    /// explicit-`Setting` core of [`Self::settings_activate`], so the command palette
    /// can activate a value-less setting (Rescan, Add dir, a rebind, Spotify re-auth…)
    /// without it being the overlay's current selection. The `self.settings_adjust(1)`
    /// arms below still read the overlay selection, but those are only reached via
    /// `settings_activate` (where the selection *is* `item`); the palette drives
    /// value-bearing settings through `apply_setting_value` instead.
    pub(crate) fn activate_setting(&mut self, item: Setting) {
        match Some(item) {
            // a music dir is removed with Del / ^d (not Enter — too easy to hit)
            Some(Setting::MusicDir(_)) => {}
            Some(Setting::AddDir) => {
                self.input.naming = Some(NameTarget::AddMusicDir);
                self.input.buffer.clear();
            }
            Some(Setting::Rescan) => {
                self.request_rescan();
                self.notify("Rescanning library…".into());
            }
            Some(Setting::Theme) => self.settings_adjust(1),
            Some(Setting::ThemeFollowSystem) => {
                self.set_theme_follows_system(!self.config.theme_follows_system)
            }
            Some(Setting::LightTheme) | Some(Setting::DarkTheme) => self.settings_adjust(1),
            Some(Setting::AlbumArt) => {
                self.config.album_art = !self.config.album_art;
                self.config.save();
                self.reload_cover();
            }
            Some(Setting::DynamicAccent) => {
                self.config.dynamic_accent = !self.config.dynamic_accent;
                self.config.save();
                self.reload_cover(); // re-tint from (or restore) the current cover
            }
            Some(Setting::IconSet) => self.cycle_icon_set(1),
            Some(Setting::PlayerViz) => self.toggle_setting(|c| &mut c.player_viz),
            Some(Setting::PanesLayout) => self.toggle_setting(|c| &mut c.panes_horizontal),
            Some(Setting::GridList) => self.toggle_grid_current(),
            Some(Setting::GridShape) => self.toggle_grid_shape(),
            Some(Setting::GridSize) => self.cycle_grid_size(1),
            Some(Setting::TrackColumns) => self.toggle_setting(|c| &mut c.track_columns),
            Some(Setting::PlayerVizMode) => self.cycle_player_viz_mode(1),
            Some(Setting::Mouse) => self.toggle_setting(|c| &mut c.mouse),
            Some(Setting::NextHint) => self.toggle_setting(|c| &mut c.next_hint),
            Some(Setting::OsMediaControls) => self.toggle_setting(|c| &mut c.os_media_controls),
            Some(Setting::TouchpadScroll) => self.cycle_touchpad_speed(1),
            Some(Setting::GridScrollLock) => self.toggle_setting(|c| &mut c.grid_scroll_lock),
            Some(Setting::OverlaySize) => self.cycle_overlay_size(1),
            Some(Setting::ReducedMotion) => self.toggle_setting(|c| &mut c.reduced_motion),
            Some(Setting::PeakCaps) => self.toggle_setting(|c| &mut c.peak_caps),
            Some(Setting::ArabicShaping) => self.toggle_setting(|c| &mut c.arabic_shaping),
            Some(Setting::RadioDvr) => self.toggle_setting(|c| &mut c.radio_dvr),
            Some(Setting::Gapless) => {
                self.toggle_setting(|c| &mut c.gapless);
                self.update_gapless_next();
            }
            Some(Setting::SilenceSkip) => {
                self.toggle_setting(|c| &mut c.silence_skip);
                self.apply_silence_skip();
            }
            Some(Setting::ReplayGain) => self.cycle_replaygain(),
            Some(Setting::ColIndex) => self.toggle_setting(|c| &mut c.columns.index),
            Some(Setting::ColArtist) => self.toggle_setting(|c| &mut c.columns.artist),
            Some(Setting::ColAlbumArtist) => self.toggle_setting(|c| &mut c.columns.album_artist),
            Some(Setting::ColAlbum) => self.toggle_setting(|c| &mut c.columns.album),
            Some(Setting::ColYear) => self.toggle_setting(|c| &mut c.columns.year),
            Some(Setting::ColGenre) => self.toggle_setting(|c| &mut c.columns.genre),
            Some(Setting::ColComposer) => self.toggle_setting(|c| &mut c.columns.composer),
            Some(Setting::ColFormat) => self.toggle_setting(|c| &mut c.columns.format),
            Some(Setting::ColBitrate) => self.toggle_setting(|c| &mut c.columns.bitrate),
            Some(Setting::ColRating) => self.toggle_setting(|c| &mut c.columns.rating),
            Some(Setting::ColTime) => self.toggle_setting(|c| &mut c.columns.time),
            Some(Setting::ColPlays) => self.toggle_setting(|c| &mut c.columns.plays),
            Some(Setting::ColComment) => self.toggle_setting(|c| &mut c.columns.comment),
            Some(Setting::LyricsGradient) => self.toggle_setting(|c| &mut c.lyrics_gradient),
            Some(Setting::LyricsColor) => self.settings_adjust(1),
            Some(Setting::LyricsTranslate) => self.settings_adjust(1),
            Some(Setting::LyricsKaraoke) => self.toggle_setting(|c| &mut c.lyrics_karaoke),
            Some(Setting::LyricsDual) => self.toggle_setting(|c| &mut c.lyrics_dual),
            Some(Setting::LyricsTeleprompter) => {
                self.toggle_setting(|c| &mut c.lyrics_teleprompter)
            }
            Some(Setting::PanelShow(p)) => {
                self.toggle_panel(p);
                if p == Panel::Artist && self.panel(Panel::Artist).shown {
                    self.request_artist_info();
                }
            }
            Some(Setting::PanelDock(p)) => self.move_panel(p),
            Some(Setting::PanelSize(p)) => self.resize_panel(p, 1),
            Some(Setting::SpotifyLogout) => {
                if self.settings.confirm_logout {
                    // confirmed (⏎/y): log out and close the popup
                    self.settings.confirm_logout = false;
                    self.spotify_logout();
                    self.settings.popup = None;
                } else {
                    // first press: arm the status-bar confirmation
                    self.settings.confirm_logout = true;
                }
            }
            Some(Setting::SpotifyClientId) => {
                // open the paste prompt prefilled with the current id (clear it +
                // confirm to revert to the shared id); the prompt is modal, so the
                // popup steps aside.
                self.settings.popup = None;
                self.input.naming = Some(NameTarget::SpotifyClientId);
                self.input.buffer = self.config.spotify_client_id.clone();
            }
            Some(Setting::SpotifyReauth) => {
                self.settings.popup = None;
                self.spotify_reauthenticate();
            }
            Some(Setting::SpotifyBitrate) => self.cycle_spotify_bitrate(1),
            Some(Setting::SpotifyShowAccount) => {
                self.toggle_setting(|c| &mut c.spotify_show_account)
            }
            Some(Setting::Keybind(i)) => {
                // start capturing the next key press for this action
                if let Some(action) = crate::keymap::configurable_actions().get(i) {
                    self.settings.rebinding = Some(action.to_string());
                }
            }
            Some(Setting::LyricsAlign) => self.settings_adjust(1),
            // Enter on Crossfade cycles handy presets (off → 2 → 4 → 8 s)
            Some(Setting::Crossfade) => {
                let next = match self.config.crossfade_ms {
                    0 => 2000,
                    ms if ms < 4000 => 4000,
                    ms if ms < 8000 => 8000,
                    _ => 0,
                };
                self.set_crossfade(next);
            }
            // any other numeric setting: Enter nudges it up (h/l also adjust)
            _ => self.settings_adjust(1),
        }
    }

    /// h / l (or ←/→) on the selected settings row: cycle / adjust the value.
    pub(crate) fn settings_adjust(&mut self, dir: i32) {
        match self.settings_item() {
            Some(Setting::Theme) => {
                let order = self.all_themes();
                let cur = self.theme.name.as_str();
                let idx = order.iter().position(|n| n == cur).unwrap_or(0) as i32;
                let n = order.len() as i32;
                let next = order[(((idx + dir) % n + n) % n) as usize].clone();
                self.set_theme(&next);
                self.config.save();
            }
            Some(Setting::ThemeFollowSystem) => self.set_theme_follows_system(dir > 0),
            Some(Setting::LightTheme) => self.cycle_slot_theme(false, dir),
            Some(Setting::DarkTheme) => self.cycle_slot_theme(true, dir),
            Some(Setting::Fps) => {
                self.config.fps = (self.config.fps as i32 + dir * 5).clamp(10, 144) as u8;
                self.config.save();
            }
            Some(Setting::RadioRefresh) => {
                // cycle handy presets: manual → daily → 3d → weekly → 2wk → monthly
                const STEPS: [u32; 6] = [0, 1, 3, 7, 14, 30];
                let cur = self.config.radio_refresh_days;
                let idx = STEPS.iter().position(|&d| d == cur).unwrap_or(3) as i32;
                let n = STEPS.len() as i32;
                self.config.radio_refresh_days = STEPS[(((idx + dir) % n + n) % n) as usize];
                self.config.save();
            }
            Some(Setting::PanelSize(p)) => self.resize_panel(p, dir),
            Some(Setting::AlbumArt) => {
                self.config.album_art = dir > 0;
                self.config.save();
                self.reload_cover();
            }
            Some(Setting::DynamicAccent) => {
                self.config.dynamic_accent = dir > 0;
                self.config.save();
                self.reload_cover();
            }
            Some(Setting::IconSet) => self.cycle_icon_set(dir.signum()),
            Some(Setting::PlayerViz) => self.set_setting(|c| &mut c.player_viz, dir > 0),
            Some(Setting::PanesLayout) => self.set_setting(|c| &mut c.panes_horizontal, dir > 0),
            Some(Setting::GridList) => self.toggle_grid_current(),
            Some(Setting::GridShape) => self.toggle_grid_shape(),
            Some(Setting::GridSize) => self.cycle_grid_size(dir.signum()),
            Some(Setting::TrackColumns) => self.set_setting(|c| &mut c.track_columns, dir > 0),
            Some(Setting::PlayerVizMode) => self.cycle_player_viz_mode(dir.signum()),
            Some(Setting::Mouse) => self.set_setting(|c| &mut c.mouse, dir > 0),
            Some(Setting::NextHint) => self.set_setting(|c| &mut c.next_hint, dir > 0),
            Some(Setting::OsMediaControls) => {
                self.set_setting(|c| &mut c.os_media_controls, dir > 0)
            }
            Some(Setting::TouchpadScroll) => self.cycle_touchpad_speed(dir.signum()),
            Some(Setting::GridScrollLock) => self.set_setting(|c| &mut c.grid_scroll_lock, dir > 0),
            Some(Setting::OverlaySize) => self.cycle_overlay_size(dir.signum()),
            Some(Setting::ReducedMotion) => self.set_setting(|c| &mut c.reduced_motion, dir > 0),
            Some(Setting::PeakCaps) => self.set_setting(|c| &mut c.peak_caps, dir > 0),
            Some(Setting::ArabicShaping) => self.set_setting(|c| &mut c.arabic_shaping, dir > 0),
            Some(Setting::RadioDvr) => self.set_setting(|c| &mut c.radio_dvr, dir > 0),
            Some(Setting::Gapless) => {
                self.set_setting(|c| &mut c.gapless, dir > 0);
                self.update_gapless_next();
            }
            Some(Setting::SilenceSkip) => {
                self.set_setting(|c| &mut c.silence_skip, dir > 0);
                self.apply_silence_skip();
            }
            Some(Setting::ColIndex) => self.set_setting(|c| &mut c.columns.index, dir > 0),
            Some(Setting::ColArtist) => self.set_setting(|c| &mut c.columns.artist, dir > 0),
            Some(Setting::ColAlbumArtist) => {
                self.set_setting(|c| &mut c.columns.album_artist, dir > 0)
            }
            Some(Setting::ColAlbum) => self.set_setting(|c| &mut c.columns.album, dir > 0),
            Some(Setting::ColYear) => self.set_setting(|c| &mut c.columns.year, dir > 0),
            Some(Setting::ColGenre) => self.set_setting(|c| &mut c.columns.genre, dir > 0),
            Some(Setting::ColComposer) => self.set_setting(|c| &mut c.columns.composer, dir > 0),
            Some(Setting::ColFormat) => self.set_setting(|c| &mut c.columns.format, dir > 0),
            Some(Setting::ColBitrate) => self.set_setting(|c| &mut c.columns.bitrate, dir > 0),
            Some(Setting::ColRating) => self.set_setting(|c| &mut c.columns.rating, dir > 0),
            Some(Setting::ColTime) => self.set_setting(|c| &mut c.columns.time, dir > 0),
            Some(Setting::ColPlays) => self.set_setting(|c| &mut c.columns.plays, dir > 0),
            Some(Setting::ColComment) => self.set_setting(|c| &mut c.columns.comment, dir > 0),
            Some(Setting::LyricsAlign) => {
                self.config.lyrics_align =
                    (self.config.lyrics_align as i32 + dir).rem_euclid(3) as u8;
                self.config.save();
            }
            Some(Setting::LyricsGap) => {
                // wrap 0..=3 so Enter / l cycles back to 0 instead of pinning at 3
                self.config.lyrics_gap = (self.config.lyrics_gap as i32 + dir).rem_euclid(4) as u8;
                self.config.save();
            }
            Some(Setting::LyricsGradient) => self.set_setting(|c| &mut c.lyrics_gradient, dir > 0),
            Some(Setting::LyricsColor) => {
                self.config.lyrics_color =
                    (self.config.lyrics_color as i32 + dir).rem_euclid(5) as u8;
                self.config.save();
            }
            Some(Setting::LyricsTranslate) => self.cycle_lyrics_translate(dir),
            Some(Setting::LyricsKaraoke) => self.set_setting(|c| &mut c.lyrics_karaoke, dir > 0),
            Some(Setting::LyricsDual) => self.set_setting(|c| &mut c.lyrics_dual, dir > 0),
            Some(Setting::LyricsTeleprompter) => {
                self.set_setting(|c| &mut c.lyrics_teleprompter, dir > 0)
            }
            Some(Setting::PanelShow(p)) => self.toggle_panel(p),
            Some(Setting::PanelDock(p)) => self.move_panel(p),
            Some(Setting::Crossfade) => {
                let ms = (self.config.crossfade_ms as i32 + dir * 250).clamp(0, 12000) as u32;
                self.set_crossfade(ms);
            }
            Some(Setting::ReplayGain) => {
                self.config.replaygain = (self.config.replaygain as i32 + dir).rem_euclid(3) as u8;
                self.config.save();
                self.refresh_replaygain();
            }
            Some(Setting::SpotifyBitrate) => self.cycle_spotify_bitrate(dir),
            Some(Setting::SpotifyShowAccount) => {
                self.set_setting(|c| &mut c.spotify_show_account, dir > 0)
            }
            _ => {}
        }
    }

    /// Cycle the Spotify streaming quality (96 ↔ 160 ↔ 320 kbps), persist it, and
    /// note that it takes effect on the next session spawn — librespot's bitrate is
    /// fixed for the life of a session, so a live one keeps its current quality.
    pub(crate) fn cycle_spotify_bitrate(&mut self, dir: i32) {
        const STEPS: [u16; 3] = [96, 160, 320];
        let cur = STEPS
            .iter()
            .position(|&b| b == self.config.spotify_bitrate)
            .unwrap_or(1) as i32;
        let n = STEPS.len() as i32;
        let next = STEPS[(((cur + dir) % n + n) % n) as usize];
        self.config.spotify_bitrate = next;
        self.config.save();
        // a running session already streams at the old bitrate; flag the reconnect
        let live = self.spov.session_cmd.is_some();
        let note = if live { "  (reconnect to apply)" } else { "" };
        self.notify(format!("Spotify quality: {next} kbps{note}"));
    }

    pub(crate) fn toggle_setting(&mut self, f: impl Fn(&mut Config) -> &mut bool) {
        let p = f(&mut self.config);
        *p = !*p;
        self.config.save();
    }

    pub(crate) fn set_setting(&mut self, f: impl Fn(&mut Config) -> &mut bool, v: bool) {
        *f(&mut self.config) = v;
        self.config.save();
    }

    /// Cycle the touchpad grid-scroll speed (slow ↔ normal ↔ fast) and persist it.
    pub(crate) fn cycle_touchpad_speed(&mut self, dir: i32) {
        self.config.touchpad_speed = self.config.touchpad_speed.step(dir);
        self.config.save();
    }

    /// Step the big-overlay size (`f` / the "Overlay size" row) and persist it.
    /// Wraps `0..OVERLAY_SIZE_COUNT` so a single key cycles Small → … → X-Large →
    /// Small; drives [`crate::ui::components::overlay_dims`].
    pub(crate) fn cycle_overlay_size(&mut self, dir: i32) {
        let n = crate::config::OVERLAY_SIZE_COUNT as i32;
        let cur = self.config.overlay_size as i32;
        self.config.overlay_size = (((cur + dir) % n + n) % n) as u8;
        self.config.save();
    }

    /// Cycle the lyric display format: plain → karaoke → teleprompter → plain.
    pub(crate) fn cycle_lyrics_format(&mut self) {
        let (nk, nt, name) = match (self.config.lyrics_karaoke, self.config.lyrics_teleprompter) {
            (false, false) => (true, false, "karaoke"),
            (true, false) => (false, true, "teleprompter"),
            _ => (false, false, "plain"),
        };
        self.config.lyrics_karaoke = nk;
        self.config.lyrics_teleprompter = nt;
        self.config.save();
        self.notify(format!("Lyrics: {name}"));
    }

    /// Cycle the lyric translation target through `translate::LANGS` (off →
    /// English → …) in the direction `dir`, persist it, and re-derive the current
    /// track's translation for the new language (reloading the lyrics clears any
    /// machine translation from the previous language first).
    pub(crate) fn cycle_lyrics_translate(&mut self, dir: i32) {
        let langs = crate::translate::LANGS;
        let cur = langs
            .iter()
            .position(|(c, _)| *c == self.config.lyrics_translate_to)
            .unwrap_or(0);
        let next = (cur as i32 + dir).rem_euclid(langs.len() as i32) as usize;
        self.config.lyrics_translate_to = langs[next].0.to_string();
        self.config.save();
        let (_, name) = langs[next];
        self.notify(if name == "off" {
            "Lyrics translation off".into()
        } else {
            format!("Translate lyrics → {name}")
        });
        // re-parse the lyrics (drops the old language's machine translation), then
        // fetch/apply the new language via `maybe_request_translation`.
        self.reload_lyrics();
    }

    /// Nudge the manual lyric-sync offset by `delta` ms (bounded to ±5s) and
    /// persist it. Positive delays the highlight (for lyrics running ahead of the
    /// vocal), negative advances it. See `playback_elapsed`.
    pub(crate) fn nudge_lyrics_offset(&mut self, delta: i32) {
        let ms = (self.config.lyrics_offset_ms + delta).clamp(-5000, 5000);
        self.config.lyrics_offset_ms = ms;
        self.config.save();
        let sign = if ms > 0 { "+" } else { "" };
        self.notify(format!("Lyrics sync {sign}{ms} ms"));
    }

    /// Cycle the playback-bar visualizer's mode (independent of the per-view big
    /// visualizer). Persisted, since it's a config setting like its on/off.
    pub(crate) fn cycle_player_viz_mode(&mut self, dir: i32) {
        let n = crate::ui::components::VIZ_MODES.len() as i32;
        let m = self.config.player_viz_mode as i32;
        self.config.player_viz_mode = (((m + dir) % n + n) % n) as u8;
        self.config.save();
        let name =
            crate::ui::components::VIZ_MODES[self.config.player_viz_mode as usize % n as usize];
        self.notify(format!("Playback visualizer: {name}"));
    }

    /// Switch the transport icon preset and re-resolve glyphs (+ overrides).
    pub(crate) fn set_icon_set(&mut self, name: &str) {
        self.config.icon_set = name.to_string();
        self.config.save();
        self.icons = crate::icons::Icons::resolve(&self.config.icon_set, &self.config.icons);
    }

    /// Cycle through the built-in icon presets.
    pub(crate) fn cycle_icon_set(&mut self, dir: i32) {
        let p = crate::icons::Icons::PRESETS;
        let cur = p
            .iter()
            .position(|&n| n == self.config.icon_set)
            .unwrap_or(0) as i32;
        let n = p.len() as i32;
        self.set_icon_set(p[(((cur + dir) % n + n) % n) as usize]);
    }

    /// `auto` (match the terminal), the built-ins, then any custom `themes/` files.
    pub fn all_themes(&self) -> Vec<String> {
        let mut v: Vec<String> = std::iter::once("auto".to_string())
            .chain(
                crate::ui::theme::BUILTIN_THEMES
                    .iter()
                    .map(|s| s.to_string()),
            )
            .collect();
        v.extend(self.config.custom_themes());
        v
    }

    /// Switch to theme `name`: resolve it, persist the choice, and re-derive the
    /// dynamic accent. The artwork cache is deliberately kept: circle covers have
    /// transparent corners, so the new panel just shows through the same images —
    /// no re-decode/re-fetch, the covers recolor on the next render.
    /// Shared by every theme switch (cycle, settings, palette).
    /// Resolve a theme by name, honoring the `auto` palette detected from the
    /// terminal at startup. Re-resolving `auto` any other way (via `Theme::resolve`)
    /// loses the terminal colours and returns the default, so EVERY (re)build of the
    /// current theme — set, cycle, and `reload_cover` — must go through here.
    pub(crate) fn resolve_theme(&self, name: &str) -> Theme {
        if name == "auto" {
            self.auto_theme
                .clone()
                .unwrap_or_else(|| Theme::resolve("auto", &self.config.themes_dir()))
        } else {
            Theme::resolve(name, &self.config.themes_dir())
        }
    }

    /// Activate theme `name` LIVE — resolve it, swap `self.theme`, and re-derive the
    /// dynamic accent — WITHOUT persisting it as the single `config.theme`. This is
    /// the shared engine behind both [`Self::set_theme`] (which then persists) and the
    /// follow-system switch (which must not clobber the user's single-theme choice).
    pub(crate) fn apply_theme(&mut self, name: &str) {
        self.theme = self.resolve_theme(name);
        self.apply_accent();
    }

    pub(crate) fn set_theme(&mut self, name: &str) {
        self.apply_theme(name);
        self.config.theme = self.theme.name.clone();
        // `auto` but the terminal never reported its colours → we're on the fallback;
        // surface it in the status bar instead of silently showing the default.
        if name == "auto" && self.auto_theme.is_none() {
            self.notify_error(
                "Auto theme: couldn't read the terminal's colours — using the default. \
                 The terminal may not support OSC color queries."
                    .into(),
            );
        }
    }

    /// The theme name to show for a given OS appearance. `None` (undetectable, e.g.
    /// non-macOS) falls back to the dark theme.
    fn system_theme_name(&self, appr: Option<crate::appearance::Appearance>) -> String {
        match appr {
            Some(crate::appearance::Appearance::Light) => self.config.light_theme.clone(),
            _ => self.config.dark_theme.clone(),
        }
    }

    /// Apply the light/dark theme for `appr`, but only when it differs from what the
    /// follow-system switch last applied — so a per-tick poll never re-resolves or
    /// re-decodes the cover for the accent when nothing changed. Marks the UI dirty on
    /// a real switch (a colour change alone doesn't set `is_animating`). Split from the
    /// detection so the switching logic is testable without the OS.
    pub(crate) fn apply_appearance(&mut self, appr: Option<crate::appearance::Appearance>) {
        let name = self.system_theme_name(appr);
        if self.applied_sys_theme.as_deref() == Some(name.as_str()) {
            return;
        }
        self.apply_theme(&name);
        self.applied_sys_theme = Some(name);
        self.mark_dirty();
    }

    /// Poll the OS appearance and apply the matching theme (follow-system mode only).
    /// Called from the render tick; cheap and a no-op when the appearance is unchanged.
    pub(crate) fn poll_system_appearance(&mut self) {
        if !self.config.theme_follows_system {
            return;
        }
        self.apply_appearance(crate::appearance::detect());
    }

    /// Cycle the light (`dark = false`) or dark slot's theme through `all_themes()`,
    /// persist it, and apply it live *iff* that slot is the one currently showing —
    /// [`Self::apply_appearance`] no-ops when the active theme name is unchanged, so
    /// editing the off-screen slot just stores the name for the next system switch.
    fn cycle_slot_theme(&mut self, dark: bool, dir: i32) {
        let order = self.all_themes();
        let cur = if dark {
            &self.config.dark_theme
        } else {
            &self.config.light_theme
        };
        let idx = order.iter().position(|n| n == cur.as_str()).unwrap_or(0) as i32;
        let n = order.len() as i32;
        let next = order[(((idx + dir) % n + n) % n) as usize].clone();
        if dark {
            self.config.dark_theme = next;
        } else {
            self.config.light_theme = next;
        }
        self.config.save();
        self.apply_appearance(crate::appearance::detect());
    }

    /// Turn follow-system mode on or off, persist it, and apply the result live: on →
    /// switch to the light/dark theme matching the current OS appearance; off →
    /// restore the single `config.theme`.
    pub(crate) fn set_theme_follows_system(&mut self, on: bool) {
        if self.config.theme_follows_system == on {
            return;
        }
        self.config.theme_follows_system = on;
        self.config.save();
        self.applied_sys_theme = None; // force the next apply to re-resolve
        if on {
            self.apply_appearance(crate::appearance::detect());
        } else {
            let single = self.config.theme.clone();
            self.apply_theme(&single);
        }
        self.mark_dirty();
    }

    pub(crate) fn cycle_theme(&mut self) {
        let order = self.all_themes();
        let cur = self.theme.name.as_str();
        let idx = order.iter().position(|n| n == cur).unwrap_or(0);
        let next = order[(idx + 1) % order.len()].clone();
        if self.config.theme_follows_system {
            // following the OS → `t` retunes the CURRENT appearance's theme (its
            // light/dark slot), applied live and persisted to that slot — not the
            // single `config.theme`, which is dormant while following.
            let appr = crate::appearance::detect();
            match appr {
                Some(crate::appearance::Appearance::Light) => {
                    self.config.light_theme = next.clone()
                }
                _ => self.config.dark_theme = next.clone(),
            }
            self.applied_sys_theme = None;
            self.apply_appearance(appr);
            self.config.save();
            let slot = match appr {
                Some(crate::appearance::Appearance::Light) => "light",
                _ => "dark",
            };
            self.notify(format!("Theme ({slot}): {}", self.theme.name));
        } else {
            self.set_theme(&next);
            self.config.save();
            self.notify(format!("Theme: {}", self.theme.name));
        }
    }
}

/// Settings overlay/popup UI state (slice 4 of the AppState split): the cursor +
/// active group tab + the key-rebind capture. Pure UI state — the actual config
/// lives in `AppState::config`.
#[derive(Default)]
pub struct SettingsUi {
    /// Highlighted row in the current list.
    pub sel: usize,
    /// The full, tabbed Settings overlay is open (command-palette / key).
    pub overlay: bool,
    /// Active group tab in the full overlay — `Some(i)` into `SETTINGS_GROUPS`
    /// while it's open (Tab / ⇧Tab switch). `None` only before it has opened.
    pub group: Option<usize>,
    /// A settings group shown as a popup over the current view (the `;` shortcut).
    pub popup: Option<usize>,
    /// `Some(action)` while the Keys settings wait for a key press to rebind it.
    pub rebinding: Option<String>,
    /// Armed when the Spotify log-out/reset row is selected: the status bar shows
    /// a `⏎/y confirm · esc cancel` prompt and input is captured until answered.
    pub confirm_logout: bool,
    /// Sticky scroll offset of the settings list.
    pub off: std::cell::Cell<usize>,
}
