//! Tag editor + tag/cover search overlay bodies (split from overlays).

use super::*;
use crate::app::AppState;
use crate::ui::theme::Theme;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Render a field value with a block cursor at `caret`, windowed to `valw` cells
/// so the caret stays visible (a leading … marks scrolled-off text).
fn field_cursor_spans(val: &str, caret: usize, valw: usize, th: &Theme) -> Vec<Span<'static>> {
    let chars: Vec<char> = val.chars().collect();
    let len = chars.len();
    let caret = caret.min(len);
    let win = valw.max(2);
    let start = (caret + 1).saturating_sub(win);
    let text = Style::default()
        .fg(col(th.text))
        .add_modifier(Modifier::BOLD);
    let cursor = Style::default()
        .fg(col(th.bg))
        .bg(col(th.accent[1]))
        .add_modifier(Modifier::BOLD);

    let mut spans: Vec<Span<'static>> = Vec::new();
    if start > 0 {
        spans.push(Span::styled("…", Style::default().fg(col(th.text_faint))));
    }
    let end = (start + win).min(len);
    for (idx, ch) in chars.iter().enumerate().take(end).skip(start) {
        let st = if idx == caret { cursor } else { text };
        spans.push(Span::styled(ch.to_string(), st));
    }
    if caret >= len {
        spans.push(Span::styled(" ".to_string(), cursor)); // caret past the last char
    }
    spans
}

/// Docked tag-editor panel (EDIT mode). The focused field is directly editable;
/// the command keys live in the status bar, not here.
/// The tag editor as a centered floating modal (consistent with the cover/tag
/// search popups), instead of the old docked side panel.
/// The Edit tab's body (manual field editor) rendered into `inner` — no frame of
/// its own; the unified modal provides the panel + tab bar.
fn tag_editor_body(f: &mut Frame, inner: Rect, app: &AppState) {
    let Some(te) = &app.tags.edit else { return };
    let th = &app.theme;
    let fields = crate::tags::FIELDS;
    let bulk = te.targets.len() > 1;
    if inner.width == 0 || inner.height < 3 {
        return;
    }

    // shortcuts live in the status bar (no in-modal footer)
    let [head, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);

    // what's being edited
    let subject = if bulk {
        format!("{} tracks", te.targets.len())
    } else {
        te.paths
            .first()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("—")
            .to_string()
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ♪ ", Style::default().fg(col(th.accent[0]))),
            Span::styled(subject, Style::default().fg(col(th.text_dim))),
        ])),
        head,
    );

    // cap (1) + marker (2) + label (13) → value column starts at 16, with the
    // right cap reserving the final col.
    let valw = (body.width as usize).saturating_sub(17).max(6);
    let mut rows: Vec<Line> = Vec::with_capacity(fields.len());
    for (i, label) in fields.iter().enumerate() {
        let focused = i == te.cursor;
        // the focused field is a rounded capsule (its inline edit caret keeps its
        // own background — `pill_line` leaves spans that set a bg untouched).
        let bg = if focused { Some(th.selection) } else { None };
        // a touched field is shown as its (possibly empty) value, not <keep>
        let keep = te.keep[i] && !te.touched[i];
        let modified = te.touched[i];
        let (marker, marker_col) = if focused {
            ("▸", th.accent[1])
        } else if modified {
            ("•", th.accent[2])
        } else {
            (" ", th.text_faint)
        };
        let label_style = Style::default()
            .fg(col(if focused { th.accent[1] } else { th.text_faint }))
            .add_modifier(Modifier::BOLD);
        let val = te.draft.get(i);
        let mut spans = vec![
            Span::styled(format!("{marker} "), Style::default().fg(col(marker_col))),
            Span::styled(format!("{label:<13}"), label_style),
        ];
        if focused && te.editing {
            // a block cursor at the caret, with the value windowed to keep it visible
            spans.extend(field_cursor_spans(val, te.caret, valw, th));
        } else if keep {
            spans.push(Span::styled(
                "<keep>",
                Style::default().fg(col(th.text_faint)),
            ));
        } else {
            let c = if modified { th.warning } else { th.text };
            let st = if focused {
                Style::default().fg(col(c)).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(col(c))
            };
            // collapse newlines to a one-line preview (lyrics), then clip to width
            let disp = if val.contains('\n') {
                trunc(&val.replace('\r', "").replace('\n', " ⏎ "), valw)
            } else {
                val.to_string()
            };
            spans.push(Span::styled(disp, st));
            if focused {
                spans.push(Span::styled("  ↵", Style::default().fg(col(th.accent[1]))));
            }
        }
        rows.push(pill_line(app, body.width as usize, Line::from(spans), bg));
    }
    f.render_widget(Paragraph::new(rows), body);
}

/// The Cover tab's body (album-art search) rendered into `inner`.
fn cover_search_body(f: &mut Frame, inner: Rect, app: &AppState) {
    use crate::app::CoverStatus;
    let Some(cs) = &app.tags.cover else {
        return;
    };
    let th = &app.theme;
    if inner.height < 4 {
        return;
    }
    let faint = Style::default().fg(col(th.text_faint));

    // query line
    let qrow = Rect::new(inner.x, inner.y, inner.width, 1);
    let lab = if cs.editing {
        th.accent[0]
    } else {
        th.text_dim
    };
    let mut qspans = vec![Span::styled(
        " search: ",
        Style::default().fg(col(lab)).add_modifier(Modifier::BOLD),
    )];
    if cs.editing {
        qspans.extend(field_cursor_spans(
            &cs.query,
            cs.qcaret,
            inner.width.saturating_sub(9) as usize,
            th,
        ));
    } else {
        qspans.push(Span::styled(
            cs.query.clone(),
            Style::default().fg(col(th.text)),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(qspans)), qrow);

    let body = Rect::new(
        inner.x,
        inner.y + 2,
        inner.width,
        inner.height.saturating_sub(2),
    );

    let centered = |f: &mut Frame, msg: &str, style: Style| {
        let r = Rect::new(body.x, body.y + body.height / 2, body.width, 1);
        f.render_widget(
            Paragraph::new(Span::styled(msg.to_string(), style)).alignment(Alignment::Center),
            r,
        );
    };
    match &cs.status {
        CoverStatus::Searching => centered(f, "Searching iTunes + Deezer…", faint),
        CoverStatus::Embedding => centered(
            f,
            "Embedding cover…",
            Style::default().fg(col(th.accent[0])),
        ),
        CoverStatus::Empty => centered(f, "No covers found — press / to edit the query", faint),
        CoverStatus::Error(e) => centered(
            f,
            &format!("Error: {e}"),
            Style::default().fg(col(th.warning)),
        ),
        CoverStatus::Results => {
            let [list, _g, prev] = Layout::horizontal([
                Constraint::Length(24),
                Constraint::Length(2),
                Constraint::Min(10),
            ])
            .areas(body);
            let mut lines: Vec<Line> = Vec::new();
            for (i, c) in cs.candidates.iter().enumerate() {
                let sel = i == cs.sel;
                let bg = if sel { Some(th.selection) } else { None };
                let style = if sel {
                    Style::default()
                        .fg(col(th.accent[0]))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(col(th.text_dim))
                };
                let content = Line::from(Span::styled(
                    format!(
                        "{}{}  {}×{}",
                        if sel { "▸ " } else { "  " },
                        c.source,
                        c.width,
                        c.height
                    ),
                    style,
                ));
                lines.push(pill_line(app, list.width as usize, content, bg));
            }
            f.render_widget(Paragraph::new(lines), list);
            // live preview of the highlighted candidate: a centred square that
            // grows with the popup but is capped to the thumbnail's native px so
            // it's never upscaled (kept sharp).
            if let Some(Some(cell)) = cs.previews.get(cs.sel)
                && let Ok(mut proto) = cell.proto.try_borrow_mut()
                && prev.width > 2
                && prev.height > 2
            {
                // size from the cover's own resolution (no upscaling); only tmux
                // imposes an extra ceiling on what it can transmit.
                let native = cs
                    .candidates
                    .get(cs.sel)
                    .map(|c| c.thumb.width().max(c.thumb.height()))
                    .unwrap_or(256);
                let max_px = if app.in_tmux {
                    native.min(TMUX_IMG_PX)
                } else {
                    native
                };
                let prect = square_image_rect(prev, image_font(app), max_px);
                f.render_stateful_widget(
                    ratatui_image::StatefulImage::default(),
                    prect,
                    &mut *proto,
                );
            }
        }
    }
}

/// Field-by-field diff of a fetched candidate against current tags. Changed
/// fields show `current → Fetched` (fetched in accent); unchanged/blank show the
/// kept value dimmed. A blank fetched field never counts as a change.
fn tag_field_diff(
    th: &Theme,
    c: &crate::tagsearch::TagCandidate,
    cur: &crate::tags::EditableTags,
) -> Vec<Line<'static>> {
    let faint = Style::default().fg(col(th.text_faint));
    let accent = Style::default()
        .fg(col(th.accent[0]))
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(col(th.text_dim));
    let row = |label: &str, old: &str, new: String| -> Line<'static> {
        let changed = !new.is_empty() && new != old;
        let mut spans = vec![Span::styled(format!(" {label:<8}"), faint)];
        if changed {
            if !old.is_empty() {
                spans.push(Span::styled(format!("{old} "), faint));
                spans.push(Span::styled("→ ", faint));
            }
            spans.push(Span::styled(new, accent));
        } else {
            let v = if new.is_empty() { old.to_string() } else { new };
            spans.push(Span::styled(if v.is_empty() { "—".into() } else { v }, dim));
        }
        Line::from(spans)
    };
    let aa = if c.album_artist.is_empty() {
        c.artist.clone()
    } else {
        c.album_artist.clone()
    };
    let track = match (c.track_no, c.track_total) {
        (Some(n), Some(t)) => format!("{n}/{t}"),
        (Some(n), None) => n.to_string(),
        _ => String::new(),
    };
    vec![
        row("Title", &cur.title, c.title.clone()),
        row("Artist", &cur.artist, c.artist.clone()),
        row("Album", &cur.album, c.album.clone()),
        row("Alb.Art", &cur.album_artist, aa),
        row(
            "Year",
            &cur.year,
            c.year.map(|y| y.to_string()).unwrap_or_default(),
        ),
        row("Genre", &cur.genre, c.genre.clone().unwrap_or_default()),
        row("Track", &cur.track_no, track),
    ]
}

/// The Auto Tag tab's body (online tag search) rendered into `inner`: editable
/// query, candidate/track list on the left, the chosen field diff on the right.
fn tag_search_body(f: &mut Frame, inner: Rect, app: &AppState) {
    use crate::app::CoverStatus;
    let Some(ts) = &app.tags.search else {
        return;
    };
    let th = &app.theme;
    if inner.height < 4 {
        return;
    }
    let faint = Style::default().fg(col(th.text_faint));

    // query line
    let lab = if ts.editing {
        th.accent[0]
    } else {
        th.text_dim
    };
    let mut qspans = vec![Span::styled(
        " search: ",
        Style::default().fg(col(lab)).add_modifier(Modifier::BOLD),
    )];
    if ts.editing {
        qspans.extend(field_cursor_spans(
            &ts.query,
            ts.qcaret,
            inner.width.saturating_sub(9) as usize,
            th,
        ));
    } else {
        qspans.push(Span::styled(
            ts.query.clone(),
            Style::default().fg(col(th.text)),
        ));
    }
    f.render_widget(
        Paragraph::new(Line::from(qspans)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let body = Rect::new(
        inner.x,
        inner.y + 2,
        inner.width,
        inner.height.saturating_sub(2),
    );
    let centered = |f: &mut Frame, msg: &str, style: Style| {
        f.render_widget(
            Paragraph::new(Span::styled(msg.to_string(), style)).alignment(Alignment::Center),
            Rect::new(body.x, body.y + body.height / 2, body.width, 1),
        );
    };
    match &ts.status {
        CoverStatus::Searching => {
            let what = if ts.album_mode {
                "Searching iTunes + Deezer + MusicBrainz…"
            } else {
                "Searching iTunes + Deezer…"
            };
            centered(f, what, faint)
        }
        CoverStatus::Embedding => {
            centered(f, "Applying tags…", Style::default().fg(col(th.accent[0])))
        }
        CoverStatus::Empty => centered(f, "No matches — press / to edit the query", faint),
        CoverStatus::Error(e) => centered(
            f,
            &format!("Error: {e}"),
            Style::default().fg(col(th.warning)),
        ),
        CoverStatus::Results if ts.album_mode => {
            let [list, _g, detail] = Layout::horizontal([
                Constraint::Length(30),
                Constraint::Length(2),
                Constraint::Min(20),
            ])
            .areas(body);
            // match local tracks to the selected source's tracks
            let src = ts.albums.get(ts.album_sel);
            let local: Vec<(u16, String)> = ts
                .album_tracks
                .iter()
                .map(|(_, t)| (t.track_no.parse::<u16>().unwrap_or(0), t.title.clone()))
                .collect();
            let assign = src
                .map(|s| crate::tagsearch::match_album(&local, &s.tracks))
                .unwrap_or_default();
            // local track list (left)
            let h = list.height as usize;
            let off = ts.track_sel.saturating_sub(h.saturating_sub(1));
            let mut lines: Vec<Line> = Vec::new();
            for (i, (_, t)) in ts.album_tracks.iter().enumerate().skip(off).take(h) {
                let sel = i == ts.track_sel;
                let matched = assign.get(i).copied().flatten().is_some();
                let style = if sel {
                    Style::default()
                        .fg(col(th.accent[0]))
                        .add_modifier(Modifier::BOLD)
                } else if matched {
                    Style::default().fg(col(th.text_dim))
                } else {
                    faint
                };
                let tno = if t.track_no.is_empty() {
                    "  ".to_string()
                } else {
                    format!("{:>2}", t.track_no)
                };
                let mark = if matched { ' ' } else { '·' };
                let label = clip(
                    &format!("{}{tno} {mark} {}", if sel { "▸" } else { " " }, t.title),
                    list.width as usize,
                );
                lines.push(Line::from(Span::styled(label, style)));
            }
            f.render_widget(Paragraph::new(lines), list);
            // diff for the selected track
            let body_lines = match (src, assign.get(ts.track_sel).copied().flatten()) {
                (Some(s), Some(fi)) => {
                    tag_field_diff(th, &s.tracks[fi], &ts.album_tracks[ts.track_sel].1)
                }
                _ => vec![Line::from(Span::styled(
                    " (no match for this track)",
                    faint,
                ))],
            };
            f.render_widget(Paragraph::new(body_lines), detail);
        }
        CoverStatus::Results => {
            let [list, _g, detail] = Layout::horizontal([
                Constraint::Length(30),
                Constraint::Length(2),
                Constraint::Min(20),
            ])
            .areas(body);
            // candidate list
            let mut lines: Vec<Line> = Vec::new();
            for (i, c) in ts.candidates.iter().enumerate() {
                let sel = i == ts.sel;
                let style = if sel {
                    Style::default()
                        .fg(col(th.accent[0]))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(col(th.text_dim))
                };
                let label = clip(&format!("{} · {}", c.source, c.title), list.width as usize);
                lines.push(Line::from(Span::styled(
                    format!("{}{label}", if sel { "▸ " } else { "  " }),
                    style,
                )));
            }
            f.render_widget(Paragraph::new(lines), list);
            if let Some(c) = ts.candidates.get(ts.sel) {
                f.render_widget(Paragraph::new(tag_field_diff(th, c, &ts.current)), detail);
            }
        }
    }
}

/// The unified **Tag Edit** modal: one frame + a tab bar (Edit · Auto Tag ·
/// Cover); the active tab's body is rendered inside. Tabs switch with 1 / 2 / 3
/// (see the keymap).
/// Footer key-hints per tag-editor tab.
fn tag_footer(tab: u8) -> &'static [(&'static str, &'static str)] {
    match tab {
        1 => &[
            ("⇥", "tab"),
            ("↑↓", "pick"),
            ("⏎", "apply"),
            ("esc", "close"),
        ],
        2 => &[
            ("⇥", "tab"),
            ("↑↓", "pick"),
            ("⏎", "embed"),
            ("esc", "close"),
        ],
        _ => &[
            ("⇥", "tab"),
            ("↑↓", "field"),
            ("⏎", "edit"),
            ("s", "save"),
            ("f", "size"),
            ("esc", "close"),
        ],
    }
}

pub fn tags_overlay(f: &mut Frame, area: Rect, app: &AppState) {
    let th = &app.theme;
    // modal title — what's being edited
    let label = app
        .tags
        .edit
        .as_ref()
        .map(|te| {
            if te.targets.len() > 1 {
                format!("{} tracks", te.targets.len())
            } else if !te.draft.title.is_empty() && !te.draft.artist.is_empty() {
                format!("{} · {}", te.draft.title, te.draft.artist)
            } else if !te.draft.title.is_empty() {
                te.draft.title.clone()
            } else {
                te.paths
                    .first()
                    .and_then(|p| p.file_name())
                    .and_then(|s| s.to_str())
                    .unwrap_or("track")
                    .to_string()
            }
        })
        .unwrap_or_else(|| "track".to_string());

    let (w, h) = overlay_dims(app, area);
    let body = overlay_frame(
        f,
        area,
        app,
        w,
        h,
        &FrameSpec {
            title: &format!("Tag Edit — {label}"),
            tabs: &["Edit", "Auto Tag", "Cover"],
            active_tab: app.tags.tab as usize,
            footer: tag_footer(app.tags.tab),
        },
    );
    if body.height < 3 {
        return;
    }

    // right-aligned source/edition indicator on the Auto tab (album mode)
    if app.tags.tab == 1
        && let Some(ts) = &app.tags.search
        && ts.album_mode
        && let Some(s) = ts.albums.get(ts.album_sel)
    {
        let info = format!(
            "{} · {} trk ‹{}/{}› ",
            s.source,
            s.tracks.len(),
            ts.album_sel + 1,
            ts.albums.len()
        );
        let iw = (info.chars().count() as u16).min(body.width);
        let r = Rect::new(body.x + body.width - iw, body.y, iw, 1);
        f.render_widget(
            Paragraph::new(Span::styled(info, Style::default().fg(col(th.text_dim)))),
            r,
        );
    }

    match app.tags.tab {
        1 => tag_search_body(f, body, app),
        2 => cover_search_body(f, body, app),
        _ => tag_editor_body(f, body, app),
    }
}
