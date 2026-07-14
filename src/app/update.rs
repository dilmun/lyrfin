//! The central `update(Action)` dispatch on `AppState`.

use super::*;

impl AppState {
    /// Pop one layer of context: cancel a prompt, close the topmost overlay, drop
    /// a selection, or exit a drilled-into album/playlist — in priority order.
    /// Returns `true` if a layer was popped, `false` if there was nothing to back
    /// out of (i.e. we're at the top level). Shared by `Back` (Esc) and
    /// `QuitOrBack` (`q`, which quits when this returns `false`).
    pub(crate) fn go_back(&mut self) -> bool {
        if self.settings.confirm_logout {
            self.settings.confirm_logout = false; // cancel the logout prompt
        } else if self.settings.rebinding.is_some() {
            self.settings.rebinding = None; // cancel a pending key rebind
        } else if self.eq.open {
            // Equalizer overlay: back out of the save-name entry first, else close it
            if self.eq.naming.is_some() {
                self.eq.naming = None;
            } else {
                self.close_equalizer();
            }
        } else if self.tags_open() {
            // unified Tag Edit modal: back out of a sub-mode on the active
            // tab, else close the whole modal.
            match self.tags.tab {
                2 => match self.tags.cover.as_mut() {
                    Some(cs) if cs.confirm => cs.confirm = false,
                    Some(cs) if cs.editing => cs.editing = false,
                    _ => self.close_tags(),
                },
                1 => match self.tags.search.as_mut() {
                    Some(ts) if ts.pending.is_some() => ts.pending = None,
                    Some(ts) if ts.editing => ts.editing = false,
                    _ => self.close_tags(),
                },
                _ => self.close_tags(), // Edit tab: editor's own Esc handles sub-modes
            }
        } else if self.settings.popup.is_some() {
            // close the settings popup overlay (back to the view)
            self.settings.popup = None;
            self.settings.sel = 0;
            self.settings.off.set(0);
        } else if let Some(p) = self.palette.as_mut() {
            // in a value picker, Esc pops back to the setting list; at the root it closes
            if matches!(p.ctx, crate::app::PaletteCtx::Setting(_)) {
                p.ctx = crate::app::PaletteCtx::Root;
                p.query.clear();
                p.sel = 0;
            } else {
                self.palette = None;
            }
        } else if self.input.confirm_delete.is_some() {
            // dismiss the delete-confirmation dialog without deleting
            self.input.confirm_delete = None;
        } else if self.input.naming.is_some() {
            self.input.naming = None;
            self.input.buffer.clear();
        } else if !self.input.add_targets.is_empty() {
            self.input.add_targets.clear();
        } else if self.spotify.pl_confirm_delete.is_some() {
            // dismiss the Spotify playlist unfollow ("delete") confirmation
            self.spotify.pl_confirm_delete = None;
        } else if self.spotify.pl_modal.is_some() {
            // Spotify add/create/rename modal: step name-prompt → picker, or close
            self.spotify_playlist_modal_back();
        } else if self.search.active {
            self.search.active = false;
            self.focus = Focus::Main;
        } else if self.info.is_some() {
            // the unified Info overlay closes as one (Keys/Stats/Health/Track)
            self.close_info();
        } else if self.settings.overlay {
            // tabs replace the old group-list step — Esc closes the overlay
            self.settings.overlay = false;
            self.settings.sel = 0;
            self.settings.off.set(0);
        } else if self.marks.anchor.is_some() {
            self.marks.anchor = None;
        } else if !self.marks.ids.is_empty() {
            self.marks.ids.clear();
        } else if self.layout == Layout::Dashboard && self.local_back() {
            // Dashboard drill-in: pop one level back to the parent list
        } else if self.layout == Layout::Spotify && self.spotify_cancel() {
            // Spotify drill-in: pop one level back to the parent list
        } else if !self.browser.list.is_empty() {
            // (other local views) exit a browsed album/playlist back to the queue
            self.browser.list.clear();
            self.browser.title.clear();
            self.selection = 0;
        } else {
            return false; // nothing to back out of — we're at the top level
        }
        true
    }

    /// The sole state transition. Returns side-effect commands in later
    /// milestones (e.g. `Vec<Command>` for the audio/library workers); for now
    /// it mutates state directly.
    pub fn update(&mut self, action: Action) {
        use Action::*;
        // Any action other than an idle tick / no-op may change the UI, so the
        // event loop should redraw. (Tick sets dirty itself only when it changes
        // something visible; Redraw forces a redraw.)
        if !matches!(action, Tick | Noop) {
            self.dirty = true;
        }
        match action {
            Quit => self.running = false,
            Tick => self.on_tick(),
            Redraw | Noop => {}

            Move(m) => self.move_selection(m),
            Activate => {
                if self.input.confirm_delete.is_some() {
                    self.confirm_delete_playlist();
                } else if self.input.naming.is_some() {
                    self.confirm_name();
                } else {
                    self.activate();
                }
            }
            SwitchView(v) => self.view = v,
            SwitchLayout(l) => {
                // any view switch closes the settings overlay / per-view popup
                self.settings.popup = None;
                self.settings.overlay = false;
                self.set_layout(l);
            }
            OpenSettings => self.open_settings(),
            OpenTags => self.open_tags(),
            OpenCoverSearch => self.open_cover_search(),
            CoverMove(m) => self.cover_move(m),
            CoverInput(s) => self.cover_input(s),
            CoverActivate => self.cover_activate(),
            OpenTagSearch => self.open_tag_search(),
            TagMove(m) => self.tag_move(m),
            TagInput(s) => self.tag_input(s),
            TagActivate => self.tag_activate(),
            TagApplyAlbum => {
                let kind = if self.tags.search.as_ref().is_some_and(|ts| ts.album_mode) {
                    PendingApply::AlbumFull
                } else {
                    PendingApply::AlbumBasic
                };
                self.tag_request(kind);
            }
            TagConfirm => self.tag_confirm(),
            CoverConfirm => self.cover_confirm(),
            CoverToggleScope => self.cover_toggle_scope(),
            QueryInsert(c) => self.query_insert(c),
            QueryBackspace => self.query_del(false),
            QueryDelete => self.query_del(true),
            QueryCaret(c) => self.query_caret(c),
            TagToggleAlbum => self.toggle_tag_album(),
            TagSource(d) => self.tag_source(d),
            FocusPane(p) => self.set_focus(p),
            CyclePane => self.cycle_pane(),
            CyclePaneRev => self.cycle_pane_rev(),
            NavDown => self.nav(Motion::Down),
            NavUp => self.nav(Motion::Up),

            TogglePlay => self.toggle_play(),
            Stop => {
                self.engine.send(AudioCommand::Stop);
                self.player.stop();
                self.loaded_track = None;
            }
            Next => self.advance_next(),
            Previous => self.advance_prev(),
            PlayCurrentAlbum => self.play_current_album(),
            PlayCurrentArtist => self.play_current_artist(),
            Seek(delta) => {
                // h/l (and ←/→): an open settings popup/overlay adjusts the
                // selected row (must win over playback, even with Spotify
                // streaming); else browser columns / sidebar tabs / seek.
                if self.settings.popup.is_some() {
                    self.settings_adjust(delta.signum() as i32); // popup is a detail
                } else if self.settings.overlay {
                    // a group tab is always active — h/l adjust the selected row
                    self.settings_adjust(delta.signum() as i32);
                } else if self.layout == Layout::Spotify && self.spov.now_spotify.is_some() {
                    self.spotify_seek(delta);
                } else {
                    self.seek_relative(delta);
                }
            }
            GoLive => self.radio_go_live(),
            GoStreamStart => self.radio_go_start(),
            ToggleShuffle => self.toggle_shuffle_active(),
            CycleRepeat => self.cycle_repeat_active(),
            VolumeDelta(d) => {
                self.player.adjust_volume(d);
                self.engine
                    .send(AudioCommand::SetVolume(self.player.volume));
            }
            SetVolume(v) => {
                self.player.volume = v.min(100);
                self.engine
                    .send(AudioCommand::SetVolume(self.player.volume));
            }
            SetSpeed(s) => {
                self.player.speed = s.clamp(0.25, 2.0);
                self.engine.send(AudioCommand::SetSpeed(self.player.speed));
            }

            ToggleLyrics => {
                // in the dedicated Lyrics view lyrics are always shown, so L
                // cycles the display format (its "lyrics options"); elsewhere it
                // toggles the movable Lyrics pane (like `i` toggles Artist). Lyrics
                // auto-load on track change, so revealing the pane shows them.
                if self.layout == Layout::LyricsFocus {
                    self.cycle_lyrics_format();
                } else {
                    self.toggle_panel(Panel::Lyrics);
                }
            }
            ToggleArtistInfo => {
                self.toggle_panel(Panel::Artist);
                if self.panel(Panel::Artist).shown {
                    self.request_artist_info();
                }
            }
            ToggleQueue => self.toggle_panel(Panel::Queue),
            ToggleQueueSide => self.move_panel(Panel::Queue),
            MoveArtistPanel => self.move_panel(Panel::Artist),
            MoveLyricsViz => self.move_panel(Panel::Visualizer),
            MoveSidebar => self.move_panel(Panel::Sidebar),
            ResizeFocusedPane(d) => self.resize_focused_pane(d),
            ResizePaneHeight(d) => self.resize_pane_height(d),
            MoveFocusedPane => self.move_focused_pane(),
            ToggleSidebar => self.toggle_panel(Panel::Sidebar),
            SettingsRemove => self.settings_remove(),
            RebindKey(label) => {
                if let Some(action) = self.settings.rebinding.take() {
                    self.config.keymap.rebind(&action, &label);
                    self.config.keymap.save(&self.config.dir);
                    self.notify(format!(
                        "Bound {label} → {}",
                        crate::keymap::keybind_desc(&action)
                    ));
                }
            }
            RestoreKeybinds => {
                self.settings.rebinding = None;
                self.config.keymap = crate::config::Keymap::with_defaults();
                let _ = std::fs::remove_file(self.config.dir.join("keybindings.toml"));
                self.notify("Keybindings reset to defaults".into());
            }
            CycleLyricsFormat => self.cycle_lyrics_format(),
            LyricsOffset(delta) => self.nudge_lyrics_offset(delta),
            OpenViewSettings => {
                // toggle a popup overlay for the current view's settings group —
                // stays in the view (the full window is reached via the palette)
                self.settings.confirm_logout = false;
                // toggle the per-view quick popup, opening on its first tab (Panes)
                self.settings.popup = if self.settings.popup.is_some() {
                    None
                } else {
                    Some(0)
                };
                self.settings.sel = 0;
                self.settings.off.set(0);
            }
            CycleOverlaySize => self.cycle_overlay_size(1),
            OverlayTab(d) => {
                // step whichever tabbed overlay owns the screen
                if self.info.is_some() {
                    self.info_tab_step(d);
                } else if self.tags_open() {
                    self.tags_tab_step(d);
                } else if self.settings.overlay {
                    let n = self.settings_tabs().len() as i32;
                    if n > 0 {
                        let cur = self.settings.group.unwrap_or(0) as i32;
                        self.set_overlay_tab((((cur + d) % n + n) % n) as usize);
                    }
                } else if self.settings.popup.is_some() {
                    let n = self.popup_tab_names().len() as i32;
                    if n > 0 {
                        let cur = self.settings.popup.unwrap_or(0) as i32;
                        self.set_overlay_tab((((cur + d) % n + n) % n) as usize);
                    }
                }
            }
            ResetLayout => self.reset_layout(),
            FitLayout => self.fit_layout(),
            ToggleGridView => self.toggle_grid_current(),
            GridMove(dx, dy) => self.grid_move_current(dx, dy),
            ToggleLyricsViz => self.toggle_panel(Panel::Visualizer),
            ToggleHelp => self.toggle_info(InfoTab::Keys),
            ToggleStats => self.toggle_info(InfoTab::Stats),
            ToggleTrackInfo => self.toggle_info(InfoTab::Track),
            HelpInput(q) => {
                if let Some(i) = &mut self.info {
                    i.keys_query = q;
                    i.keys_scroll = 0; // re-filtering resets the scroll
                }
            }
            OpenEqualizer => self.toggle_equalizer(),
            EqSelect(d) => self.eq_select(d),
            EqAdjust(d) => self.eq_adjust(d),
            EqTogglePower => self.eq_toggle_power(),
            EqCyclePreset(d) => self.eq_cycle_preset(d),
            EqReset => self.eq_reset(),
            EqResetBand => self.eq_reset_selected(),
            EqBeginSave => self.eq_begin_save(),
            EqNameInput(s) => self.eq_name_input(s),
            EqSavePreset => self.eq_save_preset(),
            EqDeletePreset => self.eq_delete_preset(),
            OpenPalette => {
                self.palette = Some(Palette {
                    query: String::new(),
                    sel: 0,
                    ctx: crate::app::PaletteCtx::Root,
                })
            }
            PaletteInput(q) => {
                if let Some(p) = &mut self.palette {
                    p.query = q;
                    p.sel = 0;
                }
            }
            PaletteMove(m) => {
                let n = self.palette_matches().len();
                if let Some(p) = &mut self.palette {
                    p.sel = step(p.sel, m, n);
                }
            }
            PaletteActivate => self.palette_activate(),
            PaletteOpenSetting(s) => self.palette_open_setting(s),
            PaletteReveal => self.palette_reveal_selected(),
            RunCommand(s) => {
                let msg = self.run_command(&s);
                self.notify(msg);
            }
            CycleVisualizer => {
                // cycle whichever visualizer this view actually shows: the big
                // per-view one where a view has it, else the playback-bar viz
                // (which has its own mode, independent of the big one).
                if layout_has_big_viz(self.layout) {
                    let nmodes = crate::ui::components::VIZ_MODES.len() as u8;
                    let m = self.views.viz_modes.entry(self.layout).or_insert(0);
                    *m = (*m + 1) % nmodes;
                    // persist now (not just on a clean quit) so the mode can't be
                    // lost to a Ctrl-C / crash / rebuild that kills the process
                    self.save_session();
                    let name = crate::ui::components::VIZ_MODES
                        [self.viz_mode() as usize % crate::ui::components::VIZ_MODES.len()];
                    self.notify(format!("Visualizer: {name}"));
                } else {
                    self.cycle_player_viz_mode(1);
                }
            }
            CycleTheme => self.cycle_theme(),
            SetSleepTimer(min) => self.set_sleep_timer(min),
            CycleSleepTimer => {
                let cur = self.sleep_remaining_secs().map(|s| s.div_ceil(60));
                let next = match cur {
                    None => 15,
                    Some(m) if m <= 15 => 30,
                    Some(m) if m <= 30 => 45,
                    Some(m) if m <= 45 => 60,
                    _ => 0,
                };
                self.set_sleep_timer(next);
            }
            AbLoopCycle => self.ab_loop_cycle(),
            CycleReplayGain => self.cycle_replaygain(),
            Notify(text) => self.notify(text),

            BeginSearch => {
                self.search.active = true;
                self.focus = Focus::Search;
                self.search.query.clear();
            }
            SearchInput(q) => self.search.query = q,
            OpenRadio => self.open_radio(),
            RadioInput(q) => self.radio_search(q),
            RadioActivate => self.radio_activate(),
            RadioFocusSearch => self.radio.editing = true,
            RadioCancel => self.radio_cancel(),
            RadioOpenCountry => self.radio_open_picker(PickerKind::Country),
            RadioOpenGenre => self.radio_open_picker(PickerKind::Genre),
            RadioPickerInput(q) => {
                if let Some(p) = &mut self.radio.picker {
                    p.query = q;
                }
                // auto-highlight the closest match (skip the leading "clear" row)
                // so a typed filter + Enter applies it directly
                let has_match = self.radio_picker_match_count() > 1;
                if let Some(p) = &mut self.radio.picker {
                    p.sel = if !p.query.is_empty() && has_match {
                        1
                    } else {
                        0
                    };
                }
            }
            RadioPickerStartSearch => {
                if let Some(p) = &mut self.radio.picker {
                    p.editing = true;
                }
            }
            RadioPickerEndSearch => {
                if let Some(p) = &mut self.radio.picker {
                    p.editing = false;
                }
            }
            RadioStar => self.radio_star(),
            RadioCycleSort => self.radio_cycle_sort(),
            RadioStation(delta) => self.radio_station(delta),
            RadioRefresh => self.refresh_directory(),
            RadioNewPlaylist => self.radio_begin_new_playlist(),
            RadioRenamePlaylist => self.radio_begin_rename_playlist(),
            RadioDeletePlaylist => self.radio_delete_playlist_prompt(),
            RadioAddToPlaylist => self.radio_add_current_to_playlist(),
            RadioRemoveFromPlaylist => self.radio_remove_from_playlist(),
            RadioNameInput(s) => self.radio_name_input(s),
            RadioModalConfirm => self.radio_modal_confirm(),
            RadioModalCancel => self.radio_modal_cancel(),
            OpenSpotify => self.open_spotify(),
            SpotifyLogin => self.spotify_login(),
            SpotifyLogout => self.spotify_logout(),
            SpotifyToggleSidebar => self.spotify_toggle_sidebar(),
            SpotifyCycleFocus(d) => self.cycle_focus(d),
            SpotifyFocusSearch => {
                self.spotify.searching = true;
                self.focus = crate::app::Focus::Main;
            }
            SpotifyInput(q) => self.spotify_search(q),
            SpotifyCancel => {
                self.spotify_cancel();
            }
            SpotifyActivate => self.spotify_activate(),
            SpotifyTrack(delta) => self.spotify_track(delta),
            SpotifyLike => self.spotify_toggle_saved(),
            SpotifyFollow => self.spotify_toggle_follow(),
            SpotifyWriteConfig => {
                // prompt for the client id right here; we write the config + log in
                self.input.naming = Some(NameTarget::SpotifyClientId);
                self.input.buffer = self.config.spotify_client_id.clone();
            }
            // Spotify playlist management (Web API writes)
            SpotifyAddToPlaylist => self.spotify_add_to_playlist_prompt(),
            SpotifyBeginNewPlaylist => self.spotify_playlist_begin_new(),
            SpotifyNewPlaylist => self.spotify_new_playlist(),
            SpotifyRenamePlaylist => self.spotify_begin_rename_playlist(),
            SpotifyDeletePlaylist => self.spotify_delete_playlist_prompt(),
            SpotifyRemoveFromPlaylist => self.spotify_remove_from_playlist(),
            SpotifyNameInput(s) => self.spotify_playlist_name_input(s),
            BookmarkSearch => {
                if self.search.query.trim().is_empty() {
                    self.notify("Search something first to bookmark it".into());
                } else {
                    self.input.naming = Some(NameTarget::Bookmark);
                    self.input.buffer = self.search.query.clone();
                }
            }
            RunSearch(q) => {
                self.search.query = q;
                self.search.active = false;
                self.browser.list.clear();
                self.browser.title.clear();
                self.selection = 0;
                self.focus = Focus::Main;
            }
            Back => {
                self.go_back();
            }
            QuitOrBack => {
                // `q`: pop the current context — an open overlay, a drilled-into
                // album/playlist, or a selection. If there's nothing to pop (top
                // level), quit the app.
                if !self.go_back() {
                    self.running = false;
                }
            }
            CopyError => self.copy_last_error(),
            ToggleErrorLog => self.toggle_info(InfoTab::Health),
            NameInput(s) => self.input.buffer = s,
            BeginNewPlaylist => {
                self.input.naming = Some(NameTarget::New);
                self.input.buffer.clear();
            }
            NewSmartPlaylist => {
                if self.search.query.trim().is_empty() {
                    self.notify("Search/filter first, then save it as a smart playlist".into());
                } else {
                    self.input.naming = Some(NameTarget::SmartPlaylist);
                    self.input.buffer.clear();
                }
            }
            BeginRenamePlaylist => {
                if let Some(id) = self.selected_local_playlist() {
                    let name = self
                        .library
                        .playlists
                        .get(&id)
                        .map(|p| p.name.clone())
                        .unwrap_or_default();
                    self.input.naming = Some(NameTarget::Rename(id));
                    self.input.buffer = name;
                }
            }
            DeletePlaylist => {
                // open the confirm dialog rather than deleting on a single keypress
                if let Some(id) = self.selected_local_playlist() {
                    self.input.confirm_delete = Some(id);
                }
            }
            RemoveFromPlaylist => self.remove_selected_from_playlist(),
            AddCurrentToPlaylist => {
                if let (Some(track), Some(p)) =
                    (self.player.current, self.selected_local_playlist())
                {
                    self.library.add_to_playlist(p, track);
                    self.save_playlists();
                    self.notify("Added to playlist".into());
                }
            }
            AddToPlaylistPrompt => {
                let ids = self.selected_track_ids();
                if !ids.is_empty() {
                    self.input.add_targets = ids;
                    self.input.add_sel = 0;
                    self.marks.anchor = None;
                }
            }
            ToggleMark => self.toggle_mark(),
            VisualSelect => self.toggle_visual(),
            BeginTagEdit => self.begin_tag_edit(),
            TagEditBeginEdit => {
                if let Some(te) = &mut self.tags.edit {
                    te.editing = true;
                }
                self.tag_edit_caret_to_end();
            }
            TagEditStopEdit => {
                if let Some(te) = &mut self.tags.edit {
                    te.editing = false;
                }
            }
            TagEditMove(m) => self.tag_edit_move(m),
            TagEditInsert(c) => self.tag_edit_insert(c),
            TagEditBackspace => self.tag_edit_del(false),
            TagEditDelete => self.tag_edit_del(true),
            TagEditCaret(c) => self.tag_edit_caret(c),
            TagEditType(s) => self.tag_edit_set_field(s),
            TagEditCase(m) => self.tag_edit_case(m),
            TagEditClear => self.tag_edit_clear(),
            TagRemoveField => self.tag_remove_field(),
            TagEditAutoNumber => self.tag_edit_autonumber(),
            TagConvertBegin => {
                if let Some(te) = &mut self.tags.edit {
                    te.convert = Some((false, "%artist% - %title%".into()));
                }
            }
            TagRenameBegin => {
                if let Some(te) = &mut self.tags.edit {
                    te.convert = Some((true, "#. T".into()));
                }
            }
            TagConvertType(s) => {
                if let Some(te) = &mut self.tags.edit {
                    let dir = te.convert.as_ref().map(|(d, _)| *d).unwrap_or(false);
                    te.convert = Some((dir, s));
                }
            }
            TagConvertCancel => {
                if let Some(te) = &mut self.tags.edit {
                    te.convert = None;
                }
            }
            TagConvertApply => self.tag_convert_apply(),
            TagReplaceBegin => {
                if let Some(te) = &mut self.tags.edit {
                    te.replace = Some((String::new(), String::new(), false));
                }
            }
            TagReplaceType(s) => {
                if let Some(te) = &mut self.tags.edit
                    && let Some((find, repl, on_repl)) = &mut te.replace
                {
                    if *on_repl {
                        *repl = s;
                    } else {
                        *find = s;
                    }
                }
            }
            TagReplaceToggle => {
                if let Some(te) = &mut self.tags.edit
                    && let Some((_, _, on_repl)) = &mut te.replace
                {
                    *on_repl = !*on_repl;
                }
            }
            TagReplaceCancel => {
                if let Some(te) = &mut self.tags.edit {
                    te.replace = None;
                }
            }
            TagReplaceApply => self.tag_replace_apply(),
            TagEditSave => self.tag_edit_save(),
            TagEditAlbumPrompt => {
                // arm the confirmation only when there's actually something to write
                let has_changes = self
                    .tags
                    .edit
                    .as_ref()
                    .is_some_and(|te| te.touched.iter().any(|&t| t));
                if has_changes {
                    if let Some(te) = &mut self.tags.edit {
                        te.confirm_album = true;
                    }
                } else {
                    self.notify("No changes".into());
                }
            }
            TagEditAlbumCancel => {
                if let Some(te) = &mut self.tags.edit {
                    te.confirm_album = false;
                }
            }
            TagEditSaveAlbum => self.tag_edit_apply(true),
            TagEditCancel => self.close_tags(),
            ToggleFavoriteSel => self.toggle_favorite_selection(),
            Rate(id, stars) => {
                let stars = stars.min(5);
                if let Some(t) = self.library.tracks.get_mut(&id) {
                    t.rating = stars;
                }
                self.search.lib_gen += 1;
                self.notify(format!("Rated {stars} stars"));
            }
            ClearQueue => {
                let keep = self.player.current;
                self.player.queue.items = keep.into_iter().collect();
                self.player.queue.position = 0;
                self.update_gapless_next();
                self.notify("Queue cleared".into());
            }
            QueueMove(m) => {
                self.queue_move(m);
                self.update_gapless_next();
            }
            QueueRemove => {
                self.queue_remove();
                self.update_gapless_next();
            }
            QueueClearUpcoming => {
                self.queue_clear_upcoming();
                self.update_gapless_next();
            }
            RescanLibrary => {
                self.request_rescan();
                self.notify("Rescanning library…".into());
            }
            RandomAlbum => self.random_album(),

            // remaining arms are wired as their milestones land
            _ => {}
        }
    }

    /// Toggle shuffle on the active source (Spotify or the local queue), keep the
    /// preloaded next track in sync, and show the one unified status toast — so the
    /// feedback reads the same in every view, not just Spotify. Shared by the `s` key
    /// and the transport-button click.
    pub(crate) fn toggle_shuffle_active(&mut self) {
        let on = if self.showing_spotify() {
            self.spotify_toggle_shuffle();
            self.spov.sp_shuffle
        } else {
            self.player.toggle_shuffle();
            self.update_gapless_next(); // reordered upcoming → recompute the next track
            self.player.shuffle
        };
        self.notify(if on { "Shuffle on" } else { "Shuffle off" }.into());
    }

    /// Cycle repeat on the active source + the same unified toast. Shared by the `r`
    /// key and the transport-button click.
    pub(crate) fn cycle_repeat_active(&mut self) {
        let repeat = if self.showing_spotify() {
            self.spotify_cycle_repeat();
            self.spov.sp_repeat
        } else {
            self.player.cycle_repeat();
            self.update_gapless_next(); // repeat-one/all changes the next track
            self.player.repeat
        };
        self.notify(format!("Repeat: {}", repeat_to_str(repeat)));
    }
}
