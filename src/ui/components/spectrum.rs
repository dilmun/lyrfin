//! spectrum rendering (extracted from ui/components).

use super::*;
use crate::app::AppState;
use crate::ui::theme::{Rgb, Theme};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

pub const VIZ_MODES: [&str; 7] = [
    "Bars",
    "Mirror",
    "Wave",
    "Fill",
    "Dots",
    "Tunnel",
    "Waterfall",
];

type VCell = Option<(char, Rgb)>;

/// A Braille sub-pixel canvas: each terminal cell holds 2×4 dots, giving 8×
/// the resolution for genuinely smooth curves and fills.
struct Braille {
    w: usize,
    h: usize,
    bits: Vec<u8>,
    color: Vec<Option<Rgb>>,
}

impl Braille {
    // dot (x%2, y%4) → bit in the Braille pattern byte
    const MAP: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

    fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            bits: vec![0; w * h],
            color: vec![None; w * h],
        }
    }
    fn dot(&mut self, x: usize, y: usize, c: Rgb) {
        if x >= self.w * 2 || y >= self.h * 4 {
            return;
        }
        let idx = (y / 4) * self.w + x / 2;
        self.bits[idx] |= Self::MAP[x % 2][y % 4];
        self.color[idx] = Some(match self.color[idx] {
            Some(old) => old.mix(c, 0.5),
            None => c,
        });
    }
    fn blit(&self, grid: &mut [Vec<VCell>]) {
        for (cy, row) in grid.iter_mut().enumerate().take(self.h) {
            for (cx, cell) in row.iter_mut().enumerate().take(self.w) {
                let idx = cy * self.w + cx;
                let b = self.bits[idx];
                if b != 0 {
                    let ch = char::from_u32(0x2800 + b as u32).unwrap_or(' ');
                    *cell = Some((ch, self.color[idx].unwrap_or(Rgb(255, 255, 255))));
                }
            }
        }
    }
}

pub fn spectrum_panel(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    title: &str,
    mode: u8,
    focused: bool,
) {
    let th = &app.theme;
    let block = rounded(th, title, focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    spectrum_bare(f, inner, app, mode);
}

/// Render the spectrum bars straight into `area` with no panel/border, so they
/// blend onto whatever background is behind them (used by Concert mode).
pub fn spectrum_bare(f: &mut Frame, inner: Rect, app: &AppState, mode: u8) {
    let th = &app.theme;
    let (n, h) = (inner.width as usize, inner.height as usize);
    if n == 0 || h == 0 {
        return;
    }
    let levels = level_bars(app, n);
    let peaks = resample(&app.viz.peaks, n);
    let bases: Vec<Rgb> = (0..n)
        .map(|c| th.accent_at(c as f32 / n.max(1) as f32))
        .collect();

    let mut grid: Vec<Vec<VCell>> = vec![vec![None; n]; h];
    match mode as usize % VIZ_MODES.len() {
        0 => fill_columns(
            &mut grid,
            &levels,
            &peaks,
            th.accent[0], // single bar hue (no per-column rainbow)
            th.warning,   // contrasting cap colour
            h,
            n,
            app.config.peak_caps,
        ),
        1 => fill_mirror(&mut grid, &levels, &bases, h, n),
        2 => fill_wave(&mut grid, &levels, &bases, h, n),
        3 => fill_area(&mut grid, &levels, &bases, h, n),
        4 => fill_dots(&mut grid, &levels, &bases, h, n),
        5 => fill_tunnel(&mut grid, &levels, &bases, h, n),
        _ => fill_waterfall(&mut grid, &app.viz.history, th, h, n),
    }

    let mut lines: Vec<Line> = Vec::with_capacity(h);
    for row in grid {
        let mut spans = Vec::with_capacity(n);
        for cell in row {
            match cell {
                Some((ch, c)) => {
                    spans.push(Span::styled(ch.to_string(), Style::default().fg(col(c))))
                }
                None => spans.push(Span::raw(" ")),
            }
        }
        lines.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// Classic cava bars: evenly-spaced columns in a single colour with a vertical
/// brightness gradient, sub-cell tops, and a contrasting floating peak cap.
/// `bar` is the single bar hue, `cap` the (different) cap colour; `caps` toggles
/// the cap (config `[visualizer] peak_caps`).
#[allow(clippy::too_many_arguments)]
fn fill_columns(
    grid: &mut [Vec<VCell>],
    levels: &[f32],
    peaks: &[f32],
    bar: Rgb,
    cap: Rgb,
    h: usize,
    n: usize,
    caps: bool,
) {
    const BAR_W: usize = 2; // 2-cell columns → vertical rectangles
    const GAP_W: usize = 2; // clear gap between columns
    const STRIDE: usize = BAR_W + GAP_W;
    if n < BAR_W {
        return;
    }
    // Center the bar grid so any leftover width is split evenly (no left skew).
    let bars = (n + GAP_W) / STRIDE;
    let used = (bars * STRIDE).saturating_sub(GAP_W);
    let off = n.saturating_sub(used) / 2;

    for b in 0..bars {
        let bx = off + b * STRIDE;
        let center = (bx + BAR_W / 2).min(n - 1);
        let v = levels[center].clamp(0.0, 1.0);
        let full = ((v * h as f32).round() as usize).min(h);
        // Segmented column: stacked solid blocks (SEG rows) split by a blank row
        // (VGAP) so the bar reads as LED segments, not one long bar.
        const SEG: usize = 2;
        const VGAP: usize = 1;
        for k in 0..full {
            if k % (SEG + VGAP) >= SEG {
                continue; // blank gap row between segments
            }
            let frac = if full > 1 {
                k as f32 / (full - 1) as f32
            } else {
                1.0
            };
            let color = heat(bar, frac);
            let row = h - 1 - k;
            for w in 0..BAR_W {
                if bx + w < n {
                    grid[row][bx + w] = Some(('█', color));
                }
            }
        }
        // Peak cap: a wide segment in a distinct (theme) colour, drawn in the
        // empty space ABOVE the column (never on a bar cell) — so peak_hang only
        // moves the cap and never recolours/extends the column.
        if caps && full < h {
            let pk = (peaks[center].clamp(0.0, 1.0) * h as f32).round() as usize;
            let cap_h = pk.max(full).min(h - 1);
            // No cap resting on the floor: only show it when there's a bar or a
            // live peak still falling (cap_h > 0). Idle/silent bands stay clear.
            if cap_h > 0 {
                let row = h - 1 - cap_h;
                for w in 0..BAR_W {
                    if bx + w < n {
                        grid[row][bx + w] = Some(('▄', cap));
                    }
                }
            }
        }
    }
}

/// Mirrored EQ: bars grow up and down from a center axis (down side dimmer).
fn fill_mirror(grid: &mut [Vec<VCell>], levels: &[f32], bases: &[Rgb], h: usize, n: usize) {
    let cr = h / 2; // center row
    for c in 0..n {
        let v = levels[c].clamp(0.0, 1.0);
        let base = bases[c];
        let total = v * (h as f32 / 2.0);
        let full = total.floor() as usize;
        for k in 0..full {
            let frac = if full > 1 {
                1.0 - k as f32 / (full - 1) as f32
            } else {
                1.0
            };
            let up = heat(base, frac);
            let dn = up.mix(Rgb(10, 12, 22), 0.30);
            if cr > k {
                grid[cr - 1 - k][c] = Some(('█', up));
            }
            if cr + k < h {
                grid[cr + k][c] = Some(('█', dn));
            }
        }
        let frac = total - full as f32;
        if frac > 0.1 {
            if cr > full {
                grid[cr - 1 - full][c] = Some((vblock(frac), heat(base, 1.0)));
            }
            if cr + full < h {
                grid[cr + full][c] = Some(('▔', heat(base, 0.5)));
            }
        }
    }
}

/// Smooth Braille oscilloscope (connected high-res line).
fn fill_wave(grid: &mut [Vec<VCell>], levels: &[f32], bases: &[Rgb], h: usize, n: usize) {
    let mut br = Braille::new(n, h);
    let (dw, dh) = (n * 2, h * 4);
    let center = (dh - 1) as f32 / 2.0;
    let mut prev: Option<usize> = None;
    for x in 0..dw {
        let li = (x * levels.len() / dw).min(levels.len() - 1);
        let v = levels[li].clamp(0.0, 1.0);
        let base = bases[(x / 2).min(n - 1)];
        let c = heat(base, 0.45 + 0.55 * v);
        let y = ((center - (v * 2.0 - 1.0) * center) as usize).min(dh - 1);
        let (lo, hi) = match prev {
            Some(py) if py < y => (py, y),
            Some(py) => (y, py),
            None => (y, y),
        };
        for yy in lo..=hi {
            br.dot(x, yy, c);
        }
        prev = Some(y);
    }
    br.blit(grid);
}

/// Filled gradient area / mountain (Braille top edge, vertical gradient fill).
fn fill_area(grid: &mut [Vec<VCell>], levels: &[f32], bases: &[Rgb], h: usize, n: usize) {
    let mut br = Braille::new(n, h);
    let (dw, dh) = (n * 2, h * 4);
    for x in 0..dw {
        let li = (x * levels.len() / dw).min(levels.len() - 1);
        let v = levels[li].clamp(0.0, 1.0);
        let base = bases[(x / 2).min(n - 1)];
        let top = ((1.0 - v) * (dh - 1) as f32) as usize;
        for y in top..dh {
            let f = 1.0 - (y - top) as f32 / (dh - top).max(1) as f32; // bright at edge
            br.dot(x, y, heat(base, 0.25 + 0.75 * f));
        }
    }
    br.blit(grid);
}

/// Dot-matrix mirrored EQ (Braille, every other dot for the "LED" look).
fn fill_dots(grid: &mut [Vec<VCell>], levels: &[f32], bases: &[Rgb], h: usize, n: usize) {
    let mut br = Braille::new(n, h);
    let (dw, dh) = (n * 2, h * 4);
    let center = (dh / 2) as i32;
    for x in 0..dw {
        let li = (x * levels.len() / dw).min(levels.len() - 1);
        let v = levels[li].clamp(0.0, 1.0);
        let base = bases[(x / 2).min(n - 1)];
        let amp = (v * (dh as f32 / 2.0)) as i32;
        let mut d = 0;
        while d <= amp {
            if d % 2 == 0 {
                let c = heat(base, 1.0 - d as f32 / amp.max(1) as f32);
                if center - d >= 0 {
                    br.dot(x, (center - d) as usize, c);
                }
                if ((center + d) as usize) < dh {
                    br.dot(x, (center + d) as usize, c);
                }
            }
            d += 1;
        }
    }
    br.blit(grid);
}

/// 3D spectrum waterfall: recent frames recede into the distance as a
/// perspective heightmap. The newest frame is the wide, bright ridge at the
/// front; older frames are narrower, higher up, and dimmer — so you watch the
/// music flow away from you. Drawn back-to-front on the Braille canvas.
fn fill_waterfall(
    grid: &mut [Vec<VCell>],
    history: &std::collections::VecDeque<Vec<f32>>,
    th: &Theme,
    h: usize,
    n: usize,
) {
    let depth = history.len();
    if depth == 0 {
        return;
    }
    let mut br = Braille::new(n, h);
    let (dw, dh) = (n * 2, h * 4);
    let last = (depth - 1).max(1) as f32;
    for (i, row) in history.iter().enumerate() {
        if row.is_empty() {
            continue;
        }
        // t: 0 = newest (front, low + wide + bright), 1 = oldest (back, high + narrow + dim)
        let t = (depth - 1 - i) as f32 / last;
        let scale = 1.0 - 0.55 * t;
        let floor = dh as f32 * (0.9 - 0.6 * t);
        let amp = dh as f32 * 0.24 * (1.0 - 0.45 * t);
        let bright = 1.0 - 0.72 * t;
        let width = (dw as f32 * scale) as i32;
        if width < 2 {
            continue;
        }
        let off = (dw as i32 - width) / 2;
        let mut prev: Option<i32> = None;
        for sx in 0..width {
            let x = off + sx;
            if x < 0 || x as usize >= dw {
                continue;
            }
            let frac = sx as f32 / width as f32;
            let v = row[(frac * row.len() as f32) as usize % row.len()].clamp(0.0, 1.0);
            let y = (floor - v * amp) as i32;
            let c = heat(th.accent_at(frac), 0.4 + 0.6 * v).mix(Rgb(7, 9, 17), 1.0 - bright);
            // join to the previous column so each frame draws a continuous ridge
            let (lo, hi) = match prev {
                Some(py) if py < y => (py, y),
                Some(py) => (y, py),
                None => (y, y),
            };
            for yy in lo..=hi {
                if yy >= 0 && (yy as usize) < dh {
                    br.dot(x as usize, yy as usize, c);
                }
            }
            prev = Some(y);
        }
    }
    br.blit(grid);
}

/// Tunnel: bars mirrored out from the center line, bright core fading to edges.
fn fill_tunnel(grid: &mut [Vec<VCell>], levels: &[f32], bases: &[Rgb], h: usize, n: usize) {
    let center = (h - 1) as i32 / 2;
    for c in 0..n {
        let v = levels[c].clamp(0.0, 1.0);
        let base = bases[c];
        let half = (v * center as f32).round() as i32;
        for d in 0..=half {
            let frac = if half > 0 {
                1.0 - d as f32 / half as f32
            } else {
                1.0
            };
            for r in [center - d, center + d] {
                if r >= 0 && (r as usize) < h {
                    grid[r as usize][c] = Some(('█', heat(base, frac)));
                }
            }
        }
    }
}

// ---- help overlay --------------------------------------------------------
