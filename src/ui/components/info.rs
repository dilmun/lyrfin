//! The unified **Info** overlay — `Keys` / `Stats` / `Health` / `Track` tabs in
//! one tabbed frame. Replaces the old separate help, stats, error-log, and
//! metadata overlay bodies; each is now a body renderer drawing into the frame's
//! content rect. Pure rendering — reads `&AppState` / `&Info`, draws into `f`.

use super::*;
use crate::app::{AppState, Info, InfoTab};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

/// Footer key-hints per tab (the Keys tab types to filter; Health can copy).
fn footer_for(tab: InfoTab) -> &'static [(&'static str, &'static str)] {
    match tab {
        // the Keys tab types to filter, so `f` isn't free there → size stays on `=`
        InfoTab::Keys => &[
            ("⇥", "tab"),
            ("type", "filter"),
            ("=", "size"),
            ("esc", "close"),
        ],
        InfoTab::Health => &[
            ("⇥", "tab"),
            ("j/k", "scroll"),
            ("y", "copy"),
            ("f", "size"),
            ("esc", "close"),
        ],
        _ => &[
            ("⇥", "tab"),
            ("j/k", "scroll"),
            ("f", "size"),
            ("esc", "close"),
        ],
    }
}

/// Draw the Info overlay: the shared tabbed frame + the active tab's body.
pub fn info_overlay(f: &mut Frame, area: Rect, app: &AppState, info: &Info) {
    let tabs = InfoTab::labels();
    let (w, h) = overlay_dims(app, area);
    let inner = overlay_frame(
        f,
        area,
        app,
        w,
        h,
        &FrameSpec {
            title: "Info",
            tabs: &tabs,
            active_tab: info.tab.index(),
            footer: footer_for(info.tab),
        },
    );
    if inner.height == 0 {
        return;
    }
    match info.tab {
        InfoTab::Keys => keys_body(f, inner, app, info),
        InfoTab::Stats => stats_body(f, inner, app, info),
        InfoTab::Health => health_body(f, inner, app, info),
        InfoTab::Track => track_body(f, inner, app, info),
    }
}

/// Render `lines` into `area` with a clamped vertical scroll, recording the
/// overflow in `max` (read back by `AppState::info_scroll`). Shared by every tab.
fn scroll_body(
    f: &mut Frame,
    area: Rect,
    lines: Vec<Line>,
    max: &std::cell::Cell<usize>,
    scroll: usize,
) {
    let h = area.height as usize;
    let max_off = lines.len().saturating_sub(h);
    max.set(max_off);
    let off = scroll.min(max_off);
    let view: Vec<Line> = lines.into_iter().skip(off).take(h).collect();
    f.render_widget(Paragraph::new(view), area);
}

// ---- Keys tab (keybinding search) ----------------------------------------
fn keys_body(f: &mut Frame, area: Rect, app: &AppState, info: &Info) {
    let th = &app.theme;
    let iw = area.width as usize;
    let matches = app.help_matches();
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled(
            "  Search ",
            Style::default()
                .fg(col(th.accent[0]))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(info.keys_query.clone(), Style::default().fg(col(th.text))),
        Span::styled("▌", Style::default().fg(col(th.accent[0]))),
    ]));
    lines.push(Line::raw(""));
    if matches.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no matching keys",
            Style::default().fg(col(th.text_faint)),
        )));
    }
    for (k, d) in &matches {
        let desc = trunc(d, iw.saturating_sub(14));
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {k:<11}"),
                Style::default()
                    .fg(col(th.accent[0]))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(desc, Style::default().fg(col(th.text_dim))),
        ]));
    }
    scroll_body(f, area, lines, &info.keys_max, info.keys_scroll);
}

// ---- Stats tab (library + listening) -------------------------------------
fn stats_body(f: &mut Frame, area: Rect, app: &AppState, info: &Info) {
    let th = &app.theme;
    let s = crate::stats::Stats::compute(app.library.tracks.values());
    let hist = crate::stats::History::compute(&app.play_history, crate::datetime::now_unix());
    let iw = area.width as usize;

    let fmt_dur = |d: std::time::Duration| {
        let secs = d.as_secs();
        let (h, m) = (secs / 3600, (secs % 3600) / 60);
        if h >= 24 {
            format!("{}d {}h", h / 24, h % 24)
        } else {
            format!("{h}h {m}m")
        }
    };
    let head = |t: &str| {
        Line::from(Span::styled(
            format!(" {t}"),
            Style::default()
                .fg(col(th.accent[0]))
                .add_modifier(Modifier::BOLD),
        ))
    };
    let stat = |label: &str, value: String| {
        let fill = iw.saturating_sub(2 + label.chars().count() + value.chars().count());
        Line::from(vec![
            Span::styled(format!("  {label}"), Style::default().fg(col(th.text_dim))),
            Span::raw(" ".repeat(fill)),
            Span::styled(value, Style::default().fg(col(th.text))),
        ])
    };
    let rank = |i: usize, name: &str, n: String| {
        let name = trunc(name, iw.saturating_sub(8 + n.chars().count()));
        Line::from(vec![
            Span::styled(
                format!("  {}. ", i + 1),
                Style::default().fg(col(th.text_faint)),
            ),
            Span::styled(name, Style::default().fg(col(th.text))),
            Span::styled(format!("  {n}"), Style::default().fg(col(th.accent[1]))),
        ])
    };

    let mut lines: Vec<Line> = Vec::new();
    lines.push(head("LIBRARY"));
    lines.push(stat("Tracks", s.tracks.to_string()));
    lines.push(stat("Artists", s.artists.to_string()));
    lines.push(stat("Albums", s.albums.to_string()));
    lines.push(stat("Total length", fmt_dur(s.total_time)));
    lines.push(stat("Lossless", format!("{} / {}", s.lossless, s.tracks)));
    lines.push(stat(
        "Favorites · rated",
        format!("{} · {} ({:.1}★)", s.favorites, s.rated, s.avg_rating),
    ));
    lines.push(Line::raw(""));
    lines.push(head("LISTENING"));
    lines.push(stat("Total plays", s.total_plays.to_string()));
    lines.push(stat("Time listened", fmt_dur(s.played_time)));
    if hist.total > 0 {
        // sparkline of bucket counts using eighth-blocks scaled to the max
        let spark = |counts: &[usize]| -> String {
            const B: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
            let max = counts.iter().copied().max().unwrap_or(0).max(1);
            counts
                .iter()
                .map(|&c| if c == 0 { ' ' } else { B[(c * 7 / max).min(7)] })
                .collect()
        };
        lines.push(stat(
            "Streak (now · best)",
            format!("{} · {} days", hist.current_streak, hist.longest_streak),
        ));
        lines.push(stat("Active days", hist.days_active.to_string()));
        lines.push(stat(
            "Plays 7d · 30d",
            format!("{} · {}", hist.plays_7d, hist.plays_30d),
        ));
        lines.push(stat("Weekday M→S", spark(&hist.by_weekday)));
        lines.push(stat("Hour 0–23", spark(&hist.by_hour)));
    }

    let mut section = |title: &str, rows: &[(String, String)]| {
        if rows.is_empty() {
            return;
        }
        lines.push(Line::raw(""));
        lines.push(head(title));
        for (i, (name, n)) in rows.iter().enumerate() {
            lines.push(rank(i, name, n.clone()));
        }
    };
    let to_rows = |v: &[(String, u32)]| {
        v.iter()
            .map(|(k, n)| (k.clone(), format!("{n} plays")))
            .collect::<Vec<_>>()
    };
    section("TOP ARTISTS", &to_rows(&s.top_artists));
    section("TOP TRACKS", &to_rows(&s.top_tracks));
    section("TOP ALBUMS", &to_rows(&s.top_albums));
    let genre_rows: Vec<(String, String)> = s
        .top_genres
        .iter()
        .map(|(g, c)| (g.clone(), format!("{c}")))
        .collect();
    section("TOP GENRES", &genre_rows);

    scroll_body(f, area, lines, &info.stats_max, info.stats_scroll);
}

// ---- Health tab (health summary + recent errors) -------------------------
/// A short "error type" label for the health summary: the text up to the first
/// `(` or `:` (the full details live in the recent-errors list below), capped so
/// the row stays tidy.
fn error_kind(msg: &str) -> String {
    let cut = msg.find(['(', ':']).unwrap_or(msg.len());
    let head = msg[..cut].trim();
    let head = if head.is_empty() { msg.trim() } else { head };
    if head.chars().count() > 48 {
        format!("{}…", head.chars().take(47).collect::<String>())
    } else {
        head.to_string()
    }
}

fn health_body(f: &mut Frame, area: Rect, app: &AppState, info: &Info) {
    let th = &app.theme;
    let iw = area.width as usize;
    let now = crate::datetime::now_unix();
    let ago = |ts: u64| {
        let s = now.saturating_sub(ts);
        match s {
            0..=59 => format!("{s}s ago"),
            60..=3599 => format!("{}m ago", s / 60),
            3600..=86399 => format!("{}h ago", s / 3600),
            _ => format!("{}d ago", s / 86400),
        }
    };
    let head = |t: &str| {
        Line::from(Span::styled(
            format!(" {t}"),
            Style::default()
                .fg(col(th.accent[0]))
                .add_modifier(Modifier::BOLD),
        ))
    };
    let row = |label: &str, value: String, ok: bool| {
        Line::from(vec![
            Span::styled(
                format!(" {} ", if ok { '●' } else { '✗' }),
                Style::default().fg(col(if ok { th.text_dim } else { th.accent[0] })),
            ),
            Span::styled(
                format!("{label}: "),
                Style::default().fg(col(th.text_faint)),
            ),
            Span::styled(value, Style::default().fg(col(th.text))),
        ])
    };

    let mut lines: Vec<Line> = vec![head("HEALTH")];
    match &app.config.config_error {
        Some(e) => lines.push(row("Config", e.clone(), false)),
        None => lines.push(row("Config", "OK".into(), true)),
    }
    use crate::spotify::ConnState;
    let (sp_ok, sp) = match &app.spotify.conn {
        ConnState::Connected { name, .. } => (true, format!("connected as {name}")),
        ConnState::Connecting { .. } => (true, "connecting…".into()),
        ConnState::Reconnecting { .. } => (true, "reconnecting…".into()),
        ConnState::Disconnected => (true, "not connected".into()),
        ConnState::Error { msg } => (false, error_kind(msg)),
    };
    lines.push(row("Spotify", sp, sp_ok));
    // when connected, a detail row names the exact account + whether it can stream —
    // so a playback failure (Spotify's account-level audio-key block) is pinned to a
    // specific account, not a vague "Spotify won't play".
    if let ConnState::Connected { premium, .. } = &app.spotify.conn {
        let id = app.spotify.account_id.as_deref().unwrap_or("—");
        // only the CONFIRMED account-level block, not a transient audio-key blip
        let blocked = app.spotify_key_block_confirmed();
        let stream = if blocked {
            "streaming BLOCKED (account-level)"
        } else if *premium {
            "streaming ok"
        } else {
            "no streaming — needs Premium"
        };
        lines.push(row(
            "Account",
            format!(
                "{id} · {} · {stream}",
                if *premium { "Premium" } else { "Free" }
            ),
            *premium && !blocked,
        ));
    }
    lines.push(row(
        "Library",
        format!("{} tracks", app.library.tracks.len()),
        true,
    ));
    lines.push(row(
        "Errors logged",
        app.error_log.len().to_string(),
        app.error_log.is_empty(),
    ));
    lines.push(Line::raw(""));
    lines.push(head("RECENT ERRORS (latest 10)"));
    if app.error_log.is_empty() {
        lines.push(Line::from(Span::styled(
            "   none — all good",
            Style::default().fg(col(th.text_faint)),
        )));
    } else {
        for e in app.error_log.iter().rev().take(10) {
            let prefix = format!(" {:>8}  ", ago(e.ts));
            let pad = prefix.chars().count();
            let avail = iw.saturating_sub(pad).max(8);
            for (i, chunk) in e.msg.chars().collect::<Vec<_>>().chunks(avail).enumerate() {
                let text: String = chunk.iter().collect();
                lines.push(Line::from(vec![
                    Span::styled(
                        if i == 0 {
                            prefix.clone()
                        } else {
                            " ".repeat(pad)
                        },
                        Style::default().fg(col(th.text_faint)),
                    ),
                    Span::styled(text, Style::default().fg(col(th.text))),
                ]));
            }
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "   full history in errors.log · y copies the latest",
        Style::default().fg(col(th.text_faint)),
    )));

    scroll_body(f, area, lines, &info.errors_max, info.errors_scroll);
}

// ---- Track tab (current track's tags / format) ---------------------------
fn track_body(f: &mut Frame, area: Rect, app: &AppState, info: &Info) {
    let th = &app.theme;
    // Show the track of the source the now-bar is currently displaying — dispatched
    // by view like `now_bar` — so hitting Info while a Spotify track (or a radio
    // station) is on screen shows THAT, not the frozen local library track. Each
    // source selects its own (label, value) rows; rendering is shared below.
    let pairs = if app.showing_spotify() {
        spotify_track_pairs(app)
    } else if app.showing_radio() {
        radio_track_pairs(app)
    } else {
        local_track_pairs(app)
    };
    if pairs.is_empty() {
        info.track_max.set(0);
        f.render_widget(
            Paragraph::new("  no track playing").style(Style::default().fg(col(th.text_dim))),
            area,
        );
        return;
    }
    let lines: Vec<Line> = pairs
        .into_iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(
                    format!("  {k:<10}"),
                    Style::default()
                        .fg(col(th.text_faint))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(v, Style::default().fg(col(th.text))),
            ])
        })
        .collect();
    let h = area.height as usize;
    let max_off = lines.len().saturating_sub(h);
    info.track_max.set(max_off);
    let off = info.track_scroll.min(max_off);
    let view: Vec<Line> = lines.into_iter().skip(off).take(h).collect();
    f.render_widget(Paragraph::new(view).wrap(Wrap { trim: false }), area);
}

/// (label, value) rows for the local library track — the file's full tag/audio
/// metadata. Empty when nothing is loaded → the caller shows the idle message.
fn local_track_pairs(app: &AppState) -> Vec<(&'static str, String)> {
    let Some(t) = app.current_track() else {
        return Vec::new();
    };
    let a = t.audio;
    vec![
        ("Title", t.title.clone()),
        ("Artist", t.artist.to_string()),
        (
            "Album",
            format!(
                "{} ({})",
                t.album,
                t.year.map(|y| y.to_string()).unwrap_or_else(|| "—".into())
            ),
        ),
        ("Album Art.", t.album_artist.to_string()),
        ("Track", format!("{} · disc {}", t.track_no, t.disc_no)),
        (
            "Genre",
            t.genre
                .as_ref()
                .map(|g| g.to_string())
                .unwrap_or_else(|| "—".into()),
        ),
        (
            "Format",
            a.map(|a| {
                format!(
                    "{:?} · {} Hz · {}-bit · {} ch",
                    a.codec, a.sample_rate, a.bit_depth, a.channels
                )
            })
            .unwrap_or_else(|| "—".into()),
        ),
        (
            "Bitrate",
            a.map(|a| format!("{} kbps", a.bitrate_kbps))
                .unwrap_or_else(|| "—".into()),
        ),
        ("Rating", stars(t.rating)),
        ("Plays", t.play_count.to_string()),
        ("Path", t.path.to_string_lossy().into_owned()),
    ]
}

/// (label, value) rows for the now-playing Spotify item — streamed, so no
/// file/format; the URI stands in for the path. Handles music tracks + episodes.
fn spotify_track_pairs(app: &AppState) -> Vec<(&'static str, String)> {
    let Some(tr) = app.spov.now_spotify.as_ref() else {
        return Vec::new();
    };
    let secs = if app.spov.sp_dur > 0.0 {
        app.spov.sp_dur
    } else {
        tr.duration_ms as f64 / 1000.0
    };
    let length = mmss(std::time::Duration::from_secs_f64(secs.max(0.0)));
    let mut rows = vec![("Source", "Spotify".to_string())];
    if tr.uri.contains(":episode:") {
        // an episode's `subtitle` is the publisher and `album` is the show name
        rows.push(("Episode", tr.name.clone()));
        rows.push(("Show", tr.album.clone()));
        rows.push(("Publisher", tr.subtitle.clone()));
    } else {
        rows.push(("Title", tr.name.clone()));
        rows.push(("Artist", tr.subtitle.clone()));
        rows.push((
            "Album",
            match tr.year {
                Some(y) => format!("{} ({y})", tr.album),
                None => tr.album.clone(),
            },
        ));
    }
    rows.push(("Length", length));
    rows.push(("Link", tr.uri.clone()));
    rows
}

/// (label, value) rows for the tuned radio station — a live stream (no fixed
/// length; the ICY "now playing" title when the stream sends one).
fn radio_track_pairs(app: &AppState) -> Vec<(&'static str, String)> {
    let Some(st) = app.rnow.now_station.as_ref() else {
        return Vec::new();
    };
    let mut rows = vec![
        ("Source", "Radio".to_string()),
        ("Station", st.name.clone()),
    ];
    if let Some(title) = &app.rnow.now_station_title {
        rows.push(("Now Playing", title.clone()));
    }
    let genre = st.genre();
    if !genre.is_empty() {
        rows.push(("Genre", genre.to_string()));
    }
    let country = if !st.country.is_empty() {
        st.country.clone()
    } else {
        st.countrycode.clone()
    };
    if !country.is_empty() {
        rows.push(("Country", country));
    }
    if !st.codec.is_empty() || st.bitrate > 0 {
        rows.push(("Format", format!("{} · {}k", st.codec, st.bitrate)));
    }
    let link = if !st.homepage.is_empty() {
        st.homepage.clone()
    } else {
        st.url.clone()
    };
    if !link.is_empty() {
        rows.push(("Link", link));
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::error_kind;

    #[test]
    fn error_kind_is_just_the_type_not_the_details() {
        assert_eq!(
            error_kind(
                "Session expired (Spotify error 400: invalid_client). Set your own Client ID"
            ),
            "Session expired"
        );
        assert_eq!(
            error_kind("Couldn't update Liked Songs: 411 Length Required"),
            "Couldn't update Liked Songs"
        );
        assert_eq!(error_kind("network down"), "network down");
        let long = "x".repeat(80);
        let k = error_kind(&long);
        assert!(k.chars().count() <= 48 && k.ends_with('…'));
    }
}
