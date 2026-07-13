//! Dialog overlay bodies: add-to-playlist, name entry, confirm delete, and the
//! command palette. (The read-only Keys/Stats/Health/Track overlays live in
//! `info.rs`; the tag editor in `tag_ui.rs`.)

use super::*;
use crate::app::AppState;
use crate::app::MouseTarget;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

// ---- "add to playlist" picker overlay ------------------------------------
pub fn add_playlist_overlay(f: &mut Frame, area: Rect, app: &AppState) {
    let th = &app.theme;
    let pls = app.library.playlists_sorted();
    let total = pls.len() + 1; // + "New playlist"
    let sel = app.input.add_sel.min(total - 1);

    let naming = matches!(app.input.naming, Some(crate::app::NameTarget::New));
    let footer: &[(&str, &str)] = if naming {
        &[("⏎", "create"), ("esc", "cancel")]
    } else {
        &[("⏎", "add"), ("＋", "new"), ("esc", "close")]
    };
    let w = 46u16.min(area.width.saturating_sub(4));
    // subject + blank + body (naming chrome | list rows), plus a footer + borders
    let content = if naming { 4 } else { total as u16 + 2 };
    let h = (content + 3).clamp(7, area.height.saturating_sub(2));
    let inner = overlay_frame(
        f,
        area,
        app,
        w,
        h,
        &FrameSpec::dialog("ADD TO PLAYLIST", footer),
    );
    if inner.height == 0 {
        return;
    }
    let iw = inner.width as usize;

    // subject: a single track's title, or "N tracks" for a bulk add
    let subject = if app.input.add_targets.len() == 1 {
        app.input
            .add_targets
            .first()
            .and_then(|id| app.library.track(*id))
            .map(|t| t.title.clone())
            .unwrap_or_default()
    } else {
        format!("{} tracks", app.input.add_targets.len())
    };
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!(" ♪ {}", clip(&subject, iw.saturating_sub(4))),
            Style::default().fg(col(th.text_dim)),
        )),
        Line::raw(""),
    ];

    // naming a new playlist: show the text input instead of the list
    if naming {
        lines.push(Line::from(Span::styled(
            " New playlist name:",
            Style::default()
                .fg(col(th.text_faint))
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                app.input.buffer.clone(),
                Style::default()
                    .fg(col(th.text))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("▌", Style::default().fg(col(th.accent[0]))),
        ]));
        f.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let body = inner.height.saturating_sub(2) as usize;
    let off = if total > body && body > 0 {
        sel.saturating_sub(body / 2).min(total - body)
    } else {
        0
    };
    for i in off..(off + body).min(total) {
        let on = i == sel;
        let bg = if on { Some(th.selection) } else { None };
        let marker = if on { "▸ " } else { "  " };
        let content = if i < pls.len() {
            Line::from(vec![
                Span::styled(
                    format!("{marker}♫ "),
                    Style::default().fg(col(th.accent[2])),
                ),
                Span::styled(
                    clip(&pls[i].name, iw.saturating_sub(6)),
                    Style::default().fg(col(th.text)).add_modifier(if on {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
                ),
            ])
        } else {
            Line::from(Span::styled(
                format!("{marker}＋ New playlist…"),
                Style::default()
                    .fg(col(th.accent[0]))
                    .add_modifier(Modifier::BOLD),
            ))
        };
        lines.push(pill_line(app, iw, content, bg));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

// ---- Spotify "add to / create / rename playlist" picker ------------------

/// The Spotify analogue of [`add_playlist_overlay`]: pick one of the account's
/// playlists to add the track to, create a new one, or (in rename mode) type a new
/// name. Rows come from the async `MyPlaylists` fetch, so it shows a loading /
/// error note until they land. Keyboard-driven, matching the local picker.
pub fn spotify_playlist_overlay(f: &mut Frame, area: Rect, app: &AppState) {
    use crate::app::spotify::playlist::SpNaming;
    let Some(m) = app.spotify.pl_modal.as_ref() else {
        return;
    };
    let th = &app.theme;

    // name-entry sub-mode (create / rename): a single text field, its own titles
    if let Some(naming) = m.naming.as_ref() {
        let (title, label, verb) = match naming {
            SpNaming::New => ("NEW SPOTIFY PLAYLIST", "Name", "create"),
            SpNaming::Rename { .. } => ("RENAME SPOTIFY PLAYLIST", "New name", "rename"),
        };
        let w = 52u16.min(area.width.saturating_sub(4));
        let h = 7u16.min(area.height.saturating_sub(2));
        let inner = overlay_frame(
            f,
            area,
            app,
            w,
            h,
            &FrameSpec::dialog(title, &[("⏎", verb), ("esc", "cancel")]),
        );
        if inner.height == 0 {
            return;
        }
        let iw = inner.width as usize;
        let lines = vec![
            Line::raw(""),
            Line::from(Span::styled(
                format!("  {label}:"),
                Style::default()
                    .fg(col(th.text_faint))
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    clip(&m.buffer, iw.saturating_sub(4)),
                    Style::default()
                        .fg(col(th.text))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("▌", Style::default().fg(col(th.accent[0]))),
            ]),
        ];
        f.render_widget(Paragraph::new(lines), inner);
        return;
    }

    // picker: the account's playlists + a trailing "New playlist" row
    let total = m.playlists.len() + 1;
    let sel = m.sel.min(total - 1);
    let w = 48u16.min(area.width.saturating_sub(4));
    let content = total as u16 + 3; // subject + blank + rows + a status line
    let h = (content + 3).clamp(8, area.height.saturating_sub(2));
    let inner = overlay_frame(
        f,
        area,
        app,
        w,
        h,
        &FrameSpec::dialog(
            "ADD TO SPOTIFY PLAYLIST",
            &[("⏎", "add"), ("＋", "new"), ("esc", "close")],
        ),
    );
    if inner.height == 0 {
        return;
    }
    let iw = inner.width as usize;

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!(" ♪ {}", clip(&m.subject, iw.saturating_sub(4))),
            Style::default().fg(col(th.text_dim)),
        )),
        Line::raw(""),
    ];

    // a loading / error note replaces the (empty) list until playlists arrive
    if m.loading {
        lines.push(Line::from(Span::styled(
            "  Loading your playlists…",
            Style::default().fg(col(th.text_faint)),
        )));
    } else if !m.note.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("  {}", clip(&m.note, iw.saturating_sub(3))),
            Style::default().fg(col(th.accent[0])),
        )));
        lines.push(Line::raw(""));
    }

    if !m.loading {
        let body = inner.height.saturating_sub(3) as usize;
        let off = if total > body && body > 0 {
            sel.saturating_sub(body / 2).min(total - body)
        } else {
            0
        };
        for i in off..(off + body).min(total) {
            let on = i == sel;
            let bg = if on { Some(th.selection) } else { None };
            let marker = if on { "▸ " } else { "  " };
            let content = if i < m.playlists.len() {
                Line::from(vec![
                    Span::styled(
                        format!("{marker}♫ "),
                        Style::default().fg(col(th.accent[2])),
                    ),
                    Span::styled(
                        clip(&m.playlists[i].name, iw.saturating_sub(6)),
                        Style::default().fg(col(th.text)).add_modifier(if on {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                    ),
                ])
            } else {
                Line::from(Span::styled(
                    format!("{marker}＋ New playlist…"),
                    Style::default()
                        .fg(col(th.accent[0]))
                        .add_modifier(Modifier::BOLD),
                ))
            };
            lines.push(pill_line(app, iw, content, bg));
        }
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// Unfollow ("delete") confirmation for a Spotify playlist — names it and makes
/// the action explicit. The Spotify analogue of [`confirm_delete_overlay`].
pub fn spotify_confirm_delete_overlay(f: &mut Frame, area: Rect, app: &AppState) {
    let Some((_, name)) = app.spotify.pl_confirm_delete.as_ref() else {
        return;
    };
    let th = &app.theme;
    let w = 52u16.min(area.width.saturating_sub(4));
    let h = 7u16.min(area.height.saturating_sub(2));
    let inner = overlay_frame(
        f,
        area,
        app,
        w,
        h,
        &FrameSpec::dialog(
            "DELETE SPOTIFY PLAYLIST",
            &[("⏎/y", "delete"), ("esc", "cancel")],
        ),
    );
    if inner.height == 0 {
        return;
    }
    let iw = inner.width as usize;
    let lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  Unfollow "),
            Span::styled(
                format!("“{}”", clip(name, iw.saturating_sub(14))),
                Style::default()
                    .fg(col(th.accent[0]))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("?"),
        ]),
        Line::from(Span::styled(
            "  Removes it from your Spotify library.",
            Style::default().fg(col(th.text_faint)),
        )),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

// ---- name entry + confirm dialogs ----------------------------------------

/// Centered text-entry dialog for the active naming prompt. Shows a clear title,
/// the live input with a cursor, and the confirm/cancel hint — so creating or
/// renaming a playlist (and the other name prompts) always shows what you type,
/// instead of typing blind into the status line.
pub fn name_overlay(f: &mut Frame, area: Rect, app: &AppState) {
    use crate::app::NameTarget;
    let Some(target) = app.input.naming.as_ref() else {
        return;
    };
    let (title, label, verb): (&str, &str, &str) = match target {
        NameTarget::New => ("NEW PLAYLIST", "Name", "create"),
        NameTarget::Rename(_) => ("RENAME PLAYLIST", "New name", "rename"),
        NameTarget::AddMusicDir => ("ADD MUSIC FOLDER", "Folder path", "add"),
        NameTarget::Bookmark => ("BOOKMARK SEARCH", "Name", "save"),
        NameTarget::SmartPlaylist => ("NEW SMART PLAYLIST", "Name", "create"),
        NameTarget::SpotifyClientId => ("SPOTIFY CLIENT ID", "Paste ID", "save"),
    };
    let th = &app.theme;
    let w = 52u16.min(area.width.saturating_sub(4));
    let h = 7u16.min(area.height.saturating_sub(2));
    let inner = overlay_frame(
        f,
        area,
        app,
        w,
        h,
        &FrameSpec::dialog(title, &[("⏎", verb), ("esc", "cancel")]),
    );
    if inner.height == 0 {
        return;
    }
    let iw = inner.width as usize;
    let lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            format!("  {label}:"),
            Style::default()
                .fg(col(th.text_faint))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::raw("  "),
            Span::styled(
                clip(&app.input.buffer, iw.saturating_sub(4)),
                Style::default()
                    .fg(col(th.text))
                    .add_modifier(Modifier::BOLD),
            ),
            // a block cursor so the caret position is obvious while typing
            Span::styled("▌", Style::default().fg(col(th.accent[0]))),
        ]),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

/// Centered confirmation dialog for deleting a playlist — names the playlist and
/// makes the irreversible action explicit before it happens (the `d` two-step).
pub fn confirm_delete_overlay(f: &mut Frame, area: Rect, app: &AppState) {
    let Some(id) = app.input.confirm_delete else {
        return;
    };
    let name = app
        .library
        .playlists
        .get(&id)
        .map(|p| p.name.clone())
        .unwrap_or_default();
    let th = &app.theme;
    let w = 52u16.min(area.width.saturating_sub(4));
    let h = 7u16.min(area.height.saturating_sub(2));
    let inner = overlay_frame(
        f,
        area,
        app,
        w,
        h,
        &FrameSpec::dialog("DELETE PLAYLIST", &[("⏎/y", "delete"), ("esc", "cancel")]),
    );
    if inner.height == 0 {
        return;
    }
    let iw = inner.width as usize;
    let lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::raw("  Delete "),
            Span::styled(
                format!("“{}”", clip(&name, iw.saturating_sub(12))),
                Style::default()
                    .fg(col(th.accent[0]))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("?"),
        ]),
        Line::from(Span::styled(
            "  This can't be undone.",
            Style::default().fg(col(th.text_faint)),
        )),
    ];
    f.render_widget(Paragraph::new(lines), inner);
}

/// Command palette: grouped, fuzzy-runnable actions. Browsing shows tidy
/// category sections (View / Playback / …); typing flat-filters across all.
pub fn command_palette(f: &mut Frame, area: Rect, app: &AppState) {
    let Some(p) = &app.palette else { return };
    let th = &app.theme;
    let entries = app.palette_entries(); // (category, label, action)
    let matches = app.palette_matches(); // indices into entries, best first
    let browsing = p.query.trim().is_empty();
    let sel = p.sel.min(matches.len().saturating_sub(1));

    // display rows: in browse mode insert a header when the category changes
    enum Row {
        Header(String),
        Item { ci: usize, pos: usize },
    }
    let mut rows: Vec<Row> = Vec::new();
    let mut last_cat: Option<&str> = None;
    for (pos, &ci) in matches.iter().enumerate() {
        if browsing {
            let cat = entries[ci].0.as_str();
            if last_cat != Some(cat) {
                rows.push(Row::Header(cat.to_string()));
                last_cat = Some(cat);
            }
        }
        rows.push(Row::Item { ci, pos });
    }
    let sel_disp = rows
        .iter()
        .position(|r| matches!(r, Row::Item { pos, .. } if *pos == sel))
        .unwrap_or(0);

    let w = 58.min(area.width.saturating_sub(4));
    // input + rows + footer + borders; the palette stays anchored near the top
    // (a spotlight bar), so it isn't vertically centered like the dialogs.
    let h = (rows.len() as u16 + 4)
        .clamp(6, 23)
        .min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + 2;
    let rect = Rect::new(x, y, w, h);
    f.render_widget(ratatui::widgets::Clear, rect);
    let inner = panel(f, rect, app, "COMMANDS", true);

    let [input, list, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    footer_bar(
        f,
        foot,
        app,
        &[("↑↓", "select"), ("⏎", "run"), ("esc", "close")],
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ❯ ", Style::default().fg(col(th.accent[0]))),
            Span::styled(p.query.clone(), Style::default().fg(col(th.text))),
            Span::styled("▌", Style::default().fg(col(th.accent[0]))),
        ])),
        input,
    );

    let body = list.height as usize;
    let off = if rows.len() > body && body > 0 {
        sel_disp.saturating_sub(body / 2).min(rows.len() - body)
    } else {
        0
    };
    let mut lines: Vec<Line> = Vec::new();
    for (d, r) in rows.iter().skip(off).take(body).enumerate() {
        match r {
            Row::Header(cat) => lines.push(Line::from(Span::styled(
                format!(" {} ", cat.to_uppercase()),
                Style::default()
                    .fg(col(th.accent[2]))
                    .add_modifier(Modifier::BOLD),
            ))),
            Row::Item { ci, pos } => {
                // clickable row: single-click selects, double-click runs the command
                app.register_click(
                    Rect::new(list.x, list.y + d as u16, list.width, 1),
                    MouseTarget::PaletteRow(*pos),
                );
                let selected = *pos == sel;
                let bg = if selected { Some(th.selection) } else { None };
                let st = if selected {
                    Style::default()
                        .fg(col(th.accent[0]))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(col(th.text_dim))
                };
                let marker = if selected { "❯ " } else { "  " };
                let mut spans = vec![Span::styled(format!("{marker}{}", entries[*ci].1), st)];
                if !browsing {
                    // show which group it belongs to when results are flat
                    spans.push(Span::styled(
                        format!("  · {}", entries[*ci].0),
                        Style::default().fg(col(th.text_faint)),
                    ));
                }
                lines.push(pill_line(app, list.width as usize, Line::from(spans), bg));
            }
        }
    }
    if matches.is_empty() {
        lines.push(Line::from(Span::styled(
            "   no matching command",
            Style::default().fg(col(th.text_faint)),
        )));
    }
    f.render_widget(Paragraph::new(lines), list);
}
