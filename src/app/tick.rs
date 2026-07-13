//! The per-frame tick: `on_tick` advances the animation clock, drains the
//! worker channels (library / online / audio events), ages notifications, and
//! marks the UI dirty when something visible changed. Driven by `Action::Tick`
//! in the reducer.

use super::*;

impl AppState {
    pub(super) fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        // Follow-system theme: check the OS light/dark setting at ~1s intervals and
        // switch the active theme when it flips. Throttled so the (macOS CFPreferences)
        // read stays cheap regardless of frame rate; a no-op unless following.
        if self.config.theme_follows_system
            && self.appearance_at.elapsed() >= Duration::from_millis(1000)
        {
            self.appearance_at = std::time::Instant::now();
            self.poll_system_appearance();
        }
        let sleep_was = self.fx.sleep_until.is_some();
        self.update_viz();
        self.tick_sleep_timer();
        if let Some(n) = &mut self.notification {
            n.ttl_ticks = n.ttl_ticks.saturating_sub(1);
            if n.ttl_ticks == 0 {
                self.notification = None;
                self.dirty = true; // redraw once more to clear it
            }
        }
        // the sleep timer firing (auto-pause) flips is_animating off → redraw once
        if sleep_was && self.fx.sleep_until.is_none() {
            self.dirty = true;
        }
        // Spotify position clock: tick locally while playing (librespot emits a
        // position only on play/pause), clamped to the track length. Hold at zero
        // until librespot reports the first Playing (sp_started) — otherwise the
        // clock runs during the 2-4s buffer and then snaps back when audio begins.
        // Skipped for a streamed episode: the engine emits real Progress, so the
        // local clock would double-count.
        if self.spov.now_spotify.is_some()
            && self.spov.sp_started
            && !self.spov.spotify_paused
            && !self.spov.sp_stream
            && self.spov.sp_dur > 0.0
        {
            // advance by the real frame delta (not an assumed 1/fps) so the clock
            // tracks wall time — librespot only re-anchors it on play/pause, so any
            // per-tick error would otherwise accumulate into visible lyric drift.
            let step = self.frame_dt.as_secs_f64();
            self.spov.sp_pos = (self.spov.sp_pos + step).min(self.spov.sp_dur);
        }
        // Demo clock: smoothly advance elapsed between the engine's (whole-second)
        // Progress events — and drive it entirely for the demo library / before a
        // device attaches. Track-time advances at the playback *speed*, so the
        // sub-second interpolation matches the engine's speed-adjusted clock (else
        // the timer reads ~1× while a sped-up track really moves faster).
        let engine_driving = self.tick.saturating_sub(self.last_audio_progress) <= 6;
        if self.player.status == Status::Playing
            && !self.player.duration.is_zero()
            && !engine_driving
        {
            let secs = self.player.speed.max(0.05) as f64 * self.frame_dt.as_secs_f64();
            self.player.elapsed += Duration::from_secs_f64(secs);
            if self.player.elapsed >= self.player.duration {
                if self.engine_active {
                    // a real engine drives track changes (gapless Advanced /
                    // Finished) — don't simulate an advance, just hold at the end
                    // so we never race the engine and desync the queue.
                    self.player.elapsed = self.player.duration;
                } else {
                    self.player.next();
                    let dur = self
                        .player
                        .current
                        .and_then(|id| self.library.track(id))
                        .map(|t| t.duration());
                    if let Some(d) = dur {
                        self.player.duration = d;
                    }
                    self.player.elapsed = Duration::ZERO;
                }
            }
        }
        self.enforce_ab_loop();
    }
}
