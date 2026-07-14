//! Local playback + queue methods on `AppState` (extracted from app/mod.rs).

use super::*;

impl AppState {
    pub(crate) fn seek_to_fraction(&mut self, frac: f32) {
        // A timeshifted live stream (DVR): a progress-bar scrub maps the fraction
        // onto the window `[start, live]`. A live stream without a DVR buffer is
        // forward-only (no-op).
        if let Some(dvr) = self.rnow.dvr {
            let target = dvr.start + (dvr.live - dvr.start) * frac.clamp(0.0, 1.0) as f64;
            self.engine
                .send(AudioCommand::Seek(Duration::from_secs_f64(target)));
            if let Some(d) = self.rnow.dvr.as_mut() {
                d.pos = target;
            }
            return;
        }
        if self.rnow.is_live() {
            return;
        }
        let dur = self.player.duration.as_secs_f32();
        let pos = (dur * frac.clamp(0.0, 1.0)).clamp(0.0, dur);
        self.player.elapsed = Duration::from_secs_f32(pos);
        self.engine.send(AudioCommand::Seek(self.player.elapsed));
    }

    /// Total items in the queue (played + current + upcoming — the queue keeps
    /// every track; playing one only moves the position cursor).
    pub(crate) fn queue_len(&self) -> usize {
        self.player.queue.items.len()
    }

    /// Absolute index in `queue.items` of the selected row, if any.
    pub(crate) fn queue_sel_index(&self) -> Option<usize> {
        (self.queue_sel < self.player.queue.items.len()).then_some(self.queue_sel)
    }

    /// Reorder the selected track up/down. The position cursor follows whichever
    /// track is currently playing so playback isn't disrupted.
    pub(crate) fn queue_move(&mut self, m: Motion) {
        let Some(i) = self.queue_sel_index() else {
            return;
        };
        let n = self.player.queue.items.len();
        let j = match m {
            Motion::Up if i > 0 => i - 1,
            Motion::Down if i + 1 < n => i + 1,
            _ => return,
        };
        self.player.queue.items.swap(i, j);
        if self.player.queue.position == i {
            self.player.queue.position = j;
        } else if self.player.queue.position == j {
            self.player.queue.position = i;
        }
        self.queue_sel = j;
    }

    /// Remove the selected track from the queue (never the one playing now).
    pub(crate) fn queue_remove(&mut self) {
        let Some(i) = self.queue_sel_index() else {
            return;
        };
        if i == self.player.queue.position {
            return; // don't yank the currently-playing track
        }
        self.player.queue.items.remove(i);
        if i < self.player.queue.position {
            self.player.queue.position -= 1;
        }
        let max = self.player.queue.items.len().saturating_sub(1);
        self.queue_sel = self.queue_sel.min(max);
    }

    /// Drop everything after the current track.
    pub(crate) fn queue_clear_upcoming(&mut self) {
        let keep = self.player.queue.position + 1;
        self.player.queue.items.truncate(keep);
        self.queue_sel = 0;
        self.notify("Cleared upcoming".into());
    }

    /// Load + play the current track on the engine.
    pub(crate) fn play_current(&mut self) {
        let Some(id) = self.player.current else {
            return;
        };
        let Some(track) = self.library.track(id) else {
            return;
        };
        // Local playback takes the engine — pause (but KEEP) any radio/Spotify
        // overlay so switching back to that view shows it, resumable, and it
        // persists across a session. (Full teardown is only on logout / re-tune.)
        if self.rnow.now_station.is_some() {
            self.rnow.radio_paused = true;
            self.rnow.dvr = None; // the engine drops the timeshift buffer on Load
        }
        let path = track.path.clone();
        let duration = track.duration();
        // …and the Spotify stream (release the engine). After the `track` borrow.
        self.pause_spotify_overlay();
        // an A-B loop belongs to a single track — drop it when the track changes
        if self.loaded_track != Some(id) {
            self.fx.ab_a = None;
            self.fx.ab_loop = None;
        }
        self.load_lyrics();
        self.reload_cover();
        let (gain, rg_status) = self.replaygain_eval(&path);
        self.fx.rg_status = rg_status;
        self.engine.send(AudioCommand::Load(path));
        self.engine.send(AudioCommand::SetGain(gain));
        self.engine.send(AudioCommand::SetSpeed(self.player.speed));
        self.engine
            .send(AudioCommand::SetCrossfade(self.config.crossfade_ms));
        self.engine
            .send(AudioCommand::SetSilenceSkip(self.config.silence_skip));
        self.apply_eq(); // carry the equalizer curve onto the freshly loaded track
        self.engine.send(AudioCommand::Play);
        self.loaded_track = Some(id);
        self.player.duration = duration;
        self.player.elapsed = Duration::ZERO;
        self.player.status = Status::Playing;
        self.last_audio_progress = self.tick;
        self.record_play(id);
        self.request_artist_info();
        self.update_gapless_next();
        self.follow_current();
    }

    /// Tell the engine which track (if any) to preload for gapless playback.
    pub(crate) fn update_gapless_next(&self) {
        self.engine
            .send(AudioCommand::SetNext(self.gapless_next_path()));
    }

    /// Push the silence-skip toggle to the engine; it applies at the next decoded
    /// track boundary (leading/trailing near-silence is trimmed at the seam).
    pub(crate) fn apply_silence_skip(&self) {
        self.engine
            .send(AudioCommand::SetSilenceSkip(self.config.silence_skip));
    }

    /// Play/pause for whichever source the *current view* controls. A dispatcher
    /// over the per-source toggles ([`toggle_spotify_play`] / [`toggle_radio_play`]
    /// / [`toggle_local_play`]); the OS-media-control path routes to those same
    /// helpers by the *active audio* source instead (see `app/nowplaying.rs`).
    pub(crate) fn toggle_play(&mut self) {
        // In the Spotify view, play/pause controls the librespot stream only.
        if self.showing_spotify() {
            self.toggle_spotify_play();
            return;
        }
        // Still in the Spotify view but nothing's loaded (e.g. logging in): space
        // is inert here — never fall through and start the LOCAL player.
        if self.layout == Layout::Spotify {
            return;
        }
        // In a local view while Spotify is actively streaming, play/pause means
        // "play my music": play_current pauses the (preserved) Spotify overlay.
        // A merely-paused overlay (now_spotify still set) falls through to the
        // local player toggle below.
        if self.spov.now_spotify.is_some() && !self.spov.spotify_paused {
            self.play_current();
            return;
        }
        // In the Radio view, play/pause controls the live stream.
        if self.showing_radio() && self.rnow.now_station.is_some() {
            self.toggle_radio_play();
            return;
        }
        // In a local view while radio is actively streaming, play/pause means
        // "play my music": play_current pauses the (preserved) radio overlay. A
        // paused station falls through to the local player toggle below.
        if self.rnow.is_live() {
            self.play_current();
            return;
        }
        self.toggle_local_play();
    }

    /// Pause/resume the Spotify overlay *in place* (source-based — no view check).
    /// Instant: the ring is kept so resume continues without a re-buffer.
    pub(crate) fn toggle_spotify_play(&mut self) {
        // the user is taking manual control → cancel any pending auto-resume so a
        // deliberate pause isn't undone (and a manual resume owns the recovery)
        self.spov.sp_resume_at = None;
        // an externally-streamed episode plays through lyrfin's own engine, not
        // librespot — pause/resume is a plain engine toggle (the ring is kept,
        // so it resumes in place).
        if self.spov.sp_stream {
            let pause = !self.spov.spotify_paused;
            self.engine.send(if pause {
                AudioCommand::Pause
            } else {
                AudioCommand::Play
            });
            self.spov.spotify_paused = pause;
            return;
        }
        // genuinely not streaming — restored from a session, or paused while it
        // never started: (re)load at the saved position rather than toggling a
        // stream that isn't there.
        if !self.spov.sp_started && self.spov.spotify_paused {
            self.spotify_resume();
            return;
        }
        // playing, OR still buffering an autoplay load: a normal pause/resume
        // toggle. Pausing while it buffers cancels the pending play, so the
        // track won't start once the buffer fills — space works during buffering.
        let pause = !self.spov.spotify_paused;
        if let Some(cmd) = &self.spov.session_cmd {
            let _ = cmd.send(if pause {
                crate::spotify::session::SessionCommand::Pause
            } else {
                crate::spotify::session::SessionCommand::Play
            });
        }
        // Mirror onto the engine's output so pause is *instant*: the device
        // callback stops draining the ring at once (outputs silence) instead of
        // playing out the ~1s of audio already buffered from librespot. The ring
        // is preserved (not flushed), so resume continues seamlessly — no skip.
        self.engine.send(if pause {
            AudioCommand::Pause
        } else {
            AudioCommand::Play
        });
        self.spov.spotify_paused = pause;
    }

    /// Pause/resume the tuned radio station (source-based — no view check).
    /// Pausing stops the live stream; resuming re-tunes (you can't resume a live
    /// stream in place), unless a timeshift (DVR) buffer lets it continue.
    pub(crate) fn toggle_radio_play(&mut self) {
        let Some(st) = self.rnow.now_station.clone() else {
            return;
        };
        if self.rnow.radio_paused {
            // Resume: a timeshifted stream continues from where it was paused
            // (the buffer kept filling); a plain live stream can't resume in
            // place, so re-tune.
            if self.rnow.dvr.is_some() {
                self.rnow.radio_paused = false;
                self.engine.send(AudioCommand::Play);
            } else {
                self.play_station(st);
            }
        } else {
            self.rnow.radio_paused = true;
            self.engine.send(AudioCommand::Pause);
        }
    }

    /// Pause/resume the local player (source-based — no view check).
    pub(crate) fn toggle_local_play(&mut self) {
        match self.player.status {
            Status::Playing => {
                self.player.status = Status::Paused;
                self.engine.send(AudioCommand::Pause);
            }
            Status::Paused | Status::Stopped => {
                if self.loaded_track == self.player.current && self.player.current.is_some() {
                    self.player.status = Status::Playing;
                    self.engine.send(AudioCommand::Play);
                    self.update_gapless_next(); // a resumed track still needs its preload
                } else {
                    self.play_current();
                }
            }
        }
    }

    pub(crate) fn seek_relative(&mut self, delta: i64) {
        // A timeshifted live stream (DVR): seek within its window, clamped to
        // `[start, live]`. Works whether the station is playing or paused — the
        // buffer persists while paused.
        if let Some(dvr) = self.rnow.dvr {
            let target = (dvr.pos + delta as f64).clamp(dvr.start, dvr.live);
            self.engine
                .send(AudioCommand::Seek(Duration::from_secs_f64(target)));
            if let Some(d) = self.rnow.dvr.as_mut() {
                d.pos = target; // reflect immediately; Progress will confirm
            }
            return;
        }
        // A live stream with no DVR buffer is forward-only → seeking is a no-op (it
        // can't be sought, and the shown position is the preserved local track's).
        if self.rnow.is_live() {
            return;
        }
        let cur = self.player.elapsed.as_secs() as i64;
        let dur = self.player.duration.as_secs() as i64;
        let np = (cur + delta).clamp(0, dur.max(0)) as u64;
        self.player.elapsed = Duration::from_secs(np);
        self.engine.send(AudioCommand::Seek(self.player.elapsed));
    }

    /// Replace the queue with `ids` and start playing at `anchor` (or the front
    /// if the anchor isn't in the list). The queue is the single source of truth,
    /// so the tracklist/queue panel now show exactly what plays next.
    pub(crate) fn play_scope(&mut self, ids: Vec<TrackId>, anchor: TrackId) {
        if ids.is_empty() {
            return;
        }
        let pos = ids.iter().position(|&x| x == anchor).unwrap_or(0);
        let id = ids[pos];
        self.player.queue.items = ids;
        self.player.queue.position = pos;
        self.player.queue.history.clear();
        self.player.current = Some(id);
        self.play_current();
    }

    /// Queue the whole album of the current/selected track (combine with repeat-all
    /// to loop it — the replacement for the old "repeat album" mode).
    pub(crate) fn play_current_album(&mut self) {
        let Some(r) = self.scope_ref() else { return };
        match self.library.track(r).and_then(|t| t.album_id) {
            Some(a) => {
                let ids: Vec<TrackId> = self.library.tracks_of(a).iter().map(|t| t.id).collect();
                let title = self.library.albums.get(&a).map(|al| al.title.clone());
                self.play_scope(ids, r);
                if let Some(t) = title {
                    self.notify(format!("Playing album: {t}"));
                }
            }
            None => self.notify("No album for this track".into()),
        }
    }

    /// Queue every track by the current/selected track's artist (combine with
    /// repeat-all to loop — the replacement for the old "repeat artist" mode).
    pub(crate) fn play_current_artist(&mut self) {
        let Some(r) = self.scope_ref() else { return };
        match self.library.track(r).and_then(|t| t.artist_id) {
            Some(a) => {
                let ids = self.library.tracks_of_artist(a);
                let name = self.library.artists.get(&a).map(|ar| ar.name.clone());
                self.play_scope(ids, r);
                if let Some(n) = name {
                    self.notify(format!("Playing artist: {n}"));
                }
            }
            None => self.notify("No artist for this track".into()),
        }
    }

    /// Advance to the next track in the queue (wrapping under repeat-all). Used by
    /// Next + auto-advance.
    pub(crate) fn advance_next(&mut self) {
        // local next: in the Radio view, n/p change station via RadioStation, so
        // this path is always about the local queue (and stops any radio overlay)
        let before = self.player.queue.position;
        self.player.next();
        if self.player.queue.position != before || self.player.repeat == Repeat::One {
            self.play_current();
        } else {
            self.player.status = Status::Stopped;
        }
    }

    /// Previous track (the queue's history; linear step back as a fallback).
    pub(crate) fn advance_prev(&mut self) {
        self.player.previous();
        self.play_current();
    }

    pub(crate) fn advance_after_finish(&mut self) {
        if self.player.repeat == Repeat::One {
            self.player.elapsed = Duration::ZERO;
            self.play_current();
            return;
        }
        self.advance_next();
    }

    /// Move the tracklist cursor onto the now-playing track when that track is
    /// part of the list currently on screen, so advancing scrolls it into view
    /// and highlights it — mirroring how the queue panel tracks the playing row.
    /// No-op when the playing track isn't in the shown list (a different
    /// search/browse context), so it never yanks the cursor somewhere unrelated.
    ///
    /// Also follows the now-playing track in the QUEUE pane: a focused pane scrolls
    /// to its cursor (`queue_sel`), so without this an auto-advance (esp. the
    /// repeat-all wrap back to the top) would strand the highlight on the old row
    /// while a different track plays. j/k still browses the pane between changes.
    fn follow_current(&mut self) {
        let Some(cur) = self.player.current else {
            return;
        };
        self.queue_sel = self.player.queue.position;
        let idx = if self.is_searching() {
            self.search_results().iter().position(|&id| id == cur)
        } else if !self.browser.list.is_empty() {
            self.browser.list.iter().position(|&id| id == cur)
        } else {
            self.player.queue.items.iter().position(|&id| id == cur)
        };
        if let Some(i) = idx {
            self.selection = i;
        }
    }

    /// Register a play of `id`: bump play count + `last_played`, push to the
    /// recently-played list and the persisted listening history (heatmap/streaks).
    pub(crate) fn record_play(&mut self, id: TrackId) {
        let now = crate::datetime::now_unix();
        if let Some(t) = self.library.tracks.get_mut(&id) {
            t.play_count += 1;
            t.last_played = now as u32;
        }
        // move-to-top: drop any earlier instance, then put this play at the front,
        // so the list never holds duplicates of the same track.
        self.library.recently_played.retain(|&x| x != id);
        self.library.recently_played.insert(0, id);
        self.library.recently_played.truncate(100);
        self.play_history.push(now);
        // bound the in-RAM history too (the cap was only applied at save time)
        let cap = crate::library::store::HistoryStore::CAP;
        if self.play_history.len() > cap {
            let drop = self.play_history.len() - cap;
            self.play_history.drain(..drop);
        }
        // debounce: rewriting the whole file every play is wasteful — flush every
        // 16 plays, and once more on exit (see `flush_history`).
        self.plays_since_flush += 1;
        if self.plays_since_flush >= 16 {
            self.flush_history();
        }
    }

    /// Queue position the engine will advance to (sequentially, or wrapping under
    /// repeat-all). `None` when there's nothing to advance to.
    fn gapless_next_pos(&self) -> Option<usize> {
        let n = self.player.queue.items.len();
        let np = self.player.queue.position + 1;
        if np < n {
            Some(np)
        } else if self.player.repeat == Repeat::All && n > 0 {
            Some(0)
        } else {
            None
        }
    }

    /// Title of the next track for the status-bar "▶ Next:" hint — derived purely
    /// from the queue + current position (the single source of truth, no separate
    /// up-next state). Mirrors [`Self::advance_after_finish`] so the hint matches
    /// what actually plays: repeat-one replays the current track, otherwise the
    /// sequential next, wrapping only under repeat-all. Shuffle pre-reorders the
    /// upcoming tracks ([`crate::core::player::PlayerState::toggle_shuffle`]), so the
    /// next is deterministic even when shuffled — the hint stays populated. `None`
    /// only at the end of a non-repeating queue.
    pub fn next_queue_title(&self) -> Option<String> {
        let q = &self.player.queue;
        let n = q.items.len();
        let idx = if self.player.repeat == Repeat::One {
            q.position // repeat-one replays the current track
        } else if q.position + 1 < n {
            q.position + 1
        } else if self.player.repeat == Repeat::All && n > 0 {
            0
        } else {
            return None;
        };
        let id = *q.items.get(idx)?;
        self.library.track(id).map(|t| t.title.clone())
    }

    /// Source-aware next-track title for the status-bar "▶ Next:" hint, so it
    /// reflects whatever the QUEUE pane shows: the Spotify up-next track while
    /// Spotify is the active source, otherwise the local queue's next track. Radio
    /// streams have no queue → `None`. Keeps the status bar a pure display layer.
    pub fn status_next_title(&self) -> Option<String> {
        // Follow the *view*, exactly like the now-playing bar (`now_bar` dispatches on
        // `layout`): the Spotify view shows Spotify's up-next — even when paused — Radio
        // has no queue, and every other view shows the local queue. So the "Next:" hint
        // and the now-bar can never disagree about the source.
        match self.layout {
            Layout::Radio => None,
            Layout::Spotify => self.spotify_next_title(),
            _ => self.next_queue_title(),
        }
    }

    /// Path of the track to preload for a seamless transition (gapless *or*
    /// crossfade). Works whenever the next track is deterministic — every mode
    /// except repeat-one, which replays the current track rather than advancing.
    /// Shuffle pre-reorders the queue tail ([`crate::core::player::PlayerState::toggle_shuffle`]),
    /// so the sequential next is correct under shuffle too.
    pub(crate) fn gapless_next_path(&self) -> Option<std::path::PathBuf> {
        let seamless = self.config.gapless || self.config.crossfade_ms > 0;
        if !seamless || self.player.repeat == Repeat::One {
            return None;
        }
        let id = *self.player.queue.items.get(self.gapless_next_pos()?)?;
        self.library.track(id).map(|t| t.path.clone())
    }

    /// The engine started the preloaded next track gaplessly — sync app state to
    /// it (now-playing, counts, lyrics/cover, ReplayGain) without reloading audio.
    pub(crate) fn soft_advance(&mut self) {
        let Some(pos) = self.gapless_next_pos() else {
            return;
        };
        if let Some(cur) = self.player.queue.current() {
            self.player.queue.history.push(cur);
            if self.player.queue.history.len() > 500 {
                self.player.queue.history.remove(0);
            }
        }
        self.player.queue.position = pos;
        self.player.current = self.player.queue.current();
        self.player.elapsed = Duration::ZERO;
        self.fx.ab_a = None;
        self.fx.ab_loop = None;
        let Some(id) = self.player.current else {
            return;
        };
        let Some((path, duration)) = self
            .library
            .track(id)
            .map(|t| (t.path.clone(), t.duration()))
        else {
            return;
        };
        self.player.duration = duration;
        // the engine loaded the next track without ReplayGain — apply it now
        let (gain, rg_status) = self.replaygain_eval(&path);
        self.fx.rg_status = rg_status;
        self.engine.send(AudioCommand::SetGain(gain));
        self.loaded_track = Some(id);
        self.last_audio_progress = self.tick;
        self.record_play(id);
        self.load_lyrics();
        self.reload_cover();
        self.request_artist_info();
        self.update_gapless_next();
        // Visible confirmation that a seamless transition just fired — the swap is
        // otherwise silent by design, so without this it's indistinguishable from
        // a normal track change. Names the mode that carried it.
        if let Some(title) = self.library.track(id).map(|t| t.title.clone()) {
            let mode = if self.config.crossfade_ms > 0 {
                "crossfade"
            } else {
                "gapless"
            };
            self.notify(format!("⊳ {mode} → {title}"));
        }
        self.follow_current();
    }
}
