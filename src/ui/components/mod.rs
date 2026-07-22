//! Reusable widgets + the chrome shared across views. All rendering is pure:
//! read `&AppState`, draw into the `Frame`.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders};

use crate::app::{AppState, Dock, Focus, Panel};

/// The main content keeps at least this slice (cells): bands are clamped so a
/// pane can never push the main below it — a percentage would otherwise blank the
/// main on a small terminal. A collapse floor, not a pane size.
const MIN_MAIN_W: u16 = 30;
const MIN_MAIN_H: u16 = 8;
/// A docked pane clamped thinner than this can't show usefully, so it collapses
/// (least-important first) instead of rendering a sliver.
const MIN_PANE_W: u16 = 14;
const MIN_PANE_H: u16 = 3;

/// Cells a pane spans along its dock axis, from its `pct` of `area` (width for
/// left/right docks, height for top/bottom). The pane model is percentage-only;
/// this is the single place a percentage becomes a cell count.
pub fn pane_span(area: Rect, dock: Dock, pct: u16) -> u16 {
    let dim = if matches!(dock, Dock::Left | Dock::Right) {
        area.width
    } else {
        area.height
    };
    ((dim as u32 * pct as u32) / 100) as u16
}

/// Lay the `keep` panes out around `area` (percentage-derived bands), returning
/// each pane's slot + the leftover main rect. Pure geometry — no rendering and no
/// collapse decision (that belongs to [`dock_panels`]).
fn layout_docks(area: Rect, app: &AppState, keep: &[Panel]) -> (Vec<(Panel, Rect)>, Rect) {
    let mut main = area;
    let mut slots = Vec::new();
    for edge in [Dock::Left, Dock::Right, Dock::Top, Dock::Bottom] {
        let on_edge: Vec<Panel> = keep
            .iter()
            .copied()
            .filter(|&p| app.panel(p).dock == edge)
            .collect();
        if on_edge.is_empty() {
            continue;
        }
        // A Left/Right cluster of 2+ panes can sit side-by-side instead of stacked
        // (`panes_horizontal`): side-by-side sums the percentages, stacked takes the
        // largest. The band is a percentage of the *original* window dimension.
        let lr = matches!(edge, Dock::Left | Dock::Right);
        let side_by_side = lr && app.config.panes_horizontal && on_edge.len() > 1;
        let pcts = || on_edge.iter().map(|&p| app.panel(p).size);
        let band_pct = if side_by_side {
            pcts().sum::<u16>()
        } else {
            pcts().max().unwrap_or(25)
        };
        // clamp the band so the main never drops below its floor; a pane that ends
        // up too thin is dropped by `dock_panels`, not shrunk to a sliver here.
        let span = pane_span(area, edge, band_pct);
        let (band, rest) = if lr {
            dock_split(
                main,
                edge,
                span.min(main.width.saturating_sub(MIN_MAIN_W)),
                0,
            )
        } else {
            dock_split(
                main,
                edge,
                0,
                span.min(main.height.saturating_sub(MIN_MAIN_H)),
            )
        };
        // Share the band between its panels by their cross-axis `len` weights
        // (equal by default): stacked along a Left/Right edge (vertical slots), or
        // side-by-side / along a Top/Bottom edge (horizontal).
        let total: u32 = on_edge
            .iter()
            .map(|&p| app.panel(p).len.max(1) as u32)
            .sum();
        let cons: Vec<Constraint> = on_edge
            .iter()
            .map(|&p| Constraint::Ratio(app.panel(p).len.max(1) as u32, total))
            .collect();
        let pieces = if lr && !side_by_side {
            Layout::vertical(cons).split(band)
        } else {
            Layout::horizontal(cons).split(band)
        };
        for (slot, &panel) in pieces.iter().zip(on_edge.iter()) {
            slots.push((panel, *slot));
        }
        main = rest;
    }
    (slots, main)
}

/// Dock every shown pane around `area` and return the leftover main rect. Pane
/// sizes are percentages of the window, so the layout scales with the terminal.
/// Bands are clamped so the main always keeps a usable floor (a pane never blanks
/// it); a pane the clamp squeezes below a usable minimum is dropped instead,
/// least-important first — a smooth, importance-ordered collapse as the terminal
/// shrinks (Lyrics → Visualizer → Artist → Queue → Sidebar; the main content and
/// the navigation survive longest).
pub fn dock_panels<F>(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    panels: &[Panel],
    render: F,
) -> Rect
where
    F: Fn(&mut Frame, Rect, &AppState, Panel),
{
    let mut keep: Vec<Panel> = panels
        .iter()
        .copied()
        .filter(|&p| app.panel(p).shown)
        .collect();
    let (slots, main) = loop {
        let (slots, main) = layout_docks(area, app, &keep);
        // If the clamp squeezed any pane below a usable minimum, they can't all fit
        // — drop the *globally* least-important pane (not just the thin one, so a
        // narrow-but-important pane like the sidebar isn't sacrificed before a wide
        // but trivial one) and re-lay-out. Repeats into a smooth, importance-ordered
        // collapse as the terminal shrinks.
        let unfit = slots.iter().any(|(p, r)| {
            if matches!(app.panel(*p).dock, Dock::Left | Dock::Right) {
                r.width < MIN_PANE_W
            } else {
                r.height < MIN_PANE_H
            }
        });
        if !unfit {
            break (slots, main);
        }
        let Some(victim) = keep.iter().min_by_key(|p| p.collapse_rank()).copied() else {
            break (slots, main);
        };
        keep.retain(|&p| p != victim);
    };
    for &(panel, slot) in &slots {
        render(f, slot, app, panel);
        // drag-to-resize handle on the pane's edge facing `main` (changes its size)
        app.register_pane_edge(area, app.panel(panel).dock, slot, panel);
        // where this pane sits, for directional focus movement (ctrl+h/j/k/l)
        app.register_focus(slot, focus_of(panel));
    }
    // dividers between panes sharing an edge (drag shifts their stacked split)
    app.register_pane_dividers(area, &slots);
    main
}

/// The [`Focus`] a docked [`Panel`] carries. The sidebar is docked like any other
/// pane but focuses as `Focus::Sidebar`, not `Focus::Pane(Sidebar)` — it predates
/// the movable-pane system and every view's focus ring still names it directly.
fn focus_of(panel: Panel) -> Focus {
    match panel {
        Panel::Sidebar => Focus::Sidebar,
        p => Focus::Pane(p),
    }
}

/// Split `area` into a docked panel rect and the remaining main rect. `w` is the
/// panel width for Left/Right docks; `h` is the panel height for Top/Bottom.
pub fn dock_split(area: Rect, dock: Dock, w: u16, h: u16) -> (Rect, Rect) {
    use ratatui::layout::{Constraint, Layout};
    match dock {
        Dock::Left => {
            let [p, m] =
                Layout::horizontal([Constraint::Length(w), Constraint::Min(0)]).areas(area);
            (p, m)
        }
        Dock::Right => {
            let [m, p] =
                Layout::horizontal([Constraint::Min(0), Constraint::Length(w)]).areas(area);
            (p, m)
        }
        Dock::Top => {
            let [p, m] = Layout::vertical([Constraint::Length(h), Constraint::Min(0)]).areas(area);
            (p, m)
        }
        Dock::Bottom => {
            let [m, p] = Layout::vertical([Constraint::Min(0), Constraint::Length(h)]).areas(area);
            (p, m)
        }
    }
}
use crate::ui::theme::{Rgb, Theme};

pub mod cover;
pub use cover::*;

pub mod shell;
pub use shell::*;
pub mod search_bar;
pub use search_bar::*;
pub mod sidebar;
pub use sidebar::*;
pub mod table;
pub use table::*;
pub mod selection;
pub(crate) use selection::*;
pub mod artist;
pub use artist::*;
pub mod queue;
pub use queue::*;
pub mod spotify_tracklist;
pub(crate) use spotify_tracklist::*;
pub mod tracklist;
pub use tracklist::*;
pub mod grid;
pub use grid::*;

pub mod onboarding;
pub use onboarding::*;
pub mod now_bar;
pub use now_bar::*;
pub mod status_bar;
pub use status_bar::*;

pub mod info;
pub use info::*;
pub mod overlay;
pub use overlay::*;
pub mod overlays;
pub use overlays::*;
pub mod tag_ui;
pub use tag_ui::*;
pub mod toggle;
pub use toggle::*;

pub mod spectrum;
pub use spectrum::*;

// ---- small helpers -------------------------------------------------------
fn col(c: Rgb) -> Color {
    c.into()
}
pub(crate) fn mmss(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}
/// Sticky scroll offset: keep the previous offset, only scrolling when `sel`
/// would fall outside the `h`-row viewport. Unlike a recentring window, this
/// holds position when the list grows/shrinks (e.g. expanding a tree node).
pub(crate) fn sticky_off(prev: &std::cell::Cell<usize>, sel: usize, len: usize, h: usize) -> usize {
    sticky_off_margin(prev, sel, len, h, 0)
}

/// Sticky scroll offset that keeps `margin` rows of context between `sel` and
/// the top/bottom edges. Like [`sticky_off`] (which is the `margin == 0` case),
/// but scrolls one step *early* so a followed row — e.g. the now-playing queue
/// entry — never hugs the border and upcoming rows stay visible. `margin` is
/// capped to what the viewport can hold; near the list ends the clamp still lets
/// the true first/last row reach the edge.
pub(crate) fn sticky_off_margin(
    prev: &std::cell::Cell<usize>,
    sel: usize,
    len: usize,
    h: usize,
    margin: usize,
) -> usize {
    if h == 0 || len <= h {
        prev.set(0);
        return 0;
    }
    let max = len - h;
    // leave room for the anchor between the two margins.
    let margin = margin.min(h.saturating_sub(1) / 2);
    let mut off = prev.get().min(max);
    if sel < off + margin {
        off = sel.saturating_sub(margin);
    } else if sel + margin >= off + h {
        off = (sel + margin + 1).saturating_sub(h);
    }
    off = off.min(max);
    prev.set(off);
    off
}

pub fn stars(n: u8) -> String {
    let n = n.min(5) as usize;
    let mut s = String::new();
    s.extend(std::iter::repeat_n('★', n));
    s.extend(std::iter::repeat_n('☆', 5 - n));
    s
}

/// Compose a Spotify item's secondary "row metadata" from its structured fields,
/// per kind — track: artist · album · year; album: artist · year; artist: just
/// the name (followers are optional, off by default); playlist: owner · N tracks.
/// (Per-field show/hide toggles will gate these; for now it's the defaults.)
pub(crate) fn item_meta(it: &crate::spotify::api::Item) -> String {
    use crate::spotify::api::Kind;
    let mut parts: Vec<String> = Vec::new();
    if !it.subtitle.is_empty() {
        parts.push(it.subtitle.clone()); // artist / owner
    }
    match it.kind {
        Kind::Track => {
            if !it.album.is_empty() {
                parts.push(it.album.clone());
            }
            if let Some(y) = it.year {
                parts.push(y.to_string());
            }
        }
        Kind::Album => {
            if let Some(y) = it.year {
                parts.push(y.to_string());
            }
        }
        Kind::Artist => {}   // name only by default; followers are opt-in
        Kind::Show => {}     // publisher (the subtitle) only
        Kind::Category => {} // a browse tile — name only
        Kind::Playlist => {
            if let Some(n) = it.count {
                parts.push(format!("{n} tracks"));
            }
        }
    }
    parts.join("  ·  ")
}
/// Draw a rounded panel and return its inner content area.
pub fn panel(f: &mut Frame, area: Rect, app: &AppState, title: &str, focused: bool) -> Rect {
    panel_titled(f, area, app, title, None, focused)
}

/// Like [`panel`], but with an optional secondary label floated to the RIGHT end of
/// the title border (e.g. the connected Spotify account). The chip is dropped when
/// the pane is too narrow to fit it beside the main title, so the border never shows
/// a collided/overlapping title.
/// Like [`panel_titled`] but the left title is a **styled line** rather than a
/// plain string — for a title that is itself interactive, i.e. the search field
/// embedded in the border (see [`search_title`]). Kept separate because the
/// plain-string form is what almost every pane wants and it stays the simple path.
pub fn panel_titled_line(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    title: Line<'static>,
    title_right: Option<Line<'static>>,
    focused: bool,
) -> Rect {
    let th = &app.theme;
    let border = if focused { th.border_focus } else { th.border };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(col(border)))
        .style(Style::default().bg(col(th.panel)))
        .title(title.clone());
    if let Some(right) =
        title_right.filter(|r| title_right_fits(area.width, title.width(), r.width()))
    {
        block = block.title(right.right_aligned());
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

pub fn panel_titled(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    title: &str,
    title_right: Option<Line<'_>>,
    focused: bool,
) -> Rect {
    let mut block = rounded(&app.theme, title, focused);
    // float the chip to the right end of the border, but only when it won't collide
    // with the left title on a narrow pane (else drop it — the border stays clean).
    if let Some(right) = title_right.filter(|r| {
        use unicode_width::UnicodeWidthStr;
        title_right_fits(area.width, UnicodeWidthStr::width(title), r.width())
    }) {
        block = block.title(right.right_aligned());
    }
    let inner = block.inner(area);
    f.render_widget(block, area);
    inner
}

/// Whether a right-aligned title `right_w` cells wide fits on a pane border beside a
/// left title `left_w` cells wide. Budget: the two rounded corners, the left title's
/// two padding spaces, and a 2-cell gap between the two titles. Pure → unit-tested.
fn title_right_fits(pane_w: u16, left_w: usize, right_w: usize) -> bool {
    left_w + right_w + 6 <= pane_w as usize
}

fn rounded<'a>(th: &Theme, title: &'a str, focused: bool) -> Block<'a> {
    let border = if focused { th.border_focus } else { th.border };
    let mut b = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(col(border)))
        .style(Style::default().bg(col(th.panel)));
    if !title.is_empty() {
        b = b.title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(col(th.title_color(focused)))
                .add_modifier(Modifier::BOLD),
        ));
    }
    b
}

/// Linearly resample a level array to `n` points (smooth interpolation).
fn resample(src: &[f32], n: usize) -> Vec<f32> {
    if src.is_empty() || n == 0 {
        return vec![0.0; n];
    }
    if src.len() == 1 {
        return vec![src[0]; n];
    }
    (0..n)
        .map(|i| {
            let p = i as f32 * (src.len() - 1) as f32 / (n - 1).max(1) as f32;
            let i0 = p.floor() as usize;
            let i1 = (i0 + 1).min(src.len() - 1);
            let f = p - i0 as f32;
            src[i0] + (src[i1] - src[i0]) * f
        })
        .collect()
}

/// Smoothed visualizer band levels interpolated to width `n`.
fn level_bars(app: &AppState, n: usize) -> Vec<f32> {
    resample(&app.viz.levels, n)
}

/// Vertical "heat" colour for a bar cell: dim+saturated at the base, bright and
/// white-hot toward the peak. `base` is the column's gradient hue, `f` is the
/// height fraction within the bar (0 = base, 1 = top).
fn heat(base: Rgb, f: f32) -> Rgb {
    let lo = base.mix(Rgb(10, 12, 22), 0.42); // deep, keeps hue
    let hi = base.mix(Rgb(255, 255, 255), 0.55); // white-hot tip
    lo.mix(hi, f.clamp(0.0, 1.0))
}

/// Spans of a gradient progress bar: a filled heavy line up to the playhead, a
/// white ● knob, and a faint track for the remainder.
pub(crate) fn progress_spans(th: &Theme, frac: f32, width: usize) -> Vec<Span<'static>> {
    let width = width.max(2);
    // the knob sits on the last filled cell; clamp it to the final column so a
    // fraction that rounds up to `width` still draws the dot (instead of placing it
    // one past the end, where it vanishes just before the total)
    let filled = ((frac.clamp(0.0, 1.0) * width as f32).round() as usize).min(width - 1);
    let mut spans = Vec::with_capacity(width);
    for i in 0..width {
        if i < filled {
            let g = th.accent_at(i as f32 / (width - 1) as f32);
            spans.push(Span::styled("━", Style::default().fg(col(g))));
        } else if i == filled {
            // the play-head knob rides the accent gradient (so it belongs to the
            // bar's aesthetic) but is pulled toward `text` for guaranteed contrast
            // on any background — a fixed white dot vanished on light themes.
            let knob = th.accent_at(frac.clamp(0.0, 1.0)).mix(th.text, 0.3);
            spans.push(Span::styled("●", Style::default().fg(col(knob))));
        } else {
            spans.push(Span::styled("─", Style::default().fg(col(th.text_faint))));
        }
    }
    spans
}

/// Karaoke-wipe spans for an active lyric line: the first `cut` characters are
/// "sung" (gradient or `solid`, bold), the rest stay in the base text colour.
fn karaoke_spans(
    th: &Theme,
    shaped: &str,
    pad: usize,
    cut: usize,
    gradient: bool,
    solid: Rgb,
) -> Vec<Span<'static>> {
    let cnt = shaped.chars().count().max(2);
    let mut spans: Vec<Span> = Vec::new();
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(pad)));
    }
    for (j, ch) in shaped.chars().enumerate() {
        let style = if j < cut {
            let fg = if gradient {
                th.accent_at(j as f32 / cnt as f32)
            } else {
                solid
            };
            Style::default().fg(col(fg)).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(col(th.text))
        };
        spans.push(Span::styled(ch.to_string(), style));
    }
    spans
}

/// A block of gradient "album art".
fn art_lines(th: &Theme, w: usize, h: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(h);
    for y in 0..h {
        let mut spans = Vec::with_capacity(w);
        for x in 0..w {
            let t = ((x as f32 / (w.max(2) - 1) as f32) + (y as f32 / (h.max(2) - 1) as f32)) / 2.0;
            spans.push(Span::styled("█", Style::default().fg(col(th.accent_at(t)))));
        }
        lines.push(Line::from(spans));
    }
    lines
}

// ---- top bar -------------------------------------------------------------

/// Truncate a string to `max` display *columns*, appending an ellipsis. Measured
/// in display width (unicode-width), so a CJK/wide glyph counts as the two columns
/// it actually occupies — otherwise a Chinese/Japanese/Korean title reads as
/// "narrow" by char count, overruns its slot, and shoves the adjacent column (e.g.
/// a track's duration) off the row. ASCII/Arabic are width-1, so unaffected.
fn clip(s: &str, max: usize) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
    if s.width() <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    // Keep whole chars up to `max - 1` columns, leaving one for the ellipsis.
    let mut t = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw > max - 1 {
            break;
        }
        t.push(c);
        used += cw;
    }
    t.push('…');
    t
}

/// Cap a string to `max` chars, appending an ellipsis (for bios/descriptions).
fn trunc(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

pub fn vblock(level: f32) -> char {
    const B: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    B[(level.clamp(0.0, 1.0) * 8.0).round() as usize]
}

#[cfg(test)]
mod tests {
    use super::{progress_spans, sticky_off, sticky_off_margin, title_right_fits};

    #[test]
    fn title_right_chip_only_fits_when_the_pane_is_wide_enough() {
        // "SPOTIFY · MUSIC · 100" (21) + "◉ guest" (7) + 6 budget = 34 cells needed
        assert!(title_right_fits(40, 21, 7), "wide pane fits both titles");
        assert!(
            title_right_fits(34, 21, 7),
            "exactly at the budget still fits"
        );
        assert!(
            !title_right_fits(33, 21, 7),
            "one cell short → drop the chip"
        );
        assert!(!title_right_fits(20, 21, 7), "narrow pane → drop the chip");
    }

    #[test]
    fn progress_marker_moves_right() {
        let th = crate::ui::theme::Theme::aurora();
        let dot = |frac| {
            progress_spans(&th, frac, 20)
                .iter()
                .position(|s| s.content == "●")
        };
        // the ● playhead advances with the song
        assert!(dot(0.1) < dot(0.9));
        // …and never falls off the end: even a fraction that rounds up to `width`
        // keeps the knob on the last column (regression: it vanished near the total)
        for &(frac, width) in &[(1.0_f32, 20usize), (0.999, 155), (0.9999, 40)] {
            let pos = progress_spans(&th, frac, width)
                .iter()
                .position(|s| s.content == "●");
            assert_eq!(
                pos,
                Some(width - 1),
                "knob at the last column for {frac}/{width}"
            );
        }
    }
    use std::cell::Cell;

    #[test]
    fn sticky_off_holds_position_when_list_grows() {
        let off = Cell::new(5);
        // sel visible in [5,15) → offset stays put
        assert_eq!(sticky_off(&off, 12, 30, 10), 5);
        // list grows (artist expanded) but sel still visible → no recentre
        assert_eq!(sticky_off(&off, 12, 33, 10), 5);
        // sel scrolls past the bottom edge → scroll just enough to show it
        assert_eq!(sticky_off(&off, 16, 33, 10), 7);
        // sel above the top edge → scroll up to it
        assert_eq!(sticky_off(&off, 3, 33, 10), 3);
        // list shorter than viewport → no offset
        assert_eq!(sticky_off(&off, 0, 4, 10), 0);
    }

    #[test]
    fn sticky_off_margin_keeps_lookahead_below_the_anchor() {
        // Regression: the now-playing queue row used to hug the bottom border
        // (margin 0 scrolls just enough to land it on the last visible row),
        // hiding upcoming tracks. A margin scrolls one step early so the anchor
        // never reaches the bottom `margin` rows.
        let off = Cell::new(0);
        // anchor at 12, viewport 10, margin 3: bottom edge would be row 21;
        // keep 3 rows of context below → anchor sits at off+h-1-margin = 6.
        let o = sticky_off_margin(&off, 12, 30, 10, 3);
        assert_eq!(o, 6);
        assert!(12 - o < 10 - 3, "anchor kept out of the bottom margin");
        assert!(
            o + 10 > 12 + 3,
            "upcoming rows are visible below the anchor"
        );

        // Near the list end the clamp still lets the true last row reach the edge
        // (a short tail can't show phantom rows below it).
        let end = Cell::new(0);
        assert_eq!(sticky_off_margin(&end, 29, 30, 10, 3), 20);

        // margin 0 is exactly sticky_off (bottom row lands on the last line).
        let z = Cell::new(0);
        assert_eq!(sticky_off_margin(&z, 12, 30, 10, 0), 3);

        // an oversized margin is capped to (h-1)/2 = 4 so the anchor still fits.
        let big = Cell::new(0);
        assert_eq!(sticky_off_margin(&big, 12, 30, 10, 99), 7);
    }
}
