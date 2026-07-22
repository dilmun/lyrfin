//! Spotify playback / transport methods on `AppState` (extracted from
//! app/spotify): play/begin/resume, seek, advance + shuffle/repeat, the
//! engine bridge attach/release, the failure cooldown, externally-hosted
//! podcast-episode streaming, and the now-bar position clock + lyrics.

use super::*;

/// Quick same-session retries for a throttled audio key before escalating to the
/// reconnect + back-off path. Two is enough to ride out a rapid-skip throttle while
/// still surfacing a *genuine* DRM/unavailable block within a couple of seconds.
const SP_KEY_RETRY_MAX: u8 = 2;
/// How many tracks must fail their audio key — with **no** track ever having played
/// this session — before lyrfin concludes it's Spotify's account-level key block (which
/// no librespot client can work around) rather than a transient CDN/throttle blip.
/// A single mid-playback denial (the common "bad CDN node" case) never reaches this:
/// either a track already played (see `sp_played_ok`) or the count is still low, so
/// the reconnect-and-replay recovery keeps trying instead of giving up. See
/// [`AppState::spotify_key_block_confirmed`].
const SP_KEY_BLOCK_CONFIRM: u16 = 3;
/// Delay before a quick key-retry: long enough for Spotify's audio-key throttle to
/// clear, short enough to read as ordinary buffering rather than a stall.
const SP_KEY_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(900);
/// How long a manual skip waits for the next press before loading. Longer than a
/// fast tap / key-repeat gap (so a burst coalesces into one load), far shorter than
/// the ~1–2s librespot buffer (so a single deliberate skip feels immediate).
const SP_SKIP_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(250);
/// How long a streamed-episode seek waits for scrubbing to settle before the engine
/// re-opens the stream — so holding `,`/`.` coalesces into one ranged re-open instead of
/// a burst that stalls the stream. Short enough that a single seek feels immediate.
const SP_SEEK_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(160);
/// A seek counts as a "continued hold" (ramping the step) when the next press lands
/// within this window; a longer gap resets the acceleration.
const SP_SEEK_STREAK_WINDOW: std::time::Duration = std::time::Duration::from_millis(400);
/// Cap on the seek-acceleration streak, bounding how large the step can grow.
const SP_SEEK_STREAK_MAX: u32 = 40;
/// In-place re-buffers of a track that stalled mid-play before giving up and
/// skipping. Enough to ride out a burst of network congestion (each retry re-runs
/// librespot's whole fetch, which usually clears a transient stall) while still
/// moving on from a segment that genuinely won't stream.
const SP_STALL_RETRY_MAX: u8 = 3;
/// Delay before an in-place re-buffer: long enough for transient congestion to ease
/// (librespot only reports the stall *after* exhausting its own fetch deadline), and
/// it reads as ordinary buffering rather than a stutter.
const SP_STALL_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(700);
/// Forward progress (seconds) past the last stall point that refills the stall-retry
/// budget: a genuinely separate hiccup deserves fresh retries, but a corrupt segment
/// that re-stalls within this margin keeps depleting the budget so it eventually
/// skips instead of rebuffering the same spot forever. Comfortably larger than the
/// few seconds the wall-clock over-counts during a stall.
const SP_STALL_PROGRESS: f64 = 10.0;
/// Seconds without any engine `Progress` before the streamed-episode (podcast)
/// watchdog treats the HTTP stream as stalled and re-opens it. Well above the
/// normal 2–4 s buffer so it never fires during ordinary buffering.
const SP_STREAM_STALL_SECS: u64 = 12;
/// Slack below the track's full duration within which an `EndOfTrack` counts as a
/// genuine end rather than a mid-track stall. librespot's `EndOfTrack` is overloaded
/// (a real end AND a "couldn't fetch/decode the next packet" abort), and lyrfin's
/// position clock free-runs on wall time between librespot's sparse play/pause
/// anchors — and keeps ticking through a stall — so at a true end it can sit a little
/// under the real duration. A few seconds absorbs that without mistaking a mid-track
/// stall (tens of seconds to minutes short) for an ending.
const SP_END_SLACK: f64 = 6.0;

impl AppState {
    /// Whether `uri` is a podcast episode (`spotify:episode:…`).
    pub(crate) fn is_episode_uri(uri: &str) -> bool {
        uri.contains(":episode:")
    }

    /// Point the engine at the librespot bridge so its PCM feeds the shared output.
    /// Called when librespot playback *starts* — not when the session is spawned —
    /// so metadata-only sessions (artist page / playlist tracks) never grab the
    /// shared audio output away from local/radio playback.
    fn spotify_attach_bridge(&mut self) {
        if let Some(bridge) = self.spov.sp_bridge.clone() {
            self.engine.send(AudioCommand::SetExternalSource(bridge));
        }
        self.spov.sp_stream = false;
    }

    /// After a streamed episode (which took the engine over via `LoadStream`), switch
    /// back to the librespot bridge. No-op when no stream was active.
    fn spotify_reattach_bridge(&mut self) {
        if self.spov.sp_stream {
            self.spotify_attach_bridge();
        }
    }

    /// Start playing a Spotify queue at `idx`. Overlay model: the local player is
    /// preserved (paused), any radio overlay stopped.
    pub(crate) fn spotify_play(&mut self, queue: Vec<crate::spotify::api::Item>, idx: usize) {
        if !self.spotify_playback_allowed() || self.spotify_cooldown_active() {
            return;
        }
        if queue.is_empty() {
            return;
        }
        if !self.spotify_ensure_session() {
            // false ⇒ no token (log in) or a login/resume is mid-flight (auth_rx) —
            // tell the user it's connecting rather than appearing to do nothing
            self.notify(
                if self.spotify.tokens.is_none() {
                    "Log in to Spotify first"
                } else {
                    "Connecting to Spotify — try again in a moment."
                }
                .into(),
            );
            return;
        }
        let idx = idx.min(queue.len() - 1);
        let track = queue[idx].clone();
        // stop other overlays; preserve the local player
        self.rnow.now_station = None;
        self.rnow.now_station_title = None;
        self.rnow.radio_paused = false;
        if self.player.status == Status::Playing {
            self.player.status = Status::Paused;
        }
        self.spov.sp_dur = track.duration_ms as f64 / 1000.0;
        self.spov.sp_pos = 0.0;
        // a fresh track cancels any pending scrub seek + its acceleration
        self.spov.sp_seek_at = None;
        self.spov.sp_seek_target = None;
        self.spov.sp_seek_streak = 0;
        self.spov.spotify_paused = false;
        self.spov.sp_started = false; // set true when librespot reports Playing
        self.spov.sp_resume_at = None; // an explicit play supersedes a pending auto-resume
        self.spotify_reset_retries(); // fresh track → fresh throttle/stall-retry budget
        self.spotify_clear_pending_skip(); // this load supersedes any debounced skip
        self.spov.now_spotify = Some(track.clone());
        self.spotify_load_art(&track);
        self.spotify_load_artist();
        self.load_spotify_lyrics();
        self.spotify_check_saved(&track);
        self.spov.sp_queue = queue;
        self.spov.sp_idx = idx;
        // Follow the now-playing track in the QUEUE pane: a focused pane scrolls
        // to the cursor, so without this an auto-advance (esp. the repeat-all wrap
        // back to the top) would leave the highlight stranded on the old row while
        // a different track plays. Browsing the pane with j/k moves the cursor
        // between track changes; it re-syncs here the moment a track actually starts.
        self.spotify.queue_sel = idx;
        self.spotify_begin(&track.uri, 0);
        self.notify(format!("Spotify: {}", track.name));
    }

    /// Begin playback of `uri` at `position_ms`. Podcast episodes are resolved
    /// first (most are externally hosted and stream outside librespot — see
    /// [`crate::spotify::session::SessionCommand::ResolveEpisode`]); everything else
    /// loads through librespot, re-pointing the engine at the bridge in case a
    /// previous streamed episode had taken it over.
    fn spotify_begin(&mut self, uri: &str, position_ms: u32) {
        let Some(cmd) = self.spov.session_cmd.clone() else {
            return;
        };
        if Self::is_episode_uri(uri) {
            let _ = cmd.send(crate::spotify::session::SessionCommand::ResolveEpisode {
                uri: uri.to_string(),
                position_ms,
            });
            return; // the engine source is set when EpisodeResolved arrives
        }
        log::info!(target: "lyrfin::spotify", "Load uri={uri} pos={position_ms}ms");
        self.spotify_attach_bridge(); // own the shared engine output for librespot playback
        let _ = cmd.send(crate::spotify::session::SessionCommand::Load {
            uri: uri.to_string(),
            position_ms,
        });
        // re-enable the engine output (a prior pause left `playing` false)
        self.engine.send(AudioCommand::Play);
    }

    /// Resume the shown now-playing track when it isn't actually streaming yet
    /// (restored from a session, or never started): (re)load it at the saved
    /// position. Mirrors how a restored radio station re-tunes on space.
    pub(crate) fn spotify_resume(&mut self) {
        let Some(track) = self.spov.now_spotify.clone() else {
            return;
        };
        if !self.spotify_playback_allowed() || self.spotify_cooldown_active() {
            return;
        }
        if !self.spotify_ensure_session() {
            self.notify(
                if self.spotify.tokens.is_none() {
                    "Log in to Spotify first"
                } else {
                    "Connecting to Spotify — try again in a moment."
                }
                .into(),
            );
            return;
        }
        let pos_ms = (self.spov.sp_pos.max(0.0) * 1000.0) as u32;
        self.spov.spotify_paused = false;
        self.spov.sp_started = false; // set true when librespot reports Playing
        self.spotify_reset_retries(); // fresh user action → fresh throttle/stall-retry budget
        self.spotify_load_art(&track);
        self.spotify_load_artist();
        self.load_spotify_lyrics();
        self.spotify_check_saved(&track);
        self.spotify_begin(&track.uri, pos_ms);
        self.notify(format!("Spotify: {}", track.name));
    }

    /// Ask whether `track` is in Liked Songs (drives the ♥). Optimistically clears
    /// until the answer arrives.
    pub(crate) fn spotify_check_saved(&mut self, track: &crate::spotify::api::Item) {
        self.spov.sp_saved = false;
        if let (Some(tokens), Some(tx)) =
            (self.spotify.tokens.as_ref(), self.workers.spotify.as_ref())
        {
            let _ = tx.send(crate::spotify::api::SpRequest::CheckSaved {
                uri: track.uri.clone(),
                token: tokens.access_token.clone(),
            });
        }
    }

    /// Toggle the now-playing Spotify track in/out of Liked Songs (`f`). Optimistic
    /// UI; a failure reverts via the echoed result / error toast.
    pub(crate) fn spotify_toggle_saved(&mut self) {
        let Some(track) = self.spov.now_spotify.clone() else {
            return;
        };
        let want = !self.spov.sp_saved;
        self.spov.sp_saved = want; // optimistic
        if let (Some(tokens), Some(tx)) =
            (self.spotify.tokens.as_ref(), self.workers.spotify.as_ref())
        {
            let _ = tx.send(crate::spotify::api::SpRequest::SetSaved {
                uri: track.uri,
                saved: want,
                token: tokens.access_token.clone(),
            });
        }
        self.notify(if want {
            "♥ Added to Liked Songs".into()
        } else {
            "Removed from Liked Songs".into()
        });
    }

    /// `f`: like the selection. With nothing multi-selected this is the single-track
    /// now-playing toggle above; with a marked set / visual range it *adds* every
    /// selected track to Liked Songs (bulk like is add-only — we don't track each
    /// row's saved state).
    pub(crate) fn spotify_like_selection(&mut self) {
        if self.marks.ids.is_empty() && self.marks.anchor.is_none() {
            self.spotify_toggle_saved();
            return;
        }
        let uris = self.selected_spotify_uris();
        if uris.is_empty() {
            return;
        }
        if let (Some(tokens), Some(tx)) =
            (self.spotify.tokens.as_ref(), self.workers.spotify.as_ref())
        {
            let token = tokens.access_token.clone();
            for uri in &uris {
                let _ = tx.send(crate::spotify::api::SpRequest::SetSaved {
                    uri: uri.clone(),
                    saved: true,
                    token: token.clone(),
                });
            }
            if self
                .spov
                .now_spotify
                .as_ref()
                .is_some_and(|t| uris.contains(&t.uri))
            {
                self.spov.sp_saved = true;
            }
        }
        let noun = if uris.len() == 1 { "track" } else { "tracks" };
        self.notify(format!("♥ Added {} {noun} to Liked Songs", uris.len()));
        self.clear_marks();
    }

    /// The `(uri, kind, display name)` to follow for the selected Spotify row: a Show
    /// or Artist directly, or a track's primary artist. `None` for anything that isn't
    /// followable (a track with no artist uri, a playlist, an empty list).
    pub(crate) fn spotify_follow_target(
        &self,
    ) -> Option<(String, crate::spotify::api::Kind, String)> {
        use crate::spotify::api::Kind;
        // 1. the selected list item, when it's followable
        if let Some(item) = self.spotify.items.get(self.spotify.sel) {
            match item.kind {
                Kind::Show => return Some((item.uri.clone(), Kind::Show, item.name.clone())),
                Kind::Artist => return Some((item.uri.clone(), Kind::Artist, item.name.clone())),
                // a track row → follow its artist (the row shows the artist as subtitle)
                Kind::Track => {
                    if let Some(u) = item.artist_uri.clone() {
                        return Some((u, Kind::Artist, item.primary_artist().to_string()));
                    }
                }
                _ => {}
            }
        }
        // 2. otherwise follow the NOW-PLAYING item's owner (e.g. the cursor is on the
        //    artist pane, or on a non-followable row): an episode → its show, a music
        //    track → its primary artist.
        let tr = self.spov.now_spotify.as_ref()?;
        if Self::is_episode_uri(&tr.uri) {
            tr.show_uri
                .clone()
                .map(|u| (u, Kind::Show, tr.album.clone()))
        } else {
            tr.artist_uri
                .clone()
                .map(|u| (u, Kind::Artist, tr.primary_artist().to_string()))
        }
    }

    /// Follow/unfollow the selected show or artist (`F`). The worker checks the
    /// current state and flips it (so we never guess the direction); its result toast
    /// names the target. A no-op with a hint when nothing followable is selected or
    /// the account isn't connected to the Web API.
    pub(crate) fn spotify_toggle_follow(&mut self) {
        let Some((uri, kind, name)) = self.spotify_follow_target() else {
            self.notify("Select a show or artist to follow".into());
            return;
        };
        let Some((token, tx)) = self.spotify_worker() else {
            self.notify("Connect Spotify to follow shows and artists".into());
            return;
        };
        self.spov.sp_follow_pending = Some((uri.clone(), name));
        let _ = tx.send(crate::spotify::api::SpRequest::ToggleFollow { uri, kind, token });
    }

    /// Seek the current Spotify track to a fraction of its length (mouse click /
    /// drag on the progress bar).
    pub(crate) fn spotify_seek_to_fraction(&mut self, frac: f32) {
        if self.spov.now_spotify.is_none() || !self.spov.sp_started || self.spov.sp_dur <= 0.0 {
            return;
        }
        let pos = (self.spov.sp_dur * frac.clamp(0.0, 1.0) as f64).clamp(0.0, self.spov.sp_dur);
        self.spov.sp_pos = pos;
        self.spotify_seek_engine_or_librespot(pos);
    }

    /// Seek the current Spotify track (`,`/`.`). No-op until playback has actually
    /// started (can't seek mid-buffer). The `delta` sign gives the direction; the
    /// magnitude is lyrfin's own **accelerating** step (grows while `,`/`.` is held, scaled
    /// to the episode length) so scrubbing a multi-hour podcast isn't a 5s-at-a-time
    /// crawl.
    pub(crate) fn spotify_seek(&mut self, delta: i64) {
        if self.spov.now_spotify.is_none() || !self.spov.sp_started {
            return;
        }
        // accelerate a held seek: consecutive presses within a short window ramp the
        // streak up; a gap resets it. The step scales with the streak AND the episode
        // length, capped, so long podcasts scrub in usefully large — but bounded — jumps.
        let now = std::time::Instant::now();
        let held = self
            .spov
            .sp_seek_streak_at
            .is_some_and(|t| now.duration_since(t) < SP_SEEK_STREAK_WINDOW);
        self.spov.sp_seek_streak = if held {
            (self.spov.sp_seek_streak + 1).min(SP_SEEK_STREAK_MAX)
        } else {
            0
        };
        self.spov.sp_seek_streak_at = Some(now);

        let dur = self.spov.sp_dur.max(1.0);
        let base = (dur * 0.0025).clamp(5.0, 60.0); // ~0.25% of length, 5–60s
        let cap = (dur * 0.005).clamp(base, 600.0); // up to ~0.5% of length
        let step = (base * (1 + self.spov.sp_seek_streak) as f64).min(cap);

        let new = (self.spov.sp_pos + delta.signum() as f64 * step).clamp(0.0, dur);
        self.spov.sp_pos = new;
        self.spotify_seek_engine_or_librespot(new);
    }

    /// Send a seek to `secs`: a streamed episode scrubs via lyrfin's engine (ranged
    /// HTTP re-open); a librespot track seeks via the session.
    ///
    /// The streamed re-open is **debounced** ([`SP_SEEK_DEBOUNCE`]): the bar (`sp_pos`)
    /// has already moved, but the actual re-open is deferred so a held `,`/`.` coalesces
    /// into one ranged request instead of a burst that queues dozens of re-buffers and
    /// stalls the stream. [`Self::spotify_tick_seek`] fires it once scrubbing settles.
    /// A librespot seek is cheap (no re-buffer), so it fires immediately.
    fn spotify_seek_engine_or_librespot(&mut self, secs: f64) {
        if self.spov.sp_stream {
            // lock the bar to the target (so the engine's lagging progress can't pull
            // it back) and defer the ranged re-open until scrubbing settles
            self.spov.sp_seek_target = Some(secs);
            self.spov.sp_seek_at = Some(std::time::Instant::now() + SP_SEEK_DEBOUNCE);
        } else if let Some(cmd) = &self.spov.session_cmd {
            let _ = cmd.send(crate::spotify::session::SessionCommand::Seek(
                (secs * 1000.0) as u32,
            ));
        }
    }

    /// Fire a debounced streamed-episode seek once scrubbing (holding `,`/`.`) stops:
    /// re-open the stream at the settled bar position. Driven by `pump_spotify` each
    /// frame. Coalescing the burst means one ranged re-open instead of dozens.
    pub(super) fn spotify_tick_seek(&mut self) {
        let Some(at) = self.spov.sp_seek_at else {
            return;
        };
        if std::time::Instant::now() < at {
            return;
        }
        self.spov.sp_seek_at = None;
        // re-open the stream at the settled target; the bar stays LOCKED to it (via
        // `sp_seek_target`) until the engine's progress reaches it, so it can't jump
        // back to the old position while the re-open is in flight.
        if let Some(target) = self.spov.sp_seek_target {
            self.engine
                .send(AudioCommand::Seek(std::time::Duration::from_secs_f64(
                    target.max(0.0),
                )));
        }
    }

    /// Skip to the next/previous queue track (n/p / transport click). The load is
    /// **debounced** ([`SP_SKIP_DEBOUNCE`]): hammering next accumulates the landing
    /// index and previews it immediately (queue cursor + toast) but only loads the
    /// track finally landed on — so a burst of skips fires a single audio-key request
    /// instead of one per intermediate track, which is what trips Spotify's key
    /// throttle. [`Self::spotify_tick_skip`] fires the actual load once skipping stops.
    pub(crate) fn spotify_track(&mut self, delta: i32) {
        let n = self.spov.sp_queue.len();
        if n == 0 {
            return;
        }
        // Accumulate from the pending target if one is mid-debounce, so repeated
        // presses keep advancing from where the last landed (not the playing track).
        let base = self.spov.sp_skip_target.unwrap_or(self.spov.sp_idx);
        let target = (base as i32 + delta).rem_euclid(n as i32) as usize;
        self.spov.sp_skip_target = Some(target);
        self.spov.sp_skip_at = Some(std::time::Instant::now() + SP_SKIP_DEBOUNCE);
        // Instant feedback while the load waits: preview the landing row + its name.
        self.spotify.queue_sel = target;
        if let Some(t) = self.spov.sp_queue.get(target) {
            self.notify(format!("▶ {}", t.name));
        }
        self.dirty = true;
    }

    /// Fire a debounced manual skip once the user stops pressing next/prev: load the
    /// track finally landed on. Driven by `pump_spotify` every frame. Coalescing the
    /// burst means only this one track requests an audio key, so rapid skipping can't
    /// trip Spotify's key-rate throttle in the first place.
    pub(super) fn spotify_tick_skip(&mut self) {
        let Some(at) = self.spov.sp_skip_at else {
            return;
        };
        if std::time::Instant::now() < at {
            return;
        }
        self.spov.sp_skip_at = None;
        let Some(target) = self.spov.sp_skip_target.take() else {
            return;
        };
        if self.spov.sp_queue.is_empty() {
            return;
        }
        let q = self.spov.sp_queue.clone();
        self.spov.sp_fail_streak = 0; // user-initiated skip → fresh failure budget
        self.spov.sp_recovery = SpRecovery::Normal;
        self.spotify_play(q, target);
    }

    /// Drop any pending debounced skip (a direct play supersedes it, or the overlay
    /// was torn down). Keeps a stale target from loading after the fact.
    fn spotify_clear_pending_skip(&mut self) {
        self.spov.sp_skip_target = None;
        self.spov.sp_skip_at = None;
    }

    /// Recover from a dropped librespot connection by replaying the current track on
    /// a fresh session. Spotify can close a session's access-point connection at any
    /// time; librespot's bare `Session` can't be reused afterward and emits no event
    /// for it, so the first sign is a track failing to load. Rather than dead-ending
    /// in a back-off + "re-authenticate" prompt (which only a full app restart
    /// cleared), drop the dead session so [`Self::spotify_play`] respawns a live one,
    /// and replay the same track — the in-process equivalent of that restart.
    ///
    /// Fires at most once per track (guarded by [`SpRecovery`]); if the *fresh*
    /// session also fails the track it's genuinely unavailable, and the caller falls
    /// through to the normal skip / back-off. Returns true when a reconnect was
    /// kicked off, so the caller stops.
    fn spotify_try_reconnect_retry(&mut self) -> bool {
        // A *genuine* account-level audio-key block can't be reconnected around — a
        // fresh session hits the same refusal, and the extra connect churn is exactly
        // what aggravates a rate-limit. But that verdict needs EVIDENCE (several tracks
        // denied with none ever playing — see `spotify_key_block_confirmed`); a lone
        // mid-playback key blip is a transient CDN/throttle a fresh session clears (and
        // that respawn also resets the sticky probe flag), so recover it rather than
        // stopping dead until the user restarts lyrfin.
        if self.spotify_key_block_confirmed() {
            return false;
        }
        // Only for a librespot music track we haven't already tried to recover, with
        // a queue position to replay (a streamed episode plays via the engine, not a
        // librespot session, so a reconnect wouldn't apply). If a back-off is already
        // armed (repeated genuine failures), respect it rather than reconnect-storming
        // — `sp_fail_streak` is deliberately left intact so persistent failure still
        // trips the cooldown within a couple of tracks.
        if self.spov.sp_recovery != SpRecovery::Normal
            || self.spov.sp_stream
            || self.spov.now_spotify.is_none()
            || self.spov.sp_queue.is_empty()
            || self.spotify_cooldown_remaining() > 0
        {
            return false;
        }
        self.spov.sp_recovery = SpRecovery::Reconnecting;
        // Drop the dead session handle so `spotify_play` → `spotify_ensure_session`
        // respawns a live one.
        self.spov.session_cmd = None;
        self.spov.session_rx = None;
        self.notify("Spotify connection dropped — reconnecting…".into());
        let (q, idx) = (self.spov.sp_queue.clone(), self.spov.sp_idx);
        self.spotify_play(q, idx);
        true
    }

    /// A track failed to load (Unavailable / ended without playing). First assume a
    /// dropped connection and reconnect + replay this track once; only if that also
    /// fails skip to the next, and if a couple fail in a row stop and back off rather
    /// than racing the whole queue.
    pub(crate) fn spotify_load_failed(&mut self) {
        // already stopped (e.g. by an audio-key denial) — don't restart the queue
        if self.spov.now_spotify.is_none() {
            return;
        }
        // A quick key-retry is already scheduled (audio-key throttle) — let it own
        // the recovery; a timed-out key surfaces as *both* `AudioKeyDenied` and this
        // `Unavailable`/end-without-start echo, so a reconnect here would double-fire.
        // Likewise a pending mid-play stall re-buffer (which cleared `sp_started`, so
        // a duplicate `EndOfTrack` echo now lands here) owns its own recovery.
        if self.spov.sp_keyretry_at.is_some() || self.spov.sp_stall_at.is_some() {
            return;
        }
        // A reconnect is in flight — this is an echo failure from the dead session,
        // not the fresh one. Ignore it; the fresh session's `Connected`/`Playing`
        // (or its own failure once up) drives the next decision.
        if self.spov.sp_recovery == SpRecovery::Reconnecting {
            return;
        }
        // The likeliest cause is a dropped connection: reconnect + replay this track.
        if self.spotify_try_reconnect_retry() {
            return;
        }
        // The fresh session also failed (or it's a non-recoverable case): skip once,
        // then back off rather than racing the whole queue at Spotify.
        if self.spov.sp_fail_streak >= 1 {
            self.spotify_trip_cooldown();
            let secs = self.spotify_cooldown_remaining();
            self.spotify_playback_failed(format!(
                "Spotify wouldn't play these tracks (unavailable / key denied). Backing off {secs}s — {}",
                self.spotify_failure_hint()
            ));
            return;
        }
        self.spov.sp_fail_streak += 1;
        self.spotify_advance();
    }

    /// Spotify refused the audio decryption key — the log probe caught it (see
    /// [`crate::spotify::logprobe`]). librespot resolves the track and connects fine
    /// but can't decrypt it, so it would buffer forever.
    ///
    /// The dominant cause isn't real DRM: it's a **transient throttle** from
    /// requesting keys too fast (skipping quickly fires a burst — the `error audio
    /// key 0 2` in the error log), and the same track's key succeeds a second later.
    /// So try a quick same-session retry first ([`Self::spotify_arm_key_retry`]);
    /// only once the bounded retries are spent fall through to the heavier
    /// reconnect + back-off (which a genuine DRM/unavailable block needs).
    pub(crate) fn spotify_playback_blocked(&mut self) {
        // ignore unless a librespot music track is the buffering context (a streamed
        // podcast episode plays via the engine, not the bridge — no audio key needed)
        if self.spov.now_spotify.is_none() || self.spov.sp_stream {
            return;
        }
        // Already playing → a stale key-denied echo for a track we skipped past (the
        // probe flag is process-global and lags rapid skips); don't disturb the one
        // now underway.
        if self.spov.sp_started {
            return;
        }
        // Echo from the dead session while a reconnect is in flight — ignore (see
        // [`Self::spotify_load_failed`]).
        if self.spov.sp_recovery == SpRecovery::Reconnecting {
            return;
        }
        // Transient audio-key throttle → quick same-session retry (the common case).
        if self.spotify_arm_key_retry() {
            return;
        }
        // Retries spent → this track's key stayed denied. Count it toward the
        // account-block verdict (reset the instant any track plays): a lone denial
        // won't confirm a block, but several across tracks with nothing ever playing
        // will. This must persist across the reconnect below, so it lives outside the
        // per-track retry budgets `spotify_reset_retries` clears.
        self.spov.sp_key_denials = self.spov.sp_key_denials.saturating_add(1);
        // A denied key is also the classic symptom of a dropped connection — reconnect
        // + replay this track once (a fresh session clears a transient block) before
        // treating it as a genuine DRM/throttle block.
        if self.spotify_try_reconnect_retry() {
            return;
        }
        self.spotify_trip_cooldown();
        let secs = self.spotify_cooldown_remaining();
        self.spotify_playback_failed(format!(
            "Spotify blocked this track (no decryption key — DRM/throttle). Backing off {secs}s — {}",
            self.spotify_failure_hint()
        ));
    }

    /// Schedule a quick retry of the current track on the **same** live session
    /// after a short delay, for a transient audio-key throttle. Returns true when a
    /// retry is now owning the recovery (armed here, or already pending — a key-error
    /// burst sets the global probe flag repeatedly, so collapse the echoes into the
    /// one pending retry). Returns false when the bounded budget is spent or there's
    /// no live session to retry on, so the caller escalates.
    fn spotify_arm_key_retry(&mut self) -> bool {
        if self.spov.sp_keyretry_at.is_some() {
            return true; // one retry per throttle; fold in the burst's echoes
        }
        if self.spov.sp_keyretry_n >= SP_KEY_RETRY_MAX || self.spov.session_cmd.is_none() {
            return false; // budget spent (likely genuine DRM) / no session → escalate
        }
        self.spov.sp_keyretry_n += 1;
        self.spov.sp_keyretry_at = Some(std::time::Instant::now() + SP_KEY_RETRY_DELAY);
        self.notify("Spotify throttled the track key — retrying…".into());
        true
    }

    /// Fire a scheduled key-retry once its delay elapses (driven by `pump_spotify`
    /// every frame): re-issue the current track on the same session at its saved
    /// position — by now the audio-key throttle has cleared. A genuine denial simply
    /// fails again and, once the budget is spent, escalates to reconnect/back-off.
    pub(super) fn spotify_tick_keyretry(&mut self) {
        let Some(at) = self.spov.sp_keyretry_at else {
            return;
        };
        if std::time::Instant::now() < at {
            return;
        }
        self.spov.sp_keyretry_at = None;
        // Recovered on its own, or the overlay was torn down meanwhile — nothing to do.
        if self.spov.sp_started || self.spov.sp_stream {
            return;
        }
        let Some(track) = self.spov.now_spotify.clone() else {
            return;
        };
        let pos_ms = (self.spov.sp_pos.max(0.0) * 1000.0) as u32;
        self.spotify_begin(&track.uri, pos_ms);
    }

    /// Fire a scheduled in-place re-buffer once its delay elapses (driven by
    /// `pump_spotify` every frame): re-issue the current track on the same session
    /// at the position it stalled — by now the transient congestion has hopefully
    /// eased. A re-buffer that plays on past the stall refills the budget (progress
    /// check in [`Self::spotify_arm_stall_retry`]); one that keeps stalling at the
    /// same spot spends it, then `spotify_track_ended` advances. Bails when the
    /// overlay is no longer trying to play this track (recovered on its own,
    /// paused/stopped, a streamed episode took over, or a manual skip is pending —
    /// which must win).
    pub(super) fn spotify_tick_stall(&mut self) {
        let Some(at) = self.spov.sp_stall_at else {
            return;
        };
        if std::time::Instant::now() < at {
            return;
        }
        self.spov.sp_stall_at = None;
        if self.spov.sp_started
            || self.spov.sp_stream
            || self.spov.spotify_paused
            || self.spov.sp_skip_target.is_some()
        {
            return;
        }
        let Some(track) = self.spov.now_spotify.clone() else {
            return;
        };
        let pos_ms = (self.spov.sp_pos.max(0.0) * 1000.0) as u32;
        self.spotify_begin(&track.uri, pos_ms);
    }

    /// Watchdog for a **streamed episode** (podcast) that stops progressing mid-play.
    /// The HTTP stream can stall on a flaky CDN with no event — the engine simply
    /// stops emitting `Progress` — leaving it "buffering…" forever until the user
    /// changes tracks ([`Self::spotify_tick_stall`] deliberately skips `sp_stream`,
    /// so it has no other recovery). If no `Progress` arrives for
    /// [`SP_STREAM_STALL_SECS`], re-open the stream at the current position (a fresh
    /// connection usually clears it), bounded by [`SP_STALL_RETRY_MAX`] so a
    /// permanently-dead stream pauses (retryable) instead of looping forever.
    pub(super) fn spotify_tick_stream_watchdog(&mut self) {
        // only a streamed episode meant to be playing, and not while a seek/skip
        // re-open (which clears the clock itself) is already in flight.
        if !self.spov.sp_stream
            || self.spov.spotify_paused
            || self.spov.now_spotify.is_none()
            || self.spov.sp_seek_target.is_some()
            || self.spov.sp_skip_target.is_some()
        {
            return;
        }
        // meaningful forward progress since the last stall → a fresh re-open budget
        if self.spov.sp_pos > self.spov.sp_stall_pos + SP_STALL_PROGRESS {
            self.spov.sp_stall_n = 0;
            self.spov.sp_stall_pos = self.spov.sp_pos;
        }
        // not stalled yet? (engine `Progress` arrived within the window)
        let window = (self.config.fps as u64)
            .saturating_mul(SP_STREAM_STALL_SECS)
            .max(60);
        if self.tick.saturating_sub(self.last_audio_progress) < window {
            return;
        }
        let Some(uri) = self.spov.now_spotify.as_ref().map(|t| t.uri.clone()) else {
            return;
        };
        // budget spent → give up: pause in place (space re-loads from here), rather
        // than reconnecting a dead stream on a loop.
        if self.spov.sp_stall_n >= SP_STALL_RETRY_MAX {
            self.spov.spotify_paused = true;
            self.spov.sp_started = false;
            self.spov.sp_stall_n = 0;
            self.notify("Episode stream stalled — paused. Press space to retry.".into());
            return;
        }
        self.spov.sp_stall_n += 1;
        self.spov.sp_stall_pos = self.spov.sp_pos;
        self.last_audio_progress = self.tick; // fresh window for the re-open
        self.notify("Episode stream stalled — reconnecting…".into());
        let pos_ms = (self.spov.sp_pos.max(0.0) * 1000.0) as u32;
        self.spotify_begin(&uri, pos_ms);
    }

    /// Auto-resume Spotify playback once a transient-failure back-off elapses (driven
    /// by `pump_spotify` every frame). A bad-CDN / throttle / brief-drop stall trips
    /// the cooldown and stops the queue; without this it would stay stopped until the
    /// user restarted lyrfin. Armed by [`Self::spotify_playback_failed`] (never for a
    /// confirmed account-level block); bails if the user/stream took over meanwhile.
    /// Respawns a fresh session — the transient cause is usually cleared by a new
    /// connection — and re-loads the paused track at its saved position, exactly as
    /// pressing space would. A resume that fails again re-arms with a longer back-off.
    pub(super) fn spotify_tick_cooldown_resume(&mut self) {
        let Some(at) = self.spov.sp_resume_at else {
            return;
        };
        if crate::datetime::now_unix() < at {
            return;
        }
        self.spov.sp_resume_at = None;
        // Nothing to resume, or the user/stream took over since it was armed (a manual
        // play clears `spotify_paused`, a stream sets `sp_stream`, a fresh play sets
        // `sp_started`). Leave those alone.
        if self.spov.now_spotify.is_none()
            || self.spov.sp_started
            || self.spov.sp_stream
            || !self.spov.spotify_paused
        {
            return;
        }
        // Drop the (likely dead) session so `spotify_resume` → `spotify_ensure_session`
        // respawns a live one on a fresh connection — which also resets the sticky
        // audio-key probe flag (see `crate::spotify::session::spawn`).
        self.spov.session_cmd = None;
        self.spov.session_rx = None;
        self.spov.sp_recovery = SpRecovery::Normal;
        self.notify("Spotify back-off elapsed — resuming…".into());
        self.spotify_resume();
    }

    /// Clear any pending/spent per-track retry budgets — the audio-key throttle
    /// retry and the mid-play stall re-buffer — on a fresh track load or user
    /// action. Keeps each budget scoped to a single track.
    fn spotify_reset_retries(&mut self) {
        self.spov.sp_keyretry_at = None;
        self.spov.sp_keyretry_n = 0;
        self.spov.sp_stall_at = None;
        self.spov.sp_stall_n = 0;
        self.spov.sp_stall_pos = 0.0;
    }

    /// A track failed to play (unavailable / key-denied). **Keep** the now-playing
    /// track, artist pane, cover, and queue on screen — a momentary failure must
    /// not blank the overlay — and instead just stop the audio/buffering, mark it
    /// paused (not started, so space re-loads from the saved position), and surface
    /// `msg` in the status bar + error log. The librespot session stays alive (only
    /// this track was refused). Contrast [`Self::stop_spotify_overlay`], which fully
    /// tears the overlay down (logout / handing the engine to local/radio).
    fn spotify_playback_failed(&mut self, msg: String) {
        if self.spov.now_spotify.is_none() {
            return;
        }
        self.spov.sp_recovery = SpRecovery::Normal; // this attempt is over
        self.spov.spotify_paused = true;
        self.spov.sp_started = false;
        if let Some(cmd) = &self.spov.session_cmd {
            let _ = cmd.send(crate::spotify::session::SessionCommand::Pause);
        }
        self.spotify_release_engine(); // stop the buffering attempt / free the engine
        // Auto-resume once the back-off elapses, UNLESS it's the confirmed
        // account-level block (which won't recover and shouldn't loop): a transient
        // failure — bad CDN node, key throttle, brief drop — clears on the fresh
        // session the resume kicks off, so the user never has to restart lyrfin to get
        // audio back. Naturally bounded: each failed resume re-trips an escalating
        // back-off (20s → 5 min cap), and any success clears it.
        self.spov.sp_resume_at = (self.spov.sp_cooldown_until > 0
            && !self.spotify_key_block_confirmed())
        .then_some(self.spov.sp_cooldown_until);
        self.notify_error(msg);
        let diag = self.spotify_diag();
        self.log_error(diag); // record the WHY-context next to the failure
    }

    /// One-line snapshot of the Spotify setup, recorded in the error log next to a
    /// playback failure so the WHY-context (which client id, premium, scopes) is
    /// captured even without `RUST_LOG`.
    pub(crate) fn spotify_diag(&self) -> String {
        let premium = matches!(
            self.spotify.conn,
            crate::spotify::ConnState::Connected { premium: true, .. }
        );
        let streaming = self
            .spotify
            .tokens
            .as_ref()
            .is_some_and(|t| t.scopes.contains("streaming"));
        format!(
            "[spotify diag] web_client_id={}, audio_client_id=keymaster, premium={}, streaming_scope={}, account={}, bitrate={}",
            if crate::spotify::auth::has_custom_client_id() {
                "custom"
            } else {
                "keymaster(shared)"
            },
            premium,
            streaming,
            self.spotify.account_id.as_deref().unwrap_or("?"),
            self.config.spotify_bitrate,
        )
    }

    /// The tail of a persistent-playback-failure toast. When the account is Premium,
    /// has the `streaming` scope, AND librespot's audio-key request was refused (the
    /// log probe caught it), the cause is Spotify's **account-level audio-key block**
    /// — a restriction it applies to some (typically newer) accounts that breaks
    /// *every* librespot-based player, so re-auth can't fix it and switching accounts
    /// is the only workaround. Otherwise the usual re-auth hint (a token/scope lapse
    /// genuinely might be the problem there).
    pub(crate) fn spotify_failure_hint(&self) -> &'static str {
        if self.spotify_key_block_confirmed() {
            "Spotify is refusing this account's audio keys — a block it applies to some (usually newer) accounts that breaks every librespot-based player, so lyrfin can't work around it. Switch to an account that can stream."
        } else {
            "re-authenticate if it persists (; → Re-authenticate)."
        }
    }

    /// Whether the evidence confirms Spotify's **account-level audio-key block** — the
    /// restriction some (typically newer) accounts carry that breaks *every*
    /// librespot-based player, so lyrfin genuinely can't work around it. Distinct from a
    /// transient CDN/throttle blip (which a reconnect clears): a real block never lets a
    /// single track play, so it's confirmed only when
    /// - the account is Premium with the `streaming` scope (a free account fails for a
    ///   different, already-handled reason), AND
    /// - librespot's audio-key request was refused (the sticky probe flag), AND
    /// - **no** track has ever played this session ([`SpOverlay::sp_played_ok`]) despite
    ///   [`SP_KEY_BLOCK_CONFIRM`]+ tracks being denied.
    ///
    /// Gates both the "switch accounts" messaging and the refusal to keep
    /// reconnecting, so a momentary blip mid-playback is never mistaken for the block.
    pub(crate) fn spotify_key_block_confirmed(&self) -> bool {
        if self.spov.sp_played_ok || self.spov.sp_key_denials < SP_KEY_BLOCK_CONFIRM {
            return false;
        }
        let premium = matches!(
            self.spotify.conn,
            crate::spotify::ConnState::Connected { premium: true, .. }
        );
        let streaming = self
            .spotify
            .tokens
            .as_ref()
            .is_some_and(|t| t.scopes.contains("streaming"));
        premium
            && streaming
            && crate::spotify::logprobe::AUDIO_KEY_BLOCKED
                .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Whether the connected account can stream audio through librespot. Browsing
    /// works on any account, but playback needs Spotify **Premium** — on a free
    /// account every track comes back denied/unavailable, which would otherwise
    /// race the queue straight into a cooldown with no clear reason. Refuse up
    /// front with one clear, logged message instead. Errs toward *allowing*
    /// playback when premium is unknown (a transient `/me` failure defaults to
    /// `premium = true`), so a real Premium user is never wrongly blocked.
    fn spotify_playback_allowed(&mut self) -> bool {
        if matches!(
            self.spotify.conn,
            crate::spotify::ConnState::Connected { premium: false, .. }
        ) {
            self.notify_error(
                "Spotify playback needs Premium — librespot can't stream audio on a free \
                 account. Browsing and podcast episodes still work."
                    .into(),
            );
            return false;
        }
        true
    }

    /// Seconds remaining on the Spotify playback back-off (0 = clear to play).
    pub(crate) fn spotify_cooldown_remaining(&self) -> u64 {
        self.spov
            .sp_cooldown_until
            .saturating_sub(crate::datetime::now_unix())
    }

    /// Record a playback/connection failure and arm an exponential back-off
    /// (20s, 40s, 80s … capped at 5 min) before the next attempt, so a failing
    /// account can't reconnect/re-request in a tight loop and trip Spotify's limits.
    /// Kept short on purpose: the dominant "every track fails" cause (a free
    /// account) is now refused up front by [`Self::spotify_playback_allowed`], so
    /// this only guards genuine transient failures — no need to lock the user out
    /// for half an hour.
    pub(super) fn spotify_trip_cooldown(&mut self) {
        self.spov.sp_fail_streak = self.spov.sp_fail_streak.saturating_add(1);
        let n = self.spov.sp_fail_streak.min(5);
        let secs = (20u64 << n.saturating_sub(1)).min(300);
        self.spov.sp_cooldown_until = crate::datetime::now_unix() + secs;
    }

    /// If a back-off is active, tell the user how long is left and return true so
    /// the caller skips the attempt. Cleared on a successful play or fresh login.
    fn spotify_cooldown_active(&mut self) -> bool {
        let remain = self.spotify_cooldown_remaining();
        if remain > 0 {
            self.notify(format!(
                "Spotify is cooling down after repeated failures — try again in {remain}s (the reason is in the error log)."
            ));
            return true;
        }
        false
    }

    /// Clear the failure back-off (a track played, or a fresh login).
    pub(crate) fn spotify_clear_cooldown(&mut self) {
        self.spov.sp_fail_streak = 0;
        self.spov.sp_cooldown_until = 0;
    }

    /// librespot reported `EndOfTrack` for a track that WAS playing. That event is
    /// **overloaded**: it fires at a genuine end AND when librespot aborts mid-track
    /// because it couldn't fetch/decode the next packet in time (a network stall —
    /// *"Skipping to next track, unable to get next packet … Deadline expired"*).
    /// Only a genuine end (position at/near the duration) advances the queue; a
    /// premature abort re-buffers the SAME track where it stalled, so a transient
    /// hiccup doesn't silently skip — and, under sustained congestion, doesn't race
    /// the whole queue ("buffering and flipping tracks until it settles"). Once the
    /// bounded re-buffers are spent the segment is treated as unplayable and we
    /// advance, so a genuinely broken track still moves on.
    pub(crate) fn spotify_track_ended(&mut self) {
        if !self.spotify_track_near_end() && self.spotify_arm_stall_retry() {
            return; // re-buffering the same track in place
        }
        self.spotify_advance();
    }

    /// Whether the now-playing position has reached (within [`SP_END_SLACK`]) the
    /// track's full duration — i.e. an `EndOfTrack` that's a genuine end, not a
    /// mid-track stall. An unknown/zero duration can't be judged, so it counts as an
    /// end (advance) — preserving the old always-advance behaviour where we can't tell.
    fn spotify_track_near_end(&self) -> bool {
        self.spov.sp_dur <= 0.0 || self.spov.sp_pos >= self.spov.sp_dur - SP_END_SLACK
    }

    /// Schedule an in-place re-buffer of the current track after a mid-play stall
    /// ([`SP_STALL_RETRY_DELAY`]). Returns true when a re-buffer now owns the recovery
    /// (armed here, or already pending — duplicate `EndOfTrack` echoes fold into the
    /// one retry); false when the budget is spent or there's no live session, so the
    /// caller falls through to advancing. Drops the running clock (`sp_started =
    /// false`) so the UI shows "buffering…" and the position freezes at the stall
    /// point until audio resumes.
    ///
    /// The budget refills when playback has advanced past the previous stall (a fresh
    /// hiccup), and only depletes on stalls clustered at the same spot (a corrupt
    /// segment) — so a long track on a flaky link keeps retrying each new hiccup, but
    /// one bad segment is skipped after [`SP_STALL_RETRY_MAX`] tries rather than
    /// rebuffering forever.
    fn spotify_arm_stall_retry(&mut self) -> bool {
        if self.spov.sp_stall_at.is_some() {
            return true; // one re-buffer in flight; fold in the echoes
        }
        if self.spov.session_cmd.is_none() {
            return false; // no live session to retry on → advance
        }
        if self.spov.sp_pos > self.spov.sp_stall_pos + SP_STALL_PROGRESS {
            self.spov.sp_stall_n = 0; // real progress since the last stall → fresh budget
        }
        if self.spov.sp_stall_n >= SP_STALL_RETRY_MAX {
            return false; // repeated stalls at one spot (segment won't stream) → advance
        }
        self.spov.sp_stall_n += 1;
        self.spov.sp_stall_pos = self.spov.sp_pos;
        self.spov.sp_stall_at = Some(std::time::Instant::now() + SP_STALL_RETRY_DELAY);
        self.spov.sp_started = false; // buffering again → hide the clock, show "buffering…"
        self.notify("Spotify stalled — rebuffering…".into());
        true
    }

    /// Auto-advance when a track ends, honoring repeat: One replays the track,
    /// All wraps to the start, Off stops at the end of the queue.
    pub(crate) fn spotify_advance(&mut self) {
        if self.spov.sp_queue.is_empty() {
            return;
        }
        // A manual skip is mid-debounce → it's the user's explicit target; let it win
        // over an auto-advance (e.g. the old track ending inside the debounce window).
        if self.spov.sp_skip_target.is_some() {
            return;
        }
        self.spov.sp_recovery = SpRecovery::Normal; // moving to a track — fresh budget
        if self.spov.sp_repeat == Repeat::One {
            let (q, idx) = (self.spov.sp_queue.clone(), self.spov.sp_idx);
            self.spotify_play(q, idx);
            return;
        }
        let mut next = self.spov.sp_idx + 1;
        if next >= self.spov.sp_queue.len() {
            if self.spov.sp_repeat == Repeat::All {
                next = 0;
            } else {
                self.spov.spotify_paused = true;
                self.spov.sp_pos = self.spov.sp_dur;
                return;
            }
        }
        let q = self.spov.sp_queue.clone();
        self.spotify_play(q, next);
    }

    /// Index in `sp_queue` of the track that will play next, from the same rules as
    /// [`Self::spotify_advance`]: One replays the current track, All wraps to the
    /// start, Off yields `None` at the end of the queue. Spotify pre-shuffles the
    /// queue array (see [`Self::spotify_toggle_shuffle`]), so `sp_idx + 1` is the true
    /// next even under shuffle. Backs both the "▶ Next:" hint and next-track
    /// preloading. `None` when the queue is empty.
    fn spotify_next_index(&self) -> Option<usize> {
        let q = &self.spov.sp_queue;
        if q.is_empty() {
            return None;
        }
        if self.spov.sp_repeat == Repeat::One {
            Some(self.spov.sp_idx)
        } else if self.spov.sp_idx + 1 < q.len() {
            Some(self.spov.sp_idx + 1)
        } else if self.spov.sp_repeat == Repeat::All {
            Some(0)
        } else {
            None // Off: end of the queue
        }
    }

    /// Title of the track that will play next, for the status-bar "▶ Next:" hint —
    /// shaped for display like the QUEUE pane. See [`Self::spotify_next_index`].
    pub(crate) fn spotify_next_title(&self) -> Option<String> {
        let i = self.spotify_next_index()?;
        self.spov
            .sp_queue
            .get(i)
            .map(|it| crate::arabic::shaped(&it.name, self.config.arabic_shaping))
    }

    /// Prefetch the next Spotify track so the upcoming transition is gapless — the
    /// Spotify analog of local gapless preloading ([`Self::gapless_next_path`]).
    /// librespot dedups internally (re-preloading the same track is a no-op; a
    /// changed next cancels + replaces), so this is safe to call whenever the next
    /// might have changed: a track starting, a skip, a shuffle/repeat toggle. Skipped
    /// when the "next" is already loaded (repeat-one / single-track wrap), for
    /// streamed episodes (they bypass librespot), and with no live session.
    pub(crate) fn spotify_preload_next(&self) {
        if self.spov.sp_stream {
            return;
        }
        let Some(cmd) = &self.spov.session_cmd else {
            return;
        };
        let Some(i) = self.spotify_next_index() else {
            return;
        };
        if i == self.spov.sp_idx {
            return; // the next is the current track — already loaded
        }
        let Some(uri) = self.spov.sp_queue.get(i).map(|it| it.uri.clone()) else {
            return;
        };
        if Self::is_episode_uri(&uri) {
            return; // episodes stream outside librespot; a preload wouldn't apply
        }
        let _ = cmd.send(crate::spotify::session::SessionCommand::Preload { uri });
    }

    /// Toggle shuffle for the Spotify queue. Turning it on shuffles the upcoming
    /// tracks (the current one stays put), like the desktop client.
    pub(crate) fn spotify_toggle_shuffle(&mut self) {
        self.spov.sp_shuffle = !self.spov.sp_shuffle;
        if self.spov.sp_shuffle {
            // the same tail shuffle the local queue uses, with OS entropy per pick.
            let start = self.spov.sp_idx + 1;
            let mut buf = [0u8; 8];
            crate::core::shuffle::shuffle_tail(&mut self.spov.sp_queue, start, |k| {
                let _ = getrandom::fill(&mut buf);
                (u64::from_le_bytes(buf) % k as u64) as usize
            });
        }
        self.spotify_preload_next(); // the upcoming track changed → re-prefetch
    }

    /// Cycle the Spotify queue's repeat mode (off → one → all).
    pub(crate) fn spotify_cycle_repeat(&mut self) {
        self.spov.sp_repeat = match self.spov.sp_repeat {
            Repeat::Off => Repeat::One,
            Repeat::One => Repeat::All,
            Repeat::All => Repeat::Off,
        };
        self.spotify_preload_next(); // repeat mode changed the upcoming track → re-prefetch
    }

    /// Whether the now-bar should show the Spotify track (Spotify view only).
    pub fn showing_spotify(&self) -> bool {
        self.layout == Layout::Spotify && self.spov.now_spotify.is_some()
    }

    /// Elapsed position of whatever's actually playing — the Spotify overlay in
    /// the Spotify view, else the local player. Drives the lyrics karaoke wipe so
    /// it follows the right clock.
    pub fn playback_elapsed(&self) -> std::time::Duration {
        // same source as `active_lyrics_pane`, or the wipe would track one track's
        // clock while showing another's words
        let base = if self.lyrics_source_is_spotify() {
            std::time::Duration::from_secs_f64(self.spov.sp_pos.max(0.0))
        } else {
            self.player.elapsed
        };
        // apply the manual lyric-sync nudge: +offset shifts the clock *earlier* so
        // the highlight lands later, matching a vocal the automatic clock leads.
        let ms = base.as_millis() as i64 - self.config.lyrics_offset_ms as i64;
        std::time::Duration::from_millis(ms.max(0) as u64)
    }

    /// Progress 0.0..=1.0 for **plain** (unsynced) lyric line selection, derived
    /// from the offset-adjusted [`Self::playback_elapsed`] rather than raw
    /// progress — so the manual `,`/`.` sync nudge shifts unsynced lyrics too
    /// (synced lyrics already honour it via `playback_elapsed`).
    pub fn lyrics_progress(&self) -> f32 {
        let dur_secs = if self.showing_spotify() {
            self.spov.sp_dur as f32
        } else {
            self.player.duration.as_secs_f32()
        };
        if dur_secs <= 0.0 {
            return 0.0;
        }
        (self.playback_elapsed().as_secs_f32() / dur_secs).clamp(0.0, 1.0)
    }

    /// Fetch lyrics for the now-playing Spotify track from the same online source
    /// the local player uses (by artist + title + album + duration), de-duped by
    /// the cache key. Cheap: sidecar/embedded paths don't apply, so it's a cache
    /// hit or one online request.
    pub(crate) fn load_spotify_lyrics(&mut self) {
        self.reset_lyrics_scroll();
        self.meta.lyrics_pending = false;
        let Some(tr) = self.spov.now_spotify.clone() else {
            self.meta.lyrics = None;
            self.meta.lyrics_for = None;
            return;
        };
        // podcasts have no sung lyrics — leave the lyrics pane idle ("—") rather than
        // burn a lyrics-DB lookup on the episode title (the show's info lives in the
        // artist/show pane instead).
        if Self::is_episode_uri(&tr.uri) {
            self.meta.lyrics = None;
            self.meta.lyrics_for = None;
            return;
        }
        // first listed artist reads better against the lyrics DB than "A, B, C"
        let artist = tr.primary_artist().to_string();
        let title = tr.name.clone();
        let album = tr.album.clone();
        let dur = (tr.duration_ms / 1000) as u64;
        let key = crate::lyrics::cache_key(&artist, &title);
        self.meta.lyrics_for = Some(key.clone());
        if let Some(l) = crate::lyrics::Lyrics::load_cached(&self.config.dir, &key) {
            self.meta.lyrics = Some(l);
            self.maybe_request_translation();
            return;
        }
        self.meta.lyrics = None;
        if let Some(tx) = &self.workers.lyrics {
            self.meta.lyrics_pending = true;
            let _ = tx.send(crate::lyricsfetch::LyricsRequest {
                artist,
                title,
                album,
                duration_secs: dur,
                translate_to: self.config.lyrics_translate_to.clone(),
                key,
            });
        }
    }

    /// A Spotify track is loaded and meant to play, but librespot hasn't reported
    /// the first `Playing` yet (the 2-4s buffer). Drives a "buffering…" hint.
    pub fn sp_buffering(&self) -> bool {
        self.spov.now_spotify.is_some() && !self.spov.spotify_paused && !self.spov.sp_started
    }

    /// Stop the Spotify overlay (pause librespot + release the engine's external
    /// source) so local/radio audio can take over. Keeps the session alive for a
    /// quick resume.
    pub(crate) fn stop_spotify_overlay(&mut self) {
        if self.spov.now_spotify.is_none() {
            return;
        }
        self.spov.now_spotify = None;
        self.spov.spotify_paused = true;
        self.spov.sp_cover = None;
        self.spov.sp_cover_url = None;
        self.spov.sp_saved = false;
        // teardown = logout / account switch → the block-status evidence belongs to the
        // old account; reset it (a new account re-earns "played ok" on its first play)
        self.spov.sp_played_ok = false;
        self.spov.sp_key_denials = 0;
        self.spov.sp_resume_at = None; // no queue to auto-resume once torn down
        self.spotify_clear_pending_skip(); // don't let a debounced skip load post-teardown
        self.clear_spotify_artist();
        if let Some(cmd) = &self.spov.session_cmd {
            let _ = cmd.send(crate::spotify::session::SessionCommand::Pause);
        }
        self.spotify_release_engine();
    }

    /// Pause the Spotify overlay but KEEP its context — now-playing track, queue,
    /// position, and cover — so returning to the Spotify view shows it, resumable
    /// with space (and it persists across a session). Used when local or radio
    /// playback takes over the engine. (Contrast [`stop_spotify_overlay`], which
    /// fully tears the overlay down for logout / re-auth.)
    pub(crate) fn pause_spotify_overlay(&mut self) {
        if self.spov.now_spotify.is_none() {
            return;
        }
        self.spov.spotify_paused = true;
        self.spov.sp_started = false; // no longer streaming → space re-loads at sp_pos
        // KEEP the librespot session alive — just pause it and detach the engine
        // bridge. Reconnecting on every local/radio interlude is what risks tripping
        // Spotify's rate-limit; reusing the open session avoids that churn. (Full
        // teardown + session drop happen only on logout / re-auth.)
        if let Some(cmd) = &self.spov.session_cmd {
            let _ = cmd.send(crate::spotify::session::SessionCommand::Pause);
        }
        self.spotify_release_engine();
    }

    /// Release the engine from whatever the overlay was driving: stop the episode
    /// stream if one was playing, else detach the librespot bridge. Resets the
    /// stream flag so the next librespot load re-attaches the bridge.
    fn spotify_release_engine(&mut self) {
        if self.spov.sp_stream {
            self.engine.send(AudioCommand::Stop);
            self.spov.sp_stream = false;
        } else {
            self.engine.send(AudioCommand::ClearExternalSource);
        }
    }

    /// Stream an externally-hosted episode's MP3 through lyrfin's own engine —
    /// librespot can't decode Spotify-hosted episode audio (key denied), so this is
    /// the only way episodes play. The engine then drives pause + the clock.
    pub(crate) fn spotify_stream_episode(&mut self, url: String) {
        self.spov.sp_stream = true;
        self.spov.spotify_paused = false;
        self.engine.send(AudioCommand::ClearExternalSource);
        self.engine.send(AudioCommand::SetSpeed(1.0));
        // a podcast episode is a finite, ranged file — natively seekable, no DVR.
        self.engine
            .send(AudioCommand::LoadStream { url, dvr: None });
        self.engine.send(AudioCommand::Play);
    }

    /// No playable source for the now-playing episode (Spotify-hosted/DRM'd and no
    /// public RSS match) — stop the spinner with an honest message, and re-point
    /// the engine at librespot so the next music track still plays.
    pub(crate) fn spotify_episode_unplayable(&mut self) {
        self.spotify_reattach_bridge();
        self.spov.spotify_paused = true;
        self.spov.sp_started = false;
        self.notify(
            "Spotify Exclusive — not streamable. Spotify hosts this episode's audio with DRM and it has no public feed; only externally-hosted episodes play here."
                .into(),
        );
    }

    /// Ask the podcast resolver to find the now-playing episode's public MP3 (its
    /// show name + title come from the Item; `episode_item` stores the show in
    /// `album`). Returns false when we can't ask — no worker, or no show name.
    pub(super) fn spotify_request_podcast(&mut self) -> bool {
        let Some(tr) = self.spov.now_spotify.clone() else {
            return false;
        };
        if tr.album.trim().is_empty() {
            return false;
        }
        let Some(tx) = &self.workers.podcast else {
            return false;
        };
        let _ = tx.send(crate::podcastfetch::PodcastRequest {
            show: tr.album.clone(),
            title: tr.name.clone(),
            key: tr.uri.clone(),
        });
        true
    }
}
