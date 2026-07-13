//! Spectrum analysis for the visualizer: a windowed FFT over the playback
//! stream, grouped into log-spaced bands with attack/decay smoothing — the
//! "cava" look driving `design/mockups/05-visualizer.svg`.

use std::sync::Arc;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

/// Number of frequency bands rendered as bars.
pub const DEFAULT_BANDS: usize = 72;
const FFT_SIZE: usize = 2048;

pub struct Analyzer {
    pub bands: usize,
    pub smoothing: f32, // 0.0..=1.0 decay factor
    levels: Vec<f32>,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    scratch: Vec<Complex<f32>>,
}

impl Analyzer {
    pub fn new(bands: usize, smoothing: f32) -> Self {
        let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
        // Hann window
        let window = (0..FFT_SIZE)
            .map(|i| {
                let x = std::f32::consts::PI * i as f32 / (FFT_SIZE as f32 - 1.0);
                x.sin().powi(2)
            })
            .collect();
        Self {
            bands,
            smoothing,
            levels: vec![0.0; bands],
            fft,
            window,
            scratch: vec![Complex::new(0.0, 0.0); FFT_SIZE],
        }
    }

    /// Feed the latest mono samples; returns smoothed band levels (0.0..=1.0).
    pub fn process(&mut self, mono: &[f32]) -> &[f32] {
        if mono.len() < FFT_SIZE {
            return &self.levels;
        }
        let start = mono.len() - FFT_SIZE;
        for (i, c) in self.scratch.iter_mut().enumerate() {
            *c = Complex::new(mono[start + i] * self.window[i], 0.0);
        }
        self.fft.process(&mut self.scratch);

        let bins = FFT_SIZE / 2;
        let min_bin = 1usize;
        for b in 0..self.bands {
            // log-spaced bin range for this band
            let f0 = b as f32 / self.bands as f32;
            let f1 = (b + 1) as f32 / self.bands as f32;
            let lo = (min_bin as f32 * (bins as f32 / min_bin as f32).powf(f0)) as usize;
            let hi =
                ((min_bin as f32 * (bins as f32 / min_bin as f32).powf(f1)) as usize).max(lo + 1);
            // Peak (not average) magnitude in the band. Wide high-frequency bands
            // span many near-zero bins, so averaging buries them — the loudest
            // bin keeps treble content visible.
            let mut mag = 0.0f32;
            for bin in lo..hi.min(bins) {
                mag = mag.max(self.scratch[bin].norm());
            }
            // log-scale magnitude into 0..1 (tuned for typical music levels)
            let level = ((mag / FFT_SIZE as f32 * 160.0 + 1.0).ln() / 4.5).clamp(0.0, 1.0);
            // attack instantly, decay smoothly
            self.levels[b] = if level > self.levels[b] {
                level
            } else {
                self.levels[b] * self.smoothing + level * (1.0 - self.smoothing)
            };
        }
        &self.levels
    }
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new(DEFAULT_BANDS, 0.80)
    }
}
