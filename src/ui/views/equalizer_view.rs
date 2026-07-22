//! The Equalizer overlay: a centered, modal graphic-EQ card. Ten vertical band
//! faders plus a preamp fader, a dB scale gutter, per-band value read-outs, and
//! the current preset — all drawn from `config.eq_*` + `app.eq` state. Rendering
//! only; the edits live in `app::equalizer`, the DSP in `audio::eq`.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{AppState, EQ_CONTROLS, EQ_PREAMP};
use crate::audio::eq::{EQ_BANDS, EQ_FREQ_LABELS, EQ_MAX_DB};
use crate::ui::components;
use crate::ui::theme::Theme;

/// Width (cells) of one fader column — sized to hold the widest freq label ("125").
const FIELD: usize = 3;
/// Gap between adjacent band columns.
const GAP: usize = 1;
/// Gap before the preamp column (sets it apart from the 10 bands).
const SEP: usize = 2;
/// Width of the left dB-scale gutter ("+12" / "  0" / "-12").
const GUTTER: usize = 4;
/// Graph (fader) heights per overlay-size step — kept odd so 0 dB sits dead center.
const GRAPH_HEIGHTS: [u16; 4] = [9, 11, 13, 15];

/// Fixed content width of the fader block: gutter + 10 bands + separator + preamp.
fn content_width() -> usize {
    GUTTER + EQ_BANDS * (FIELD + GAP) + SEP + FIELD
}

/// The graph height for the current overlay-size step.
fn graph_height(app: &AppState) -> u16 {
    let step = (app.config.overlay_size as usize).min(GRAPH_HEIGHTS.len() - 1);
    GRAPH_HEIGHTS[step]
}

/// Overlay frame size: content-driven width (the fader block is fixed) and a
/// height that grows with the overlay-size step (taller faders = finer control).
/// Clamped to the screen so it always fits.
pub(crate) fn eq_dims(app: &AppState, area: Rect) -> (u16, u16) {
    let w = (content_width() as u16 + 6).min(area.width); // + side padding + borders
    // header(1) + gap(1) + values(1) + graph + labels(1) + tab/footer chrome(4)
    let h = (graph_height(app) + 8).min(area.height);
    (w, h)
}

/// Draw the Equalizer overlay. No-op when it isn't open.
pub fn equalizer_overlay(f: &mut Frame, area: Rect, app: &AppState) {
    if !app.eq_open() {
        return;
    }
    let th = &app.theme;
    let (w, h) = eq_dims(app, area);
    let preset = if app.config.eq_preset.is_empty() {
        "Custom"
    } else {
        &app.config.eq_preset
    };
    let title = format!("Equalizer · {preset}");
    let inner = components::overlay_frame(
        f,
        area,
        app,
        w,
        h,
        &components::FrameSpec {
            title: &title,
            tabs: &[],
            active_tab: 0,
            footer: &[
                ("←→", "band"),
                ("↑↓", "adjust"),
                ("⏎", "on/off"),
                ("⇥", "preset"),
                ("r", "reset"),
                ("s", "save"),
                ("esc", "close"),
            ],
        },
    );
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let lines = eq_lines(app, th, inner.width as usize, graph_height(app) as usize);
    f.render_widget(Paragraph::new(lines), inner);
}

/// Build every rendered row: the power/preset header, the value read-out row, the
/// fader graph, the frequency labels, and (while saving) the name field.
fn eq_lines(app: &AppState, th: &Theme, iw: usize, gh: usize) -> Vec<Line<'static>> {
    let on = app.config.eq_enabled;
    let sel = app.eq.sel.min(EQ_CONTROLS - 1);
    let mut lines: Vec<Line> = Vec::new();

    lines.push(header_line(app, th, iw));
    lines.push(Line::raw(""));
    lines.push(values_row(app, th, on, sel));
    for r in 0..gh {
        lines.push(graph_row(app, th, on, sel, r, gh));
    }
    lines.push(labels_row(th, on, sel));

    // save-preset name field (inline, replaces nothing — appended under the graph)
    if let Some(buf) = app.eq.naming.as_ref() {
        lines.push(Line::raw(""));
        lines.push(Line::from(vec![
            Span::styled("  Save preset  ", Style::default().fg(th.text_dim.into())),
            Span::styled(
                buf.clone(),
                Style::default()
                    .fg(th.text.into())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("▌", Style::default().fg(th.accent[0].into())),
        ]));
    }
    lines
}

/// The power switch (left) and a bypass/active hint, with the selected-control
/// value echoed on the right for quick reference.
fn header_line(app: &AppState, th: &Theme, iw: usize) -> Line<'static> {
    let on = app.config.eq_enabled;
    let sw = components::toggle_span(th, on);
    let status = if on { "active" } else { "bypassed" };
    let status_fg: Color = if on {
        th.accent[0].into()
    } else {
        th.text_faint.into()
    };
    let sel = app.eq.sel.min(EQ_CONTROLS - 1);
    let sel_txt = if sel == EQ_PREAMP {
        format!("preamp {}", fmt_db(app.config.eq_preamp))
    } else {
        format!(
            "{}Hz {}",
            EQ_FREQ_LABELS[sel],
            fmt_db(app.config.eq_bands[sel])
        )
    };

    let left = vec![
        Span::styled("  EQ  ", Style::default().fg(th.text_dim.into())),
        sw,
        Span::raw("  "),
        Span::styled(status, Style::default().fg(status_fg)),
    ];
    let left_w: usize = 6 + components::TOGGLE_W + 2 + status.chars().count();
    let right_w = sel_txt.chars().count() + 2;
    let fill = iw.saturating_sub(left_w + right_w);
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(fill)));
    spans.push(Span::styled(sel_txt, Style::default().fg(th.text.into())));
    spans.push(Span::raw("  "));
    Line::from(spans)
}

/// The per-band dB values, above the faders (the selected one bright).
fn values_row(app: &AppState, th: &Theme, on: bool, sel: usize) -> Line<'static> {
    let cells: Vec<(String, Style)> = (0..EQ_CONTROLS)
        .map(|i| {
            let v = control_value(app, i);
            let is_sel = i == sel;
            let fg: Color = if is_sel {
                th.text.into()
            } else if on {
                th.text_dim.into()
            } else {
                th.text_faint.into()
            };
            let mut style = Style::default().fg(fg);
            if is_sel {
                style = style.add_modifier(Modifier::BOLD);
            }
            (center(&fmt_db(v), FIELD), style)
        })
        .collect();
    assemble_row(gutter_blank(), &cells)
}

/// One row of the fader graph (`r` from the top). Each column shows a solid bar
/// rising or falling from a faint 0-dB axis to its gain.
fn graph_row(
    app: &AppState,
    th: &Theme,
    on: bool,
    sel: usize,
    r: usize,
    gh: usize,
) -> Line<'static> {
    let mid = gh / 2;
    let gutter = match r {
        0 => (format!("{:>3} ", "+12"), th.text_faint.into()),
        _ if r == mid => (format!("{:>3} ", "0"), th.text_faint.into()),
        _ if r == gh - 1 => (format!("{:>3} ", "-12"), th.text_faint.into()),
        _ => ("    ".to_string(), th.text_faint.into()),
    };
    let gutter = (gutter.0, Style::default().fg(gutter.1));

    let cells: Vec<(String, Style)> = (0..EQ_CONTROLS)
        .map(|i| {
            let v = control_value(app, i);
            let (filled, is_center) = cell_state(v, r, gh);
            let is_sel = i == sel;
            if filled {
                let fg: Color = if !on {
                    th.text_faint.into()
                } else if is_sel {
                    th.accent[0].into()
                } else {
                    th.accent[1].into()
                };
                ("█".repeat(FIELD), Style::default().fg(fg))
            } else if is_center {
                // the faint 0-dB reference axis, drawn where no bar covers it
                ("─".repeat(FIELD), Style::default().fg(th.text_faint.into()))
            } else {
                (" ".repeat(FIELD), Style::default())
            }
        })
        .collect();
    assemble_row(gutter, &cells)
}

/// The frequency labels beneath the faders (`Pre` for the preamp column).
fn labels_row(th: &Theme, on: bool, sel: usize) -> Line<'static> {
    let cells: Vec<(String, Style)> = (0..EQ_CONTROLS)
        .map(|i| {
            let label = if i == EQ_PREAMP {
                "Pre"
            } else {
                EQ_FREQ_LABELS[i]
            };
            let is_sel = i == sel;
            let fg: Color = if is_sel {
                th.accent[0].into()
            } else if on {
                th.text_dim.into()
            } else {
                th.text_faint.into()
            };
            let mut style = Style::default().fg(fg);
            if is_sel {
                style = style.add_modifier(Modifier::BOLD);
            }
            (center(label, FIELD), style)
        })
        .collect();
    assemble_row(gutter_blank(), &cells)
}

/// Lay out a gutter cell + 10 band cells + a separated preamp cell into one line,
/// so the value / graph / label rows all share identical column positions.
fn assemble_row(gutter: (String, Style), cells: &[(String, Style)]) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::with_capacity(EQ_CONTROLS + 3);
    spans.push(Span::styled(gutter.0, gutter.1));
    for cell in cells.iter().take(EQ_BANDS) {
        spans.push(Span::styled(cell.0.clone(), cell.1));
        spans.push(Span::raw(" ".repeat(GAP)));
    }
    spans.push(Span::raw(" ".repeat(SEP)));
    let pre = &cells[EQ_PREAMP];
    spans.push(Span::styled(pre.0.clone(), pre.1));
    Line::from(spans)
}

/// The dB value of control `i` (`EQ_PREAMP` = the preamp, else a band).
fn control_value(app: &AppState, i: usize) -> f32 {
    if i == EQ_PREAMP {
        app.config.eq_preamp
    } else {
        app.config.eq_bands[i.min(EQ_BANDS - 1)]
    }
}

/// Whether row `r` of a `gh`-tall fader is filled for gain `v`, and whether it is
/// the center (0-dB) row. The bar fills from the center to the gain level.
fn cell_state(v: f32, r: usize, gh: usize) -> (bool, bool) {
    let mid = (gh / 2) as i32;
    let span = mid as f32;
    let frac = (v / EQ_MAX_DB).clamp(-1.0, 1.0);
    let knob = (mid as f32 - frac * span).round() as i32;
    let (lo, hi) = if knob <= mid {
        (knob, mid)
    } else {
        (mid, knob)
    };
    let r = r as i32;
    (r >= lo && r <= hi, r == mid)
}

/// A blank gutter cell (for the value + label rows).
fn gutter_blank() -> (String, Style) {
    (" ".repeat(GUTTER), Style::default())
}

/// Format a dB value as a compact signed integer: `+3`, `0`, `-6`.
fn fmt_db(v: f32) -> String {
    let iv = v.round() as i32;
    if iv > 0 {
        format!("+{iv}")
    } else {
        iv.to_string()
    }
}

/// Center `s` within a `w`-wide field (truncating if it's too long).
fn center(s: &str, w: usize) -> String {
    let len = s.chars().count();
    if len >= w {
        return s.chars().take(w).collect();
    }
    let left = (w - len) / 2;
    let right = w - len - left;
    format!("{}{s}{}", " ".repeat(left), " ".repeat(right))
}
