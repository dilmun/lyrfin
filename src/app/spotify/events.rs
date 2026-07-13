//! Spotify worker-event handling on `AppState` (extracted from app/spotify):
//! draining librespot session events (`pump_spotify_session`) and applying
//! Web-API / librespot fetch results (`on_spotify_result`) into browse/now-
//! playing/artist-pane state.

use super::*;

impl AppState {
    /// Fold librespot's own WARN/ERROR log lines (captured by the log probe) into
    /// lyrfin's error log, so the real reason a track failed — region lock, premium
    /// requirement, audio-key refusal — is visible in-app without `RUST_LOG`.
    fn absorb_librespot_logs(&mut self) {
        for line in crate::spotify::logprobe::drain_librespot_log() {
            self.log_error(format!("[librespot] {line}"));
        }
    }

    /// Drain librespot session events (called from `pump_spotify`).
    pub(crate) fn pump_spotify_session(&mut self) {
        self.absorb_librespot_logs();
        let Some(rx) = &self.spov.session_rx else {
            return;
        };
        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        if events.is_empty() {
            return;
        }
        self.dirty = true;
        use crate::spotify::session::SessionEvent::*;
        for ev in events {
            match ev {
                Connected => {
                    // If this is the fresh session spawned by a reconnect-and-retry,
                    // it's now up: a failure from here on is real (the track is
                    // genuinely unavailable), not an echo from the dead session.
                    if self.spov.sp_recovery == SpRecovery::Reconnecting {
                        self.spov.sp_recovery = SpRecovery::Reconnected;
                    }
                }
                TokenRefreshed(tokens) => {
                    // adopt the fresh token so the Web API uses it too
                    self.spotify.tokens = Some(tokens);
                }
                ArtistMeta {
                    key,
                    popularity,
                    bio,
                } => {
                    // librespot's popularity + Spotify's own bio for the artist pane
                    // (the dev-mode shared id strips the follower count, and the bio
                    // is the official right-language text). Merge into the Web result.
                    if key == ARTIST_PANE_KEY {
                        let entry = self.spov.sp_artist.get_or_insert_with(SpArtist::default);
                        if entry.name.is_empty() {
                            entry.name = self
                                .spov
                                .now_spotify
                                .as_ref()
                                .map(|t| t.subtitle.clone())
                                .unwrap_or_default();
                        }
                        entry.popularity = popularity;
                        if !bio.is_empty() {
                            entry.bio = bio;
                        }
                    }
                }
                Tracks { key, items } => {
                    // the artist pane's top-tracks (sentinel key) → its own slot
                    if key == ARTIST_PANE_KEY {
                        self.spov.sp_artist_top = items;
                        continue;
                    }
                    // metadata-fetched playlist tracks
                    if self.spotify.key == key {
                        self.spotify.loading = false;
                        self.spotify.note = if items.is_empty() {
                            "Nothing to play here".into()
                        } else {
                            String::new()
                        };
                        self.spotify.items = items;
                        // Keep the cursor where it is (clamped) rather than snapping to
                        // the top. A fresh drill-in already parked it at 0
                        // (`spotify_open`), so this is a no-op there; an in-place reload
                        // after removing a track keeps the selection put — the track
                        // below slides up under the cursor (last row → the new last).
                        self.spotify.sel = self
                            .spotify
                            .sel
                            .min(self.spotify.items.len().saturating_sub(1));
                    }
                }
                PlaylistUris { key, uris, ok } => {
                    // the raw track list for a remove-a-track (see spotify_apply_remove)
                    if key == super::playlist::REMOVE_FETCH_KEY {
                        self.spotify_apply_remove(uris, ok);
                    }
                }
                ArtistPage { key, items } => {
                    // a grouped artist page (popular + albums + singles): items carry
                    // their own Group tags, so the view renders the sections
                    if self.spotify.key == key {
                        self.spotify.loading = false;
                        self.spotify.note = if items.is_empty() {
                            "Nothing here".into()
                        } else {
                            String::new()
                        };
                        self.spotify.items = items;
                        self.spotify.sel = 0;
                    }
                }
                EpisodeResolved {
                    uri,
                    url,
                    position_ms,
                } => {
                    // ignore if the user moved to a different track meanwhile
                    if self.spov.now_spotify.as_ref().map(|t| t.uri.as_str()) != Some(uri.as_str())
                    {
                        continue;
                    }
                    let _ = position_ms; // streams start at 0 (no seek into a stream)
                    match url {
                        // librespot exposed an external MP3 → stream it directly
                        Some(url) => self.spotify_stream_episode(url),
                        // Spotify-hosted/DRM'd: librespot can't decode it, so match
                        // the episode to its public RSS feed and stream that MP3
                        // instead. Stays "buffering…" until on_podcast_result (the
                        // watchdog backstops if it never resolves).
                        None => {
                            if !self.spotify_request_podcast() {
                                self.spotify_episode_unplayable();
                            }
                        }
                    }
                }
                ConnectError(msg) => {
                    // the session died → drop it so the next play respawns. KEEP the
                    // now-playing overlay + artist pane on screen (a failure must not
                    // blank the UI); just pause it, free the engine, and report below.
                    // A reconnect-and-retry whose fresh session couldn't even connect
                    // ends here — clear the recovery state so it isn't left mid-flight.
                    self.spov.sp_recovery = SpRecovery::Normal;
                    self.spov.session_cmd = None;
                    self.spov.session_rx = None;
                    self.spov.spotify_paused = true;
                    self.spov.sp_started = false;
                    self.engine.send(AudioCommand::ClearExternalSource);
                    if self.spotify.loading {
                        self.spotify.loading = false;
                        self.spotify.note = "Couldn't reach Spotify".into();
                    }
                    // back off before the next connect so a failing session can't
                    // reconnect in a tight loop (and trip Spotify's rate-limit).
                    // Log it (not just a toast) — this `msg` is the actual reason
                    // (token refresh / login failure / timeout / rate-limit), so it
                    // belongs in the error log the user inspects when nothing plays.
                    self.spotify_trip_cooldown();
                    self.notify_error(format!("Spotify playback: {msg}"));
                }
                // These events only track librespot's position + whether playback
                // has begun. They deliberately DON'T touch `spotify_paused`: that's
                // the user's intent, set synchronously in `toggle_play` (alongside
                // the librespot + engine commands). These events lag and can arrive
                // out of order, so letting them write the pause flag desynced the
                // icon/visualizer from the real state on rapid space presses.
                Playing { position_ms } => {
                    log::info!(target: "lyrfin::spotify", "librespot Playing @ {position_ms}ms");
                    self.spov.sp_started = true;
                    // A track actually played → this account is provably NOT audio-key
                    // blocked at the account level, so any earlier "blocked" verdict was
                    // a transient blip: clear the block evidence (and the sticky probe
                    // flag) so recovery, the header, and the failure hint all reset.
                    self.spov.sp_played_ok = true;
                    self.spov.sp_key_denials = 0;
                    self.spov.sp_resume_at = None; // playing → cancel any pending auto-resume
                    crate::spotify::logprobe::AUDIO_KEY_BLOCKED
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                    self.spotify_clear_cooldown(); // a track actually played → reset back-off
                    self.spov.sp_keyretry_at = None; // played → drop any pending key-retry
                    self.spov.sp_keyretry_n = 0;
                    self.spov.sp_recovery = SpRecovery::Normal; // recovered / healthy again
                    self.spov.sp_pos = position_ms as f64 / 1000.0;
                    // the current track is underway → prefetch the next so its
                    // transition is gapless (max lead time, no fetch on EndOfTrack).
                    self.spotify_preload_next();
                }
                Paused { position_ms } => {
                    self.spov.sp_pos = position_ms as f64 / 1000.0;
                }
                EndOfTrack => {
                    log::info!(
                        target: "lyrfin::spotify",
                        "librespot EndOfTrack (started={}, uri={})",
                        self.spov.sp_started,
                        self.spov.now_spotify.as_ref().map(|t| t.uri.as_str()).unwrap_or("-")
                    );
                    // a track that actually played → let spotify_track_ended decide
                    // (a genuine end advances; a mid-track stall re-buffers in place —
                    // librespot's EndOfTrack is overloaded); one that never started →
                    // count it as a failed load (guarded below)
                    if self.spov.sp_started {
                        self.spotify_track_ended();
                    } else {
                        self.spotify_load_failed();
                    }
                }
                Unavailable => {
                    log::warn!(
                        target: "lyrfin::spotify",
                        "librespot Unavailable (uri={})",
                        self.spov.now_spotify.as_ref().map(|t| t.uri.as_str()).unwrap_or("-")
                    );
                    self.spotify_load_failed();
                }
                AudioKeyDenied => {
                    log::warn!(
                        target: "lyrfin::spotify",
                        "librespot AudioKeyDenied/DRM (uri={})",
                        self.spov.now_spotify.as_ref().map(|t| t.uri.as_str()).unwrap_or("-")
                    );
                    self.spotify_playback_blocked();
                }
                Browse { key, items, error } => {
                    // home/browse feed from pathfinder GraphQL (keyed like Tracks)
                    if self.spotify.key == key {
                        self.spotify.loading = false;
                        match error {
                            Some(err) => {
                                log::warn!(target: "lyrfin::spotify", "browse failed: {err}");
                                self.spotify.items.clear();
                                self.spotify.note =
                                    "Browse unavailable (Spotify API changed)".into();
                            }
                            None if self.spotify.browse_loading_more => {
                                // a scroll-triggered "load more": the same page re-fetched
                                // with a bigger limit. Keep the cursor where it is and just
                                // extend the grid; stop paging once it stops growing.
                                self.spotify.browse_loading_more = false;
                                self.spotify.browse_exhausted = items.len()
                                    <= self.spotify.items.len()
                                    || items.len() < self.spotify.browse_limit;
                                self.spotify.items = items;
                                let n = self.spotify.items.len();
                                self.spotify.sel = self.spotify.sel.min(n.saturating_sub(1));
                                self.spotify.note = String::new();
                            }
                            None => {
                                // A region without podcasts returns an empty hub — or
                                // just the lone "Browse all categories" link — which
                                // reads as a broken stray card. Treat that as empty and
                                // show a clear note instead.
                                let only_link = !items.is_empty()
                                    && items.iter().all(|i| {
                                        i.name == crate::spotify::pathfinder::ALL_CATEGORIES_LABEL
                                    });
                                if items.is_empty() || only_link {
                                    self.spotify.items.clear();
                                    self.spotify.note = if self.spotify.section
                                        == crate::spotify::api::Section::Podcasts
                                    {
                                        "No podcasts to browse here — they may not be available in your region."
                                            .into()
                                    } else {
                                        "Nothing here".into()
                                    };
                                } else {
                                    self.spotify.note = String::new();
                                    self.spotify.items = items;
                                    // re-open a session-restored drill-in (the Home/Browse
                                    // feed lands here, not via the Web-API `Library`
                                    // result, so it needs the same restore hook or a chart
                                    // drilled from Browse is dropped on reopen) — else
                                    // place the cursor at the top.
                                    self.spotify_apply_initial_restore();
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Apply a Spotify Web API result (ignoring stale keys).
    pub fn on_spotify_result(&mut self, res: crate::spotify::api::SpResult) {
        use crate::spotify::api::SpResult::*;
        match res {
            Library { key, items } => {
                if self.spotify.key != key {
                    return;
                }
                self.spotify.loading = false;
                let mut items = items;
                // The Podcasts tab doubles as the podcast hub: lead the saved-shows
                // list with an entry into Spotify's editorial Top-Podcasts + categories
                // browse page (pathfinder). It's a normal `Kind::Category`, so ⏎/click
                // drills in via the very same path as the music Browse grid — no special
                // handling downstream. Only the top-level section list lands here
                // (drilling a show goes through `Open`), so it's added exactly once.
                if self.spotify.section == crate::spotify::api::Section::Podcasts {
                    // these saved shows ARE the user's follows — cache their URIs so
                    // browse rows/cards can mark an already-followed show with a ♥
                    self.spotify.followed_shows = items
                        .iter()
                        .filter(|i| i.kind == crate::spotify::api::Kind::Show)
                        .map(|i| i.uri.clone())
                        .collect();
                    items.insert(
                        0,
                        crate::spotify::api::Item {
                            name: "Browse Top Podcasts".into(),
                            subtitle: "Charts, trending & categories".into(),
                            kind: crate::spotify::api::Kind::Category,
                            uri: crate::spotify::pathfinder::PODCASTS_BROWSE_ROOT.to_string(),
                            ..Default::default()
                        },
                    );
                }
                self.spotify.note = if items.is_empty() {
                    "Nothing here yet".into()
                } else {
                    String::new()
                };
                self.spotify.items = items;
                self.spotify_apply_initial_restore();
            }
            Search {
                key,
                tracks,
                albums,
                artists,
                playlists,
                shows,
            } => {
                if self.spotify.key != key {
                    return;
                }
                self.spotify.loading = false;
                let mut items = tracks;
                items.extend(albums);
                items.extend(artists);
                items.extend(playlists);
                items.extend(shows); // podcast shows group under PODCASTS in results
                self.spotify.note = if items.is_empty() {
                    "No results".into()
                } else {
                    String::new()
                };
                self.spotify.items = items;
                self.spotify_apply_initial_restore();
            }
            Opened { key, items } => {
                if self.spotify.key != key {
                    return;
                }
                self.spotify.loading = false;
                self.spotify.note = if items.is_empty() {
                    "Nothing to play here".into()
                } else {
                    String::new()
                };
                self.spotify.items = items;
                self.spotify.sel = self.spotify_restored_sel();
            }
            Saved { uri, saved } => {
                // only adopt it if it's still the playing track
                if self.spov.now_spotify.as_ref().map(|t| t.uri.as_str()) == Some(uri.as_str()) {
                    self.spov.sp_saved = saved;
                }
            }
            Follow { uri, followed } => {
                // keep the followed-show cache live so the ♥ browse markers update
                // instantly, before any list refresh
                if followed {
                    self.spotify.followed_shows.insert(uri.clone());
                } else {
                    self.spotify.followed_shows.remove(&uri);
                }
                // name the show/artist from the pending request the worker echoed
                let name = match self.spov.sp_follow_pending.take() {
                    Some((u, n)) if u == uri => n,
                    other => {
                        self.spov.sp_follow_pending = other; // a different toggle is pending
                        String::new()
                    }
                };
                let label = if name.is_empty() {
                    "this"
                } else {
                    name.as_str()
                };
                self.notify(if followed {
                    format!("✓ Following {label}")
                } else {
                    format!("Unfollowed {label}")
                });
                // if we're viewing the followed-shows/artists list, reflect the change
                if matches!(
                    self.spotify.section,
                    crate::spotify::api::Section::Podcasts | crate::spotify::api::Section::Artists
                ) {
                    self.spotify_load_section();
                }
            }
            ShowResolved { uri, name } => match uri {
                // the Artist pane's async open for an episode without a `show_uri`:
                // the show is now known → drill into it, exactly like the direct path
                Some(uri) => {
                    self.open_spotify_container(uri, name, crate::spotify::api::Kind::Show)
                }
                None => self.notify("Couldn't find this show on Spotify".into()),
            },
            Artist {
                uri,
                name,
                image,
                genres,
                followers,
            } => {
                // ignore if the now-playing artist changed since we asked
                if self.spov.sp_artist_uri.as_deref() != Some(uri.as_str()) {
                    return;
                }
                // merge into whatever librespot's ArtistMeta may have already filled
                // (genres/popularity) — the Web API supplies the name + follower count
                let entry = self.spov.sp_artist.get_or_insert_with(SpArtist::default);
                entry.name = name;
                entry.followers = followers;
                if !genres.is_empty() {
                    entry.genres = genres; // dev-mode strips these; keep librespot's
                }
                // fetch the artist photo via the artwork worker (→ on_spotify_art);
                // masked into a circle (Spotify-style round avatar) with transparent
                // corners, so it blends into whatever panel the pane sits on
                self.spov.sp_artist_cover = None;
                self.spov.sp_artist_cover_url = image.clone();
                if let (Some(url), Some(tx)) = (image.as_ref(), self.workers.spotify_art.as_ref()) {
                    let _ = tx.send(crate::spotify::artwork::ArtRequest {
                        url: url.clone(),
                        circle: true,
                    });
                }
            }
            ShowMeta {
                uri,
                publisher,
                description,
            } => {
                // ignore if the now-playing episode's show changed since we asked
                if self
                    .spov
                    .now_spotify
                    .as_ref()
                    .and_then(|t| t.show_uri.as_deref())
                    != Some(uri.as_str())
                {
                    return;
                }
                self.spov.sp_show_meta = Some(crate::app::spotify::ShowMeta {
                    uri,
                    publisher,
                    description,
                });
                self.dirty = true;
            }
            MyPlaylists { key, items, error } => self.on_spotify_my_playlists(key, items, error),
            PlaylistWrite { op, ok, msg } => self.on_spotify_playlist_write(op, ok, msg),
            Unauthorized { key } => {
                // key "" = a control action (like/save) hit a stale token; list
                // loads carry a key. Either way, refresh once (if not already).
                if !key.is_empty() && self.spotify.key != key {
                    return;
                }
                self.spotify.loading = false;
                if self.spotify.auth_rx.is_none()
                    && let Some(tokens) = self.spotify.tokens.clone()
                {
                    self.spotify.note = "Refreshing session…".into();
                    self.spotify.auth_rx = Some(crate::spotify::spawn_resume(
                        self.config.dir.clone(),
                        tokens,
                    ));
                }
            }
            Error { key, msg } => {
                // an empty key marks a control action (like/save) — surface it as
                // a toast, not a list note; a real load error replaces the note.
                if key.is_empty() {
                    self.notify_error(msg);
                    // the like/save was optimistic — it failed, so the ♥ is now
                    // lying. Reconcile it with Spotify's real state (a failed check
                    // falls back to "not saved", clearing the optimistic heart).
                    if let Some(track) = self.spov.now_spotify.clone() {
                        self.spotify_check_saved(&track);
                    }
                } else if self.spotify.key == key {
                    self.spotify.loading = false;
                    self.spotify.note = msg;
                }
            }
        }
    }
}
