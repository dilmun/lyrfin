//! Sidebar: the flat drill-in [`crate::app::LocalSection`] list (extracted from
//! ui/components). Shared section-list renderer used by the local + Spotify views.

use super::*;
use crate::app::{AppState, Focus, MouseTarget, ScrollBox};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

// ---- sidebar (flat drill-in sections) ------------------------------------
/// The local library sidebar title (the shared shell draws the border + title).
pub fn sidebar_title(_app: &AppState) -> &'static str {
    "LIBRARY"
}

/// The local library sidebar: a flat list of drill-in [`LocalSection`]s (the same
/// shape as the Spotify sidebar). The shared shell draws the border + title.
pub fn sidebar_body(f: &mut Frame, inner: Rect, app: &AppState) {
    if inner.height == 0 {
        return;
    }
    app.register_click(inner, MouseTarget::Scroll(ScrollBox::Tree));
    let rows: Vec<(&str, &str)> = crate::app::LocalSection::ALL
        .iter()
        .map(|s| (s.icon(), s.label()))
        .collect();
    let selected = crate::app::LocalSection::ALL
        .iter()
        .position(|&s| s == app.local.section)
        .unwrap_or(0);
    // per-section click targets (click selects + loads that section)
    for i in 0..rows.len().min(inner.height as usize) {
        app.register_click(
            Rect::new(inner.x, inner.y + i as u16, inner.width, 1),
            MouseTarget::Tree(i),
        );
    }
    section_list(f, inner, app, &rows, selected, app.focus == Focus::Sidebar);
}

/// Render a flat sidebar section list — `(icon, label)` rows with the selected row
/// highlighted (bright when `focused`). Shared by the local library and Spotify
/// sidebars so both look identical.
pub fn section_list(
    f: &mut Frame,
    inner: Rect,
    app: &AppState,
    rows: &[(&str, &str)],
    selected: usize,
    focused: bool,
) {
    let th = &app.theme;
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .map(|(i, (icon, label))| {
            let on = i == selected;
            // the focused-selected row is a rounded capsule; a selected-but-
            // unfocused row keeps its accent+bold text without a bar (so the two
            // read differently), and unselected rows are dim.
            let bg = if on && focused {
                Some(th.selection)
            } else {
                None
            };
            let style = if on {
                Style::default()
                    .fg(col(th.accent[0]))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(col(th.text_dim))
            };
            let content = Line::from(Span::styled(format!("{icon} {label}"), style));
            pill_line(app, inner.width as usize, content, bg)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}
