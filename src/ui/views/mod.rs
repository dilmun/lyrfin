//! Top-level views: compose `components` into each layout from `design/`.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{AppState, Panel};
use crate::ui::components;
use crate::ui::theme::Rgb;

mod equalizer_view;
pub use equalizer_view::*;
pub mod mini;
pub(crate) mod settings_rows;
mod settings_view;
pub use settings_view::*;
mod radio_view;
pub use radio_view::*;
mod spotify_view;
pub use spotify_view::*;

/// 01 — Home: sidebar + tracklist, with an optional right column holding the
/// queue (top) and/or the artist panel (under it). The dashboard owns its queue
/// so the artist panel can stack beneath it (the global chrome skips Home).
pub fn dashboard(f: &mut Frame, area: Rect, app: &AppState) {
    use crate::app::Focus;
    // Standardized shell: a fixed bordered LIBRARY sidebar + a titled MUSIC main
    // pane (with inline `/`-search), with the movable Queue/Artist panes docked
    // around them — the same chrome as the Spotify view.
    let searching = app.search.active || !app.search.query.is_empty();
    let title = components::tracklist_title(app);
    let search_info = if searching {
        format!("{} results", app.search_results().len())
    } else {
        String::new()
    };
    components::browser_shell(
        f,
        area,
        app,
        &[Panel::Sidebar, Panel::Queue, Panel::Artist, Panel::Lyrics],
        &|f, slot, app, panel| match panel {
            // the LIBRARY sidebar is now a movable dock pane like the rest; draw
            // its border + section list the same way the other panes self-border
            Panel::Sidebar => {
                let inner = components::panel(
                    f,
                    slot,
                    app,
                    components::sidebar_title(app),
                    app.focus == Focus::Sidebar,
                );
                components::sidebar_body(f, inner, app);
            }
            Panel::Queue => components::queue(f, slot, app),
            Panel::Artist => {
                components::artist_panel(f, slot, app, app.focus == Focus::Pane(Panel::Artist))
            }
            Panel::Lyrics => components::lyrics_panel(
                f,
                slot,
                app,
                app.focus == Focus::Pane(Panel::Lyrics),
                crate::app::LyricsPane::Local,
            ),
            _ => {}
        },
        components::ShellPane {
            title: &title,
            title_right: None,
            focused: app.focus == Focus::Main || searching,
            // searching → the field takes over the border (drawn by the shell)
            search: searching.then(|| components::SearchBar {
                query: &app.search.query,
                caret: app.search.query.chars().count(),
                focused: app.focus == Focus::Search,
                loading: false,
                tick: app.tick,
                placeholder: "search your library…",
                scope: "Library",
                info: &search_info,
            }),
            render: &|f, inner, app| components::local_main_body(f, inner, app),
        },
    );
}

/// Library (key `2`): a 3-column "Miller" browser — ARTISTS ▸ ALBUMS (of the
/// selected artist) ▸ TRACKS (of the selected album), navigated with h/l (columns),
/// j/k (rows) and enter (drill in / play). The optional Queue/Artist/Lyrics panes
/// dock around the columns via the shared movable-pane system.
pub fn library(f: &mut Frame, area: Rect, app: &AppState) {
    use crate::app::Focus;

    // dock the optional movable panes, then split the leftover core into 3 columns
    let core =
        components::dock_panels(
            f,
            area,
            app,
            app.layout.panels(),
            |f, slot, app, panel| match panel {
                Panel::Queue => components::queue(f, slot, app),
                Panel::Artist => {
                    components::artist_panel(f, slot, app, app.focus == Focus::Pane(Panel::Artist))
                }
                Panel::Lyrics => components::lyrics_panel(
                    f,
                    slot,
                    app,
                    app.focus == Focus::Pane(Panel::Lyrics),
                    crate::app::LyricsPane::Local,
                ),
                _ => {}
            },
        );
    // A fresh / empty library → the onboarding panel instead of three empty columns.
    if app.library.track_count() == 0 {
        components::welcome(f, core, app);
        return;
    }
    let [acol, bcol, ccol] = Layout::horizontal([
        Constraint::Length(30),
        Constraint::Length(38),
        Constraint::Min(20),
    ])
    .areas(core);

    app.register_focus(core, Focus::Main);
    let on_main = app.focus == Focus::Main;
    for (i, area) in [acol, bcol, ccol].into_iter().enumerate() {
        browse_column(f, area, app, i as u8, on_main && app.browser.col == i as u8);
    }
}

/// Render one Miller column (0 = ARTISTS, 1 = ALBUMS of the selected artist,
/// 2 = TRACKS of the selected album) into `area`.
///
/// The three columns were three near-identical blocks; they differ only in their
/// title, row content, accent and click targets, so they share one body. Extracted
/// because the mini layout shows exactly **one** column at full width — it selects
/// by `browser.col` instead of drawing all three side by side.
///
/// Only the visible row slice is materialised: a column is `n` rows deep but at
/// most `h` are built, so a large library doesn't allocate per frame for rows that
/// are scrolled out of view.
pub(super) fn browse_column(f: &mut Frame, area: Rect, app: &AppState, col: u8, focused: bool) {
    use crate::app::{MouseTarget, ScrollBox};
    let th = &app.theme;
    let sh = app.config.arabic_shaping;

    let artists = app.library.artists_sorted();
    let (na, nb, nc) = app.browser_counts();
    let a_sel = app.browser.artist.min(na.saturating_sub(1));
    let b_sel = app.browser.album.min(nb.saturating_sub(1));
    let t_sel = app.browser.track.min(nc.saturating_sub(1));
    let cur_artist = artists.get(a_sel);
    let albums = cur_artist
        .map(|a| app.library.albums_of(a.id))
        .unwrap_or_default();
    let tracks = albums
        .get(b_sel)
        .map(|a| app.library.tracks_of(a.id))
        .unwrap_or_default();

    // everything that differs between the three columns, resolved up front
    let (title, n, sel, off_cell, accent, scroll) = match col {
        0 => (
            format!("ARTISTS · {na}"),
            na,
            a_sel,
            &app.browser.artist_off,
            th.accent[0],
            ScrollBox::BrowseArtists,
        ),
        1 => (
            cur_artist
                .map(|a| format!("ALBUMS · {}", crate::arabic::shaped(&a.name, sh)))
                .unwrap_or_else(|| "ALBUMS".into()),
            albums.len(),
            b_sel,
            &app.browser.album_off,
            th.accent[1],
            ScrollBox::BrowseAlbums,
        ),
        _ => (
            format!("TRACKS · {}", tracks.len()),
            tracks.len(),
            t_sel,
            &app.browser.track_off,
            th.accent[2],
            ScrollBox::BrowseTracks,
        ),
    };

    let inner = components::panel(f, area, app, &title, focused);
    app.register_click(inner, MouseTarget::Scroll(scroll));
    let (w, h) = (inner.width as usize, inner.height as usize);
    let off = components::sticky_off(off_cell, sel, n, h);
    let mut lines = Vec::with_capacity(h.min(n.saturating_sub(off)));
    for i in off..n.min(off + h) {
        let (target, name, trail) = match col {
            0 => (
                MouseTarget::BrowseArtist(i),
                crate::arabic::shaped(&artists[i].name, sh),
                app.library.albums_of(artists[i].id).len().to_string(),
            ),
            1 => (
                MouseTarget::BrowseAlbum(i),
                crate::arabic::shaped(&albums[i].title, sh),
                albums[i].year.map(|y| y.to_string()).unwrap_or_default(),
            ),
            _ => (
                MouseTarget::BrowseTrack(i),
                crate::arabic::shaped(&tracks[i].title, sh),
                components::mmss(tracks[i].duration()),
            ),
        };
        app.register_click(
            Rect::new(inner.x, inner.y + (i - off) as u16, inner.width, 1),
            target,
        );
        lines.push(browse_row(app, i == sel, focused, &name, &trail, w, accent));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// One row in a [`library`] column: a `▸`/highlight marker (tinted with the
/// column `accent`), the already-shaped `name` truncated with an ellipsis, and a
/// right-aligned `right` detail (album count / year / duration).
fn browse_row(
    app: &AppState,
    on: bool,
    focused: bool,
    name: &str,
    right: &str,
    w: usize,
    accent: Rgb,
) -> Line<'static> {
    let th = &app.theme;
    let bg = components::sel_fill(th, on, focused);
    // Interior width — the rounded end-caps take a col on each side, so the row
    // content (and its right-aligned detail) lays out inside `w - 2`.
    let iw = w.saturating_sub(2);
    let marker = if on { "▸ " } else { "  " };
    let name_fg: Color = if on {
        th.text.into()
    } else {
        th.text_dim.into()
    };
    let rlen = right.chars().count();
    let budget = iw.saturating_sub(2 + rlen + 1).max(1);
    let mut nm: String = name.chars().take(budget).collect();
    if name.chars().count() > budget {
        nm.pop();
        nm.push('…');
    }
    let fill = iw.saturating_sub(2 + nm.chars().count() + rlen);
    let bold = if on {
        Modifier::BOLD
    } else {
        Modifier::empty()
    };
    let content = Line::from(vec![
        Span::styled(marker, Style::default().fg(accent.into())),
        Span::styled(nm, Style::default().fg(name_fg).add_modifier(bold)),
        Span::raw(" ".repeat(fill)),
        Span::styled(right.to_string(), Style::default().fg(th.text_faint.into())),
    ]);
    components::pill_line(app, w, content, bg)
}

/// The queue pane for the source-neutral player views: whichever source is
/// playing owns the queue shown, so Now Playing / Lyrics list the upcoming
/// Spotify tracks while Spotify streams instead of a stale local queue. Radio has
/// no queue, so it keeps the local one (empty or not) rather than inventing one.
fn playing_queue(f: &mut Frame, area: Rect, app: &AppState) {
    if app.now_playing_source() == Some(crate::app::NpSource::Spotify) {
        components::spotify_queue(
            f,
            area,
            app,
            app.focus == crate::app::Focus::Pane(Panel::Queue),
        );
    } else {
        components::queue(f, area, app);
    }
}

/// 03 — Now Playing: a full-width playback bar at the bottom; above it the
/// visualizer (or album art), with the queue docked beside/over it (not full
/// height — it sits above the playback like on Home).
pub fn nowplaying(f: &mut Frame, area: Rect, app: &AppState) {
    // same playback-bar height as every other view (#1–#4): `area` here is the
    // body (frame minus the 1-row status bar), so the frame height is area+1.
    let play_h = components::now_bar_height(app.config.player_viz, area.width, area.height + 1);
    let [content, play] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(play_h)]).areas(area);
    components::now_bar(f, play, app);

    // dock the queue within the content area, above the playback bar
    let mut c = content;
    let q = app.panel(Panel::Queue);
    if q.shown && c.width >= 36 && c.height >= 4 {
        let span = components::pane_span(c, q.dock, q.size); // q.size is a percentage
        let (qr, rest) = components::dock_split(c, q.dock, span, span.min(c.height.max(2) / 2));
        playing_queue(f, qr, app);
        app.register_pane_edge(c, q.dock, qr, Panel::Queue);
        app.register_focus(qr, crate::app::Focus::Pane(Panel::Queue));
        c = rest;
    }
    app.register_focus(c, crate::app::Focus::Main);

    if app.panel(Panel::Visualizer).shown && c.height >= 4 {
        let mode = app.viz_mode();
        components::spectrum_panel(
            f,
            c,
            app,
            &format!(
                "VISUALIZER · {}",
                components::VIZ_MODES[mode as usize % components::VIZ_MODES.len()]
            ),
            mode,
            app.focus == crate::app::Focus::Main,
        );
    } else {
        components::album_art(f, c, app);
    }
}

/// Concert / Focus — fullscreen, distraction-free now-playing: large centered
/// album art, the title/artist/album, star rating, the progress beam, and a
/// visualizer strip. No panels or shared chrome.
pub fn concert(f: &mut Frame, area: Rect, app: &AppState) {
    use crate::app::MouseTarget;
    let th = &app.theme;
    // paint the whole frame with the theme background
    f.render_widget(
        ratatui::widgets::Block::default().style(Style::default().bg(th.bg.into())),
        area,
    );
    // centre a readable column
    let cw = area.width.min(86);
    let col = Rect::new(area.x + (area.width - cw) / 2, area.y, cw, area.height);

    // Concert's single visualizer is the playback one (toggle + mode live under
    // the "Playback visualizer" setting); off → the art/meta reclaim the space.
    let viz_h: u16 = if app.config.player_viz && area.height >= 22 {
        7
    } else {
        0
    };
    let [art, meta, viz] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(6), // title, artist, album, rating, gap, progress
        Constraint::Length(viz_h),
    ])
    .areas(col);

    // album art, upscaled to fill and centred. Size a rect that matches the
    // cover's true pixel aspect (using the terminal's font cell size) so the
    // upscaled image fills it exactly with no letterboxing, then centre it.
    let art_rect = match (app.art.dims, app.art.picker.as_ref()) {
        (Some((iw, ih)), Some(p)) => {
            let fs = p.font_size();
            let (fw, fh) = (fs.width.max(1) as f64, fs.height.max(1) as f64);
            let (iw, ih) = (iw.max(1) as f64, ih.max(1) as f64);
            // cells where (cw*fw)/(ch*fh) == iw/ih, maximised within `art`
            let cw_at_full_h = art.height as f64 * (iw * fh) / (ih * fw);
            let (cw, ch) = if cw_at_full_h <= art.width as f64 {
                (cw_at_full_h, art.height as f64)
            } else {
                (art.width as f64, art.width as f64 * (ih * fw) / (iw * fh))
            };
            let cw = (cw.round() as u16).clamp(1, art.width);
            let ch = (ch.round() as u16).clamp(1, art.height);
            Rect::new(
                art.x + art.width.saturating_sub(cw) / 2,
                art.y + art.height.saturating_sub(ch) / 2,
                cw,
                ch,
            )
        }
        _ => art, // no cover → gradient fills the whole region
    };
    components::album_art_filled(f, art_rect, app);

    // meta: a centred text block, then a full-width progress row
    let [text, _gap, prog_row] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(meta);

    // whatever is playing — local, Spotify or radio (this view has no source)
    let card = app.playing_card();
    let title = card
        .as_ref()
        .map(|c| c.title.clone())
        .unwrap_or_else(|| "—".into());
    let artist = card.as_ref().map(|c| c.artist.clone()).unwrap_or_default();
    let album = card.as_ref().map(|c| c.album.clone()).unwrap_or_default();
    let rating = card.as_ref().map(|c| c.rating).unwrap_or(0);
    let album_line = match card.as_ref().and_then(|c| c.year) {
        Some(y) if !album.is_empty() => format!("{album}  ·  {y}"),
        _ => album.clone(),
    };
    let cnt = title.chars().count().max(2);
    let title_spans: Vec<Span> = title
        .chars()
        .enumerate()
        .map(|(i, ch)| {
            Span::styled(
                ch.to_string(),
                Style::default()
                    .fg(th.accent_at(i as f32 / cnt as f32).into())
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect();
    let text_lines: Vec<Line> = vec![
        Line::from(title_spans),
        Line::from(Span::styled(artist, Style::default().fg(th.text.into()))),
        Line::from(Span::styled(
            album_line,
            Style::default().fg(th.text_dim.into()),
        )),
        if rating > 0 {
            Line::from(Span::styled(
                components::stars(rating),
                Style::default().fg(th.warning.into()),
            ))
        } else {
            Line::raw("")
        },
    ];
    f.render_widget(
        Paragraph::new(text_lines).alignment(Alignment::Center),
        text,
    );

    // progress row: `elapsed  ━━●── remaining`, full width, with a Seek hit-box
    // registered over exactly the beam so clicking/dragging it seeks. A live
    // stream has no total to measure against, so it gets a LIVE badge instead of
    // a beam that could never fill.
    if card.as_ref().is_some_and(|c| c.live) {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "● LIVE",
                Style::default().fg(th.accent[0].into()),
            )))
            .alignment(Alignment::Center),
            prog_row,
        );
    } else {
        let (elapsed, duration, frac) = card
            .as_ref()
            .map(|c| (c.elapsed, c.duration, c.frac))
            .unwrap_or_default();
        let pre = format!("{} ", components::mmss(elapsed));
        let suf = format!(" {}", components::mmss(duration.saturating_sub(elapsed)));
        let (plen, slen) = (pre.chars().count() as u16, suf.chars().count() as u16);
        let bar_w = prog_row.width.saturating_sub(plen + slen).max(1);
        app.register_click(
            Rect::new(prog_row.x + plen, prog_row.y, bar_w, 1),
            MouseTarget::Seek,
        );
        let mut prog: Vec<Span> = vec![Span::styled(pre, Style::default().fg(th.text_dim.into()))];
        prog.extend(components::progress_spans(th, frac, bar_w as usize));
        prog.push(Span::styled(suf, Style::default().fg(th.text_faint.into())));
        f.render_widget(Paragraph::new(Line::from(prog)), prog_row);
    }

    if viz_h > 0 {
        // borderless — blends into the Concert background, no panel box. Uses the
        // playback-bar visualizer's own mode (Concert has no separate big viz).
        components::spectrum_bare(f, viz, app, app.config.player_viz_mode);
    }
}

/// 04 — Lyrics: centered lyrics + one slim playback card, with an optional
/// visualizer (top) and queue (right). Self-contained — no shared bottom bar.
pub fn lyrics(f: &mut Frame, area: Rect, app: &AppState) {
    // playback card at the bottom — same height as every other view (#1–#4)
    let play_h = components::now_bar_height(app.config.player_viz, area.width, area.height + 1);
    let [mut content, play] =
        Layout::vertical([Constraint::Min(5), Constraint::Length(play_h)]).areas(area);
    components::now_bar(f, play, app);

    // queue + visualizer dock within the content area, above the playback bar
    let q = app.panel(Panel::Queue);
    if q.shown && content.width >= 40 && content.height >= 6 {
        let span = components::pane_span(content, q.dock, q.size); // percentage
        let (qr, rest) =
            components::dock_split(content, q.dock, span, span.min(content.height / 2).max(4));
        playing_queue(f, qr, app);
        app.register_pane_edge(content, q.dock, qr, Panel::Queue);
        app.register_focus(qr, crate::app::Focus::Pane(Panel::Queue));
        content = rest;
    }
    let viz_p = app.panel(Panel::Visualizer);
    if viz_p.shown && content.width >= 40 && content.height >= 8 {
        let span = components::pane_span(content, viz_p.dock, viz_p.size); // percentage
        let (viz, rest) = components::dock_split(
            content,
            viz_p.dock,
            span,
            span.min(content.height / 2).max(6),
        );
        let mode = app.viz_mode();
        components::spectrum_panel(
            f,
            viz,
            app,
            &format!(
                "VISUALIZER · {}",
                components::VIZ_MODES[mode as usize % components::VIZ_MODES.len()]
            ),
            mode,
            false, // the lyrics view's viz strip isn't a Tab focus target
        );
        app.register_pane_edge(content, viz_p.dock, viz, Panel::Visualizer);
        content = rest;
    }

    app.register_focus(content, crate::app::Focus::Main);
    components::lyrics_panel(
        f,
        content,
        app,
        app.focus == crate::app::Focus::Main,
        // follows whatever is playing — this view belongs to no single source
        app.active_lyrics_pane(),
    );
}

/// Internet radio: a live search box over a list of stations. Type to search
/// (Radio Browser), ctrl-n/p or ↑/↓ to move, Enter to tune in. Stations are not
/// library tracks, so this has its own list (the chrome supplies the now-bar).
/// Clip `s` to `max` columns with a trailing ellipsis. Shared by the source
/// views (radio station list, Spotify rows) via `pub(super)`.
pub(super) fn clip(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        s.chars().take(max.saturating_sub(1)).collect::<String>() + "…"
    } else {
        s.to_string()
    }
}
