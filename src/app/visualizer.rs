//! The animated bar visualizer: eased band levels, monstercat spatial smoothing,
//! falling peak caps, and the receding-history ring for the 3D waterfall. Owns
//! its own state + per-frame update so `AppState` holds one `viz` field instead
//! of five. Pure animation (presentation state); `update_viz` is the AppState
//! glue that feeds it the current view's audio spectrum each tick.

use super::*;

/// Animated bar-visualizer: eased band levels, monstercat spatial smoothing, and
/// falling peak caps.
#[derive(Debug)]
pub struct Visualizer {
    /// Smoothed band levels (eased every frame for fluid motion).
    pub levels: Vec<f32>,
    /// Falling peak-cap positions per band.
    pub peaks: Vec<f32>,
    peak_vel: Vec<f32>,
    /// Frames each cap still holds at its peak before it starts falling.
    peak_hold: Vec<u16>,
    /// Global adaptive gain (slowly-decaying overall peak) — scales the whole
    /// spectrum to fill the panel without flattening individual bands.
    gain: f32,
    /// Recent `levels` snapshots, oldest-first — the 3D waterfall's depth rows.
    pub history: std::collections::VecDeque<Vec<f32>>,
}

impl Visualizer {
    /// Depth (number of receding rows) kept for the 3D waterfall.
    const HISTORY: usize = 28;

    pub(super) fn new(bands: usize) -> Self {
        Visualizer {
            levels: vec![0.0; bands],
            peaks: vec![0.0; bands],
            peak_vel: vec![0.0; bands],
            peak_hold: vec![0; bands],
            gain: 0.3,
            history: std::collections::VecDeque::with_capacity(Self::HISTORY),
        }
    }

    /// Advance one frame toward `spectrum`. While paused (and not reduced-motion)
    /// the bars decay to silence and then this early-outs — no idle CPU burn.
    fn update(
        &mut self,
        spectrum: &[f32],
        playing: bool,
        reduced_motion: bool,
        gravity: f32,
        hang: u16,
    ) {
        let n = self.levels.len();
        if !playing
            && !reduced_motion
            && self.levels.iter().all(|&v| v < 0.005)
            && self.peaks.iter().all(|&p| p < 0.005)
        {
            return;
        }
        // 1. per-band targets
        let mut targets = vec![0.0f32; n];
        if reduced_motion {
            targets.fill(0.04);
        } else if playing {
            // perceptual high-frequency tilt, then scale the whole spectrum by one
            // slowly-adapting gain so each band keeps its own dynamics.
            let mut frame_max = 1e-4f32;
            for (i, slot) in targets.iter_mut().enumerate() {
                let raw = spectrum[(i * spectrum.len() / n).min(spectrum.len() - 1)];
                let tilt = 1.0 + 7.0 * (i as f32 / n as f32).powf(1.1);
                let t = raw * tilt;
                *slot = t;
                frame_max = frame_max.max(t);
            }
            self.gain = (self.gain * 0.992).max(frame_max).max(1e-3);
            for slot in targets.iter_mut() {
                *slot = (*slot / self.gain).min(1.0);
            }
        }
        // else (paused): targets stay 0 → bars decay to silence

        // 2. temporal easing (fast attack / slow decay)
        for (lvl, &tgt) in self.levels.iter_mut().zip(targets.iter()) {
            let rate = if tgt > *lvl { 0.42 } else { 0.16 };
            *lvl += (tgt - *lvl) * rate;
        }

        // 3. monstercat smoothing: each band lifts its neighbours by 1/factor^d
        const WIN: i32 = 4;
        const FACTOR: f32 = 1.8;
        let src = self.levels.clone();
        for i in 0..n {
            let mut v = src[i];
            for d in 1..=WIN {
                let falloff = FACTOR.powi(d);
                for j in [i as i32 - d, i as i32 + d] {
                    if j >= 0 && (j as usize) < n {
                        let infl = src[j as usize] / falloff;
                        if infl > v {
                            v = infl;
                        }
                    }
                }
            }
            self.levels[i] = v;
        }

        // 4. falling peak caps: snap up on a new peak, hang, then fall under gravity
        for i in 0..n {
            let lvl = self.levels[i];
            if lvl >= self.peaks[i] {
                self.peaks[i] = lvl;
                self.peak_vel[i] = 0.0;
                self.peak_hold[i] = hang;
            } else if self.peak_hold[i] > 0 {
                self.peak_hold[i] -= 1;
            } else {
                self.peak_vel[i] += gravity;
                self.peaks[i] = (self.peaks[i] - self.peak_vel[i]).max(lvl).max(0.0);
            }
        }

        // 5. ring of recent frames for the 3D waterfall's receding depth rows
        self.history.push_back(self.levels.clone());
        while self.history.len() > Self::HISTORY {
            self.history.pop_front();
        }
    }
}

impl AppState {
    /// Feed the visualizer this tick's spectrum — but only from the audio of the
    /// CURRENT view's context: radio audio drives it in the Radio view; the local
    /// player drives it everywhere else (flat while radio streams, since local is
    /// paused). So radio never bleeds its motion into the music-player views and
    /// vice-versa.
    pub(super) fn update_viz(&mut self) {
        let audio_live = if self.layout == Layout::Spotify {
            self.spov.now_spotify.is_some() && !self.spov.spotify_paused
        } else if self.layout == Layout::Radio {
            self.rnow.now_station.is_some() && !self.rnow.radio_paused
        } else {
            self.player.status == Status::Playing
        };
        let playing = self.player.spectrum.len() >= 2 && audio_live;
        self.viz.update(
            &self.player.spectrum,
            playing,
            self.config.reduced_motion,
            self.config.viz_gravity,
            self.config.viz_peak_hang,
        );
    }
}
