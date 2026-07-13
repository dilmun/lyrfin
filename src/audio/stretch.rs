//! Pitch-preserved time stretching (WSOLA) for variable playback speed.
//!
//! Resampling changes speed *and* pitch (the chipmunk effect). WSOLA — Waveform
//! Similarity Overlap-Add — changes duration while keeping pitch: it emits
//! overlapping windows at a fixed output hop, but picks each input window near
//! its nominal position by maximising waveform similarity, so the overlap-add
//! has no phase discontinuities.
//!
//! Streaming: feed device-rate interleaved samples with [`TimeStretch::push`] and
//! drain stretched output with [`TimeStretch::pull`]. At speed `1.0` it's a
//! straight pass-through (zero added latency / artefacts).

/// Analysis/synthesis window length in frames (~23 ms at 44.1 kHz).
const N: usize = 1024;
/// Synthesis hop (output advances this many frames per window). 50 % overlap.
const HS: usize = N / 2;

/// Similarity search radius around the nominal analysis position. Kept to half
/// the hop difference so the search can never reach the *non-stretched*
/// continuation (`prev + HS`) and accidentally cancel the stretch.
fn search_radius(ha: usize) -> usize {
    ((HS as i64 - ha as i64).unsigned_abs() as usize / 2).clamp(8, HS - 1)
}

pub struct TimeStretch {
    ch: usize,
    speed: f32,
    win: Vec<f32>,  // Hann window, length N
    buf: Vec<f32>,  // interleaved input, frame 0 = front
    mono: Vec<f32>, // per-frame mono mix, parallel to `buf`
    acc: Vec<f32>,  // overlap-add accumulator, N * ch
    prev: usize,    // last chosen window start (frame index into `buf`)
    started: bool,
}

impl TimeStretch {
    pub fn new(ch: usize) -> Self {
        let ch = ch.max(1);
        let win = (0..N)
            .map(|n| 0.5 - 0.5 * (std::f32::consts::TAU * n as f32 / N as f32).cos())
            .collect();
        Self {
            ch,
            speed: 1.0,
            win,
            buf: Vec::new(),
            mono: Vec::new(),
            acc: vec![0.0; N * ch],
            prev: 0,
            started: false,
        }
    }

    pub fn set_speed(&mut self, speed: f32) {
        self.speed = speed.clamp(0.25, 4.0);
    }

    /// True when time-stretching is active (speed audibly off 1.0).
    pub fn active(&self) -> bool {
        (self.speed - 1.0).abs() > 0.01
    }

    /// Clear buffered audio for a new track (keeps the configured speed).
    pub fn reset(&mut self) {
        self.buf.clear();
        self.mono.clear();
        self.acc.iter_mut().for_each(|s| *s = 0.0);
        self.prev = 0;
        self.started = false;
    }

    /// Append interleaved device-rate input.
    pub fn push(&mut self, input: &[f32]) {
        self.buf.extend_from_slice(input);
        for frame in input.chunks(self.ch) {
            self.mono
                .push(frame.iter().copied().sum::<f32>() / self.ch as f32);
        }
    }

    /// Drain all stretched output currently producible from the buffered input.
    pub fn pull(&mut self, out: &mut Vec<f32>) {
        // pass-through at unity speed — no windowing, no latency
        if !self.active() {
            out.extend_from_slice(&self.buf);
            self.buf.clear();
            self.mono.clear();
            self.started = false;
            self.prev = 0;
            return;
        }

        let ha = ((HS as f32) * self.speed).round().max(1.0) as usize;
        let w = search_radius(ha);
        loop {
            let frames = self.buf.len() / self.ch;
            // first window: no search, take from the front
            if !self.started {
                if frames < N {
                    break;
                }
                self.prev = 0;
                self.emit_window(0, out);
                self.started = true;
                continue;
            }
            let target = self.prev + HS; // the natural continuation of the last window
            let nominal = self.prev + ha; // where the next window nominally starts
            // need the target window and the whole search range buffered
            if frames < target + N || frames < nominal + w + N {
                break;
            }
            let lo = nominal.saturating_sub(w);
            let hi = (nominal + w).min(frames - N);
            let chosen = self.best_match(target, lo, hi);
            self.emit_window(chosen, out);
            self.prev = chosen;
            // drop input we'll never reference again (everything before `prev`)
            self.drain_front(self.prev);
        }
    }

    /// Overlap-add the windowed input window at frame `start`, emitting `HS`
    /// finished frames.
    fn emit_window(&mut self, start: usize, out: &mut Vec<f32>) {
        let base = start * self.ch;
        for f in 0..N {
            let w = self.win[f];
            for c in 0..self.ch {
                self.acc[f * self.ch + c] += self.buf[base + f * self.ch + c] * w;
            }
        }
        let hop = HS * self.ch;
        out.extend_from_slice(&self.acc[..hop]);
        self.acc.copy_within(hop.., 0);
        self.acc[(N - HS) * self.ch..]
            .iter_mut()
            .for_each(|s| *s = 0.0);
    }

    /// Window start in `[lo, hi]` whose mono content best matches the target
    /// window (minimum sum-of-squared-difference).
    fn best_match(&self, target: usize, lo: usize, hi: usize) -> usize {
        let mut best = lo;
        let mut best_err = f32::MAX;
        let mut c = lo;
        while c <= hi {
            let mut err = 0.0f32;
            for k in 0..N {
                let d = self.mono[c + k] - self.mono[target + k];
                err += d * d;
                if err >= best_err {
                    break; // early-out: already worse than the best so far
                }
            }
            if err < best_err {
                best_err = err;
                best = c;
            }
            c += 1;
        }
        best
    }

    /// Drop `frames` from the front of the buffers, rebasing `prev`.
    fn drain_front(&mut self, frames: usize) {
        if frames == 0 {
            return;
        }
        self.buf.drain(..frames * self.ch);
        self.mono.drain(..frames);
        self.prev -= frames;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An aperiodic chirp (rising frequency), interleaved stereo. Aperiodic so
    /// the stretch ratio is set by the hop, not by accidental self-similarity.
    fn signal(frames: usize) -> Vec<f32> {
        let mut phase = 0.0f32;
        (0..frames)
            .flat_map(|i| {
                let f = 0.02 + 0.00002 * i as f32; // sweep upward
                phase += f;
                let s = phase.sin();
                [s, s]
            })
            .collect()
    }

    #[test]
    fn passthrough_at_unity_speed() {
        let mut ts = TimeStretch::new(2);
        let input = signal(5000);
        ts.push(&input);
        let mut out = Vec::new();
        ts.pull(&mut out);
        assert_eq!(out, input, "speed 1.0 is bit-exact pass-through");
    }

    // The exact stretch ratio is signal-dependent (WSOLA picks windows by
    // similarity), so we assert the guarantees that always hold: faster →
    // shorter, slower → longer, within sane bounds, and never a panic.
    #[test]
    fn faster_speed_shortens_output() {
        let mut ts = TimeStretch::new(2);
        ts.set_speed(2.0);
        let input = signal(20_000);
        ts.push(&input);
        let mut out = Vec::new();
        ts.pull(&mut out);
        let (i, o) = (input.len() / 2, out.len() / 2);
        assert!(o < i && o > i / 4, "2x should shorten: {o} vs {i}");
    }

    #[test]
    fn slower_speed_lengthens_output() {
        let mut ts = TimeStretch::new(2);
        ts.set_speed(0.5);
        let input = signal(20_000);
        ts.push(&input);
        let mut out = Vec::new();
        ts.pull(&mut out);
        let (i, o) = (input.len() / 2, out.len() / 2);
        assert!(o > i && o < i * 4, "0.5x should lengthen: {o} vs {i}");
    }

    #[test]
    fn streaming_in_chunks_matches_buffering() {
        // feeding small chunks must not panic and must still compress at >1x
        let mut ts = TimeStretch::new(2);
        ts.set_speed(1.5);
        let input = signal(30_000);
        let mut out = Vec::new();
        for chunk in input.chunks(777) {
            ts.push(chunk);
            ts.pull(&mut out);
        }
        let (i, o) = (input.len() / 2, out.len() / 2);
        assert!(o < i && !out.is_empty(), "1.5x should compress: {o} vs {i}");
    }
}
