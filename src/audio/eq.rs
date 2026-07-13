//! 10-band graphic equalizer DSP: a per-channel cascade of peaking biquad
//! filters plus a master preamp. Pure signal processing — no UI, no config, no
//! threading assumptions. It runs on the audio *controller* thread (see
//! [`crate::audio::engine`]), never the UI thread and never the realtime cpal
//! callback; parameters are pushed in with [`crate::audio::AudioCommand::SetEq`],
//! so the controller owns this state exclusively (no locking on the audio path).

use std::f32::consts::PI;

/// Number of EQ bands.
pub const EQ_BANDS: usize = 10;

/// ISO-octave band center frequencies (Hz), low → high.
pub const EQ_FREQS: [f32; EQ_BANDS] = [
    31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];

/// Compact labels shown under each band slider.
pub const EQ_FREQ_LABELS: [&str; EQ_BANDS] = [
    "31", "62", "125", "250", "500", "1k", "2k", "4k", "8k", "16k",
];

/// Gain range (dB) for every band **and** the preamp.
pub const EQ_MIN_DB: f32 = -12.0;
pub const EQ_MAX_DB: f32 = 12.0;

/// Peaking-filter Q. ≈1.4 gives roughly one-octave bandwidth, matching the
/// octave spacing of the band centers so adjacent bells overlap smoothly.
const EQ_Q: f32 = 1.41;

/// The equalizer's parameters — the payload sent to the engine on any change.
/// Small and `Copy` so it rides the command channel cheaply.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EqConfig {
    pub enabled: bool,
    /// Master preamp in dB, applied before the bands.
    pub preamp_db: f32,
    /// Per-band gain in dB, low → high (aligned with [`EQ_FREQS`]).
    pub bands: [f32; EQ_BANDS],
}

impl Default for EqConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            preamp_db: 0.0,
            bands: [0.0; EQ_BANDS],
        }
    }
}

/// One normalized second-order (biquad) section (`a0` folded to 1).
#[derive(Debug, Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Biquad {
    /// Pass-through section.
    const fn identity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }

    /// RBJ "audio EQ cookbook" peaking filter at `freq` Hz with quality `q` and
    /// `gain_db`, for sample rate `fs`. A 0 dB gain returns the identity section
    /// (so a flat band is a true no-op). The center is clamped just below Nyquist
    /// so a high band on a low-rate device can't blow up.
    fn peaking(freq: f32, q: f32, gain_db: f32, fs: f32) -> Self {
        if gain_db == 0.0 || fs <= 0.0 || q <= 0.0 {
            return Self::identity();
        }
        let a = 10f32.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * (freq / fs).clamp(1.0e-4, 0.499);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * q);
        let a0 = 1.0 + alpha / a;
        Self {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cos) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cos) / a0,
            a2: (1.0 - alpha / a) / a0,
        }
    }
}

/// Direct-Form-I delay memory for one biquad on one channel.
#[derive(Debug, Clone, Copy, Default)]
struct State {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl State {
    #[inline]
    fn process(&mut self, bq: &Biquad, x: f32) -> f32 {
        let y = bq.b0 * x + bq.b1 * self.x1 + bq.b2 * self.x2 - bq.a1 * self.y1 - bq.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// A 10-band graphic equalizer over interleaved f32 audio at a fixed sample rate
/// and channel count. Cheap enough to run per-sample on the audio thread — 10
/// biquads × channels of multiply-adds per frame — and a true bypass when
/// disabled (no work at all). Built once at the device format; parameters change
/// via [`Self::configure`].
#[derive(Debug, Clone)]
pub struct Equalizer {
    enabled: bool,
    fs: f32,
    channels: usize,
    /// Linear preamp multiplier (derived from `preamp_db`).
    preamp: f32,
    /// Per-band peaking coefficients (shared across channels).
    coeffs: [Biquad; EQ_BANDS],
    /// Per-channel, per-band delay state: `state[channel][band]`.
    state: Vec<[State; EQ_BANDS]>,
}

impl Equalizer {
    /// A flat, disabled equalizer for the given device format.
    pub fn new(sample_rate: u32, channels: usize) -> Self {
        let channels = channels.max(1);
        Self {
            enabled: false,
            fs: sample_rate.max(1) as f32,
            channels,
            preamp: 1.0,
            coeffs: [Biquad::identity(); EQ_BANDS],
            state: vec![[State::default(); EQ_BANDS]; channels],
        }
    }

    /// Apply a new parameter set: recompute the preamp + band coefficients. Cheap
    /// (10 filter designs) and only called when the user changes something, so it
    /// never touches the per-sample path's budget. Enabling from a disabled state
    /// clears the filter memory so stale history can't pop.
    pub fn configure(&mut self, cfg: &EqConfig) {
        if cfg.enabled && !self.enabled {
            self.reset();
        }
        self.enabled = cfg.enabled;
        self.preamp = 10f32.powf(cfg.preamp_db / 20.0);
        for (i, bq) in self.coeffs.iter_mut().enumerate() {
            *bq = Biquad::peaking(EQ_FREQS[i], EQ_Q, cfg.bands[i], self.fs);
        }
    }

    /// Zero the filter delay memory (e.g. on enable, so no transient carries over).
    pub fn reset(&mut self) {
        for ch in self.state.iter_mut() {
            *ch = [State::default(); EQ_BANDS];
        }
    }

    /// Process an interleaved buffer of `channels`-wide frames in place. A no-op
    /// when disabled (true bypass — not even the preamp runs). Skips silently if
    /// `channels` doesn't match the layout the state was allocated for, rather
    /// than risk indexing out of bounds.
    pub fn process(&mut self, buf: &mut [f32], channels: usize) {
        if !self.enabled || channels == 0 || channels != self.channels {
            return;
        }
        for frame in buf.chunks_mut(channels) {
            for (ch, s) in frame.iter_mut().enumerate() {
                let mut v = *s * self.preamp;
                let st = &mut self.state[ch];
                for (band, section) in st.iter_mut().enumerate() {
                    v = section.process(&self.coeffs[band], v);
                }
                *s = v;
            }
        }
    }
}

/// Built-in EQ presets: `(name, per-band dB low → high)`. "Flat" (all zero) is
/// the neutral default. Curves are tasteful and conservative (within ±6 dB),
/// voiced for the 31 Hz – 16 kHz band centers.
pub const BUILTIN_EQ_PRESETS: &[(&str, [f32; EQ_BANDS])] = &[
    ("Flat", [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]),
    (
        "Bass Boost",
        [6.0, 5.0, 4.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0],
    ),
    (
        "Treble Boost",
        [0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 2.0, 4.0, 5.0, 6.0],
    ),
    (
        "Vocal",
        [-2.0, -1.0, 0.0, 2.0, 4.0, 4.0, 3.0, 1.0, 0.0, -1.0],
    ),
    ("Rock", [4.0, 3.0, 1.0, -1.0, -1.0, 1.0, 2.0, 3.0, 4.0, 4.0]),
    (
        "Pop",
        [-1.0, 0.0, 1.0, 2.0, 3.0, 2.0, 0.0, -1.0, -1.0, -1.0],
    ),
    ("Jazz", [3.0, 2.0, 1.0, 2.0, -1.0, -1.0, 0.0, 1.0, 2.0, 3.0]),
    (
        "Classical",
        [4.0, 3.0, 2.0, 0.0, -1.0, -1.0, 0.0, 2.0, 3.0, 4.0],
    ),
    (
        "Electronic",
        [5.0, 4.0, 1.0, 0.0, -2.0, 1.0, 0.0, 1.0, 4.0, 5.0],
    ),
    (
        "Acoustic",
        [4.0, 3.0, 1.0, 0.0, 2.0, 2.0, 3.0, 3.0, 2.0, 1.0],
    ),
];

/// The named preset whose curve matches `bands` exactly, if any (so the UI can
/// show "Rock" instead of "Custom" when the sliders land on a built-in). Preamp
/// is not part of the match — presets only define band gains.
pub fn matching_preset(bands: &[f32; EQ_BANDS]) -> Option<&'static str> {
    BUILTIN_EQ_PRESETS
        .iter()
        .find(|(_, curve)| curve == bands)
        .map(|(name, _)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RMS of a slice (used to compare band energy before/after filtering).
    fn rms(xs: &[f32]) -> f32 {
        (xs.iter().map(|x| x * x).sum::<f32>() / xs.len().max(1) as f32).sqrt()
    }

    /// A `freq`-Hz sine at `fs` for `n` samples, mono.
    fn sine(freq: f32, fs: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (2.0 * PI * freq * i as f32 / fs).sin())
            .collect()
    }

    #[test]
    fn disabled_eq_is_a_true_bypass() {
        let mut eq = Equalizer::new(44_100, 2);
        let mut buf = vec![0.1, -0.2, 0.3, -0.4, 0.5, -0.6];
        let before = buf.clone();
        eq.process(&mut buf, 2); // disabled → untouched
        assert_eq!(buf, before);
    }

    #[test]
    fn flat_enabled_eq_passes_signal_through_unchanged() {
        // enabled but every band + preamp at 0 dB → identity filters, unity preamp
        let mut eq = Equalizer::new(44_100, 1);
        eq.configure(&EqConfig {
            enabled: true,
            preamp_db: 0.0,
            bands: [0.0; EQ_BANDS],
        });
        let input = sine(1000.0, 44_100.0, 2048);
        let mut buf = input.clone();
        eq.process(&mut buf, 1);
        for (a, b) in input.iter().zip(&buf) {
            assert!(
                (a - b).abs() < 1e-6,
                "flat EQ altered the signal: {a} vs {b}"
            );
        }
    }

    #[test]
    fn boost_and_cut_move_band_energy_the_right_way() {
        let fs = 44_100.0;
        // band index 5 is 1 kHz; probe with a 1 kHz sine, ignore the warm-up head
        let input = sine(EQ_FREQS[5], fs, 8192);
        let warm = 1024;
        let dry = rms(&input[warm..]);

        let mut boost = Equalizer::new(44_100, 1);
        let mut bands = [0.0; EQ_BANDS];
        bands[5] = 12.0;
        boost.configure(&EqConfig {
            enabled: true,
            preamp_db: 0.0,
            bands,
        });
        let mut b = input.clone();
        boost.process(&mut b, 1);
        assert!(
            rms(&b[warm..]) > dry * 1.5,
            "a +12 dB band should clearly amplify a tone at its center"
        );

        let mut cut = Equalizer::new(44_100, 1);
        bands[5] = -12.0;
        cut.configure(&EqConfig {
            enabled: true,
            preamp_db: 0.0,
            bands,
        });
        let mut c = input.clone();
        cut.process(&mut c, 1);
        assert!(
            rms(&c[warm..]) < dry * 0.7,
            "a -12 dB band should clearly attenuate a tone at its center"
        );
    }

    #[test]
    fn a_distant_band_barely_touches_an_out_of_band_tone() {
        // boosting 31 Hz should leave a 4 kHz tone essentially unchanged
        let fs = 44_100.0;
        let input = sine(EQ_FREQS[7], fs, 8192); // 4 kHz
        let warm = 1024;
        let dry = rms(&input[warm..]);
        let mut eq = Equalizer::new(44_100, 1);
        let mut bands = [0.0; EQ_BANDS];
        bands[0] = 12.0; // boost 31 Hz
        eq.configure(&EqConfig {
            enabled: true,
            preamp_db: 0.0,
            bands,
        });
        let mut buf = input.clone();
        eq.process(&mut buf, 1);
        let wet = rms(&buf[warm..]);
        assert!(
            (wet - dry).abs() < dry * 0.1,
            "a far-off band shouldn't move an out-of-band tone much ({dry} → {wet})"
        );
    }

    #[test]
    fn preamp_scales_the_whole_signal() {
        let mut eq = Equalizer::new(44_100, 1);
        eq.configure(&EqConfig {
            enabled: true,
            preamp_db: 6.0, // ≈ ×1.995
            bands: [0.0; EQ_BANDS],
        });
        let input = sine(1000.0, 44_100.0, 4096);
        let mut buf = input.clone();
        eq.process(&mut buf, 1);
        let ratio = rms(&buf[512..]) / rms(&input[512..]);
        assert!(
            (ratio - 1.995).abs() < 0.05,
            "+6 dB preamp should scale RMS by ~1.995, got {ratio}"
        );
    }

    #[test]
    fn builtin_presets_are_well_formed() {
        assert_eq!(BUILTIN_EQ_PRESETS[0].0, "Flat");
        assert!(BUILTIN_EQ_PRESETS[0].1.iter().all(|&b| b == 0.0));
        for (name, bands) in BUILTIN_EQ_PRESETS {
            assert!(
                bands.iter().all(|&b| (EQ_MIN_DB..=EQ_MAX_DB).contains(&b)),
                "preset {name} stays within the dB range"
            );
        }
        // an exact-curve lookup resolves the name back
        assert_eq!(matching_preset(&BUILTIN_EQ_PRESETS[4].1), Some("Rock"));
        let mut odd = [0.0; EQ_BANDS];
        odd[3] = 7.5;
        assert_eq!(
            matching_preset(&odd),
            None,
            "a custom curve matches nothing"
        );
    }
}
