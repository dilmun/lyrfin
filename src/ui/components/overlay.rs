//! Shared overlay *chrome* — one rounded, centred frame with an optional tab bar
//! and footer hint bar. Every overlay routes through this so they look and
//! navigate the same. (Distinct from `overlays.rs`, which holds the per-overlay
//! *bodies*.) This factors out the `Clear` + centred-rect + rounded-block idiom
//! that was duplicated across ~10 overlays.

use super::*;
use crate::app::{AppState, MouseTarget};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

/// The fraction of the screen (each axis) a big overlay spans at each
/// [`AppState::config`]`.overlay_size` step: a compact default up to a semi-full
/// card that still clears every edge (never full-screen). Cycled with `f`. One
/// entry per [`crate::config::OVERLAY_SIZE_LABELS`] step.
pub const OVERLAY_SIZE_FRACS: [u16; crate::config::OVERLAY_SIZE_COUNT as usize] = [34, 46, 60, 76];

/// The size a big content overlay (Settings / Info / Tag editor) opens at — all
/// three share this so they're uniform, sized by `config.overlay_size` (`f`
/// cycles it). A compact centred card at step 0, growing to a "semi-full" card at
/// the top step. Returns `(w, h)` for [`overlay_frame`].
pub fn overlay_dims(app: &AppState, area: Rect) -> (u16, u16) {
    let step = (app.config.overlay_size as usize).min(OVERLAY_SIZE_FRACS.len() - 1);
    let frac = OVERLAY_SIZE_FRACS[step] as u32;
    // floors keep the card usable on tiny terminals; the `.min(area)` clamp keeps
    // it inside the screen.
    let w = ((area.width as u32 * frac / 100).max(44)).min(area.width as u32) as u16;
    let h = ((area.height as u32 * frac / 100).max(12)).min(area.height as u32) as u16;
    (w, h)
}

/// A centred rect of inner size `w`×`h`, clamped so it never exceeds `area`.
pub fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    let x = area.x + (area.width - w) / 2;
    let y = area.y + (area.height - h) / 2;
    Rect::new(x, y, w, h)
}

/// How a framed overlay is chromed. Empty `tabs` → no tab row; empty `footer` →
/// no footer row.
pub struct FrameSpec<'a> {
    pub title: &'a str,
    pub tabs: &'a [&'a str],
    pub active_tab: usize,
    /// `(key, label)` hint pairs for the footer, e.g. `("⏎", "toggle")`.
    pub footer: &'a [(&'a str, &'a str)],
}

impl<'a> FrameSpec<'a> {
    /// A plain framed dialog: title + footer hints, no tabs.
    pub fn dialog(title: &'a str, footer: &'a [(&'a str, &'a str)]) -> Self {
        Self {
            title,
            tabs: &[],
            active_tab: 0,
            footer,
        }
    }
}

/// Draw a centred, rounded overlay frame (`Clear` + block + optional tab bar +
/// optional footer) at `w`×`h` over `area`, and return the inner BODY rect
/// between the tab bar and footer for the caller to fill. Returns a zero-height
/// body when the frame is too short for its chrome. Registers a
/// [`MouseTarget::OverlayTab`] click region per tab.
pub fn overlay_frame(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    w: u16,
    h: u16,
    spec: &FrameSpec,
) -> Rect {
    let rect = centered(area, w, h);
    f.render_widget(Clear, rect);
    let inner = panel(f, rect, app, spec.title, true);
    let tab_h = if spec.tabs.is_empty() { 0 } else { 1 };
    let foot_h = if spec.footer.is_empty() { 0 } else { 1 };
    if inner.height <= tab_h + foot_h {
        return Rect::new(inner.x, inner.y, inner.width, 0);
    }
    let [tabs, body, foot] = Layout::vertical([
        Constraint::Length(tab_h),
        Constraint::Min(1),
        Constraint::Length(foot_h),
    ])
    .areas(inner);
    if tab_h == 1 {
        tab_bar(f, tabs, app, spec.tabs, spec.active_tab);
    }
    if foot_h == 1 {
        footer_bar(f, foot, app, spec.footer);
    }
    body
}

/// One-row segmented tab bar (generalised from the tag-editor bar). The active
/// tab is a pill (bg = `selection`, fg = `accent[0]`, bold); inactive tabs are
/// `text_dim`. When the tabs don't all fit, it shows a window around the active
/// tab with `‹` / `›` overflow markers so the active tab is always visible.
/// Registers a clickable region per visible tab.
pub fn tab_bar(f: &mut Frame, area: Rect, app: &AppState, names: &[&str], active: usize) {
    let th = &app.theme;
    if names.is_empty() || area.width == 0 {
        return;
    }
    let active = active.min(names.len() - 1);
    let tab_w = |i: usize| names[i].chars().count() + 2; // rendered as " name "
    let avail = area.width as usize;
    let full: usize = (0..names.len()).map(tab_w).sum::<usize>() + 1; // +1 leading pad

    // pick the visible window [start, end): all tabs when they fit, else a window
    // grown outward from the active tab (reserving 2 cols for the ‹ › markers).
    let (mut start, mut end) = (0usize, names.len());
    if full > avail {
        let budget = avail.saturating_sub(2);
        start = active;
        end = active + 1;
        let mut used = tab_w(active);
        loop {
            if end < names.len() && used + tab_w(end) <= budget {
                used += tab_w(end);
                end += 1;
            } else if start > 0 && used + tab_w(start - 1) <= budget {
                start -= 1;
                used += tab_w(start);
            } else {
                break;
            }
        }
    }

    let faint = Style::default().fg(col(th.text_faint));
    let mut spans: Vec<Span> = Vec::new();
    let mut x = area.x;
    spans.push(if start > 0 {
        Span::styled("‹", faint)
    } else {
        Span::raw(" ")
    });
    x += 1;
    for (i, name) in names.iter().enumerate().take(end).skip(start) {
        // width-preserving pill: the active tab's two padding spaces become
        // rounded caps, so `" name "` and `⟨name⟩` occupy the same cells and the
        // window/click math below is untouched.
        let len = name.chars().count() as u16 + 2;
        if x < area.x + area.width {
            let cw = len.min(area.x + area.width - x);
            app.register_click(Rect::new(x, area.y, cw, 1), MouseTarget::OverlayTab(i));
        }
        if i == active {
            let (lc, rc) = sel_caps(app);
            let cap = Style::default().fg(col(th.selection)).bg(col(th.panel));
            let body = Style::default()
                .fg(col(th.accent[0]))
                .bg(col(th.selection))
                .add_modifier(Modifier::BOLD);
            spans.push(Span::styled(lc, cap));
            spans.push(Span::styled(name.to_string(), body));
            spans.push(Span::styled(rc, cap));
        } else {
            spans.push(Span::styled(
                format!(" {name} "),
                Style::default().fg(col(th.text_dim)),
            ));
        }
        x = x.saturating_add(len);
    }
    if end < names.len() {
        spans.push(Span::styled("›", faint));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// One-row key-hint footer: " key label · key label …" — keys bold accent, labels
/// faint, separators faint. The single consistent place an overlay shows its keys.
pub fn footer_bar(f: &mut Frame, area: Rect, app: &AppState, hints: &[(&str, &str)]) {
    let th = &app.theme;
    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    for (i, (key, label)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(
                "  ·  ",
                Style::default().fg(col(th.text_faint)),
            ));
        }
        spans.push(Span::styled(
            (*key).to_string(),
            Style::default()
                .fg(col(th.accent[0]))
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            (*label).to_string(),
            Style::default().fg(col(th.text_faint)),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::centered;
    use ratatui::layout::Rect;

    #[test]
    fn centered_clamps_to_area() {
        let r = centered(Rect::new(0, 0, 40, 20), 100, 100);
        assert_eq!((r.x, r.y, r.width, r.height), (0, 0, 40, 20));
    }

    #[test]
    fn centered_centers_within_area() {
        let r = centered(Rect::new(0, 0, 40, 20), 20, 10);
        assert_eq!((r.x, r.y, r.width, r.height), (10, 5, 20, 10));
    }

    #[test]
    fn centered_respects_area_offset() {
        let r = centered(Rect::new(4, 2, 40, 20), 20, 10);
        assert_eq!((r.x, r.y), (14, 7));
    }
}
