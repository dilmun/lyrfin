//! Playback shaping & timers on `AppState` (extracted from app/mod.rs): the
//! sleep timer, the A-B loop, ReplayGain (volume normalization), and crossfade.
//! These adjust *how* the loaded track plays without owning the transport/queue
//! (that's `playback.rs`); they're driven by settings rows, palette commands,
//! and the tick.

use super::*;

/// Runtime state for the playback effects/timers in this module, grouped out of
/// `AppState`: the sleep-timer deadline (unix seconds), the A-B loop's pending A
/// marker + active `(A, B)` repeat region, and the ReplayGain status string for
/// the loaded track (`None` when RG is off).
#[derive(Default)]
pub struct PlaybackFx {
    pub sleep_until: Option<u64>,
    pub ab_a: Option<Duration>,
    pub ab_loop: Option<(Duration, Duration)>,
    pub rg_status: Option<String>,
}

impl AppState {
    // ---- sleep timer -----------------------------------------------------

    /// Arm (or cancel, when `min == 0`) the sleep timer.
    pub(super) fn set_sleep_timer(&mut self, min: u32) {
        if min == 0 {
            self.fx.sleep_until = None;
            self.notify("Sleep timer off".into());
        } else {
            self.fx.sleep_until = Some(crate::datetime::now_unix() + min as u64 * 60);
            self.notify(format!("Sleep timer: {min} min"));
        }
    }

    /// Seconds left on the sleep timer (None when disarmed/expired).
    pub fn sleep_remaining_secs(&self) -> Option<u64> {
        self.fx
            .sleep_until
            .map(|d| d.saturating_sub(crate::datetime::now_unix()))
    }

    /// Pause + clear the timer once its deadline passes (called from the tick).
    pub(super) fn tick_sleep_timer(&mut self) {
        if let Some(deadline) = self.fx.sleep_until
            && crate::datetime::now_unix() >= deadline
        {
            self.fx.sleep_until = None;
            if self.player.status == Status::Playing {
                self.player.status = Status::Paused;
                self.engine.send(AudioCommand::Pause);
            }
            self.notify("Sleep timer — paused".into());
        }
    }

    // ---- ReplayGain (normalization) --------------------------------------

    /// Read a file's ReplayGain once and return the playback gain factor plus a
    /// short status string **only when a gain is actually applied** — `None` when
    /// RG is off or the track has no RG tags, so the persistent status-bar
    /// indicator stays hidden unless normalization is really doing something.
    /// Reads tags from disk — call on track-load / mode-change, never per frame.
    pub(super) fn replaygain_eval(&self, path: &std::path::Path) -> (f32, Option<String>) {
        if self.config.replaygain == 0 {
            return (1.0, None);
        }
        let rg = crate::tags::read_replaygain(path);
        let has = match self.config.replaygain {
            1 => rg.track_gain_db.is_some(),
            _ => rg.album_gain_db.or(rg.track_gain_db).is_some(),
        };
        let factor = rg.gain_factor(self.config.replaygain, self.config.replaygain_preamp);
        let status = has.then(|| {
            let db = 20.0 * factor.max(1e-6).log10();
            format!("{db:+.1} dB")
        });
        (factor, status)
    }

    /// Re-evaluate + apply ReplayGain to the loaded track (after a mode change).
    pub(super) fn refresh_replaygain(&mut self) {
        let path = self
            .loaded_track
            .and_then(|id| self.library.track(id))
            .map(|t| t.path.clone());
        match path {
            Some(p) => {
                let (gain, status) = self.replaygain_eval(&p);
                self.fx.rg_status = status;
                self.engine.send(AudioCommand::SetGain(gain));
            }
            None => self.fx.rg_status = None,
        }
    }

    /// Cycle ReplayGain mode: Off → Track → Album → Off.
    pub(super) fn cycle_replaygain(&mut self) {
        self.config.replaygain = (self.config.replaygain + 1) % 3;
        self.config.save();
        self.refresh_replaygain();
        let mode = match self.config.replaygain {
            1 => "Track",
            2 => "Album",
            _ => "Off",
        };
        // include this track's actual gain so it's clear whether it's working.
        // The persistent indicator is now hidden on untagged tracks, so surface
        // the "no tags" reason here — in this explicit, transient toast only.
        let detail = match (self.config.replaygain, &self.fx.rg_status) {
            (0, _) => String::new(),
            (_, Some(s)) => format!(" — this track: {s}"),
            _ => " — no RG tags on this track".into(),
        };
        self.notify(format!("ReplayGain: {mode}{detail}"));
    }

    // ---- crossfade -------------------------------------------------------

    /// Set the crossfade length (ms): persist, tell the engine, refresh preload.
    pub(super) fn set_crossfade(&mut self, ms: u32) {
        self.config.crossfade_ms = ms.min(12000);
        self.config.save();
        self.engine
            .send(AudioCommand::SetCrossfade(self.config.crossfade_ms));
        self.update_gapless_next(); // crossfade needs the next track preloaded
    }

    // ---- A-B loop --------------------------------------------------------

    /// Cycle the A-B loop: first press sets A, second sets B (and activates the
    /// loop), third clears it.
    pub(super) fn ab_loop_cycle(&mut self) {
        let mmss = |d: Duration| {
            let s = d.as_secs();
            format!("{}:{:02}", s / 60, s % 60)
        };
        let pos = self.player.elapsed;
        if self.fx.ab_loop.is_some() {
            self.fx.ab_loop = None;
            self.fx.ab_a = None;
            self.notify("A-B loop off".into());
        } else if let Some(a) = self.fx.ab_a {
            let (a, b) = if pos >= a { (a, pos) } else { (pos, a) };
            if b > a {
                self.fx.ab_loop = Some((a, b));
                self.fx.ab_a = None;
                self.notify(format!("A-B loop {}–{}", mmss(a), mmss(b)));
            } else {
                self.notify("A-B: move forward, then set B".into());
            }
        } else {
            self.fx.ab_a = Some(pos);
            self.notify(format!("A-B: A at {}", mmss(pos)));
        }
    }

    /// If an A-B loop is active and playback has passed B, jump back to A.
    pub(super) fn enforce_ab_loop(&mut self) {
        if let Some((a, b)) = self.fx.ab_loop
            && self.player.elapsed >= b
        {
            self.player.elapsed = a;
            self.engine.send(AudioCommand::Seek(a));
        }
    }
}
