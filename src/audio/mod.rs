//! Audio engine boundary.
//!
//! The engine runs on its own thread and owns the decoder + output stream. The
//! app talks to it only through [`AudioCommand`]s and receives [`AudioEvent`]s
//! back — never blocking the UI. M4 implements `CpalEngine` (symphonia decode →
//! cpal output, with a lock-free ring buffer feeding both the DAC and the
//! visualizer FFT).

pub mod engine;
pub mod eq;
pub mod http_source;
pub mod resample;
pub mod stretch;
pub mod timeshift;
pub mod visualizer;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// A source of already-decoded interleaved f32 PCM at the device's sample rate,
/// pulled by the engine controller and pushed through the same ring buffer as
/// local decoding — so the visualizer, output volume/gain, and cpal callback all
/// work unchanged. Used to play audio that lyrfin doesn't decode itself (Spotify
/// via librespot). The implementor is the producer side of a lock-free queue;
/// the engine is the single consumer (keeps the ring strictly SPSC).
pub trait ExternalAudioSource: Send + Sync + std::fmt::Debug {
    /// Fill up to `out.len()` interleaved-stereo f32 samples at [`Self::sample_rate`];
    /// return how many were written (0 = nothing buffered yet). The engine
    /// resamples to the device rate, so the source needn't match it.
    fn pull(&self, out: &mut [f32]) -> usize;
    /// Source sample rate in Hz (e.g. 44100 for Spotify/librespot).
    fn sample_rate(&self) -> u32;
    /// Still connected/playing? `false` lets the engine idle this source.
    fn is_active(&self) -> bool;
}

/// Commands sent UI → engine.
#[derive(Debug, Clone)]
pub enum AudioCommand {
    Load(PathBuf),
    /// Open + play an HTTP audio stream (internet radio / streamed podcast). A live
    /// stream carries no duration; when `dvr` is `Some(window)` it's wrapped in a
    /// timeshift buffer so it can be paused, rewound within `window`, and caught
    /// back up to live. Finite (ranged) streams ignore `dvr` — they already seek.
    LoadStream {
        url: String,
        dvr: Option<Duration>,
    },
    /// Jump a timeshifted live stream back to the live edge (newest buffered audio).
    GoLive,
    /// Take audio from an external producer (Spotify/librespot) instead of the
    /// internal decoder. Its PCM flows through the ring, so volume + visualizer
    /// apply; position/duration are reported by the source's own controller.
    SetExternalSource(Arc<dyn ExternalAudioSource>),
    /// Stop pulling from the external source (back to internal decoding).
    ClearExternalSource,
    Play,
    Pause,
    Stop,
    Seek(Duration),
    SetVolume(u8),
    SetSpeed(f32),
    /// Linear playback gain (ReplayGain / normalization), 1.0 = unchanged.
    SetGain(f32),
    /// Preload the next track for gapless playback (`None` disables it). The
    /// engine decodes it into the same buffer when the current track ends, with
    /// no silence between, and reports [`AudioEvent::Advanced`] at the seam.
    SetNext(Option<PathBuf>),
    /// Crossfade duration in milliseconds (0 = off). When set, the preloaded
    /// next track is mixed in over this window as the current one fades out.
    SetCrossfade(u32),
    /// Enable trimming of leading/trailing near-silence at track boundaries, so
    /// a seamless transition flows real audio → real audio instead of through
    /// each file's silent padding. Interior silence is preserved.
    SetSilenceSkip(bool),
    /// Update the 10-band equalizer (enable + preamp + per-band gains). Applies
    /// to newly produced audio on the controller thread — playback never
    /// restarts; the change is audible within the ~sub-0.5s output buffer.
    SetEq(eq::EqConfig),
}

/// Events sent engine → UI.
#[derive(Debug, Clone)]
pub enum AudioEvent {
    /// Playback position update (emitted a few times per second).
    Progress(Duration),
    /// Decoded total duration once the stream opens.
    Duration(Duration),
    /// Current track finished — the app decides what to play next.
    Finished,
    /// The preloaded next track started playing gaplessly; the app should sync
    /// its "now playing" to the next queue entry (no reload needed).
    Advanced,
    /// A new spectrum frame for the visualizer.
    Spectrum(Vec<f32>),
    /// ICY "now playing" metadata from a radio stream (the current song/show).
    IcyTitle(String),
    /// Timeshift (DVR) window for a live stream, in seconds since tune-in: the
    /// oldest seekable position and the live edge. Emitted while a buffered live
    /// stream plays so the UI can draw a seekable radio bar. `Progress` carries the
    /// play position within `[start, live]`.
    DvrWindow { start: f64, live: f64 },
    /// Non-fatal decode/output error to surface in the notification bar.
    Error(String),
}

/// Backend-agnostic engine contract. Lets us swap rodio/cpal/symphonia or
/// provide a `NullEngine` for tests.
pub trait AudioEngine: Send {
    fn send(&self, cmd: AudioCommand);
    fn try_recv(&self) -> Option<AudioEvent>;
}

/// No-op engine used by the scaffold and unit tests.
#[derive(Default)]
pub struct NullEngine;

impl AudioEngine for NullEngine {
    fn send(&self, _cmd: AudioCommand) {}
    fn try_recv(&self) -> Option<AudioEvent> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// The shape the Spotify/librespot sink will use: a producer fills it, the
    /// engine controller pulls. (Phase 4 uses a lock-free ring; this is enough to
    /// pin the trait contract + object-safety the engine depends on.)
    #[derive(Debug, Default)]
    struct DummySource {
        buf: Mutex<VecDeque<f32>>,
        active: AtomicBool,
    }
    impl DummySource {
        fn push(&self, s: &[f32]) {
            self.buf.lock().unwrap().extend(s.iter().copied());
        }
    }
    impl ExternalAudioSource for DummySource {
        fn pull(&self, out: &mut [f32]) -> usize {
            let mut q = self.buf.lock().unwrap();
            let n = out.len().min(q.len());
            for slot in out.iter_mut().take(n) {
                *slot = q.pop_front().unwrap();
            }
            n
        }
        fn sample_rate(&self) -> u32 {
            44100
        }
        fn is_active(&self) -> bool {
            self.active.load(Ordering::Relaxed)
        }
    }

    #[test]
    fn external_source_pulls_buffered_samples() {
        let src = DummySource::default();
        src.active.store(true, Ordering::Relaxed);
        src.push(&[0.1, -0.2, 0.3]);
        // used as a trait object exactly like the engine holds it
        let dyn_src: Arc<dyn ExternalAudioSource> = Arc::new(src);
        let mut out = [0.0f32; 4];
        assert_eq!(dyn_src.pull(&mut out), 3);
        assert_eq!(&out[..3], &[0.1, -0.2, 0.3]);
        assert!(dyn_src.is_active());
        // nothing left → 0 (the engine then idles this source, no busy-spin)
        assert_eq!(dyn_src.pull(&mut [0.0; 4]), 0);
    }

    #[test]
    fn external_source_arc_is_clonable_and_debug() {
        let a: Arc<dyn ExternalAudioSource> = Arc::new(DummySource::default());
        let _b = a.clone(); // the pump loop clones the Arc each iteration
        let _ = format!("{a:?}"); // AudioCommand derives Debug → trait requires it
        assert!(!a.is_active());
    }
}
