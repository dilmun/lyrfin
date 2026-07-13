//! Audio source plumbing for the engine, split out of the controller: decode a
//! packet, build the per-track decode state, open a file or HTTP stream, mix a
//! crossfade ramp, and build the persistent cpal output stream. Behaviour-
//! preserving move — these are the same pure helpers the controller calls; only
//! the real-time loop + command handling stay in the parent module.

use super::*;
use crate::audio::timeshift::{self, Timeshift, TimeshiftSource};

/// Linear-ramp crossfade of `n` interleaved frames: A fades out, B fades in over
/// a `total`-frame window with `done` frames already elapsed. Missing samples in
/// either source count as silence (so a track that ended early just fades out).
pub(super) fn crossfade_mix(
    a: &[f32],
    b: &[f32],
    ch: usize,
    done: usize,
    total: usize,
    n: usize,
) -> Vec<f32> {
    let total = total.max(1) as f32;
    let mut out = Vec::with_capacity(n * ch);
    for f in 0..n {
        let t = ((done + f) as f32 / total).clamp(0.0, 1.0);
        // equal-power (constant-energy) ramp: gains sum in quadrature so the mix
        // doesn't dip ~3 dB at the midpoint like a linear (equal-amplitude) fade.
        let ga = (t * std::f32::consts::FRAC_PI_2).cos();
        let gb = (t * std::f32::consts::FRAC_PI_2).sin();
        for c in 0..ch {
            let av = a.get(f * ch + c).copied().unwrap_or(0.0);
            let bv = b.get(f * ch + c).copied().unwrap_or(0.0);
            // clamp guards against two ReplayGain-boosted tracks summing past
            // full-scale on the f32 device path.
            out.push((av * ga + bv * gb).clamp(-1.0, 1.0));
        }
    }
    out
}

/// Amplitude at or below which a sample counts as silence for silence-skip
/// (~-52 dBFS). Low enough to treat encoder padding / dead air as silent, high
/// enough that any real musical content ends the trim immediately.
const SILENCE_THRESHOLD: f32 = 0.0025;
/// Safety valve: a held silent run longer than this (frames; ~21s @ 48 kHz) is
/// flushed as ordinary audio, so a mostly-silent track can't grow the hold
/// buffer without bound.
const SILENCE_HOLD_CAP_FRAMES: usize = 1 << 20;

/// Streaming leading/trailing silence trimmer, one per decoded track.
///
/// Drops the silent run at the *start* of a track (until the first audible
/// frame) and holds back any silent run so it's only emitted once real audio
/// follows — interior silence. A run still held when the track ends (its
/// [`Decode`] is dropped) is the *trailing* silence and is discarded. So a
/// gapless/crossfade seam joins the last audible frame of one track to the
/// first audible frame of the next, with interior gaps left intact.
#[derive(Default)]
pub(super) struct SilenceTrim {
    /// Have we emitted the first audible frame yet (leading trim done)?
    head_done: bool,
    /// Consecutive silent frames held back — interior silence if audio resumes,
    /// trailing silence (discarded) if the track ends first.
    hold: Vec<f32>,
    /// Output builder, swapped with the caller's buffer to avoid per-call allocation.
    scratch: Vec<f32>,
}

impl SilenceTrim {
    /// Drop any held silence on a seek — the decoder is repositioned mid-track, so
    /// stale pre-seek silence must not be emitted. Leaves `head_done` set: a seek
    /// lands where the user asked, it must not re-arm leading-silence trimming.
    pub(super) fn clear_hold(&mut self) {
        self.hold.clear();
        self.scratch.clear();
    }

    /// Trim `buf` (interleaved, `ch` channels) in place: strip leading silence,
    /// hold trailing/interior silence. Reuses internal scratch — no allocation
    /// in the steady state.
    fn process(&mut self, buf: &mut Vec<f32>, ch: usize) {
        if ch == 0 || buf.is_empty() {
            return;
        }
        self.scratch.clear();
        let used = (buf.len() / ch) * ch; // whole frames only
        for frame in buf[..used].chunks_exact(ch) {
            let audible = frame.iter().any(|s| s.abs() > SILENCE_THRESHOLD);
            if !self.head_done {
                if audible {
                    self.head_done = true;
                    self.scratch.extend_from_slice(frame);
                }
                // else: leading silence — drop it
            } else if audible {
                // real audio: any held run was interior silence → emit it, then this frame
                self.scratch.append(&mut self.hold);
                self.scratch.extend_from_slice(frame);
            } else {
                self.hold.extend_from_slice(frame);
                if self.hold.len() >= SILENCE_HOLD_CAP_FRAMES * ch {
                    self.scratch.append(&mut self.hold);
                }
            }
        }
        // A frame-aligned resampler never leaves a remainder, but preserve one
        // defensively so trimming can never drop a partial trailing frame.
        if used < buf.len() {
            self.scratch.extend_from_slice(&buf[used..]);
        }
        std::mem::swap(buf, &mut self.scratch);
    }
}

/// Decode one packet → resample/channel-map into `out` (cleared + reused, so the
/// steady-state loop allocates nothing), then trim boundary silence when
/// `silence_skip` is set. Returns `true` while the stream yields packets (`out`
/// may be empty for a skipped/recoverable packet or fully-trimmed silence),
/// `false` at end-of-stream.
pub(super) fn decode_one(st: &mut Decode, out: &mut Vec<f32>, silence_skip: bool) -> bool {
    out.clear();
    let packet = match st.format.next_packet() {
        Ok(Some(p)) => p,
        Ok(None) | Err(_) => return false, // Ok(None) = end of stream
    };
    if packet.track_id != st.track_id {
        return true;
    }
    match st.decoder.decode(&packet) {
        Ok(decoded) => {
            // Copy the decoded buffer straight into the reused interleaved-f32
            // scratch (0.6 replaces SampleBuffer::new + copy_interleaved_ref +
            // samples() with this one call, which clears + grows `sbuf` as needed).
            decoded.copy_to_vec_interleaved(&mut st.sbuf);
            st.resampler.process(&st.sbuf, out);
            if silence_skip {
                st.trim.process(out, st.dev_ch);
            }
            true
        }
        Err(symphonia::core::errors::Error::DecodeError(_)) => true,
        Err(_) => false,
    }
}

/// Probe a media source and build the per-track decode state (decoder +
/// resampler). Shared by file playback and HTTP radio streams.
///
/// Note: symphonia 0.6 dropped `FormatOptions::enable_gapless` (encoder
/// delay/padding trimming). lyrfin's own silence-skip trims that leading/trailing
/// silence, and gapless advance is driven by the engine's preload + crossfade, so
/// the practical effect is unchanged.
fn build_decode(
    // 0.6's `MediaSourceStream` is lifetime-parameterised; ours always wraps a
    // `Box<dyn MediaSource>` (owned file/HTTP source), so it — and the probed
    // `FormatReader` stored in `Decode` — are `'static`.
    mss: MediaSourceStream<'static>,
    hint: &Hint,
    dev_rate: u32,
    dev_ch: usize,
) -> anyhow::Result<(Decode, Option<Duration>)> {
    let format = symphonia::default::get_probe().probe(
        hint,
        mss,
        FormatOptions::default(),
        MetadataOptions::default(),
    )?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| anyhow::anyhow!("no audio track"))?;
    let track_id = track.id;
    let file_frames = track.num_frames;
    let Some(CodecParameters::Audio(params)) = track.codec_params.as_ref() else {
        anyhow::bail!("track has no audio codec parameters");
    };
    let file_rate = params
        .sample_rate
        .ok_or_else(|| anyhow::anyhow!("unknown sample rate"))?;
    let file_ch = params
        .channels
        .as_ref()
        .map(|c| c.count())
        .unwrap_or(2)
        .max(1);
    let duration = file_frames.map(|n| Duration::from_secs_f64(n as f64 / file_rate as f64));
    let decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())?;

    Ok((
        Decode {
            format,
            decoder,
            track_id,
            resampler: Resampler::new(file_rate, dev_rate, file_ch, dev_ch),
            finished: false,
            sbuf: Vec::new(),
            dev_ch,
            trim: SilenceTrim::default(),
        },
        duration,
    ))
}

/// Rebuild the decoder for a timeshifted (DVR) live stream at the ring's current
/// read cursor: a fresh probe over the *buffered* bytes (RAM, no network), so a
/// seek is robust and codec-agnostic — MP3/AAC self-sync at the new position —
/// without relying on symphonia's fragile live-stream time-seek (which could wedge
/// the decoder). The caller moves the cursor first via [`Timeshift::seek_secs`].
pub(super) fn reopen_timeshift(
    ts: Arc<Timeshift>,
    dev_rate: u32,
    dev_ch: usize,
) -> anyhow::Result<Decode> {
    let mss = MediaSourceStream::new(Box::new(TimeshiftSource::new(ts)), Default::default());
    let (decode, _dur) = build_decode(mss, &Hint::new(), dev_rate, dev_ch)?;
    Ok(decode)
}

pub(super) fn open_track(
    path: PathBuf,
    dev_rate: u32,
    dev_ch: usize,
) -> anyhow::Result<(Decode, Option<Duration>)> {
    let file = std::fs::File::open(&path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    build_decode(mss, &hint, dev_rate, dev_ch)
}

/// Open an HTTP audio stream (internet radio / streamed podcast): connect, then
/// decode the byte stream. A finite file is ranged + seekable with a real
/// duration; a live stream is forward-only (no duration) — unless `dvr_window` is
/// `Some`, when it's wrapped in a timeshift buffer (off-thread producer + seekable
/// [`TimeshiftSource`]) so it can be paused, rewound within the window, and caught
/// up to live. Returns the timeshift handle for the controller when one was built.
pub(super) fn open_stream(
    url: String,
    dev_rate: u32,
    dev_ch: usize,
    evt_tx: Sender<AudioEvent>,
    dvr_window: Option<Duration>,
) -> anyhow::Result<(Decode, Option<Duration>, Option<Arc<Timeshift>>)> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(8)))
        // a live stream sends data continuously, so a long read gap = a dead /
        // stalled host. Bounding the read keeps a hung stream from blocking the
        // controller forever (it surfaces as a decode error and recovers).
        .timeout_recv_body(Some(Duration::from_secs(12)))
        // podcast enclosures chain through several ad-tracking redirects (podtrac →
        // pdst.fm → megaphone → …, often 6–7 hops); ureq's default cap of 5 would
        // stop short of the real MP3.
        .max_redirects(12)
        .user_agent(concat!("lyrfin/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    // ask for interleaved "now playing" metadata (Icecast/Shoutcast)
    let resp = agent
        .get(&url)
        .header("Icy-MetaData", "1")
        .call()
        .map_err(|e| anyhow::anyhow!(e))?;
    // bytes of audio between metadata blocks (absent → no metadata to strip)
    let metaint = resp
        .headers()
        .get("icy-metaint")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    // A finite file (Content-Length, no live ICY metadata) is a podcast episode →
    // make it seekable so the user can scrub. A live radio stream (chunked / ICY)
    // stays the forward-only reader.
    let content_len = resp
        .headers()
        .get("Content-Length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let seekable = matches!(content_len, Some(len) if metaint == 0 && len > 0);
    let mut dvr: Option<Arc<Timeshift>> = None;
    let mss = if let (true, Some(len)) = (seekable, content_len) {
        // finite file (podcast) → ranged + natively seekable; DVR doesn't apply.
        let src = HttpRangeSource {
            agent,
            url,
            len,
            pos: 0,
            reader: Box::new(resp.into_body().into_reader()),
        };
        MediaSourceStream::new(Box::new(src), Default::default())
    } else {
        // live stream: strip ICY metadata via HttpStream. With DVR, an off-thread
        // producer feeds a timeshift ring and the decoder reads the seekable
        // TimeshiftSource; without it, decode the forward-only reader directly.
        let http = HttpStream {
            reader: Box::new(resp.into_body().into_reader()),
            metaint,
            until_meta: metaint,
            evt_tx,
            last_title: String::new(),
        };
        if let Some(window) = dvr_window {
            let ts = Arc::new(Timeshift::new(window));
            timeshift::spawn_producer(ts.clone(), Box::new(http));
            dvr = Some(ts.clone());
            MediaSourceStream::new(Box::new(TimeshiftSource::new(ts)), Default::default())
        } else {
            MediaSourceStream::new(Box::new(http), Default::default())
        }
    };
    // probe by content (no extension)
    let (mut decode, sym_dur) = build_decode(mss, &Hint::new(), dev_rate, dev_ch)?;
    // A finite file (podcast episode): the real length is byte_len ÷ data-rate. We
    // can't trust the header frame-count — it predates dynamically-inserted ads (so
    // it under-reports, and the position would otherwise pin at 100% for the whole
    // ad tail). A live stream reports no duration.
    let dur = match content_len {
        Some(len) if seekable => estimate_stream_duration(&mut decode, len).or(sym_dur),
        _ => sym_dur,
    };
    // DVR: measure the real compressed byte-rate from the first packet so the
    // timeshift window's seconds line up with playback (nominal until then).
    if let Some(ts) = &dvr
        && let Some(rate) = measure_byte_rate(&mut decode)
    {
        ts.set_byte_rate(rate);
    }
    Ok((decode, dur, dvr))
}

/// Compressed bytes per second of a stream, from the first packet (bytes ÷ frames
/// × frame-rate). CBR — true for essentially all live radio. Consumes one packet
/// (~a few ms), imperceptible at tune-in. Used to map the timeshift window to time.
fn measure_byte_rate(decode: &mut Decode) -> Option<u64> {
    let rate = {
        let track = decode.format.default_track(TrackType::Audio)?;
        let Some(CodecParameters::Audio(params)) = track.codec_params.as_ref() else {
            return None;
        };
        params.sample_rate? as u64
    };
    let pkt = decode.format.next_packet().ok()??; // 0.6: Result<Option<Packet>>
    let frames = pkt.dur.get();
    let bytes = pkt.data.len() as u64;
    if rate == 0 || frames == 0 || bytes == 0 {
        return None;
    }
    Some(bytes.saturating_mul(rate) / frames)
}

/// Estimate a finite stream's true duration from its byte length and the first
/// frame's data-rate (bytes ÷ frames). For CBR (most podcasts) this is exact and,
/// unlike the header frame-count, includes dynamically-inserted ads. Consumes the
/// first packet (~a few ms of audio) — imperceptible at the very start.
fn estimate_stream_duration(decode: &mut Decode, byte_len: u64) -> Option<Duration> {
    // Scope the immutable track borrow so it ends before the mutable `next_packet`.
    let rate = {
        let track = decode.format.default_track(TrackType::Audio)?;
        let Some(CodecParameters::Audio(params)) = track.codec_params.as_ref() else {
            return None;
        };
        params.sample_rate? as u64
    };
    let pkt = decode.format.next_packet().ok()??; // Result<Option<Packet>>
    let frames = pkt.dur.get();
    let bytes = pkt.data.len() as u64;
    if rate == 0 || frames == 0 || bytes == 0 {
        return None;
    }
    let total_frames = byte_len.saturating_mul(frames) / bytes;
    Some(Duration::from_secs_f64(total_frames as f64 / rate as f64))
}

/// Build a persistent output stream that drains the f32 ring (lock-free, the
/// consumer half of the SPSC queue), converting to the device sample format.
/// Counts written device-samples for the position clock. A pending `flush`
/// (raised by the controller on seek/load/stop) is honoured here because the
/// consumer is the only side allowed to drop buffered samples.
pub(super) fn build_stream<C>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    fmt: SampleFormat,
    shared: &Arc<Shared>,
    cons: C,
) -> anyhow::Result<cpal::Stream>
where
    C: Consumer<Item = f32> + Send + 'static,
{
    let err_fn = |e| eprintln!("lyrfin audio stream error: {e}");
    macro_rules! build {
        ($t:ty, $conv:expr) => {{
            let shared = shared.clone();
            let mut cons = cons; // the consumer lives in (only) this stream
            let mut scratch: Vec<f32> = Vec::new();
            device.build_output_stream(
                // cpal 0.18 takes the config by value
                config.clone(),
                move |out: &mut [$t], info: &cpal::OutputCallbackInfo| {
                    // Output latency: how far ahead of the speaker this buffer is
                    // (device/OS buffering; hundreds of ms on Bluetooth). Recorded so
                    // the position clock can report the *audible* time, not the queued
                    // one. cpal 0.18: `duration_since` takes the instant by value and
                    // saturates to zero (no longer an `Option`).
                    let ts = info.timestamp();
                    let lat = ts.playback.duration_since(ts.callback);
                    shared
                        .out_latency_us
                        .store(lat.as_micros() as u64, Ordering::Relaxed);
                    // a seek/load/stop asked us to drop everything buffered
                    if shared.flush.swap(false, Ordering::AcqRel) {
                        cons.clear();
                    }
                    let vol = shared.volume.load(Ordering::Relaxed) as f32 / 100.0;
                    let gain = f32::from_bits(shared.gain_bits.load(Ordering::Relaxed));
                    let vol = vol * gain; // fold ReplayGain into the volume scalar
                    let playing = shared.playing.load(Ordering::Relaxed);
                    let n = out.len();
                    if scratch.len() < n {
                        scratch.resize(n, 0.0);
                    }
                    let got = if playing {
                        cons.pop_slice(&mut scratch[..n])
                    } else {
                        0
                    };
                    for (s, &v) in out.iter_mut().zip(scratch[..got].iter()) {
                        *s = $conv(v * vol);
                    }
                    for s in out[got..].iter_mut() {
                        *s = $conv(0.0f32);
                    }
                    if got > 0 {
                        shared
                            .samples_played
                            .fetch_add(got as u64, Ordering::Relaxed);
                    }
                },
                err_fn,
                None,
            )?
        }};
    }
    let stream = match fmt {
        SampleFormat::F32 => build!(f32, |v: f32| v),
        SampleFormat::I16 => build!(i16, |v: f32| (v.clamp(-1.0, 1.0) * i16::MAX as f32) as i16),
        SampleFormat::U16 => {
            build!(
                u16,
                |v: f32| (((v.clamp(-1.0, 1.0) * 0.5) + 0.5) * u16::MAX as f32) as u16
            )
        }
        other => return Err(anyhow::anyhow!("unsupported sample format: {other:?}")),
    };
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::{Ctl, Resampler, Shared, SilenceTrim, crossfade_mix, handle_command};
    use crate::audio::{AudioCommand, ExternalAudioSource};
    use crossbeam_channel::unbounded;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};

    /// A stand-in for a Spotify/librespot bridge attached as the engine's external
    /// source. Produces nothing — it only needs to occupy `ctl.external`.
    #[derive(Debug)]
    struct DummyExt;
    impl ExternalAudioSource for DummyExt {
        fn pull(&self, _out: &mut [f32]) -> usize {
            0
        }
        fn sample_rate(&self) -> u32 {
            44_100
        }
        fn is_active(&self) -> bool {
            true
        }
    }

    fn test_shared() -> Arc<Shared> {
        Arc::new(Shared {
            volume: AtomicU32::new(100),
            gain_bits: AtomicU32::new(1.0f32.to_bits()),
            playing: AtomicBool::new(false),
            samples_played: AtomicU64::new(0),
            flush: AtomicBool::new(false),
            out_latency_us: AtomicU64::new(0),
        })
    }

    #[test]
    fn load_drops_a_stale_external_source() {
        // Reproduces the no-audio bug: a Spotify bridge is attached (e.g. after
        // browsing an artist), then a local file is loaded. The internal decoder
        // must reclaim the engine — otherwise the pump keeps pulling from the dead
        // bridge and local playback is silent.
        let (evt_tx, _evt_rx) = unbounded();
        let (stream_tx, _stream_rx) = unbounded();
        let shared = test_shared();
        let mut ctl = Ctl::new(44_100, 2, stream_tx);
        ctl.external = Some(Arc::new(DummyExt));
        ctl.ext_resampler = Some(Resampler::new(44_100, 48_000, 2, 2));

        // a missing path still exercises the clear (it runs before the open attempt)
        handle_command(
            AudioCommand::Load("/no/such/lyrfin-test-file.flac".into()),
            48_000,
            2,
            &evt_tx,
            &shared,
            &mut ctl,
        );

        assert!(
            ctl.external.is_none(),
            "Load must drop the external (Spotify) source so local audio plays"
        );
        assert!(
            ctl.ext_resampler.is_none(),
            "Load must drop the external resampler too"
        );
    }

    /// Equal-power gain for A at fraction `t`: cos(t·π/2).
    fn ga(t: f32) -> f32 {
        (t * std::f32::consts::FRAC_PI_2).cos()
    }

    #[test]
    fn crossfade_ramps_a_out_and_b_in() {
        // 4 frames, mono, full window of 4: t = 0, .25, .5, .75
        let a = vec![1.0; 4]; // A constant 1.0
        let b = vec![0.0; 4]; // B constant 0.0
        let mix = crossfade_mix(&a, &b, 1, 0, 4, 4);
        // A fades out along the equal-power curve
        for (i, &t) in [0.0, 0.25, 0.5, 0.75].iter().enumerate() {
            assert!((mix[i] - ga(t)).abs() < 1e-5, "{} vs {}", mix[i], ga(t));
        }
        assert!(mix[0] > mix[3], "A monotonically fades out");

        // start halfway through the window → continues the same curve
        let mix2 = crossfade_mix(&a, &b, 1, 2, 4, 2);
        assert!((mix2[0] - ga(0.5)).abs() < 1e-5);
        assert!((mix2[1] - ga(0.75)).abs() < 1e-5);
    }

    #[test]
    fn silence_trim_drops_edges_keeps_interior() {
        let mut t = SilenceTrim::default();
        // mono: 2 leading zeros · audio · 2 interior zeros · audio · 2 trailing zeros
        let mut buf = vec![0.0, 0.0, 0.5, 0.6, 0.0, 0.0, 0.7, 0.0, 0.0];
        t.process(&mut buf, 1);
        // leading dropped; interior kept (audio resumes after it); trailing held
        // back (emitted only if more audio follows — here the track ends, so it's
        // dropped with the Decode).
        assert_eq!(buf, vec![0.5, 0.6, 0.0, 0.0, 0.7]);
    }

    #[test]
    fn silence_trim_leading_spans_chunks() {
        let mut t = SilenceTrim::default();
        let mut a = vec![0.0, 0.0, 0.0, 0.0];
        t.process(&mut a, 1);
        assert!(a.is_empty(), "an all-silent leading chunk emits nothing");
        let mut b = vec![0.0, 0.9, 0.8];
        t.process(&mut b, 1);
        assert_eq!(
            b,
            vec![0.9, 0.8],
            "leading silence stays trimmed across the chunk boundary"
        );
    }

    #[test]
    fn silence_trim_clear_hold_drops_pending_silence() {
        let mut t = SilenceTrim::default();
        let mut buf = vec![0.4, 0.0, 0.0]; // audio, then a held silent run
        t.process(&mut buf, 1);
        assert_eq!(buf, vec![0.4], "trailing zeros are held, not emitted");
        t.clear_hold(); // a seek repositions the decoder → drop the stale hold
        let mut next = vec![0.3];
        t.process(&mut next, 1);
        assert_eq!(
            next,
            vec![0.3],
            "post-seek audio isn't prefixed by stale silence"
        );
    }

    #[test]
    fn crossfade_missing_source_is_silence() {
        // B shorter than n → its missing frames are silence (A just fades out)
        let a = vec![1.0, 1.0, 1.0, 1.0];
        let b: Vec<f32> = vec![];
        let mix = crossfade_mix(&a, &b, 1, 0, 4, 4);
        for (i, &t) in [0.0, 0.25, 0.5, 0.75].iter().enumerate() {
            assert!((mix[i] - ga(t)).abs() < 1e-5);
        }
    }
}
