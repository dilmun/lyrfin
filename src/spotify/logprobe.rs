//! A process-wide log hook that detects librespot's **audio-key denial** —
//! Spotify refusing to grant the per-track decryption key to a third-party
//! client. When it happens librespot only logs an error and then stalls silently
//! ("continuing without decryption"), so there is no player event to react to.
//! This probe watches the log stream for that exact error and raises a flag the
//! session loop drains, so the app can report it instead of buffering forever.
//!
//! It wraps `env_logger`, so `RUST_LOG=…` still works for debugging; with no
//! `RUST_LOG` set it stays silent (the TUI owns stdout) but still watches for the
//! error at `Error` level.

use std::sync::atomic::{AtomicBool, Ordering};

use std::collections::VecDeque;
use std::sync::Mutex;

use log::{Level, LevelFilter, Log, Metadata, Record};

/// Raised when librespot logs an audio-key error. The Spotify session loop drains
/// it (swap-to-false) and forwards `SessionEvent::AudioKeyDenied`. Global because
/// the logger is installed once for the whole process and outlives each session.
pub static AUDIO_KEY_DENIED: AtomicBool = AtomicBool::new(false);

/// Sticky companion to [`AUDIO_KEY_DENIED`]: set on the SAME audio-key error but
/// never auto-consumed — it stays raised for the whole session (reset only when a
/// new one starts). [`AUDIO_KEY_DENIED`] is a one-shot the session loop swaps to
/// false the instant it forwards the event, so a later failure toast can't read it;
/// this one records that a key WAS denied this session, so the diagnosis message can
/// tell Spotify's account-level audio-key block apart from a mere re-auth lapse.
/// (A region-lock raises a different librespot error, so it never sets this.)
pub static AUDIO_KEY_BLOCKED: AtomicBool = AtomicBool::new(false);

/// librespot's target for the audio-key channel; an `Error` record here means the
/// key request was refused (`error audio key …` / "Service unavailable").
const AUDIO_KEY_TARGET: &str = "librespot_core::audio_key";

/// Recent librespot WARN/ERROR log lines, captured so the app can surface the real
/// reason a track failed (e.g. "Track is not available", "premium account required")
/// in its own error log — without the user needing `RUST_LOG` + a stderr redirect.
/// Bounded ring; drained each loop by [`crate::app::AppState`].
static LIBRESPOT_LOG: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
const LIBRESPOT_LOG_CAP: usize = 64;

/// Drain the captured librespot warnings/errors (the app folds them into its error
/// log). Empty in the common case — librespot logs at WARN+ only on real problems.
pub fn drain_librespot_log() -> Vec<String> {
    let mut q = LIBRESPOT_LOG.lock().unwrap_or_else(|e| e.into_inner());
    q.drain(..).collect()
}

/// Capture a librespot WARN/ERROR record into [`LIBRESPOT_LOG`] (deduping an
/// immediate repeat so a retry storm doesn't flood the log).
fn capture_librespot(record: &Record) {
    if record.level() > Level::Warn || !record.target().starts_with("librespot") {
        return;
    }
    let line = format!("{}: {}", record.target(), record.args());
    // Skip benign, self-recovering warnings: librespot logs one per CDN URL it
    // fails over to (the #1725 fix) when the first node is unhealthy — that's
    // normal playback, not an error worth surfacing in the app's error log.
    if line.contains("trying next") {
        return;
    }
    let mut q = LIBRESPOT_LOG.lock().unwrap_or_else(|e| e.into_inner());
    if q.back() == Some(&line) {
        return; // collapse consecutive duplicates
    }
    if q.len() >= LIBRESPOT_LOG_CAP {
        q.pop_front();
    }
    q.push_back(line);
}

struct Probe {
    /// Forwards to stderr per `RUST_LOG`; `None` when `RUST_LOG` is unset (silent).
    inner: Option<env_logger::Logger>,
}

impl Log for Probe {
    fn enabled(&self, meta: &Metadata) -> bool {
        // Always observe `Warn`+ records (so the audio-key probe + the librespot
        // capture fire even with no `RUST_LOG`); otherwise defer to the wrapped
        // logger's filter. Forwarding to stderr still happens only when inner exists.
        meta.level() <= Level::Warn || self.inner.as_ref().is_some_and(|l| l.enabled(meta))
    }

    fn log(&self, record: &Record) {
        if record.level() == Level::Error && record.target().starts_with(AUDIO_KEY_TARGET) {
            AUDIO_KEY_DENIED.store(true, Ordering::Relaxed);
            AUDIO_KEY_BLOCKED.store(true, Ordering::Relaxed);
        }
        capture_librespot(record); // librespot WARN/ERROR → the in-app error log
        if let Some(inner) = &self.inner
            && inner.enabled(record.metadata())
        {
            inner.log(record);
        }
    }

    fn flush(&self) {
        if let Some(inner) = &self.inner {
            inner.flush();
        }
    }
}

/// Install the global logger once, at startup. Honours `RUST_LOG` (forwarding to
/// stderr) and always raises [`AUDIO_KEY_DENIED`] on a librespot audio-key error.
/// Safe to call once; a second logger install is a no-op.
pub fn init() {
    let inner =
        std::env::var_os("RUST_LOG").map(|_| env_logger::Builder::from_default_env().build());
    // Ensure `Warn`+ records reach the probe even when `RUST_LOG` is unset (so it
    // sees the audio-key error AND captures librespot's warnings for the in-app
    // log); when it is set, honour its verbosity. Capturing ≠ forwarding to stderr
    // (that still needs `inner`), so the TUI stays clean.
    let max = inner
        .as_ref()
        .map(|l| l.filter())
        .unwrap_or(LevelFilter::Off)
        .max(LevelFilter::Warn);
    if log::set_boxed_logger(Box::new(Probe { inner })).is_ok() {
        log::set_max_level(max);
    }
}
