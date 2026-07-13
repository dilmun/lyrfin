//! The shared "queue" pane. One renderer (`queue_pane`) draws the played +
//! now-playing + upcoming list for every source view; thin adapters build its
//! rows from the local player queue (`queue`) or the Spotify librespot queue
//! (`spotify_queue`). Standardized chrome — sources differ only in the row data
//! and the click/scroll targets, not in layout.

use super::*;
use crate::app::{AppState, Focus, MouseTarget, Panel, ScrollBox};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::time::Duration;

/// One queue row: its 1-based display number, title, duration (`ZERO` → blank),
/// and the click target for the row.
pub struct QueueRow {
    pub number: usize,
    pub title: String,
    pub duration: Duration,
    pub click: MouseTarget,
}

/// Source-agnostic chrome for the queue pane: panel title + focus, which row is
/// now-playing (`current`) and which the cursor sits on (`selected`, honored only
/// when focused), and the wheel-scroll target over the body. `current`/`selected`
/// index into the `rows` slice.
pub struct QueuePane<'a> {
    pub title: &'a str,
    pub focused: bool,
    pub current: Option<usize>,
    pub selected: usize,
    pub scroll: MouseTarget,
    /// Persisted sticky scroll offset for this source's queue, so clicking a
    /// visible row selects it in place instead of recentring under the cursor.
    pub off: &'a std::cell::Cell<usize>,
}

/// Render a queue pane: a bordered panel whose rows follow the cursor (when
/// focused) or the now-playing row, with ▶ + accent on the now-playing row, dim
/// played rows, and a highlighted cursor row. Shared by every source view.
pub fn queue_pane(f: &mut Frame, area: Rect, app: &AppState, pane: QueuePane, rows: &[QueueRow]) {
    let th = &app.theme;
    let block = rounded(th, pane.title, pane.focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    app.register_click(inner, pane.scroll);

    let n = rows.len();
    if n == 0 {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "queue empty",
                Style::default().fg(col(th.text_faint)),
            )))
            .alignment(Alignment::Center),
            inner,
        );
        return;
    }

    let body = inner.height as usize;
    let pos = pane.current.unwrap_or(0).min(n - 1);
    let sel = pane.selected.min(n - 1);
    // Focused: window follows the cursor with no margin, so clicking a visible
    // row selects it in place instead of jumping (sticky, not recentring).
    // Unfocused: follow the now-playing row but keep a lookahead margin below it
    // so it never hugs the bottom border and upcoming tracks stay visible.
    let off = if pane.focused {
        sticky_off(pane.off, sel, n, body)
    } else {
        sticky_off_margin(pane.off, pos, n, body, body / 3)
    };
    // Column widths in *display columns* (not chars): the number field grows to fit
    // the largest row number (so a 100+ queue's "100" doesn't overflow a 2-wide
    // slot), the duration is a right-aligned fixed field, and the title fills the
    // rest. Widths are display-width–aware so CJK/wide titles can't push the time
    // off the row.
    let num_w = rows
        .iter()
        .map(|r| r.number)
        .max()
        .unwrap_or(1)
        .to_string()
        .len()
        .max(2);
    let mark_w = num_w + 1; // number/▶ + trailing space
    const DUR_W: usize = 6; // right-aligned "MM:SS" + at least one leading space
    // -2 leaves a col on each side for the selected row's rounded end-caps.
    let w = (inner.width as usize).saturating_sub(mark_w + DUR_W + 2);
    for (vis, (i, row)) in rows.iter().enumerate().skip(off).take(body).enumerate() {
        // per-row hit region: click selects, double-click jumps there and plays
        app.register_click(
            Rect::new(inner.x, inner.y + vis as u16, inner.width, 1),
            row.click,
        );
        let current = pane.current == Some(i);
        let played = pane.current.is_some_and(|c| i < c);
        let selected = pane.focused && i == sel;
        let title = fit_cols(&row.title, w);
        // marker: ▶ for the now-playing row, else the 1-based position (right-aligned
        // in the number field so titles line up across single/multi-digit numbers)
        let (marker, marker_col) = if current {
            (format!("{:>num_w$} ", "▶"), th.now_playing_color())
        } else {
            (
                format!("{:>num_w$} ", row.number),
                if selected {
                    th.accent[0]
                } else {
                    th.meta_text()
                },
            )
        };
        // the queue is a list of titles: upcoming rows read at the title tier, the
        // now-playing row takes the accent, and played rows step down to the
        // metadata tier — the same title/metadata split every other view uses.
        let base = if current {
            th.now_playing_color()
        } else if played {
            th.meta_text()
        } else {
            th.title_text()
        };
        let title_style = if current {
            Style::default().fg(col(base)).add_modifier(Modifier::BOLD)
        } else if selected {
            Style::default()
                .fg(col(th.text))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(col(base))
        };
        let dur = if row.duration.is_zero() {
            String::new()
        } else {
            mmss(row.duration)
        };
        let line = Line::from(vec![
            Span::styled(marker, Style::default().fg(col(marker_col))),
            Span::styled(title, title_style), // already fitted to exactly `w` columns
            Span::styled(
                format!("{dur:>DUR_W$}"), // right-aligned so the time column stays flush
                Style::default().fg(col(th.meta_text())),
            ),
        ]);
        let line = pill_line(
            app,
            inner.width as usize,
            line,
            sel_fill(th, selected, pane.focused),
        );
        f.render_widget(
            Paragraph::new(line),
            Rect::new(inner.x, inner.y + vis as u16, inner.width, 1),
        );
    }
}

/// Fit `s` into exactly `w` terminal columns: [`super::clip`] truncates it (in
/// display columns via unicode-width, so a CJK/wide glyph counts as its two
/// columns — otherwise a Korean/Chinese/Japanese title reads as "narrow" by char
/// count and shoves the duration off the row), then we right-pad to `w`. A wide
/// glyph at the truncation boundary can leave clip one column short of `w`, which
/// the padding fills.
fn fit_cols(s: &str, w: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    if w == 0 {
        return String::new();
    }
    let t = super::clip(s, w);
    format!("{t}{}", " ".repeat(w.saturating_sub(t.width())))
}

/// Local "queue" pane: the player queue (played + current + upcoming). Playing a
/// track only moves the position cursor — nothing is removed.
pub fn queue(f: &mut Frame, area: Rect, app: &AppState) {
    let items = &app.player.queue.items;
    let pos = app.player.queue.position;
    let sel = app.queue_sel;
    let mut rows = Vec::with_capacity(items.len());
    let mut current = None;
    let mut selected = 0;
    for (i, &id) in items.iter().enumerate() {
        // a queue id may briefly fail to resolve mid-rescan; skip it but keep the
        // original index for the marker number + click target.
        let Some(tk) = app.library.track(id) else {
            continue;
        };
        let vi = rows.len();
        if i == pos {
            current = Some(vi);
        }
        if i <= sel {
            selected = vi; // nearest resolved row at/below the cursor
        }
        rows.push(QueueRow {
            number: i + 1,
            title: tk.title.clone(),
            duration: tk.duration(),
            click: MouseTarget::QueueRow(i),
        });
    }
    queue_pane(
        f,
        area,
        app,
        QueuePane {
            title: "QUEUE",
            focused: app.focus == Focus::Pane(Panel::Queue),
            current,
            selected,
            scroll: MouseTarget::Scroll(ScrollBox::Queue),
            off: &app.scroll.queue,
        },
        &rows,
    );
}

/// Spotify queue pane: the librespot queue (`spov.sp_queue`) of `api::Item`s.
/// Shares the local queue's "QUEUE" title so both sources read identically.
/// `focused` (Tab) highlights the border and moves `spotify.queue_sel` with j/k —
/// ⏎ or a double-click jumps to (plays) that track.
pub fn spotify_queue(f: &mut Frame, area: Rect, app: &AppState, focused: bool) {
    let items = &app.spov.sp_queue;
    let rows: Vec<QueueRow> = items
        .iter()
        .enumerate()
        .map(|(i, it)| QueueRow {
            number: i + 1,
            title: crate::arabic::shaped(&it.name, app.config.arabic_shaping),
            duration: if it.duration_ms > 0 {
                Duration::from_millis(it.duration_ms as u64)
            } else {
                Duration::ZERO
            },
            click: MouseTarget::SpotifyQueueRow(i),
        })
        .collect();
    queue_pane(
        f,
        area,
        app,
        QueuePane {
            title: "QUEUE",
            focused,
            current: (!items.is_empty()).then(|| app.spov.sp_idx.min(items.len() - 1)),
            selected: app.spotify.queue_sel,
            scroll: MouseTarget::Scroll(ScrollBox::SpotifyQueue),
            off: &app.spotify.queue_off,
        },
        &rows,
    );
}

#[cfg(test)]
mod tests {
    use super::fit_cols;
    use unicode_width::UnicodeWidthStr;

    #[test]
    fn fit_cols_pads_and_truncates_by_display_width() {
        // ASCII shorter than the field → right-padded to exactly the width
        assert_eq!(fit_cols("abc", 6), "abc   ");
        assert_eq!(fit_cols("abc", 6).width(), 6);

        // ASCII longer than the field → truncated with an ellipsis, exact width
        let t = fit_cols("abcdefgh", 5);
        assert_eq!(t, "abcd…");
        assert_eq!(t.width(), 5);

        // A CJK title counted by *columns*, not chars: "좋겠다" is 3 chars but 6
        // columns. In a 12-column field the result must be 12 columns wide (not
        // 12 chars → 15+ columns, which shoved the duration off the row).
        let cjk = fit_cols("좋겠다 - Inst", 12);
        assert_eq!(cjk.width(), 12, "wide glyphs measured as 2 columns each");

        // Truncation of wide glyphs still lands on the exact column budget.
        for w in 3..20 {
            assert_eq!(fit_cols("당신이 잠든 사이에", w).width(), w, "width {w}");
        }
    }
}
