//! Spotify error/onboarding guide cards: the centered, actionable help shown
//! when Spotify rate-limits the client id, and when the signed-in account
//! isn't allow-listed on the dev app (403 "not registered"). Both are shared
//! between the connected browse pane (as an empty-note) and the auth panel, so
//! they render identically in either place. The dashboard URL is a clickable
//! mouse target.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{AppState, MouseTarget};

/// Shown when Spotify rate-limits the shared client id: an actionable guide to
/// switching to a private (unlimited) Spotify app id.
pub(super) fn spotify_ratelimit_guide(f: &mut Frame, area: Rect, app: &AppState) {
    let th = &app.theme;
    let custom = !app.config.spotify_client_id.is_empty();
    let warn = |s: &str| {
        Span::styled(
            s.to_string(),
            Style::default()
                .fg(th.warning.into())
                .add_modifier(Modifier::BOLD),
        )
    };
    let dim = |s: &str| Span::styled(s.to_string(), Style::default().fg(th.text_dim.into()));
    let faint = |s: &str| Span::styled(s.to_string(), Style::default().fg(th.text_faint.into()));
    let hi = |s: &str| Span::styled(s.to_string(), Style::default().fg(th.accent[2].into()));
    let step = |n: &str, rest: Vec<Span<'static>>| {
        let mut v = vec![Span::styled(
            format!("  {n}  "),
            Style::default()
                .fg(th.accent[0].into())
                .add_modifier(Modifier::BOLD),
        )];
        v.extend(rest);
        Line::from(v)
    };

    let mut lines: Vec<Line> = Vec::new();
    if custom {
        lines.push(Line::from(warn("⚠ Rate-limited even with your client id")));
        lines.push(Line::raw(""));
        lines.push(Line::from(dim(
            "Spotify is throttling requests. Wait a minute, then press ⏎ to retry.",
        )));
    } else {
        lines.push(Line::from(warn("⚠ Spotify rate-limited the shared app id")));
        lines.push(Line::raw(""));
        lines.push(Line::from(dim(
            "Use your own free Spotify app for your own (generous) quota:",
        )));
        lines.push(Line::raw(""));
        let link = Span::styled(
            "https://developer.spotify.com/dashboard",
            Style::default()
                .fg(th.accent[2].into())
                .add_modifier(Modifier::UNDERLINED),
        );
        lines.push(step("1", vec![dim("Open "), link, dim("  (click)")]));
        lines.push(step(
            "2",
            vec![dim("Redirect URI:  "), hi("http://127.0.0.1:8898/login")],
        ));
        lines.push(step("3", vec![dim("Copy the app's Client ID")]));
        lines.push(step(
            "4",
            vec![
                dim("Press "),
                hi("c"),
                dim(" here, paste the id, press "),
                hi("⏎"),
            ],
        ));
        lines.push(Line::raw(""));
        lines.push(Line::from(faint(
            "lyrfin saves it and logs in with your app — no file editing, no restart.",
        )));
        lines.push(Line::from(faint(
            "(Or just wait a few minutes — shared limits reset on their own.)",
        )));
    }

    let h = lines.len() as u16;
    let w = 66u16.min(area.width.saturating_sub(2));
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = (area.y + area.height.saturating_sub(h) / 3).max(area.y);
    // step 1 (the dashboard link) is line index 4 — make that row clickable
    if !custom && y + 4 < area.y + area.height {
        app.register_click(Rect::new(x, y + 4, w, 1), MouseTarget::OpenSpotifyDashboard);
    }
    f.render_widget(
        Paragraph::new(lines),
        Rect::new(x, y, w, h.min(area.height)),
    );
}

/// Centered, tidy guidance for the 403 "not registered for this application"
/// state (the signed-in account isn't on the dev app's allow-list). Shown both as
/// the connected browse view's empty-note and on the auth panel, so they match.
/// The dashboard URL is a clickable mouse target; short lines so nothing overflows.
pub(super) fn spotify_not_registered_guide(f: &mut Frame, area: Rect, app: &AppState) {
    let th = &app.theme;
    let warn = |s: &str| {
        Span::styled(
            s.to_string(),
            Style::default()
                .fg(th.warning.into())
                .add_modifier(Modifier::BOLD),
        )
    };
    let dim = |s: &str| Span::styled(s.to_string(), Style::default().fg(th.text_dim.into()));
    let faint = |s: &str| Span::styled(s.to_string(), Style::default().fg(th.text_faint.into()));
    let hi = |s: &str| {
        Span::styled(
            s.to_string(),
            Style::default()
                .fg(th.accent[2].into())
                .add_modifier(Modifier::BOLD),
        )
    };
    let dash = "developer.spotify.com/dashboard";

    // index 6 is the dashboard URL line (kept in sync with the literal below) — it's
    // the row made clickable.
    const URL_IDX: u16 = 6;
    let lines: Vec<Line> = vec![
        Line::from(warn("⚠  This account isn't on your Spotify app")), // 0
        Line::raw(""),                                                 // 1
        Line::from(dim("Login worked, but this account isn't allow-listed on")), // 2
        Line::from(dim(
            "your dev app — browsing is blocked. Playback still works.",
        )), // 3
        Line::raw(""),                                                 // 4
        Line::from(dim("Add it on the dashboard:")),                   // 5
        Line::from(Span::styled(
            dash,
            Style::default()
                .fg(th.accent[2].into())
                .add_modifier(Modifier::UNDERLINED),
        )), // 6 ← URL
        Line::from(faint("(your app → Users)")),                       // 7
        Line::raw(""),                                                 // 8
        Line::from(vec![
            dim("— or press "),
            hi("c"),
            dim(" to set this account's own client id —"),
        ]), // 9
    ];

    let h = lines.len() as u16;
    let w = 60u16.min(area.width.saturating_sub(2));
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = (area.y + area.height.saturating_sub(h) / 3).max(area.y);
    // the dashboard line is centered within the box → make exactly it clickable
    let row = y + URL_IDX;
    if row < area.y + area.height {
        let urlw = dash.chars().count() as u16;
        let ux = x + w.saturating_sub(urlw) / 2;
        app.register_click(
            Rect::new(ux, row, urlw, 1),
            MouseTarget::OpenSpotifyDashboard,
        );
    }
    f.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        Rect::new(x, y, w, h.min(area.height)),
    );
}

/// Shown on a connect failure that needs a private client id — the shared app was
/// rejected (`invalid_client`) and none is configured yet. The actionable 4-step
/// setup, with a clickable dashboard link and the `c` paste shortcut (same steps as
/// the rate-limit guide, framed for the rejection case).
pub(super) fn spotify_client_id_guide(f: &mut Frame, area: Rect, app: &AppState) {
    let th = &app.theme;
    let warn = |s: &str| {
        Span::styled(
            s.to_string(),
            Style::default()
                .fg(th.warning.into())
                .add_modifier(Modifier::BOLD),
        )
    };
    let dim = |s: &str| Span::styled(s.to_string(), Style::default().fg(th.text_dim.into()));
    let faint = |s: &str| Span::styled(s.to_string(), Style::default().fg(th.text_faint.into()));
    let hi = |s: &str| {
        Span::styled(
            s.to_string(),
            Style::default()
                .fg(th.accent[2].into())
                .add_modifier(Modifier::BOLD),
        )
    };
    let step = |n: &str, rest: Vec<Span<'static>>| {
        let mut v = vec![Span::styled(
            format!("  {n}  "),
            Style::default()
                .fg(th.accent[0].into())
                .add_modifier(Modifier::BOLD),
        )];
        v.extend(rest);
        Line::from(v)
    };
    let link = Span::styled(
        "https://developer.spotify.com/dashboard",
        Style::default()
            .fg(th.accent[2].into())
            .add_modifier(Modifier::UNDERLINED),
    );
    // step 1 (the dashboard link) is at this line index — made clickable below.
    const LINK_IDX: u16 = 5;
    let lines: Vec<Line> = vec![
        Line::from(warn("⚠  Couldn't connect — set up your own Spotify app")), // 0
        Line::raw(""),                                                         // 1
        Line::from(dim("The shared app id was rejected. Use your own free app")), // 2
        Line::from(dim("— unlimited, and it enables saving Liked Songs:")),    // 3
        Line::raw(""),                                                         // 4
        step("1", vec![dim("Open "), link, dim("  (click)")]),                 // 5 ← link
        step(
            "2",
            vec![dim("Redirect URI:  "), hi("http://127.0.0.1:8898/login")],
        ), // 6
        step("3", vec![dim("Copy the app's Client ID")]),                      // 7
        step(
            "4",
            vec![
                dim("Press "),
                hi("c"),
                dim(" here, paste it, press "),
                hi("⏎"),
            ],
        ), // 8
        Line::raw(""),                                                         // 9
        Line::from(faint(
            "lyrfin saves it and logs in with your app — no file editing, no restart.",
        )), // 10
        Line::from(faint("(Or press ⏎ to retry the shared app.)")),            // 11
    ];

    let h = lines.len() as u16;
    let w = 68u16.min(area.width.saturating_sub(2));
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = (area.y + area.height.saturating_sub(h) / 3).max(area.y);
    let row = y + LINK_IDX;
    if row < area.y + area.height {
        // steps are left-aligned, so register the whole row for the link click
        app.register_click(Rect::new(x, row, w, 1), MouseTarget::OpenSpotifyDashboard);
    }
    f.render_widget(
        Paragraph::new(lines),
        Rect::new(x, y, w, h.min(area.height)),
    );
}
