//! OS "Now Playing" / media-control integration.
//!
//! Reports the current track to the system (macOS Control Center + lock screen,
//! Linux MPRIS) and receives transport commands back (media keys, AirPods,
//! Control Center / `playerctl`). `souvlaki` wraps the platform frameworks; this
//! module is the thin seam between it and lyrfin's core.
//!
//! ## Threading / run loop
//! `souvlaki` does NOT run an event loop of its own. On macOS its command
//! callbacks are delivered as blocks on the **main thread's Cocoa run loop**,
//! which lyrfin's crossterm poll loop never runs — so [`MediaBridge::pump`] services
//! that run loop once per event-loop iteration (a no-op elsewhere; Linux's zbus
//! backend self-threads). Everything here therefore lives on the main thread,
//! owned by the event loop (not `AppState`), so the platform handle — `!Send` on
//! macOS — never leaks into the core state. The command handler forwards into a
//! channel; `AppState` only ever sees the platform-neutral [`MediaCommand`].

use std::time::Duration;

/// Default seek step (seconds) for a direction-only OS seek command, matching
/// lyrfin's on-screen ±10s transport buttons. (Only the backends that receive OS
/// seek events reference it.)
#[cfg(any(target_os = "macos", target_os = "linux"))]
const SEEK_STEP_SECS: i64 = 10;

/// A transport command from the OS media controls (media keys, AirPods, Control
/// Center / MPRIS). Platform-neutral so `AppState` routes it without touching
/// `souvlaki`. See `AppState::on_media_command`.
// On targets without a media backend (Windows/other) the variants are never
// constructed — `from_event` is macOS/Linux-only — so dead_code fires there; they
// are the real cross-platform command surface the wired backends produce.
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MediaCommand {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Stop,
    /// Seek to an absolute position, in seconds.
    SeekTo(f64),
    /// Seek by a relative delta, in seconds (negative = backward).
    SeekBy(i64),
}

/// A platform-neutral snapshot of "what's playing", handed to the OS by
/// [`MediaBridge::publish`]. Built by `AppState::now_playing_snapshot` from
/// whichever source (local / Spotify / radio) currently owns the now-bar.
// On targets without a media backend (Windows/other) the no-op `publish` reads
// none of these fields, so dead_code fires there; the wired backends read them all.
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
#[derive(Clone, PartialEq)]
pub struct NowPlayingSnapshot {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Duration,
    pub elapsed: Duration,
    pub playing: bool,
    /// A `file://…` path (local cover on disk) or `https://…` URL (Spotify) — the
    /// OS reads the image itself. `None` = no artwork.
    pub cover: Option<String>,
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
impl MediaCommand {
    /// Map a `souvlaki` event onto a transport command, or `None` for events lyrfin
    /// doesn't act on (volume / open-uri / raise / quit).
    fn from_event(ev: souvlaki::MediaControlEvent) -> Option<Self> {
        use souvlaki::{MediaControlEvent as E, SeekDirection as D};
        Some(match ev {
            E::Play => Self::Play,
            E::Pause => Self::Pause,
            E::Toggle => Self::Toggle,
            E::Next => Self::Next,
            E::Previous => Self::Previous,
            E::Stop => Self::Stop,
            E::Seek(D::Forward) => Self::SeekBy(SEEK_STEP_SECS),
            E::Seek(D::Backward) => Self::SeekBy(-SEEK_STEP_SECS),
            E::SeekBy(dir, dur) => {
                let secs = dur.as_secs() as i64;
                Self::SeekBy(if matches!(dir, D::Backward) {
                    -secs
                } else {
                    secs
                })
            }
            E::SetPosition(pos) => Self::SeekTo(pos.0.as_secs_f64()),
            E::SetVolume(_) | E::OpenUri(_) | E::Raise | E::Quit => return None,
        })
    }
}

// ---------------------------------------------------------------------------
// Real bridge (macOS + Linux)
// ---------------------------------------------------------------------------

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub use real::MediaBridge;

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod real {
    use super::*;
    use crossbeam_channel::{Receiver, Sender};
    use souvlaki::{MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig};

    /// An elapsed jump (seconds) beyond this — forward past a normal inter-publish
    /// gap, or any backward move — is treated as a user seek and re-pushed so the
    /// OS scrubber follows. Steady playback advances far less than this per publish.
    const SEEK_JUMP_FWD: f64 = 5.0;
    const SEEK_JUMP_BACK: f64 = -1.5;

    /// Owns the `souvlaki` handle and the command channel. Created and used on the
    /// main thread by the event loop.
    pub struct MediaBridge {
        /// `None` when disabled by config or when the platform backend failed to
        /// initialise — every method then degrades to a no-op.
        controls: Option<MediaControls>,
        rx: Receiver<MediaCommand>,
        /// What we last handed the OS, so we only push on real change.
        last: Option<NowPlayingSnapshot>,
    }

    impl MediaBridge {
        /// Build the bridge. `enabled` mirrors `config.os_media_controls`; when
        /// false (or if the OS backend can't start) the bridge is inert.
        pub fn new(display_name: &str, enabled: bool) -> Self {
            let (tx, rx) = crossbeam_channel::unbounded();
            let controls = if enabled {
                Self::init(display_name, tx)
            } else {
                None
            };
            Self {
                controls,
                rx,
                last: None,
            }
        }

        fn init(display_name: &str, tx: Sender<MediaCommand>) -> Option<MediaControls> {
            let config = PlatformConfig {
                dbus_name: "lyrfin",
                display_name,
                hwnd: None,
            };
            let mut controls = match MediaControls::new(config) {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("OS media controls unavailable: {e:?}");
                    return None;
                }
            };
            let attach = controls.attach(move |ev| {
                if let Some(cmd) = MediaCommand::from_event(ev) {
                    let _ = tx.send(cmd);
                }
            });
            if let Err(e) = attach {
                log::warn!("OS media controls attach failed: {e:?}");
                return None;
            }
            Some(controls)
        }

        /// Service the main-thread Cocoa run loop so macOS command callbacks fire.
        /// No-op on Linux (zbus self-threads).
        pub fn pump(&self) {
            #[cfg(target_os = "macos")]
            macos_runloop::pump();
        }

        /// Non-blocking: the next command from the OS, if any.
        pub fn poll_command(&self) -> Option<MediaCommand> {
            self.rx.try_recv().ok()
        }

        /// Push the snapshot to the OS, diffing against the last publish so
        /// `set_metadata` fires only on identity change and `set_playback` only on
        /// a play/pause flip, a track change, or a seek — never per frame (the OS
        /// extrapolates position between updates). `None` clears the OS info.
        pub fn publish(&mut self, snap: Option<&NowPlayingSnapshot>) {
            let Some(controls) = self.controls.as_mut() else {
                return;
            };
            match snap {
                None => {
                    if self.last.take().is_some() {
                        let _ = controls.set_playback(MediaPlayback::Stopped);
                        let _ = controls.set_metadata(MediaMetadata::default());
                    }
                }
                Some(s) => {
                    let meta_changed = self.last.as_ref().is_none_or(|l| {
                        l.title != s.title
                            || l.artist != s.artist
                            || l.album != s.album
                            || l.duration != s.duration
                            || l.cover != s.cover
                    });
                    if meta_changed {
                        let _ = controls.set_metadata(MediaMetadata {
                            title: Some(&s.title),
                            artist: Some(&s.artist),
                            album: Some(&s.album),
                            cover_url: s.cover.as_deref(),
                            duration: (!s.duration.is_zero()).then_some(s.duration),
                        });
                    }
                    let playback_changed = meta_changed
                        || self.last.as_ref().is_none_or(|l| {
                            l.playing != s.playing || is_seek(l.elapsed, s.elapsed)
                        });
                    if playback_changed {
                        let progress = Some(MediaPosition(s.elapsed));
                        let _ = controls.set_playback(if s.playing {
                            MediaPlayback::Playing { progress }
                        } else {
                            MediaPlayback::Paused { progress }
                        });
                    }
                    self.last = Some(s.clone());
                }
            }
        }
    }

    /// Whether the elapsed moved by more than steady playback would between two
    /// publishes (called ~once per event-loop iteration) — i.e. a user seek.
    fn is_seek(prev: Duration, now: Duration) -> bool {
        let d = now.as_secs_f64() - prev.as_secs_f64();
        // outside the steady-advance band [back-tolerance, forward-gap] → a seek
        !(SEEK_JUMP_BACK..=SEEK_JUMP_FWD).contains(&d)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn steady_advance_is_not_a_seek_but_jumps_are() {
            let s =
                |a: f64, b: f64| is_seek(Duration::from_secs_f64(a), Duration::from_secs_f64(b));
            assert!(!s(10.0, 10.25), "a frame of steady playback");
            assert!(!s(10.0, 14.0), "a normal inter-publish gap");
            assert!(s(10.0, 30.0), "a big forward jump = a seek");
            assert!(s(30.0, 10.0), "any backward move = a seek");
        }
    }

    #[cfg(target_os = "macos")]
    mod macos_runloop {
        use std::ffi::c_void;

        type CFRunLoopMode = *const c_void; // CFStringRef
        type CFTimeInterval = f64;
        type Boolean = u8;

        // CFRunLoopRunResult
        const HANDLED_SOURCE: i32 = 4;

        #[link(name = "CoreFoundation", kind = "framework")]
        unsafe extern "C" {
            static kCFRunLoopDefaultMode: CFRunLoopMode;
            fn CFRunLoopRunInMode(
                mode: CFRunLoopMode,
                seconds: CFTimeInterval,
                return_after_source_handled: Boolean,
            ) -> i32;
        }

        /// Drain all ready sources on the current (main) thread's run loop without
        /// blocking, so souvlaki's MediaPlayer command blocks fire. Bounded so a
        /// flood of sources can never stall the UI thread.
        pub fn pump() {
            for _ in 0..32 {
                // SAFETY: `kCFRunLoopDefaultMode` is a framework-owned immortal
                // CFString. `CFRunLoopRunInMode` runs the *current* thread's run loop
                // (this fn is only ever called from the main thread) for 0s, returning
                // immediately after at most one ready source. No Rust pointers cross
                // the boundary.
                let r = unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.0, 1) };
                if r != HANDLED_SOURCE {
                    break;
                }
            }
        }
    }
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod command_tests {
    use super::*;
    use souvlaki::{MediaControlEvent as E, MediaPosition, SeekDirection as D};

    #[test]
    fn maps_souvlaki_events_onto_transport_commands() {
        assert_eq!(
            MediaCommand::from_event(E::Toggle),
            Some(MediaCommand::Toggle)
        );
        assert_eq!(MediaCommand::from_event(E::Next), Some(MediaCommand::Next));
        assert_eq!(
            MediaCommand::from_event(E::Seek(D::Forward)),
            Some(MediaCommand::SeekBy(SEEK_STEP_SECS))
        );
        assert_eq!(
            MediaCommand::from_event(E::SeekBy(D::Backward, Duration::from_secs(7))),
            Some(MediaCommand::SeekBy(-7))
        );
        assert_eq!(
            MediaCommand::from_event(E::SetPosition(MediaPosition(Duration::from_secs(42)))),
            Some(MediaCommand::SeekTo(42.0))
        );
        // events lyrfin doesn't act on are dropped, not misrouted
        assert_eq!(MediaCommand::from_event(E::Raise), None);
        assert_eq!(MediaCommand::from_event(E::SetVolume(0.5)), None);
    }
}

// ---------------------------------------------------------------------------
// No-op bridge (Windows and any other target — no souvlaki dependency there)
// ---------------------------------------------------------------------------

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub use noop::MediaBridge;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod noop {
    use super::*;

    /// Inert stand-in on platforms without a wired media-control backend (e.g.
    /// Windows SMTC, which needs a hidden message-pump window a console TUI lacks).
    /// Keeps the event-loop wiring identical everywhere.
    pub struct MediaBridge;

    impl MediaBridge {
        pub fn new(_display_name: &str, _enabled: bool) -> Self {
            MediaBridge
        }
        pub fn pump(&self) {}
        pub fn poll_command(&self) -> Option<MediaCommand> {
            None
        }
        pub fn publish(&mut self, _snap: Option<&NowPlayingSnapshot>) {}
    }
}
