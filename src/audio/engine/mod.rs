//! Real audio engine: a symphonia decoder feeding a persistent cpal output
//! stream.
//!
//! Threading: the public [`CpalEngine`] is a thin handle (command sender +
//! event receiver). A dedicated controller thread owns the decoder and the cpal
//! stream; the stream's audio callback drains a shared sample ring while the
//! controller fills it and reports progress. The UI thread never blocks.
//!
//! The output stream is built **once** at the device's default format. Decoded
//! audio is linearly resampled to the device sample-rate and channel-mapped to
//! the device channel count, so any file plays on any device.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use ringbuf::HeapRb;
use ringbuf::traits::{Consumer, Producer, Split};
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;

use crate::audio::eq::Equalizer;
use crate::audio::http_source::{HttpRangeSource, HttpStream};
use crate::audio::resample::Resampler;
use crate::audio::stretch::TimeStretch;
use crate::audio::timeshift::Timeshift;
use crate::audio::visualizer::{Analyzer, DEFAULT_BANDS};
use crate::audio::{AudioCommand, AudioEngine, AudioEvent, ExternalAudioSource};

mod decode;
use decode::{
    SilenceTrim, build_stream, crossfade_mix, decode_one, open_stream, open_track, reopen_timeshift,
};

/// Minimum position change (seconds) before another `Progress` is emitted. ~20 Hz
/// — fine enough for a smooth karaoke lyric wipe, coarse enough not to flood the
/// event channel. (The old clock emitted only once per whole second, which made
/// the lyric highlight snap and lag.)
const PROGRESS_STEP_SECS: f64 = 0.05;
/// Upper bound (seconds) on the output-latency correction, so a bogus backend
/// timestamp can't shove the clock wildly out of place.
const MAX_LATENCY_SECS: f64 = 1.0;

/// Shared playback state between the controller and the cpal callback.
///
/// The sample stream itself rides a lock-free SPSC ring (producer = controller,
/// consumer = callback); this struct only carries the scalar/atomic state plus a
/// `flush` request the controller raises so the callback drops buffered audio on
/// a seek/load/stop (the consumer is the only side that may drain the ring).
struct Shared {
    volume: AtomicU32,    // 0..=100
    gain_bits: AtomicU32, // f32 bits: linear ReplayGain/normalization multiplier
    playing: AtomicBool,
    samples_played: AtomicU64, // device samples written (all channels)
    flush: AtomicBool,         // controller → callback: clear the ring (seek/load/stop)
    /// Output latency in microseconds: how long between the callback handing a
    /// sample to cpal and it being audible (device/OS buffer, large on Bluetooth).
    /// From cpal's playback-vs-callback timestamp; subtracted from the reported
    /// position so lyrics/seek align with what's *heard*, not what's queued.
    out_latency_us: AtomicU64,
}

pub struct CpalEngine {
    cmd_tx: Sender<AudioCommand>,
    evt_rx: Receiver<AudioEvent>,
}

impl CpalEngine {
    /// Try to start the engine. Fails if no output device/stream is available.
    pub fn new(initial_volume: u8) -> anyhow::Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow::anyhow!("no default audio output device"))?;
        let supported = device.default_output_config()?;
        let fmt = supported.sample_format();
        let dev_rate = supported.sample_rate(); // cpal 0.18: SampleRate is a u32 alias
        let dev_ch = supported.channels() as usize;
        let mut config: cpal::StreamConfig = supported.into();
        // Predictable callback size (≈11ms @ 44.1k) instead of the device's
        // unbounded default period — bounds latency + the per-callback work.
        config.buffer_size = cpal::BufferSize::Fixed(512);

        let (cmd_tx, cmd_rx) = unbounded::<AudioCommand>();
        let (evt_tx, evt_rx) = unbounded::<AudioEvent>();
        let shared = Arc::new(Shared {
            volume: AtomicU32::new(initial_volume as u32),
            gain_bits: AtomicU32::new(1.0f32.to_bits()),
            playing: AtomicBool::new(false),
            samples_played: AtomicU64::new(0),
            flush: AtomicBool::new(false),
            out_latency_us: AtomicU64::new(0),
        });

        // Build the stream on the controller thread and confirm success before
        // reporting the engine as ready (so callers can fall back cleanly).
        let (ready_tx, ready_rx) = unbounded::<Result<(), String>>();
        let shared2 = shared.clone();
        let evt_tx2 = evt_tx.clone();
        std::thread::Builder::new()
            .name("lyrfin-audio".into())
            .spawn(move || {
                controller(
                    device, config, fmt, dev_rate, dev_ch, cmd_rx, evt_tx2, shared2, ready_tx,
                )
            })
            .map_err(|e| anyhow::anyhow!("spawn audio thread: {e}"))?;

        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { cmd_tx, evt_rx }),
            Ok(Err(e)) => Err(anyhow::anyhow!(e)),
            Err(_) => Err(anyhow::anyhow!("audio thread died during init")),
        }
    }
}

impl AudioEngine for CpalEngine {
    fn send(&self, cmd: AudioCommand) {
        let _ = self.cmd_tx.send(cmd);
    }
    fn try_recv(&self) -> Option<AudioEvent> {
        self.evt_rx.try_recv().ok()
    }
}

/// An off-thread stream-open result delivered to the controller: the open
/// generation, then either the decoder + its (optional, finite-file) duration, or
/// an error string.
type StreamOpen = (
    u64,
    Result<(Decode, Option<Duration>, Option<Arc<Timeshift>>), String>,
);

/// Per-track decode state.
struct Decode {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    resampler: Resampler,
    finished: bool,
    /// Interleaved-f32 scratch reused across packets: symphonia 0.6 copies each
    /// decoded buffer straight into it (`copy_to_vec_interleaved`, which clears +
    /// grows it as needed), replacing 0.5's `SampleBuffer` + `copy_interleaved_ref`.
    sbuf: Vec<f32>,
    /// Device channel count — the interleave width silence-skip trims on.
    dev_ch: usize,
    /// Per-track leading/trailing silence trimmer (used when silence-skip is on).
    trim: SilenceTrim,
}

/// Controller-thread playback state (current decoder + gapless + speed).
struct Ctl {
    active: Option<Decode>,
    /// Path preloaded for gapless playback (set via `SetNext`).
    next_path: Option<PathBuf>,
    /// Position-clock baseline: `samples_played` and the track-seconds it maps to.
    /// Track position = `pos_base_secs + (played - pos_base_out)/rate * speed`,
    /// so it stays correct across gapless advances, seeks, and speed changes.
    pos_base_out: u64,
    pos_base_secs: f64,
    /// DAC sample index at which a gaplessly-loaded next track begins; when the
    /// clock passes it we emit `Advanced` and rebaseline the position clock.
    pending: Option<u64>,
    /// Pitch-preserved playback speed (1.0 = normal) + its time-stretcher.
    speed: f32,
    stretch: TimeStretch,
    /// Current track duration in seconds, for the crossfade trigger.
    cur_dur: Option<f64>,
    /// Crossfade length in device frames (0 = off).
    xfade_len: usize,
    /// Trim leading/trailing near-silence at track boundaries (config toggle).
    silence_skip: bool,
    /// The incoming track being faded in (crossfade in progress when `Some`).
    fade: Option<Decode>,
    /// Device frames mixed so far in the current crossfade.
    xfade_done: usize,
    /// Duration (seconds) of the incoming crossfade track, for the next trigger.
    fade_dur: Option<f64>,
    /// Staging buffers (interleaved) for the outgoing (A) and incoming (B)
    /// tracks while their differently-sized decode chunks are aligned + mixed.
    a_stage: Vec<f32>,
    b_stage: Vec<f32>,
    /// Monotonic id for each radio-stream open; only the latest result installs
    /// (rapid Next presses don't pile up). `stream_tx` delivers opened decoders
    /// (or an error) from the off-thread opener back to the controller loop.
    stream_gen: u64,
    stream_tx: Sender<StreamOpen>,
    /// External PCM source (Spotify/librespot) that feeds the ring instead of the
    /// internal decoder. When set, decoding is bypassed; `pull()` fills the ring.
    external: Option<Arc<dyn ExternalAudioSource>>,
    /// Resampler for the external source (its rate → device rate). Built when the
    /// source is installed; reuses the same linear resampler as file playback.
    ext_resampler: Option<Resampler>,
    /// Timeshift (DVR) buffer for the active live stream, when one is buffered.
    /// Lets the controller report the seekable window + jump to the live edge; the
    /// decoder already reads/seeks it as an ordinary seekable source. `None` for
    /// local files, podcasts, and un-buffered live streams.
    dvr: Option<Arc<Timeshift>>,
    /// A DVR seek target (seconds since tune-in) to apply after the command drain.
    /// Rapid seeks (a held key) coalesce here so only the last is applied — one
    /// decoder reopen, not a flood.
    pending_seek: Option<f64>,
    /// When the last DVR reopen ran, to throttle held-key scrubbing (bounds the
    /// reopen rate so it can't tax the CPU). No allocation; not on the audio path.
    last_seek: Option<std::time::Instant>,
    /// 10-band equalizer applied to every source's output just before it enters
    /// the ring (so local files, crossfades, and the Spotify bridge are all EQ'd
    /// by one stage). Owned solely by this controller thread — parameter updates
    /// arrive as `SetEq` commands, so the audio path never locks.
    eq: Equalizer,
}

impl Ctl {
    fn new(dev_rate: u32, dev_ch: usize, stream_tx: Sender<StreamOpen>) -> Self {
        Self {
            active: None,
            next_path: None,
            pos_base_out: 0,
            pos_base_secs: 0.0,
            pending: None,
            speed: 1.0,
            stretch: TimeStretch::new(dev_ch),
            cur_dur: None,
            xfade_len: 0,
            silence_skip: true,
            fade: None,
            xfade_done: 0,
            fade_dur: None,
            a_stage: Vec::new(),
            b_stage: Vec::new(),
            stream_gen: 0,
            stream_tx,
            external: None,
            ext_resampler: None,
            dvr: None,
            pending_seek: None,
            last_seek: None,
            eq: Equalizer::new(dev_rate, dev_ch),
        }
    }

    /// Apply a coalesced DVR seek (seconds since tune-in): move the ring cursor and
    /// rebuild the decoder there, then rebaseline the position clock and flush the
    /// output. Robust + codec-agnostic (see [`reopen_timeshift`]); a bad reopen is
    /// non-fatal — the previous decoder keeps playing. Called once per loop after
    /// the command drain, so a burst of seeks costs one reopen.
    fn apply_pending_seek(&mut self, dev_rate: u32, dev_ch: usize, shared: &Shared) {
        if self.pending_seek.is_none() {
            return; // cheap common-case exit — no clock read when nothing's pending
        }
        // Throttle reopens: a held seek key floods commands; applying every one would
        // do a decoder reopen per frame. Coalesce to at most one per window (the last
        // target wins) so scrubbing stays light on the CPU. `pending_seek` is kept
        // until the window passes, so the final position always lands.
        let now = std::time::Instant::now();
        if let Some(last) = self.last_seek
            && now.duration_since(last) < Duration::from_millis(120)
        {
            return;
        }
        let Some(target) = self.pending_seek.take() else {
            return;
        };
        let Some(ts) = self.dvr.clone() else {
            return;
        };
        self.last_seek = Some(now);
        let landed = ts.seek_secs(target);
        // a bad reopen is non-fatal — keep the current decoder rather than dropping
        // playback (never leaves the stream permanently stopped).
        if let Ok(dec) = reopen_timeshift(ts, dev_rate, dev_ch) {
            self.active = Some(dec);
            self.cancel_crossfade();
            self.stretch.reset();
            shared.flush.store(true, Ordering::Release);
            self.pos_base_out = shared.samples_played.load(Ordering::Relaxed);
            self.pos_base_secs = landed;
            self.pending = None;
        }
    }

    /// Tear down the active timeshift buffer (stop its producer thread + release any
    /// parked consumer). Called whenever the live stream is replaced or stopped.
    fn drop_dvr(&mut self) {
        if let Some(ts) = self.dvr.take() {
            ts.shutdown();
        }
    }

    /// Abandon any in-progress crossfade (e.g. on a manual jump).
    fn cancel_crossfade(&mut self) {
        self.fade = None;
        self.xfade_done = 0;
        self.a_stage.clear();
        self.b_stage.clear();
    }

    /// Track-seconds currently playing, given the DAC clock `played`.
    fn track_secs(&self, played: u64, dev_rate_ch: usize) -> f64 {
        self.pos_base_secs
            + played.saturating_sub(self.pos_base_out) as f64 / dev_rate_ch as f64
                * self.speed as f64
    }

    /// Install radio/podcast streams opened off-thread (latest tune wins): a stale
    /// generation is discarded; a fresh decoder becomes the active track (flushing
    /// the ring) and reports its duration (finite for a podcast episode); an error
    /// clears playback and surfaces the message.
    fn install_streams(
        &mut self,
        stream_rx: &Receiver<StreamOpen>,
        shared: &Shared,
        evt_tx: &Sender<AudioEvent>,
    ) {
        while let Ok((generation, res)) = stream_rx.try_recv() {
            if generation != self.stream_gen {
                continue; // a newer tune superseded this one — discard it
            }
            match res {
                Ok((dec, dur, dvr)) => {
                    shared.flush.store(true, Ordering::Release);
                    shared.samples_played.store(0, Ordering::Relaxed);
                    self.active = Some(dec);
                    // adopt the new stream's timeshift buffer (if any); stop the old.
                    self.drop_dvr();
                    self.dvr = dvr;
                    // a finite file (podcast episode) reports its real length →
                    // clamp the clock + size the progress bar; a live stream is None
                    self.cur_dur = dur.map(|d| d.as_secs_f64());
                    if let Some(d) = dur {
                        let _ = evt_tx.send(AudioEvent::Duration(d));
                    }
                    self.pos_base_out = 0;
                    self.pos_base_secs = 0.0;
                    self.pending = None;
                    self.stretch.reset();
                    shared.playing.store(true, Ordering::Relaxed);
                }
                Err(e) => {
                    self.active = None;
                    shared.playing.store(false, Ordering::Relaxed);
                    let _ = evt_tx.send(AudioEvent::Error(e));
                }
            }
        }
    }
}

/// The real-time controller loop: drain commands → install off-thread streams →
/// pump audio into the SPSC ring → emit spectrum + progress. The per-iteration
/// working state (ring producer, scratch buffers, analyzer, emit cursors) lives
/// in [`Pump`] so the phases are methods; pure source/output helpers are in
/// `decode.rs`. Runs until the command channel disconnects.
#[allow(clippy::too_many_arguments)]
fn controller(
    device: cpal::Device,
    config: cpal::StreamConfig,
    fmt: SampleFormat,
    dev_rate: u32,
    dev_ch: usize,
    cmd_rx: Receiver<AudioCommand>,
    evt_tx: Sender<AudioEvent>,
    shared: Arc<Shared>,
    ready_tx: Sender<Result<(), String>>,
) {
    // Lock-free SPSC sample ring: the controller (producer) fills it, the audio
    // callback (consumer) drains it — no mutex on the real-time path. Sized to
    // ~4s of device audio, comfortably above the ~2s max fill target so a push
    // never overflows. The consumer goes to the stream; we keep the producer.
    let cap = (dev_rate as usize * dev_ch * 4).max(8192);
    let (prod, cons) = HeapRb::<f32>::new(cap).split();

    // Build the persistent output stream once.
    let stream = match build_stream(&device, &config, fmt, &shared, cons) {
        Ok(s) => s,
        Err(e) => {
            let _ = ready_tx.send(Err(e.to_string()));
            return;
        }
    };
    if let Err(e) = stream.play() {
        let _ = ready_tx.send(Err(e.to_string()));
        return;
    }
    let _ = ready_tx.send(Ok(()));

    // radio streams open on a worker thread and arrive here when ready
    let (stream_tx, stream_rx) = unbounded::<StreamOpen>();
    let mut ctl = Ctl::new(dev_rate, dev_ch, stream_tx);
    let mut pump = Pump::new(prod, dev_rate, dev_ch);

    loop {
        // ---- drain commands ----
        loop {
            match cmd_rx.try_recv() {
                Ok(cmd) => handle_command(cmd, dev_rate, dev_ch, &evt_tx, &shared, &mut ctl),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        // apply a coalesced DVR seek (a burst of held-key seeks → one reopen)
        ctl.apply_pending_seek(dev_rate, dev_ch, &shared);
        // install radio/podcast streams opened off-thread (latest tune wins)
        ctl.install_streams(&stream_rx, &shared, &evt_tx);
        // pump audio into the ring, then report spectrum + progress
        let did_work = pump.pump(&mut ctl, &shared, &evt_tx);
        pump.emit_spectrum(&ctl, &evt_tx);
        pump.emit_progress(&mut ctl, &shared, &evt_tx);
        // nothing to do → yield briefly rather than spin the CPU
        if !did_work {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Mono-mix interleaved `s` (device channel count `dev_ch`) and append to `mono`
/// (the analyzer feed). A free fn so a caller can borrow it disjointly from the
/// source buffer when both are fields of the same [`Pump`].
fn push_mono(mono: &mut Vec<f32>, s: &[f32], dev_ch: usize) {
    for frame in s.chunks(dev_ch) {
        mono.push(frame.iter().copied().sum::<f32>() / dev_ch.max(1) as f32);
    }
}

/// The controller's per-iteration working state: the SPSC ring producer, the
/// scratch buffers reused across packets (so the steady-state loop allocates
/// nothing), the spectrum analyzer, and the spectrum/progress emit cursors. Held
/// apart from the playback state (`Ctl`) so the pump phases stay low-arg methods.
struct Pump<P> {
    /// Ring producer — the controller is the sole producer (SPSC).
    prod: P,
    /// Mono mixdown fed to the spectrum analyzer.
    mono: Vec<f32>,
    /// Resampler / external-pull output, reused across packets.
    decode_buf: Vec<f32>,
    /// Time-stretcher output, reused across packets.
    stretch_out: Vec<f32>,
    analyzer: Analyzer,
    /// Last position (seconds) emitted via `Progress`, so we only send on a
    /// meaningful change. `-1.0` is the "nothing emitted yet" sentinel.
    last_progress: f64,
    /// Spectrum emit divider (emit every other iteration).
    emit_div: u32,
    dev_rate: u32,
    dev_ch: usize,
    /// Device samples per second across all channels (`dev_rate * dev_ch`).
    dev_rate_ch: usize,
}

impl<P: Producer<Item = f32>> Pump<P> {
    fn new(prod: P, dev_rate: u32, dev_ch: usize) -> Self {
        Self {
            prod,
            mono: Vec::with_capacity(8192),
            decode_buf: Vec::new(),
            stretch_out: Vec::new(),
            analyzer: Analyzer::new(DEFAULT_BANDS, 0.80),
            last_progress: -1.0,
            emit_div: 0,
            dev_rate,
            dev_ch,
            dev_rate_ch: (dev_rate as usize * dev_ch).max(1),
        }
    }

    /// One pump iteration. While a flush is pending (a seek/load/stop asked the
    /// callback to drop buffered audio) we hold off filling so the producer doesn't
    /// race fresh audio into the ring before the consumer clears the stale tail.
    /// Dispatches to the active source; returns whether it produced audio.
    fn pump(&mut self, ctl: &mut Ctl, shared: &Shared, evt_tx: &Sender<AudioEvent>) -> bool {
        let flushing = shared.flush.load(Ordering::Acquire);
        let ring_len = self.prod.occupied_len();
        if let Some(ext) = ctl.external.clone() {
            self.pump_external(&ext, ctl, flushing, ring_len)
        } else if ctl.fade.is_some() {
            self.pump_crossfade(ctl, flushing, ring_len)
        } else {
            self.pump_normal(ctl, shared, evt_tx, flushing, ring_len)
        }
    }

    /// EXTERNAL (Spotify/librespot): pull already-decoded PCM → resample → ring.
    /// No decoding: the source produced interleaved stereo f32 at its own rate;
    /// resample to the device rate so it rides the same ring (output volume + the
    /// visualizer apply unchanged).
    fn pump_external(
        &mut self,
        ext: &Arc<dyn ExternalAudioSource>,
        ctl: &mut Ctl,
        flushing: bool,
        ring_len: usize,
    ) -> bool {
        let mut did_work = false;
        if ring_len < self.dev_rate_ch / 2 && !flushing && ext.is_active() {
            let want = (self.dev_rate_ch / 4).max(1024);
            self.decode_buf.resize(want, 0.0);
            let got = ext.pull(&mut self.decode_buf[..]);
            if got > 0 {
                did_work = true;
                self.stretch_out.clear();
                match ctl.ext_resampler.as_mut() {
                    Some(rs) => rs.process(&self.decode_buf[..got], &mut self.stretch_out),
                    None => self.stretch_out.extend_from_slice(&self.decode_buf[..got]),
                }
                ctl.eq.process(&mut self.stretch_out, self.dev_ch);
                push_mono(&mut self.mono, &self.stretch_out, self.dev_ch);
                self.prod.push_slice(&self.stretch_out);
            }
        }
        did_work
    }

    /// CROSSFADE: decode A (out) + B (in), mix with the equal-power ramp.
    fn pump_crossfade(&mut self, ctl: &mut Ctl, flushing: bool, ring_len: usize) -> bool {
        let mut did_work = false;
        let silence_skip = ctl.silence_skip;
        if ring_len < self.dev_rate_ch * 2 && !flushing {
            let a_ended = match ctl.active.as_mut() {
                Some(st) if !st.finished => {
                    if decode_one(st, &mut self.decode_buf, silence_skip) {
                        ctl.a_stage.extend_from_slice(&self.decode_buf);
                        did_work = true;
                        false
                    } else {
                        st.finished = true;
                        true
                    }
                }
                _ => true,
            };
            if let Some(st) = ctl.fade.as_mut()
                && decode_one(st, &mut self.decode_buf, silence_skip)
            {
                ctl.b_stage.extend_from_slice(&self.decode_buf);
                did_work = true;
            }
            let a_frames = ctl.a_stage.len() / self.dev_ch;
            let b_frames = ctl.b_stage.len() / self.dev_ch;
            let remain = ctl.xfade_len.saturating_sub(ctl.xfade_done);
            let n = if a_ended {
                b_frames.min(remain)
            } else {
                a_frames.min(b_frames).min(remain)
            };
            if n > 0 {
                let mut mix = crossfade_mix(
                    &ctl.a_stage,
                    &ctl.b_stage,
                    self.dev_ch,
                    ctl.xfade_done,
                    ctl.xfade_len,
                    n,
                );
                ctl.eq.process(&mut mix, self.dev_ch);
                push_mono(&mut self.mono, &mix, self.dev_ch);
                self.prod.push_slice(&mix);
                ctl.a_stage
                    .drain(..(n * self.dev_ch).min(ctl.a_stage.len()));
                ctl.b_stage
                    .drain(..(n * self.dev_ch).min(ctl.b_stage.len()));
                ctl.xfade_done += n;
                did_work = true;
            }
            // crossfade done (ramp complete, or A exhausted) → B is now the sole
            // track; flush any B already decoded at full volume.
            if ctl.xfade_done >= ctl.xfade_len || (a_ended && ctl.a_stage.is_empty()) {
                if !ctl.b_stage.is_empty() {
                    ctl.eq.process(&mut ctl.b_stage, self.dev_ch);
                    push_mono(&mut self.mono, &ctl.b_stage, self.dev_ch);
                    self.prod.push_slice(&ctl.b_stage);
                    ctl.b_stage.clear();
                }
                ctl.active = ctl.fade.take();
                ctl.cur_dur = ctl.fade_dur.take();
                ctl.cancel_crossfade();
            }
        }
        did_work
    }

    /// NORMAL: decode the active track → time-stretch → ring, starting a crossfade
    /// or a gapless swap as the track nears its end.
    fn pump_normal(
        &mut self,
        ctl: &mut Ctl,
        shared: &Shared,
        evt_tx: &Sender<AudioEvent>,
        flushing: bool,
        ring_len: usize,
    ) -> bool {
        let mut did_work = false;
        let silence_skip = ctl.silence_skip;
        // Keep ~0.5s buffered — ample underrun safety, low latency.
        let attempt: Option<bool> = if let Some(st) = ctl.active.as_mut() {
            (!st.finished && ring_len < self.dev_rate_ch / 2 && !flushing)
                .then(|| decode_one(st, &mut self.decode_buf, silence_skip))
        } else {
            None
        };
        let mut ended = false;
        match attempt {
            Some(true) => {
                did_work = true;
                ctl.stretch.push(&self.decode_buf);
                self.stretch_out.clear();
                ctl.stretch.pull(&mut self.stretch_out);
                ctl.eq.process(&mut self.stretch_out, self.dev_ch);
                push_mono(&mut self.mono, &self.stretch_out, self.dev_ch);
                self.prod.push_slice(&self.stretch_out);
            }
            Some(false) => ended = true,
            None => {}
        }

        // begin a crossfade when the decoder is within the crossfade window of the
        // end and a next track is preloaded (normal speed only).
        if !ended
            && !flushing
            && ctl.xfade_len > 0
            && (ctl.speed - 1.0).abs() < 0.01
            && let Some(dur) = ctl.cur_dur
            && let Some(path) = ctl.next_path.clone()
        {
            let played = shared.samples_played.load(Ordering::Relaxed);
            let buf = self.prod.occupied_len() as u64;
            let dec_pos = ctl.track_secs(played + buf, self.dev_rate_ch);
            let xfade_secs = ctl.xfade_len as f64 / self.dev_rate as f64;
            if dec_pos > 0.0
                && dec_pos >= dur - xfade_secs
                && let Ok((dec, bdur)) = open_track(path, self.dev_rate, self.dev_ch)
            {
                ctl.next_path = None; // consumed by the crossfade
                ctl.fade = Some(dec);
                ctl.fade_dur = bdur.map(|d| d.as_secs_f64());
                ctl.xfade_done = 0;
                ctl.a_stage.clear();
                ctl.b_stage.clear();
                // B's 0:00 lands at the end of the currently buffered audio
                ctl.pending = Some(played + buf);
            }
        }

        // end of the current track, no crossfade: gapless swap or finish.
        if ended {
            if let Some(path) = ctl.next_path.take() {
                match open_track(path, self.dev_rate, self.dev_ch) {
                    Ok((dec, dur)) => {
                        let tail = self.prod.occupied_len() as u64;
                        ctl.pending = Some(shared.samples_played.load(Ordering::Relaxed) + tail);
                        ctl.active = Some(dec);
                        ctl.cur_dur = dur.map(|d| d.as_secs_f64());
                        ctl.stretch.reset();
                        did_work = true;
                    }
                    Err(e) => {
                        let _ = evt_tx.send(AudioEvent::Error(e.to_string()));
                        if let Some(st) = ctl.active.as_mut() {
                            st.finished = true;
                        }
                    }
                }
            } else if let Some(st) = ctl.active.as_mut() {
                st.finished = true;
            }
        }
        did_work
    }

    /// Trim the analyzer feed and emit a spectrum frame (every other iteration,
    /// once there's enough buffered and something is playing).
    fn emit_spectrum(&mut self, ctl: &Ctl, evt_tx: &Sender<AudioEvent>) {
        if self.mono.len() > 4096 {
            self.mono.drain(0..self.mono.len() - 4096);
        }
        self.emit_div = self.emit_div.wrapping_add(1);
        if (ctl.active.is_some() || ctl.external.is_some())
            && self.emit_div.is_multiple_of(2)
            && self.mono.len() >= 2048
        {
            let levels = self.analyzer.process(&self.mono).to_vec();
            let _ = evt_tx.send(AudioEvent::Spectrum(levels));
        }
    }

    /// Emit Progress on a small position change (~20 Hz, latency-compensated),
    /// Advanced at a gapless seam, and Finished when the active track ends with
    /// the ring fully drained.
    fn emit_progress(&mut self, ctl: &mut Ctl, shared: &Shared, evt_tx: &Sender<AudioEvent>) {
        if ctl.active.is_none() {
            return;
        }
        let played = shared.samples_played.load(Ordering::Relaxed);
        // the seam was reached → the next track is now what's playing
        if let Some(boundary) = ctl.pending
            && played >= boundary
        {
            let _ = evt_tx.send(AudioEvent::Advanced);
            ctl.pos_base_out = boundary; // new track starts at 0:00
            ctl.pos_base_secs = 0.0;
            ctl.pending = None;
            self.last_progress = -1.0;
        }
        // Precise position, minus the output-buffer latency so the reported time
        // matches what's audible. `samples_played` counts samples handed to cpal,
        // which the device plays out later — without this the clock (and the lyric
        // highlight it drives) runs ahead of the sound.
        let latency = (shared.out_latency_us.load(Ordering::Relaxed) as f64 / 1_000_000.0)
            .min(MAX_LATENCY_SECS);
        let pos = (ctl.track_secs(played, self.dev_rate_ch) - latency).max(0.0);
        if (pos - self.last_progress).abs() >= PROGRESS_STEP_SECS {
            self.last_progress = pos;
            let _ = evt_tx.send(AudioEvent::Progress(Duration::from_secs_f64(pos)));
            // for a timeshifted live stream, report the seekable window alongside so
            // the UI can draw a DVR bar (position within `[start, live]`).
            if let Some(ts) = &ctl.dvr {
                let w = ts.window();
                let _ = evt_tx.send(AudioEvent::DvrWindow {
                    start: w.start,
                    live: w.live,
                });
            }
        }
        // true end (nothing preloaded) → let the app decide what's next
        let finished = ctl.active.as_ref().map(|s| s.finished).unwrap_or(false);
        if finished && self.prod.occupied_len() == 0 {
            shared.playing.store(false, Ordering::Relaxed);
            let _ = evt_tx.send(AudioEvent::Finished);
            ctl.active = None;
            ctl.drop_dvr();
            ctl.cur_dur = None;
            ctl.pos_base_out = 0;
            ctl.pos_base_secs = 0.0;
            ctl.pending = None;
            self.mono.clear();
            self.last_progress = -1.0;
        }
    }
}

fn handle_command(
    cmd: AudioCommand,
    dev_rate: u32,
    dev_ch: usize,
    evt_tx: &Sender<AudioEvent>,
    shared: &Arc<Shared>,
    ctl: &mut Ctl,
) {
    match cmd {
        // an explicit Load is a hard cut: clear the buffer + reset the clock and
        // the gapless bookkeeping (manual jumps are never gapless).
        AudioCommand::Load(path) => {
            // An internal decoder supersedes any external source (a Spotify/librespot
            // bridge): they're mutually exclusive, so drop the external one — else the
            // pump keeps pulling from it and ignores this track (the cause of silent
            // local playback after merely browsing Spotify, which attaches the bridge).
            ctl.external = None;
            ctl.drop_dvr(); // a local track replaces any timeshifted live stream
            ctl.ext_resampler = None;
            match open_track(path, dev_rate, dev_ch) {
                Ok((dec, dur)) => {
                    shared.flush.store(true, Ordering::Release);
                    shared.samples_played.store(0, Ordering::Relaxed);
                    ctl.active = Some(dec);
                    ctl.cur_dur = dur.map(|d| d.as_secs_f64());
                    ctl.pos_base_out = 0;
                    ctl.pos_base_secs = 0.0;
                    ctl.pending = None;
                    ctl.next_path = None;
                    ctl.stretch.reset();
                    ctl.cancel_crossfade();
                    if let Some(d) = dur {
                        let _ = evt_tx.send(AudioEvent::Duration(d));
                    }
                }
                Err(e) => {
                    ctl.active = None;
                    let _ = evt_tx.send(AudioEvent::Error(e.to_string()));
                }
            }
        }
        // an internet-radio stream: like Load, but no duration / gapless / next.
        AudioCommand::LoadStream { url, dvr } => {
            // Open the stream OFF the controller thread — the network connect +
            // format probe can take seconds (or hang on a dead host), and doing
            // it inline froze audio and command handling. We stop the current
            // stream now and install the new decoder when the worker delivers it
            // (the controller loop drains `stream_rx`). A generation counter means
            // only the latest tune wins — pressing Next rapidly never piles up.
            ctl.stream_gen += 1;
            let generation = ctl.stream_gen;
            shared.flush.store(true, Ordering::Release);
            shared.samples_played.store(0, Ordering::Relaxed);
            ctl.active = None;
            ctl.drop_dvr(); // stop the previous stream's timeshift producer
            // a stream is an internal source too — drop any external (Spotify) source
            // so its dead bridge can't shadow the stream we're about to open.
            ctl.external = None;
            ctl.ext_resampler = None;
            ctl.cur_dur = None;
            ctl.pos_base_out = 0;
            ctl.pos_base_secs = 0.0;
            ctl.pending = None;
            ctl.next_path = None;
            ctl.stretch.reset();
            ctl.cancel_crossfade();
            let tx = ctl.stream_tx.clone();
            let evt = evt_tx.clone();
            let _ = std::thread::Builder::new()
                .name("lyrfin-radio-open".into())
                .spawn(move || {
                    let res =
                        open_stream(url, dev_rate, dev_ch, evt, dvr).map_err(|e| e.to_string());
                    let _ = tx.send((generation, res));
                });
        }
        // Jump a timeshifted live stream back to the live edge. Seek the decoder to
        // the buffer's newest audio and rebaseline the position clock to it.
        AudioCommand::GoLive => {
            // seek to the current live edge, applied (coalesced) after the drain.
            if let Some(ts) = &ctl.dvr {
                ctl.pending_seek = Some(ts.window().live);
            }
        }
        // Hand audio to an external producer (Spotify/librespot). Stop internal
        // decoding + flush, then pull from the source in the loop. Its PCM rides
        // the same ring → output volume + visualizer apply.
        AudioCommand::SetExternalSource(src) => {
            ctl.stream_gen += 1; // invalidate any in-flight radio open
            shared.flush.store(true, Ordering::Release);
            shared.samples_played.store(0, Ordering::Relaxed);
            ctl.active = None;
            ctl.drop_dvr(); // an external (Spotify) source replaces the live stream
            ctl.cur_dur = None;
            ctl.pos_base_out = 0;
            ctl.pos_base_secs = 0.0;
            ctl.pending = None;
            ctl.next_path = None;
            ctl.stretch.reset();
            ctl.cancel_crossfade();
            // resample the source's rate (e.g. 44100 from Spotify) to the device;
            // librespot delivers interleaved stereo, so in_ch = 2
            ctl.ext_resampler = Some(Resampler::new(src.sample_rate(), dev_rate, 2, dev_ch));
            ctl.external = Some(src);
            shared.playing.store(true, Ordering::Relaxed);
        }
        AudioCommand::ClearExternalSource => {
            ctl.external = None;
            ctl.ext_resampler = None;
            shared.playing.store(false, Ordering::Relaxed);
            shared.flush.store(true, Ordering::Release);
            shared.samples_played.store(0, Ordering::Relaxed);
        }
        AudioCommand::SetNext(path) => ctl.next_path = path,
        AudioCommand::Play => shared.playing.store(true, Ordering::Relaxed),
        AudioCommand::Pause => shared.playing.store(false, Ordering::Relaxed),
        AudioCommand::Stop => {
            shared.playing.store(false, Ordering::Relaxed);
            shared.flush.store(true, Ordering::Release);
            shared.samples_played.store(0, Ordering::Relaxed);
            ctl.active = None;
            ctl.drop_dvr();
            ctl.cur_dur = None;
            ctl.pos_base_out = 0;
            ctl.pos_base_secs = 0.0;
            ctl.pending = None;
            ctl.next_path = None;
            ctl.stretch.reset();
            ctl.cancel_crossfade();
        }
        AudioCommand::Seek(pos) => {
            // A timeshifted (DVR) live stream can't be sought in place — coalesce to
            // one decoder reopen after the drain (robust; see apply_pending_seek).
            if ctl.dvr.is_some() {
                ctl.pending_seek = Some(pos.as_secs_f64());
            } else if let Some(st) = ctl.active.as_mut() {
                let _ = st.format.seek(
                    SeekMode::Coarse,
                    SeekTo::Time {
                        time: Time::try_from_secs_f64(pos.as_secs_f64()).unwrap_or(Time::ZERO),
                        track_id: Some(st.track_id),
                    },
                );
                // a seek makes the next packet discontinuous — the decoder must be
                // reset or it emits silence/garbage until it resyncs.
                st.decoder.reset();
                st.resampler.reset(); // drop pre-seek input carried for interpolation
                st.trim.clear_hold(); // drop held silence; don't re-trim at the seek point
                st.finished = false;
                shared.flush.store(true, Ordering::Release);
                ctl.stretch.reset();
                // rebaseline the position clock to the seek target (the DAC clock
                // keeps running; pos_base maps it back to track seconds).
                ctl.pos_base_out = shared.samples_played.load(Ordering::Relaxed);
                ctl.pos_base_secs = pos.as_secs_f64();
                ctl.pending = None;
            }
            ctl.cancel_crossfade(); // abandon any crossfade in progress
        }
        AudioCommand::SetVolume(v) => shared.volume.store(v as u32, Ordering::Relaxed),
        AudioCommand::SetGain(g) => shared
            .gain_bits
            .store(g.max(0.0).to_bits(), Ordering::Relaxed),
        AudioCommand::SetSpeed(s) => {
            // rebaseline so the position clock is continuous across the change,
            // then retune the stretcher.
            let dev_rate_ch = (dev_rate as usize * dev_ch).max(1);
            let played = shared.samples_played.load(Ordering::Relaxed);
            ctl.pos_base_secs = ctl.track_secs(played, dev_rate_ch);
            ctl.pos_base_out = played;
            ctl.speed = s.clamp(0.25, 4.0);
            ctl.stretch.set_speed(ctl.speed);
        }
        AudioCommand::SetCrossfade(ms) => {
            ctl.xfade_len = ms as usize * dev_rate as usize / 1000;
        }
        AudioCommand::SetSilenceSkip(v) => ctl.silence_skip = v,
        // Recompute the EQ filter set on the controller thread (off the realtime
        // callback). Applies to audio produced from here on — no playback restart.
        AudioCommand::SetEq(cfg) => ctl.eq.configure(&cfg),
    }
}
