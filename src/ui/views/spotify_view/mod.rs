//! The Spotify source view: auth panel, library browser (sidebar + main pane),
//! grouped artist page, podcast/empty states, and the rate-limit / not-registered
//! guides. Split out of `views` to keep the Spotify rendering self-contained.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::clip;
use crate::app::{AppState, MouseTarget, Panel};
use crate::ui::components;

mod guides;
use guides::{spotify_client_id_guide, spotify_not_registered_guide, spotify_ratelimit_guide};

/// The Spotify view: the auth panel until connected, then the library browser.
pub fn spotify(f: &mut Frame, area: Rect, app: &AppState) {
    if matches!(
        app.spotify.conn,
        crate::spotify::ConnState::Connected { .. }
    ) {
        spotify_browser(f, area, app);
    } else {
        spotify_auth(f, area, app);
    }
}

/// The main pane's title — section / drill-in crumb / search. While searching, the
/// query lives in the inline `search_bar` row over the list, so the title just names
/// the mode. Shared with the mini layout, which titles its main card the same way.
pub(in crate::ui::views) fn spotify_main_title(app: &AppState) -> String {
    use crate::spotify::api::Group;
    let sp = &app.spotify;
    let n = sp.items.len();
    if sp.searching || sp.in_search {
        "SPOTIFY  ·  SEARCH".to_string()
    } else if let Some(crumb) = &sp.crumb {
        // shape the (possibly Arabic) drill-in name for display, exactly like the
        // list rows / local view titles — otherwise an RTL name shows reversed.
        let name = crate::arabic::shaped(&crumb.to_uppercase(), app.config.arabic_shaping);
        // an artist page mixes tracks + releases, so don't label the count "tracks"
        if sp.items.iter().any(|i| i.group != Group::None) {
            format!("SPOTIFY  ·  {name}")
        } else {
            format!("SPOTIFY  ·  {name}  ·  {n} tracks")
        }
    } else {
        // the Podcasts section is your FOLLOWED shows — name it so it's clear where
        // following a show lands (the sidebar entry stays "Podcasts").
        let label = if sp.section == crate::spotify::api::Section::Podcasts {
            "YOUR SHOWS".to_string()
        } else {
            sp.section.label().to_uppercase()
        };
        format!("SPOTIFY  ·  {label}  ·  {n}")
    }
}

/// Connected library/search browser: sections sidebar + a result list.
fn spotify_browser(f: &mut Frame, area: Rect, app: &AppState) {
    use crate::app::Focus;
    let sp = &app.spotify;
    let title = spotify_main_title(app);
    let search_info = if sp.in_search {
        format!("{} results", sp.items.len())
    } else {
        String::new()
    };
    // searching highlights the whole main border + title in the accent colour
    let main_focus = app.focus == Focus::Main || sp.searching;

    components::browser_shell(
        f,
        area,
        app,
        &[Panel::Sidebar, Panel::Queue, Panel::Artist, Panel::Lyrics],
        &|f, slot, app, panel| {
            let focused = app.focus == Focus::Pane(panel);
            match panel {
                // the LIBRARY sidebar is a movable dock pane here too
                Panel::Sidebar => {
                    let inner =
                        components::panel(f, slot, app, "LIBRARY", app.focus == Focus::Sidebar);
                    spotify_sidebar_body(f, inner, app);
                }
                Panel::Queue => components::spotify_queue(f, slot, app, focused),
                Panel::Artist => components::spotify_artist_panel(f, slot, app, focused),
                // gate to the Spotify source: never show a local track's lyrics
                Panel::Lyrics => {
                    components::lyrics_panel(f, slot, app, focused, crate::app::LyricsPane::Spotify)
                }
                _ => {}
            }
        },
        components::ShellPane {
            title: &title,
            title_right: spotify_account_chip(app),
            focused: main_focus,
            // searching → the field takes over the border (drawn by the shell), so
            // the list keeps its full height and an empty query still reads as a
            // box rather than a bare caret on a blank row
            search: (sp.searching || sp.in_search).then(|| components::SearchBar {
                query: &sp.query,
                caret: sp.query.chars().count(),
                focused: sp.searching,
                loading: sp.loading,
                tick: app.tick,
                placeholder: "search Spotify…",
                scope: "Spotify",
                info: &search_info,
            }),
            render: &|f, m, app| spotify_main_body(f, m, app),
        },
    );
}

/// The right-aligned account chip for the Spotify header border: `◉ {display name}`,
/// with a `⚠` once Spotify's account-level audio-key block is detected (so the
/// blocked account is named right where the failure shows). `None` when not connected
/// or the `spotify_show_account` privacy toggle is off — the account is still in the
/// Info overlay regardless. Owned (`Line<'static>`) so it outlives the borrow.
fn spotify_account_chip(app: &AppState) -> Option<Line<'static>> {
    use crate::spotify::ConnState;
    if !app.config.spotify_show_account {
        return None;
    }
    let ConnState::Connected { name, .. } = &app.spotify.conn else {
        return None;
    };
    let th = &app.theme;
    let mut spans = vec![
        Span::styled("◉ ", Style::default().fg(th.accent[0].into())),
        Span::styled(name.clone(), Style::default().fg(th.text_dim.into())),
    ];
    // only the CONFIRMED account-level audio-key block (not a transient CDN/throttle
    // blip, which recovers on its own — see `spotify_key_block_confirmed`)
    if app.spotify_key_block_confirmed() {
        spans.push(Span::styled(
            "  ⚠ blocked",
            Style::default().fg(th.accent[0].into()),
        ));
    }
    spans.push(Span::raw(" ")); // a hair of padding off the rounded corner
    Some(Line::from(spans))
}

/// The Spotify sidebar's section list (drawn into the shell's inner rect).
pub(in crate::ui::views) fn spotify_sidebar_body(f: &mut Frame, inner: Rect, app: &AppState) {
    use crate::app::Focus;
    use crate::spotify::api::Section;
    let th = &app.theme;
    let sp = &app.spotify;
    let sidebar_focus = app.focus == Focus::Sidebar;
    let mut side_lines: Vec<Line> = Vec::new();
    for (k, s) in Section::ALL.into_iter().enumerate() {
        // clickable section row: click selects + loads that section (like ⏎/j-k)
        if (k as u16) < inner.height {
            app.register_click(
                Rect::new(inner.x, inner.y + k as u16, inner.width, 1),
                MouseTarget::SpotifySection(k),
            );
        }
        let on = s == sp.section && !sp.in_search && sp.crumb.is_none();
        let focused = on && sidebar_focus;
        // focused-selected section is a rounded capsule; selected-but-unfocused
        // keeps accent+bold text without a bar (matches the local sidebar).
        let bg = if focused { Some(th.selection) } else { None };
        let style = if on {
            Style::default()
                .fg(th.accent[0].into())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(th.text_dim.into())
        };
        let content = Line::from(Span::styled(format!("{} {}", s.icon(), s.label()), style));
        side_lines.push(components::pill_line(
            app,
            inner.width as usize,
            content,
            bg,
        ));
    }
    f.render_widget(Paragraph::new(side_lines), inner);
}

/// The Spotify main pane content (empty states / artist page / columnar tracks /
/// search+section list), drawn into the shell's inner rect `m`.
pub(in crate::ui::views) fn spotify_main_body(f: &mut Frame, m: Rect, app: &AppState) {
    use crate::app::Focus;
    use crate::spotify::api::{Kind, Section};
    let th = &app.theme;
    let sp = &app.spotify;
    let list_focus = app.focus == Focus::Main && !sp.searching;

    let n = sp.items.len();
    if n == 0 && sp.note.contains("rate-limit") {
        spotify_ratelimit_guide(f, m, app);
        return;
    }
    // a "not registered" 403 (this account isn't on the dev app's allow-list) gets
    // the tidy, centered card with a clickable dashboard link — not a raw, clipped
    // one-liner. Same card as the auth panel.
    if n == 0 && sp.note == crate::spotify::api::NOT_REGISTERED_MSG {
        spotify_not_registered_guide(f, m, app);
        return;
    }
    // the Podcasts section, empty: distinguish "follow some shows" from "your
    // Spotify market has no podcasts" (most podcasts aren't licensed outside a
    // handful of countries — e.g. they're absent in IQ, available in the US).
    if n == 0
        && !sp.loading
        && !sp.in_search
        && sp.crumb.is_none()
        && sp.section == Section::Podcasts
    {
        spotify_podcasts_empty(f, m, app);
        return;
    }
    if n == 0 {
        let msg = if sp.loading {
            "Loading…"
        } else if sp.note.is_empty() {
            "Empty"
        } else {
            &sp.note
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                msg,
                Style::default().fg(th.text_faint.into()),
            )))
            .alignment(Alignment::Center),
            Rect::new(m.x, m.y + m.height / 3, m.width, 1),
        );
        return;
    }

    // a flat cover-art grid: the Albums/Artists section (the `#` toggle), or the
    // "Browse all" categories root — covers from each item's image URL, via the
    // source-agnostic grid renderer
    if app.spotify_grid_active()
        || app.spotify_browse_grid_active()
        || app.spotify_podcast_grid_active()
    {
        components::render_grid(
            f,
            m,
            app,
            list_focus,
            components::GridData {
                n: sp.items.len(),
                sel: sp.sel,
                cols: &sp.cols,
                row_off: &sp.row_off,
                card_at: &|i| {
                    let it = &sp.items[i];
                    components::GridCard {
                        name: it.name.clone(),
                        // grid cards stay clean: the artist/owner only (shared policy)
                        subtitle: components::grid_card_subtitle(&it.subtitle),
                        year: it.year,
                        art: it.image.as_ref().map(|u| {
                            (
                                crate::artwork::ArtKey::remote(u),
                                crate::artwork::ArtSource::Url(u.clone()),
                            )
                        }),
                        followed: it.kind == Kind::Show && sp.followed_shows.contains(&it.uri),
                        tint: it.tint,
                    }
                },
                click: &MouseTarget::SpotifyItem,
            },
        );
        return;
    }

    // sectioned browse (Home feed, or a category's playlist shelves) → Netflix-style
    // carousels (one per section title), via the shared release_grid — same layout +
    // nav as the artist page's releases
    if app.spotify_sectioned_active() {
        spotify_sectioned_page(f, m, app, &sp.items, sp.sel, list_focus);
        return;
    }

    // "leading track list + card carousels": an artist page (POPULAR + releases) or
    // search results (SONGS + Albums/Artists/Playlists/Podcasts carousels)
    if app.spotify_carousels_from().is_some() {
        spotify_track_carousels(f, m, app, &sp.items, sp.sel, list_focus);
        return;
    }

    // all-tracks (Liked Songs, a playlist/album drill-in) → the shared track
    // layout (column table or compact rows per `config.track_columns`);
    // mixed/containers (section lists, search) → clean name + subtitle list.
    if sp.items.iter().all(|i| i.kind == Kind::Track) {
        if app.config.track_columns {
            components::spotify_tracks(f, m, app, &sp.items, sp.sel, list_focus);
        } else {
            components::spotify_track_rows(f, m, app, &sp.items, sp.sel, list_focus);
        }
        return;
    }
    // A flat mixed list — a container section shown as a list (e.g. Albums with the
    // grid toggled off). Search + artist pages render as carousels above; single-kind
    // track lists took the branch above. Sticky (not recentring) so clicking a
    // visible row selects it in place instead of jumping to the middle.
    let body_h = m.height as usize;
    let sel = sp.sel.min(n - 1);
    let off = components::sticky_off(&sp.list_off, sel, n, body_h);

    let w = m.width.saturating_sub(2) as usize;
    let mut rows: Vec<Line> = Vec::new();
    for i in off..(off + body_h).min(n) {
        // clickable row: click selects, double-click opens (drill/play) — like ⏎
        app.register_click(
            Rect::new(m.x, m.y + (i - off) as u16, m.width, 1),
            MouseTarget::SpotifyItem(i),
        );
        let it = &sp.items[i];
        let selected = i == sel;
        let on = list_focus && selected;
        let icon = match it.kind {
            Kind::Track => "♪",
            Kind::Album => "◉",
            Kind::Artist => "☻",
            Kind::Playlist => "≡",
            Kind::Show => "▣",
            Kind::Category => "▦",
        };
        let shape = app.config.arabic_shaping;
        // row metadata: artist · album · year / owner · tracks / … (per kind)
        let meta = components::item_meta(it);
        let name = clip(
            &crate::arabic::shaped(&it.name, shape),
            w.saturating_sub(meta.chars().count().min(w / 2) + 4),
        );
        let name_style = if on {
            Style::default()
                .fg(th.text.into())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(th.text.into())
        };
        let mut spans = vec![
            Span::styled(format!("{icon} "), Style::default().fg(th.accent[1].into())),
            Span::styled(name, name_style),
        ];
        // ♥ a show the user already follows, so it's clear which are in Your Shows
        if it.kind == Kind::Show && app.spotify.followed_shows.contains(&it.uri) {
            spans.push(Span::styled(" ♥", Style::default().fg(th.accent[0].into())));
        }
        if !meta.is_empty() {
            spans.push(Span::styled(
                format!("  —  {}", clip(&crate::arabic::shaped(&meta, shape), w / 2)),
                Style::default().fg(th.text_faint.into()),
            ));
        }
        // keep the selected row highlighted (dim) even when the list isn't
        // focused, so the cursor stays visible from the sidebar / a pane
        rows.push(components::pill_line(
            app,
            m.width as usize,
            Line::from(spans),
            components::sel_fill(th, selected, list_focus),
        ));
    }
    f.render_widget(Paragraph::new(rows), m);
}

/// Empty-state for the Podcasts section: explains both why it might be empty
/// (you follow no shows) and the likely real reason on many accounts — Spotify
/// only licenses podcasts in some countries, so the catalog is absent in others
/// (your account's market decides, not Premium). Centered, multi-line.
fn spotify_podcasts_empty(f: &mut Frame, area: Rect, app: &AppState) {
    let th = &app.theme;
    let dim = Style::default().fg(th.text_dim.into());
    let faint = Style::default().fg(th.text_faint.into());
    let title = Style::default()
        .fg(th.accent[0].into())
        .add_modifier(Modifier::BOLD);
    let lines = vec![
        Line::from(Span::styled("▣  No podcasts here", title)),
        Line::raw(""),
        Line::from(Span::styled(
            "Follow shows in the Spotify app — they'll show up here —",
            dim,
        )),
        Line::from(Span::styled("or search for one by name.", dim)),
        Line::raw(""),
        Line::from(Span::styled(
            "Heads up: Spotify only offers podcasts in some countries.",
            faint,
        )),
        Line::from(Span::styled(
            "If this stays empty and searches find none, your account's",
            faint,
        )),
        Line::from(Span::styled(
            "region likely doesn't include podcasts (Premium doesn't change",
            faint,
        )),
        Line::from(Span::styled("this — the account's market does).", faint)),
    ];
    let h = lines.len() as u16;
    let y = area.y + area.height.saturating_sub(h) / 3;
    f.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        Rect::new(area.x, y, area.width, h.min(area.height)),
    );
}

/// Render sectioned browse (the Home feed, or a category's playlist shelves) as
/// stacked carousels via the shared `release_grid` — the same renderer + `h`/`l`/
/// `j`/`k` nav as the artist page's release groups, one carousel per shelf title.
/// No leading track list — the whole area is shelves.
fn spotify_sectioned_page(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    items: &[crate::spotify::api::Item],
    sel: usize,
    focus: bool,
) {
    if items.is_empty() || area.width == 0 || area.height == 0 {
        return;
    }
    let sel = sel.min(items.len() - 1);
    components::release_grid(
        f,
        area,
        app,
        focus,
        components::ReleaseGridData {
            sel,
            cols: &app.spotify.cols,
            car_off: &app.spotify.car_off,
            car_key: &app.spotify.car_key,
            rows_at: &|| app.spotify_browse_rows(),
            card_at: &|idx| {
                let it = &items[idx];
                components::GridCard {
                    name: it.name.clone(),
                    subtitle: components::grid_card_subtitle(&it.subtitle),
                    year: it.year,
                    art: it.image.as_ref().map(|u| {
                        (
                            crate::artwork::ArtKey::remote(u),
                            crate::artwork::ArtSource::Url(u.clone()),
                        )
                    }),
                    followed: false,
                    tint: it.tint,
                }
            },
            click: &|idx| MouseTarget::SpotifyItem(idx),
        },
    );
}

/// Render a "leading track list + card carousels" page: a list pinned at the top
/// (the artist page's POPULAR tracks, or search's SONGS), then the card groups
/// (artist releases, or search's Albums/Artists/Playlists/Podcasts) as Netflix-style
/// carousels via the shared `release_grid`. Items are a flat slice; `[0..from)` are
/// the leading tracks, `[from..]` the cards. Shared by the artist page + search.
fn spotify_track_carousels(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    items: &[crate::spotify::api::Item],
    sel: usize,
    focus: bool,
) {
    let w = area.width.saturating_sub(2) as usize;
    let Some(from) = app.spotify_carousels_from() else {
        return;
    };
    if items.is_empty() || w == 0 || area.height == 0 {
        return;
    }
    let sel = sel.min(items.len() - 1);
    let header = if app.spotify_on_artist_page() {
        crate::app::release::POPULAR_HEADER
    } else {
        "SONGS"
    };

    // the leading track list, via the SHARED region renderer (honours track_columns)
    let bottom = area.y + area.height;
    let popular: Vec<components::PopularTrack> = items
        .iter()
        .take(from)
        .enumerate()
        .map(|(i, it)| components::PopularTrack {
            idx: i,
            name: it.name.clone(),
            artist: it.subtitle.clone(),
            album: it.album.clone(),
            year: it.year,
            duration_ms: it.duration_ms,
            now_playing: components::is_now_playing(app, it),
        })
        .collect();
    let used = components::render_popular_region(
        f,
        Rect::new(area.x, area.y, area.width, area.height),
        app,
        header,
        &popular,
        sel,
        focus,
        &MouseTarget::SpotifyItem,
    );
    let y = area.y + used;

    // the card groups as cover carousels below
    if from < items.len() && y < bottom {
        let region = Rect::new(area.x, y, area.width, bottom - y);
        components::release_grid(
            f,
            region,
            app,
            focus && sel >= from,
            components::ReleaseGridData {
                sel,
                cols: &app.spotify.cols,
                car_off: &app.spotify.car_off,
                car_key: &app.spotify.car_key,
                rows_at: &|| app.spotify_carousel_rows(),
                card_at: &|idx| {
                    let it = &items[idx];
                    components::GridCard {
                        name: it.name.clone(),
                        // subtitle stays the artist only; the year rides on the title
                        // line (right-aligned) via `render_card`.
                        subtitle: components::grid_card_subtitle(&it.subtitle),
                        year: it.year,
                        art: it.image.as_ref().map(|u| {
                            (
                                crate::artwork::ArtKey::remote(u),
                                crate::artwork::ArtSource::Url(u.clone()),
                            )
                        }),
                        followed: false,
                        tint: it.tint,
                    }
                },
                click: &|idx| MouseTarget::SpotifyItem(idx),
            },
        );
    }
}

/// The connection/auth panel (a smooth in-TUI login) shown until connected.
pub(in crate::ui::views) fn spotify_auth(f: &mut Frame, area: Rect, app: &AppState) {
    use crate::spotify::ConnState;
    let th = &app.theme;
    let inner = components::panel(f, area, app, "SPOTIFY", true);
    if inner.height < 3 || inner.width < 4 {
        return;
    }
    // a not-registered error gets the dedicated, tidy card (shared with the browse
    // view's empty-note) rather than a raw one-liner
    if let ConnState::Error { msg } = &app.spotify.conn
        && msg.as_str() == crate::spotify::api::NOT_REGISTERED_MSG
    {
        spotify_not_registered_guide(f, inner, app);
        return;
    }
    // the shared app was rejected (invalid_client) and there's no private client id
    // yet → the step-by-step setup guide rather than a bare error line
    if let ConnState::Error { msg } = &app.spotify.conn
        && msg.contains("invalid_client")
        && app.config.spotify_client_id.is_empty()
    {
        spotify_client_id_guide(f, inner, app);
        return;
    }
    let accent = |s: &str| {
        Span::styled(
            s.to_string(),
            Style::default()
                .fg(th.accent[0].into())
                .add_modifier(Modifier::BOLD),
        )
    };
    let dim = |s: &str| Span::styled(s.to_string(), Style::default().fg(th.text_dim.into()));
    let faint = |s: &str| Span::styled(s.to_string(), Style::default().fg(th.text_faint.into()));

    let mut lines: Vec<Line> = Vec::new();
    match &app.spotify.conn {
        ConnState::Disconnected => {
            lines.push(Line::from(accent("● Spotify")));
            lines.push(Line::raw(""));
            lines.push(Line::from(dim("Connect your Spotify Premium account.")));
            lines.push(Line::from(faint(
                "A browser opens once to log in; the token is stored only on this machine.",
            )));
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled(
                "  ⏎  Log in with Spotify  ",
                Style::default()
                    .fg(th.bg.into())
                    .bg(th.accent[0].into())
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::raw(""));
            lines.push(Line::from(faint(
                "Using your own client id? Its app needs redirect URI",
            )));
            lines.push(Line::from(Span::styled(
                "http://127.0.0.1:8898/login",
                Style::default().fg(th.text_dim.into()),
            )));
        }
        ConnState::Connecting { url } => {
            const FR: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let sp = FR[(app.tick as usize / 2) % FR.len()];
            match url {
                Some(u) => {
                    lines.push(Line::from(vec![
                        accent(sp),
                        Span::raw("  "),
                        dim("Waiting for you to authorize in your browser…"),
                    ]));
                    lines.push(Line::raw(""));
                    // blank Spotify page? usually the app's redirect URI isn't
                    // registered (or an adblocker is blocking accounts.spotify.com)
                    lines.push(Line::from(faint(
                        "Your Spotify app must register this exact redirect URI:",
                    )));
                    lines.push(Line::from(Span::styled(
                        "http://127.0.0.1:8898/login",
                        Style::default().fg(th.text_dim.into()),
                    )));
                    lines.push(Line::raw(""));
                    // keep the auth URL the LAST line — its click hit-box below
                    // is registered at the final row.
                    lines.push(Line::from(faint(
                        "Didn't open? Click the link below (or paste it):",
                    )));
                    lines.push(Line::from(Span::styled(
                        clip(u, inner.width.saturating_sub(4) as usize),
                        Style::default()
                            .fg(th.accent[2].into())
                            .add_modifier(Modifier::UNDERLINED),
                    )));
                }
                None => lines.push(Line::from(vec![
                    accent(sp),
                    Span::raw("  "),
                    dim("Starting login…"),
                ])),
            }
        }
        ConnState::Connected { name, premium } => {
            lines.push(Line::from(vec![
                Span::styled(
                    "✓ ",
                    Style::default()
                        .fg(th.good.into())
                        .add_modifier(Modifier::BOLD),
                ),
                accent(&format!("Connected as {name}")),
            ]));
            lines.push(Line::raw(""));
            if !premium {
                lines.push(Line::from(Span::styled(
                    "⚠ This account isn't Premium — full playback needs Premium.",
                    Style::default().fg(th.warning.into()),
                )));
                lines.push(Line::raw(""));
            }
            lines.push(Line::from(dim(
                "Library, search, and playback arrive in the next phases.",
            )));
            lines.push(Line::from(faint("Press L to log out.")));
        }
        ConnState::Error { msg } => {
            // (the not-registered case is handled by the early return above)
            lines.push(Line::from(Span::styled(
                "✗ Couldn't connect to Spotify",
                Style::default()
                    .fg(th.warning.into())
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::raw(""));
            lines.push(Line::from(dim(msg)));
            lines.push(Line::raw(""));
            lines.push(Line::from(faint("Press ⏎ to try again.")));
        }
        ConnState::Reconnecting { msg } => {
            // a transient network blip — softer than Error; the app is already
            // retrying on its own, so this is informational, not a call to log in
            lines.push(Line::from(Span::styled(
                "↻ Reconnecting to Spotify…",
                Style::default()
                    .fg(th.accent[0].into())
                    .add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::raw(""));
            lines.push(Line::from(dim(msg)));
            lines.push(Line::raw(""));
            lines.push(Line::from(faint(
                "Retrying automatically — press ⏎ to retry now.",
            )));
        }
    }

    let top = inner.y + inner.height.saturating_sub(lines.len() as u16) / 3;
    let body = Rect::new(
        inner.x + 1,
        top.min(inner.y + inner.height - 1),
        inner.width.saturating_sub(2),
        inner.height.saturating_sub(top - inner.y),
    );
    // make the auth URL clickable — it's the last (centered) line in the
    // Connecting state; the click opens the *full* URL (the panel shows it clipped).
    if let ConnState::Connecting { url: Some(u) } = &app.spotify.conn {
        let urlw = clip(u, inner.width.saturating_sub(4) as usize)
            .chars()
            .count() as u16;
        let row = body.y + lines.len().saturating_sub(1) as u16;
        if urlw > 0 && row < inner.y + inner.height {
            let x = body.x + body.width.saturating_sub(urlw) / 2;
            app.register_click(Rect::new(x, row, urlw, 1), MouseTarget::OpenSpotifyAuthUrl);
        }
    }
    f.render_widget(
        // wrap so a long error (e.g. the session-expired hint) flows across lines,
        // centred, instead of stretching off-screen on one line
        Paragraph::new(lines)
            .alignment(Alignment::Center)
            .wrap(ratatui::widgets::Wrap { trim: true }),
        body,
    );
}
