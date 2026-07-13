//! Session persistence + library load/seed methods on `AppState` (extracted from app/mod.rs).

use super::*;
use crate::core::model::Track;

impl AppState {
    /// Restore persisted UI/runtime state at launch: simple fields now, plus the
    /// library-dependent state once a library is present (resolved by path/name).
    pub fn apply_session(&mut self, s: crate::session::Session) {
        // the theme is NOT restored here — it's owned by config.toml (see
        // `Session`), so an explicit `theme = "auto"` is never overridden by a
        // stale session value.
        if let Some(v) = s.volume {
            self.player.volume = v.min(100);
        }
        if let Some(l) = &s.layout
            && let Some(lay) = layout_from_str(l)
        {
            self.layout = lay;
        }
        if let Some(f) = &s.focus
            && let Some(p) = focus_from_str(f)
        {
            self.focus = p;
        }
        if let Some(list) = &s.visualizer_modes {
            for (l, m) in list {
                if let Some(layout) = layout_from_str(l) {
                    self.views
                        .viz_modes
                        .insert(layout, m % crate::ui::components::VIZ_MODES.len() as u8);
                }
            }
        }
        // per-view panel state
        self.views.panels.clear();
        if let Some(list) = &s.panels {
            for (l, p, shown, d, size) in list {
                if let (Some(layout), Some(panel)) = (layout_from_str(l), panel_from_key(p)) {
                    self.views.panels.insert(
                        (layout, panel),
                        PanelCfg {
                            shown: *shown,
                            dock: Dock::from_label(d),
                            // size is a percentage now; clamp into the valid band
                            // (also caps any pre-migration cell value, e.g. 80 → 60)
                            size: (*size).clamp(10, 60),
                            len: 50, // overlaid from `pane_lens` below (older sessions lack it)
                        },
                    );
                }
            }
        }
        // overlay the per-panel cross-axis shares (separate list; absent in older
        // sessions). The panel override already exists from the loop above.
        if let Some(lens) = &s.pane_lens {
            for (l, p, len) in lens {
                if let (Some(layout), Some(panel)) = (layout_from_str(l), panel_from_key(p))
                    && let Some(cfg) = self.views.panels.get_mut(&(layout, panel))
                {
                    cfg.len = (*len).clamp(20, 80);
                }
            }
        }
        // per-section grid/list overrides
        self.views.grid.clear();
        if let Some(list) = &s.grid_sections {
            for (key, grid) in list {
                if let Some(section) = crate::app::LocalSection::from_key(key) {
                    self.views.grid.insert(section, *grid);
                }
            }
        }
        self.views.spotify_grid.clear();
        if let Some(list) = &s.spotify_grid_sections {
            self.views.spotify_grid.extend(list.iter().copied());
        }
        if let Some(sh) = s.shuffle {
            self.player.shuffle = sh;
        }
        if let Some(r) = &s.repeat {
            self.player.repeat = repeat_from_str(r);
        }
        if let Some(sp) = s.speed {
            self.player.speed = sp.clamp(0.25, 2.0);
        }
        self.selection = s.selection.unwrap_or(0);
        self.queue_sel = s.queue_sel.unwrap_or(0);

        // internet radio: restore the filters, query, sort, and last-tuned station
        // (the station list itself is re-fetched once the radio worker is ready —
        // see `set_radio_sender`). The station shows on the now-bar in a stopped
        // state; space / Enter re-tunes it, matching how a local track resumes.
        if let Some(q) = &s.radio_query {
            self.radio.query = q.clone();
        }
        self.radio.country = s.radio_country.clone();
        self.radio.tag = s.radio_tag.clone();
        if let Some(sort) = &s.radio_sort {
            self.radio.sort = crate::radio::Sort::from_label(sort);
        }
        if let Some(st) = &s.radio_station {
            // restore as a paused overlay — don't auto-stream on launch, and
            // leave the local player state alone (it's restored separately below)
            self.rnow.now_station = Some(st.clone());
            self.rnow.radio_paused = true;
        }

        // Spotify: remember which account this saved state belongs to. On reconnect
        // (`AuthEvent::Connected`) it's compared to the account we actually log in
        // as; a mismatch drops the restored now-playing/queue/browse so one
        // account's state is never shown under another.
        self.spotify.restored_account = s.spotify_account.clone();

        // Spotify: restore the now-playing track + queue as a paused overlay.
        // Nothing streams until the user opens Spotify (the session reconnects)
        // and presses space — which re-loads the track at `sp_pos` (see
        // `spotify_resume`). `sp_started = false` keeps the position clock frozen.
        if let Some(tr) = &s.spotify_now {
            self.spov.sp_dur = tr.duration_ms as f64 / 1000.0;
            self.spov.sp_pos = s.spotify_pos.unwrap_or(0.0).clamp(0.0, self.spov.sp_dur);
            self.spov.now_spotify = Some(tr.clone());
            self.spov.spotify_paused = true;
            self.spov.sp_started = false;
        }
        if let Some(q) = &s.spotify_queue {
            self.spov.sp_queue = q.clone();
        }
        self.spov.sp_idx = s.spotify_idx.unwrap_or(0);
        // Restore the queue's shuffle/repeat modes as flags only — the queue above is
        // persisted in its shuffled order, so re-shuffling here would re-randomize
        // what plays next. Independent of the local player's modes restored earlier.
        if let Some(sh) = s.spotify_shuffle {
            self.spov.sp_shuffle = sh;
        }
        if let Some(r) = &s.spotify_repeat {
            self.spov.sp_repeat = repeat_from_str(r);
        }

        // Spotify browse view: section / search are applied now; once the initial
        // list lands after reconnecting, a saved drill-in is re-opened and the
        // cursor placed (`spotify_load_initial` → `spotify_apply_initial_restore`).
        if let Some(sec) = s.spotify_section {
            self.spotify.section = sec;
        }
        if let Some(q) = &s.spotify_query {
            self.spotify.query = q.clone();
        }
        self.spotify.in_search = s.spotify_in_search.unwrap_or(false);
        self.spotify.restore_sel = s.spotify_sel;
        // Reconstruct the drilled-in container from its persisted stable primitives
        // (URI + kind + name) into a minimal `Item`, re-opened once the section/search
        // list lands (`spotify_apply_initial_restore`). This carries enough to re-open
        // even when the container isn't in the reloaded list — e.g. an artist opened
        // from the now-playing track — which a URI-only restore couldn't. Same field
        // the same-session re-auth path uses (there it's the full in-memory `Item`).
        self.spotify.restore_open = s.spotify_open.as_ref().map(|d| crate::spotify::api::Item {
            uri: d.uri.clone(),
            name: d.name.clone(),
            kind: d.kind,
            ..Default::default()
        });

        self.scan.restore = Some(s);
        self.restore_library_state(); // demo / already-loaded library
    }

    /// Resolve the pending session's library-dependent state (expanded artists,
    /// queue, last-played track + position) against the current library.
    pub(crate) fn restore_library_state(&mut self) {
        let Some(s) = self.scan.restore.clone() else {
            return;
        };

        // re-apply highlight positions (a real scan resets them in set_library)
        self.selection = s.selection.unwrap_or(self.selection);
        self.queue_sel = s.queue_sel.unwrap_or(self.queue_sel);

        // path → id lookup
        let by_path: std::collections::HashMap<String, TrackId> = self
            .library
            .tracks
            .values()
            .map(|t| (t.path.to_string_lossy().into_owned(), t.id))
            .collect();

        if let Some(paths) = &s.queue_paths {
            let q: Vec<TrackId> = paths
                .iter()
                .filter_map(|p| by_path.get(p).copied())
                .collect();
            if !q.is_empty() {
                self.player.queue.items = q;
            }
        }

        if let Some(cp) = &s.current_path
            && let Some(&id) = by_path.get(cp)
        {
            self.player.current = Some(id);
            self.player.duration = self
                .library
                .track(id)
                .map(|t| t.duration())
                .unwrap_or_default();
            self.player.elapsed = Duration::from_secs(s.elapsed_secs.unwrap_or(0));
            self.player.status = Status::Paused;
            if let Some(pos) = self.player.queue.items.iter().position(|&x| x == id) {
                self.player.queue.position = pos;
            }
            // with a real engine, preload + seek so [space] resumes in place
            if self.engine_active
                && let Some(path) = self.library.track(id).map(|t| t.path.clone())
            {
                self.engine.send(AudioCommand::Load(path));
                self.engine.send(AudioCommand::Seek(self.player.elapsed));
                self.loaded_track = Some(id);
            }
        }

        // load persisted playlists (additive, deduped by name)
        crate::library::store::PlaylistStore::load(&self.config.dir).apply_to(&mut self.library);

        // restore the local drill-in browse: the section + cursor (playlists are
        // loaded above, so the Playlists section restores with the right list).
        if let Some(sec) = s.local_section.as_deref().and_then(LocalSection::from_key) {
            self.local.section = sec;
        }
        self.local_load_section();
        // re-drill into the same container path the user left in, then the cursor
        let path = s.local_open.clone().unwrap_or_default();
        self.local_restore_drill(&path, s.local_sel.unwrap_or(0));
    }

    /// Persist the current session to disk now. Used both on exit and on the few
    /// settings-like state changes (e.g. the per-view visualizer mode) that the
    /// user expects to stick immediately, so they survive a non-graceful stop
    /// (Ctrl-C / crash / a dev rebuild killing the process), not only a clean quit.
    pub fn save_session(&self) {
        self.session().save(&self.config.dir);
    }

    /// Capture full UI/playback state to persist on exit.
    pub fn session(&self) -> crate::session::Session {
        let path_of = |id: TrackId| -> Option<String> {
            self.library
                .track(id)
                .map(|t| t.path.to_string_lossy().into_owned())
        };
        crate::session::Session {
            volume: Some(self.player.volume),
            layout: Some(layout_to_str(self.layout).to_string()),
            focus: Some(focus_to_str(self.focus).to_string()),
            visualizer_modes: Some(
                self.views
                    .viz_modes
                    .iter()
                    .map(|(l, m)| (layout_to_str(*l).to_string(), *m))
                    .collect(),
            ),
            panels: Some(
                self.views
                    .panels
                    .iter()
                    .map(|((l, p), cfg)| {
                        (
                            layout_to_str(*l).to_string(),
                            p.key().to_string(),
                            cfg.shown,
                            cfg.dock.label().to_string(),
                            cfg.size,
                        )
                    })
                    .collect(),
            ),
            pane_lens: Some(
                self.views
                    .panels
                    .iter()
                    .filter(|(_, cfg)| cfg.len != 50) // only non-default shares
                    .map(|((l, p), cfg)| {
                        (layout_to_str(*l).to_string(), p.key().to_string(), cfg.len)
                    })
                    .collect(),
            ),
            grid_sections: Some(
                self.views
                    .grid
                    .iter()
                    .map(|(s, g)| (s.key().to_string(), *g))
                    .collect(),
            ),
            spotify_grid_sections: Some(
                self.views
                    .spotify_grid
                    .iter()
                    .map(|(s, g)| (*s, *g))
                    .collect(),
            ),
            shuffle: Some(self.player.shuffle),
            repeat: Some(repeat_to_str(self.player.repeat).to_string()),
            speed: Some(self.player.speed),
            selection: Some(self.selection),
            queue_sel: Some(self.queue_sel),
            local_section: Some(self.local.section.key().to_string()),
            local_open: Some(self.local_open_path()),
            local_sel: Some(self.local.sel),
            current_path: self.player.current.and_then(path_of),
            elapsed_secs: Some(self.player.elapsed.as_secs()),
            queue_paths: Some(
                self.player
                    .queue
                    .items
                    .iter()
                    .filter_map(|id| path_of(*id))
                    .collect(),
            ),
            // internet radio: filters + the last-tuned station (None if not on radio)
            radio_query: (!self.radio.query.is_empty()).then(|| self.radio.query.clone()),
            radio_country: self.radio.country.clone(),
            radio_tag: self.radio.tag.clone(),
            radio_sort: Some(self.radio.sort.label().to_string()),
            radio_station: self.rnow.now_station.clone(),
            // Spotify: the account this state belongs to (for the reconnect check)
            spotify_account: self.spotify.account_id.clone(),
            // Spotify now-playing + queue (None when nothing is loaded / logged out)
            spotify_now: self.spov.now_spotify.clone(),
            spotify_queue: (!self.spov.sp_queue.is_empty()).then(|| self.spov.sp_queue.clone()),
            spotify_idx: Some(self.spov.sp_idx),
            spotify_pos: Some(self.spov.sp_pos),
            // Spotify queue playback modes (the queue above is saved already-shuffled)
            spotify_shuffle: Some(self.spov.sp_shuffle),
            spotify_repeat: Some(repeat_to_str(self.spov.sp_repeat).to_string()),
            // Spotify browse view (section / cursor / search / drill-in)
            spotify_section: Some(self.spotify.section),
            spotify_sel: Some(self.spotify.sel),
            spotify_query: (!self.spotify.query.is_empty()).then(|| self.spotify.query.clone()),
            spotify_in_search: Some(self.spotify.in_search),
            // persist the drilled-in container as stable primitives (URI + kind +
            // name), not the whole `Item`, so the drill-in survives an `Item` schema
            // change on the next build and can re-open even when it's off-list
            spotify_open: self
                .spotify
                .open_item
                .as_ref()
                .map(|it| crate::session::SpotifyDrill {
                    uri: it.uri.clone(),
                    kind: it.kind,
                    name: it.name.clone(),
                }),
        }
    }

    /// Persist the listening history (called on a debounce and on exit).
    pub fn flush_history(&mut self) {
        self.plays_since_flush = 0;
        crate::library::store::HistoryStore::save(&self.play_history, &self.config.dir);
    }

    /// Populate the library from the on-disk cache at launch (instant; the
    /// background sync reconciles it afterwards).
    pub fn load_cached_library(&mut self, tracks: Vec<Track>) {
        self.set_library(tracks);
    }

    /// Seed an in-memory demo library + queue for tests (snapshot assertions).
    /// No longer used at runtime — a fresh install leaves the library empty and
    /// shows the first-run onboarding instead (see `ui::components::welcome`).
    #[cfg(test)]
    pub fn seed_demo(&mut self) {
        let artist_id = ArtistId::new(1);
        let album_id = AlbumId::new(1);
        let rows: &[(&str, &str, u8, u64)] = &[
            ("Midnight Protocol", "Neon District", 4, 222),
            ("Neon Rain", "Lumina", 3, 238),
            ("Afterglow", "Kashiwa Daisuke", 5, 252),
            ("Velvet Static", "The Midnight", 4, 303),
            ("Parallel", "Carbon Based", 3, 381),
            ("Lucid", "Tycho", 5, 285),
            ("Resonance", "Boards of Canada", 4, 210),
            ("Drift", "Bonobo", 4, 248),
            ("Outrun", "Power Glove", 3, 235),
            ("Submersion", "Maribou State", 4, 273),
            ("Halcyon", "Ben Böhmer", 5, 318),
            ("Phosphor", "Lane 8", 4, 362),
            ("Aurora Bay", "Tycho", 4, 267),
            ("Nightcall", "Kavinsky", 5, 258),
        ];
        let mut track_ids = Vec::new();
        for (i, (title, artist, rating, secs)) in rows.iter().enumerate() {
            let id = TrackId::new(i as u32 + 1);
            track_ids.push(id);
            self.library.tracks.insert(
                id,
                Track {
                    id,
                    path: PathBuf::from(format!("/Music/Synthwave Drive/{title}.flac")),
                    title: (*title).into(),
                    artist: (*artist).into(),
                    album: "Afterglow".into(),
                    album_artist: "Neon District".into(),
                    album_id: Some(album_id),
                    artist_id: Some(artist_id),
                    track_no: i as u16 + 1,
                    disc_no: 1,
                    track_total: rows.len() as u16,
                    disc_total: 1,
                    duration_ms: (*secs as u32) * 1000,
                    year: Some(2025),
                    genre: Some("Synthwave".into()),
                    composer: String::new(),
                    comment: String::new(),
                    audio: Some(AudioInfo {
                        codec: Codec::Flac,
                        sample_rate: 96_000,
                        bit_depth: 24,
                        channels: 2,
                        bitrate_kbps: 2300,
                    }),
                    rating: *rating,
                    favorite: i == 0,
                    play_count: 0,
                    added_at: 0,
                    last_played: 0,
                },
            );
        }
        self.library.albums.insert(
            album_id,
            Album {
                id: album_id,
                title: "Afterglow".into(),
                artist: "Neon District".into(),
                artist_id: Some(artist_id),
                year: Some(2025),
                genre: Some("Synthwave".into()),
                track_ids: track_ids.clone(),
                cover_path: None,
            },
        );
        self.library.artists.insert(
            artist_id,
            Artist {
                id: artist_id,
                name: "Neon District".into(),
                album_ids: vec![album_id],
            },
        );
        self.library.playlists.insert(
            PlaylistId::new(1),
            Playlist {
                id: PlaylistId::new(1),
                name: "Synthwave Drive".into(),
                track_ids: track_ids.clone(),
                query: None,
                folder: None,
            },
        );
        self.library.favorites = vec![TrackId::new(1)];
        self.library.recompute_views(); // build the facet/derived-view caches

        // queue everything; start paused (demo paths aren't real files, so
        // pressing play won't make sound — point lyrfin at a real folder).
        self.player.queue.items = track_ids;
        self.player.queue.position = 0;
        self.player.current = Some(TrackId::new(1));
        self.player.duration = Duration::from_secs(261);
        self.player.elapsed = Duration::from_secs(102);
        self.player.status = Status::Paused;
        self.player.volume = self.config.volume;
        self.local_load_section(); // populate the drill-in browse list
        self.notify("Demo library — run `lyrfin /path/to/music` to load yours".into());
    }
}
