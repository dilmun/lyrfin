//! The unified inline search row shared by every source view (Library, Spotify,
//! Radio). One widget so every search looks and behaves the same: a focus accent
//! bar, a magnifier (a spinner while a request is loading), the live query with a
//! caret, a dim placeholder when empty + unfocused, and a right-aligned scope
//! label + result count. Presentation only — the caller fills [`SearchBar`] from
//! `AppState`.
//!
//! Generalised from the old `views::search_prompt` (which only Radio used); the
//! local + Spotify views previously stuffed the query into the pane *title* via
//! `shell::search_title`. This row is the single search box for all of them.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::col;
use crate::ui::theme::Theme;

/// Spinner frames shown in place of the magnifier while a search is loading
/// (e.g. a Spotify request in flight).
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// View data for one render of the inline search row. All fields are borrowed
/// from state; nothing here mutates or computes domain data.
pub struct SearchBar<'a> {
    /// The live query text.
    pub query: &'a str,
    /// Caret position as a char index into `query` (clamped on render).
    pub caret: usize,
    /// Whether the box has input focus (drives the accent bar + caret).
    pub focused: bool,
    /// A request is in flight — show a spinner instead of the magnifier.
    pub loading: bool,
    /// Animation clock for the spinner frame.
    pub tick: u64,
    /// Shown dim when the query is empty and the box is unfocused.
    pub placeholder: &'a str,
    /// Right-aligned source label (e.g. "Library"); empty = omitted.
    pub scope: &'a str,
    /// Right-aligned secondary note (e.g. "42 results"); empty = omitted.
    pub info: &'a str,
}

/// Draw the search row into the top line of `area`. The left side (focus bar ·
/// glyph · query/placeholder) is rendered first; the right side (scope · info) is
/// measured and anchored flush-right so it never overwrites the query.
pub fn search_bar(f: &mut Frame, area: Rect, th: &Theme, bar: &SearchBar) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let row = Rect::new(area.x, area.y, area.width, 1);

    // --- left: focus bar, glyph, query (caret-aware) or placeholder ---
    let lead = if bar.focused { "▎" } else { " " };
    let glyph = if bar.loading {
        SPINNER[(bar.tick as usize / 2) % SPINNER.len()]
    } else {
        "⌕"
    };
    let glyph_fg = if bar.focused {
        th.accent[0]
    } else {
        th.text_dim
    };
    let mut spans: Vec<Span> = vec![
        Span::styled(lead, Style::default().fg(th.accent[0].into())),
        Span::styled(format!("{glyph} "), Style::default().fg(glyph_fg.into())),
    ];
    if bar.query.is_empty() && !bar.focused {
        spans.push(Span::styled(
            bar.placeholder.to_string(),
            Style::default().fg(th.text_faint.into()),
        ));
    } else {
        let qstyle = Style::default()
            .fg(th.text.into())
            .add_modifier(Modifier::BOLD);
        // split the query at the caret so the cursor sits *in* the text, not only
        // at the end — the same char-indexed model the tag/cover editors use.
        let chars: Vec<char> = bar.query.chars().collect();
        let caret = bar.caret.min(chars.len());
        if bar.focused {
            let before: String = chars[..caret].iter().collect();
            let after: String = chars[caret..].iter().collect();
            spans.push(Span::styled(before, qstyle));
            spans.push(Span::styled("▌", Style::default().fg(th.accent[0].into())));
            spans.push(Span::styled(after, qstyle));
        } else {
            spans.push(Span::styled(bar.query.to_string(), qstyle));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), row);

    // --- right: scope label + info, anchored flush-right over the same row ---
    let mut right: Vec<Span> = Vec::new();
    if !bar.scope.is_empty() {
        right.push(Span::styled(
            bar.scope.to_string(),
            Style::default()
                .fg(th.text_dim.into())
                .add_modifier(Modifier::BOLD),
        ));
    }
    if !bar.info.is_empty() {
        if !right.is_empty() {
            right.push(Span::styled(
                "  ·  ",
                Style::default().fg(th.text_faint.into()),
            ));
        }
        right.push(Span::styled(
            bar.info.to_string(),
            Style::default().fg(th.text_faint.into()),
        ));
    }
    let w: u16 = right.iter().map(|s| s.content.chars().count() as u16).sum();
    // place it only if it fits with a gap after the query, so it can't clobber the
    // left content on a narrow pane.
    if w > 0 && w + 2 < row.width {
        let rx = row.x + row.width - w;
        f.render_widget(
            Paragraph::new(Line::from(right)),
            Rect::new(rx, row.y, w, 1),
        );
    }
}

/// The same search field rendered as a **title for a panel's top border**, so an
/// active search reads as part of the frame rather than as a faint extra row.
///
/// The row form ([`search_bar`]) is nearly invisible when the query is empty:
/// against a full-width blank line, a caret and a magnifier are easy to miss, and
/// the box gives no hint that typing goes anywhere. Sitting in the border the
/// field is bounded on both sides, so it reads as a field even while empty — and
/// it costs no content row.
///
/// Returns `(left, right)` for [`panel_titled`](super::panel_titled): the field
/// itself and the scope/result chip.
pub fn search_title<'a>(
    th: &Theme,
    bar: &SearchBar<'a>,
    context: &str,
    caps: (&str, &str),
) -> (Line<'static>, Option<Line<'static>>) {
    let accent = Style::default().fg(col(th.accent[0]));
    let glyph = if bar.loading {
        SPINNER[(bar.tick as usize / 3) % SPINNER.len()].to_string()
    } else {
        "⌕".to_string()
    };
    // keep naming the pane: a search field alone leaves no clue *what* is being
    // searched, and the border is the only place that context lives
    let mut spans = vec![Span::raw(" ")];
    if !context.is_empty() {
        spans.push(Span::styled(
            format!("{context}  "),
            Style::default()
                .fg(col(th.title_color(true)))
                .add_modifier(Modifier::BOLD),
        ));
    }
    // The field gets a filled pill of its own — the same rounded capsule the row
    // selection uses. Inline text on a border line still reads as a *label*; a
    // filled shape reads as somewhere you type, which matters most right after `/`
    // when nothing has been entered yet.
    let fill = th.selection;
    let cap = Style::default().fg(col(fill));
    spans.push(Span::styled(caps.0.to_string(), cap));
    spans.push(Span::styled(format!("{glyph} "), accent.bg(col(fill))));

    // the query, with the caret parked at its edit position
    let chars: Vec<char> = bar.query.chars().collect();
    let caret = bar.caret.min(chars.len());
    let text = Style::default()
        .fg(col(th.text))
        .bg(col(fill))
        .add_modifier(Modifier::BOLD);
    if chars.is_empty() {
        spans.push(Span::styled(
            bar.placeholder.to_string(),
            Style::default().fg(col(th.text_faint)).bg(col(fill)),
        ));
    } else {
        spans.push(Span::styled(
            chars[..caret].iter().collect::<String>(),
            text,
        ));
    }
    if bar.focused {
        spans.push(Span::styled("▌", accent.bg(col(fill))));
    }
    if !chars.is_empty() && caret < chars.len() {
        spans.push(Span::styled(
            chars[caret..].iter().collect::<String>(),
            text,
        ));
    }
    spans.push(Span::raw(" "));

    // right chip: "12 results · Spotify", omitted when there is nothing to say
    let chip: Vec<&str> = [bar.info, bar.scope]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    let right = (!chip.is_empty()).then(|| {
        Line::from(Span::styled(
            format!(" {} ", chip.join("  ·  ")),
            Style::default().fg(col(th.text_dim)),
        ))
    });
    (Line::from(spans), right)
}
