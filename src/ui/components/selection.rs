//! Rounded selection highlight — the single source of the app's "capsule" row
//! highlight. Every selected/highlighted list row, picker entry, and settings
//! row is drawn as a rounded pill (an interior fill plus rounded end-caps)
//! rather than a square-cornered bar, so a selection looks identical everywhere.
//!
//! Two render idioms share these helpers:
//!   * line-based rows (a `Paragraph` per row, or a multi-line one) wrap their
//!     content with [`pill_line`];
//!   * `ratatui::Table` rows (the columnar tracklists) fill the row background
//!     through the widget and overlay rounded ends with [`cap_row`] — the table
//!     is drawn 1 col in from each edge so the caps sit in the freed margins.

use super::col;
use crate::app::AppState;
use crate::ui::theme::{Rgb, Theme};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use unicode_width::UnicodeWidthStr;

/// Rounded end-cap glyphs `(left, right)` for a selection pill. Seamless
/// Powerline half-circles with the Nerd Font preset (lyrfin's default icon set),
/// else Unicode half-black-circles so a plain-font terminal still rounds the
/// corners instead of showing a box glyph.
pub(crate) fn sel_caps(app: &AppState) -> (&'static str, &'static str) {
    if app.config.icon_set == "nerd" {
        ("\u{e0b6}", "\u{e0b4}") //
    } else {
        ("\u{25d6}", "\u{25d7}") // ◖ ◗
    }
}

/// The fill colour for a selected row: the bright `selection` when its pane is
/// focused, a dim panel tint when not (so the cursor stays visible after focus
/// moves), `None` when the row isn't selected. The single place row-selection
/// colour is decided.
pub(crate) fn sel_fill(th: &Theme, selected: bool, focused: bool) -> Option<Rgb> {
    if selected && focused {
        Some(th.selection)
    } else if selected {
        Some(th.panel.mix(th.text_faint, 0.25))
    } else {
        None
    }
}

/// Wrap a row's `content` into a rounded capsule line of width `w`. `bg=Some(c)`
/// fills the interior with `c` and rounds both ends with caps in `c` over the
/// panel; `bg=None` indents the content by 1 col so unselected rows stay aligned
/// with capsuled ones (text never shifts as the selection moves). Content is
/// expected to be pre-fitted to `w - 2` columns; a longer line simply loses its
/// trailing cap to truncation.
pub(crate) fn pill_line(
    app: &AppState,
    w: usize,
    content: Line<'static>,
    bg: Option<Rgb>,
) -> Line<'static> {
    let spans = content.spans;
    match bg {
        None => {
            let mut out = Vec::with_capacity(spans.len() + 1);
            out.push(Span::raw(" "));
            out.extend(spans);
            Line::from(out)
        }
        Some(_) if w < 2 => Line::from(spans), // no room for caps
        Some(c) => {
            let th = &app.theme;
            let (l, r) = sel_caps(app);
            let cap = Style::default().fg(col(c)).bg(col(th.panel));
            let fill = Style::default().bg(col(c));
            let body_w = w - 2;
            let used: usize = spans.iter().map(|s| s.content.width()).sum();
            let mut out = Vec::with_capacity(spans.len() + 3);
            out.push(Span::styled(l, cap));
            for mut s in spans {
                // fill the row with the selection colour, but leave spans that set
                // their own background (e.g. a text-edit caret) intact.
                if s.style.bg.is_none() {
                    s.style = s.style.bg(col(c));
                }
                out.push(s);
            }
            if used < body_w {
                out.push(Span::styled(" ".repeat(body_w - used), fill));
            }
            out.push(Span::styled(r, cap));
            Line::from(out)
        }
    }
}

/// Overlay rounded end-caps onto a full-width `ratatui::Table` row highlight.
/// The table is rendered 1 col in from each edge (so the row fill spans the
/// interior); `row` is the *full-width* row rect, and the caps — colour `c`
/// over the panel — round its extreme left/right columns.
pub(crate) fn cap_row(f: &mut Frame, app: &AppState, row: Rect, c: Color) {
    if row.width < 2 || row.height == 0 {
        return;
    }
    let th = &app.theme;
    let (l, r) = sel_caps(app);
    let st = Style::default().fg(c).bg(col(th.panel));
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(l, st))),
        Rect::new(row.x, row.y, 1, 1),
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(r, st))),
        Rect::new(row.x + row.width - 1, row.y, 1, 1),
    );
}
