//! Streaming cubic-interpolation resampler + channel mapper (interleaved f32),
//! extracted from the audio engine. Pure DSP — no threading or I/O — so it's
//! numerically testable in isolation; the engine's controller drives it to
//! convert decoded/external audio to the device rate + channel count.

/// Streaming cubic-interpolation resampler + channel mapper (interleaved f32).
///
/// Resamples the fractional read position with 4-point cubic Hermite
/// (Catmull-Rom) interpolation — smoother than 2-point linear, with markedly
/// less interpolation distortion on rate conversions (e.g. a 44.1 kHz file on a
/// 48 kHz device). The trailing input frames are carried across packets in `acc`
/// so the interpolation is continuous over packet boundaries (no per-packet
/// seam); channel up/down-mixing is applied after interpolation. A matched rate
/// **and** channel count is a zero-work passthrough.
///
/// (Considered `rubato` for sinc/FFT-quality resampling; its fixed-chunk,
/// planar-buffer model fights this variable-size interleaved packet pipeline and
/// the gain is inaudible at typical 44.1↔48 ratios, so a continuous cubic
/// interpolator — testable numerically, zero added latency — was chosen.)
pub(crate) struct Resampler {
    step: f64, // input frames advanced per output frame
    in_ch: usize,
    out_ch: usize,
    identity: bool,  // same rate + channels → passthrough
    pos: f64,        // fractional read index within `acc` (frames)
    acc: Vec<f32>,   // carried left-context + unconsumed input (interleaved)
    frame: Vec<f32>, // scratch: one interpolated input frame (len `in_ch`)
    primed: bool,    // false until the leading left-context frame is seeded
}

impl Resampler {
    pub(crate) fn new(in_rate: u32, out_rate: u32, in_ch: usize, out_ch: usize) -> Self {
        let in_ch = in_ch.max(1);
        Self {
            step: in_rate as f64 / out_rate.max(1) as f64,
            in_ch,
            out_ch: out_ch.max(1),
            identity: in_rate == out_rate && in_ch == out_ch,
            pos: 0.0,
            acc: Vec::new(),
            frame: vec![0.0; in_ch],
            primed: false,
        }
    }

    /// Drop carried state for a new/seeked track (keeps the rate/channel config).
    pub(crate) fn reset(&mut self) {
        self.acc.clear();
        self.pos = 0.0;
        self.primed = false;
    }

    /// Resample one interleaved input packet into `out` (cleared + reused).
    pub(crate) fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        out.clear();
        if self.identity {
            out.extend_from_slice(input);
            return;
        }
        let ch = self.in_ch;
        if input.len() < ch {
            return; // not even one whole frame
        }
        // Seed one left-context frame on the first packet (a copy of frame 0) so
        // the very first sample is produced — `pos` then starts at 1.0, giving
        // every interpolation a real left neighbour.
        if !self.primed {
            self.acc.extend_from_slice(&input[..ch]);
            self.pos = 1.0;
            self.primed = true;
        }
        self.acc.extend_from_slice(input);
        let n = self.acc.len() / ch; // frames available

        let mut pos = self.pos;
        // Need p0..p3 at floor(pos)-1 ..= floor(pos)+2, so floor(pos)+2 <= n-1.
        while (pos.floor() as usize) + 3 <= n {
            let i = pos.floor() as usize;
            let frac = (pos - i as f64) as f32;
            for (c, slot) in self.frame.iter_mut().enumerate() {
                let p0 = self.acc[(i - 1) * ch + c];
                let p1 = self.acc[i * ch + c];
                let p2 = self.acc[(i + 1) * ch + c];
                let p3 = self.acc[(i + 2) * ch + c];
                *slot = hermite(p0, p1, p2, p3, frac);
            }
            map_channels(&self.frame, self.out_ch, out);
            pos += self.step;
        }
        // Drop consumed frames but keep one frame of left context (floor(pos)-1)
        // for the next packet's first interpolation; rebase `pos` onto it.
        let keep_from = (pos.floor() as usize).saturating_sub(1);
        if keep_from > 0 {
            self.acc.drain(..keep_from * ch);
            pos -= keep_from as f64;
        }
        self.pos = pos;
    }
}

/// 4-point cubic Hermite (Catmull-Rom) interpolation at fraction `t` in [0,1)
/// between `p1` and `p2`, using neighbours `p0`/`p3` for the tangents. Has linear
/// precision (a linear input resamples exactly), so packet splits are seamless.
fn hermite(p0: f32, p1: f32, p2: f32, p3: f32, t: f32) -> f32 {
    let a = -0.5 * p0 + 1.5 * p1 - 1.5 * p2 + 0.5 * p3;
    let b = p0 - 2.5 * p1 + 2.0 * p2 - 0.5 * p3;
    let c = -0.5 * p0 + 0.5 * p2;
    ((a * t + b) * t + c) * t + p1
}

fn map_channels(frame: &[f32], out_ch: usize, out: &mut Vec<f32>) {
    let in_ch = frame.len();
    match (in_ch, out_ch) {
        (i, o) if i == o => out.extend_from_slice(frame),
        (1, o) => {
            for _ in 0..o {
                out.push(frame[0]);
            }
        }
        (2, 1) => out.push((frame[0] + frame[1]) * 0.5),
        (i, o) => {
            for c in 0..o {
                out.push(frame[c % i]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Resampler;

    #[test]
    fn resampler_passthrough_identity() {
        // matched rate + channel count → bit-exact passthrough (no interpolation)
        let mut r = Resampler::new(48_000, 48_000, 2, 2);
        let input: Vec<f32> = (0..100).map(|i| i as f32).collect();
        let mut out = Vec::new();
        r.process(&input, &mut out);
        assert_eq!(out, input);
    }

    #[test]
    fn resampler_downsample_ramp_is_linear() {
        // 2:1 downsample of a ramp picks every other sample — cubic Hermite has
        // linear precision, so a linear input is reproduced exactly.
        let mut r = Resampler::new(2, 1, 1, 1);
        let input: Vec<f32> = (0..20).map(|i| i as f32).collect();
        let mut out = Vec::new();
        r.process(&input, &mut out);
        assert!(
            out.len() >= 8,
            "expected several output frames: {}",
            out.len()
        );
        for (k, &v) in out.iter().enumerate() {
            assert!((v - (2 * k) as f32).abs() < 1e-4, "out[{k}] = {v}");
        }
    }

    #[test]
    fn resampler_packet_split_is_seamless() {
        // The same ramp fed as one packet vs two must resample identically — the
        // carried left-context makes packet boundaries seam-free (no glitch).
        let ramp: Vec<f32> = (0..64).map(|i| i as f32 * 0.5).collect();
        let mut whole = Resampler::new(3, 2, 1, 1); // non-integer 1.5x ratio
        let mut a = Vec::new();
        whole.process(&ramp, &mut a);

        let mut split = Resampler::new(3, 2, 1, 1);
        let (mut b, mut tmp) = (Vec::new(), Vec::new());
        split.process(&ramp[..21], &mut tmp);
        b.extend_from_slice(&tmp);
        split.process(&ramp[21..], &mut tmp);
        b.extend_from_slice(&tmp);

        let m = a.len().min(b.len());
        assert!(m > 20, "produced enough output to compare: {m}");
        for k in 0..m {
            assert!(
                (a[k] - b[k]).abs() < 1e-4,
                "k={k} whole={} split={}",
                a[k],
                b[k]
            );
        }
    }

    #[test]
    fn resampler_reset_clears_carry() {
        // after reset the next packet starts clean (its first sample is emitted)
        let mut r = Resampler::new(2, 1, 1, 1);
        let mut out = Vec::new();
        r.process(&[10.0, 20.0, 30.0, 40.0], &mut out);
        r.reset();
        out.clear();
        r.process(&[1.0, 2.0, 3.0, 4.0], &mut out);
        assert_eq!(out.first().copied(), Some(1.0), "first post-reset sample");
    }
}
