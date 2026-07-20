//! First-run onboarding. When the library is empty (a fresh install, before any
//! music is scanned) the track list is replaced by a friendly welcome panel that
//! teaches how to add local music, connect Spotify, and navigate — instead of the
//! fabricated demo tracks that used to be seeded. The keys quoted here are the
//! built-in defaults (`src/keymap/catalog.rs`): `;` view settings, `7` Spotify,
//! `?` all keys.

use super::*;
use crate::app::AppState;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Draw the centered welcome / onboarding panel inside `area`. The whole block is
/// centered as a unit (so the `label → action` columns stay aligned), clipped if
/// `area` is too small.
pub fn welcome(f: &mut Frame, area: Rect, app: &AppState) {
    if area.width < 24 || area.height < 6 {
        return;
    }
    let th = &app.theme;
    let dim = Style::default().fg(col(th.text_dim));
    let faint = Style::default().fg(col(th.text_faint));
    let title = Style::default()
        .fg(col(th.accent[0]))
        .add_modifier(Modifier::BOLD);
    let section = Style::default()
        .fg(col(th.text_dim))
        .add_modifier(Modifier::BOLD);
    let key = Style::default().fg(col(th.accent[0]));

    // "▸ label……… action" — label padded so the action column lines up.
    let row = |label: &str, action: &str| {
        Line::from(vec![
            Span::styled("  ▸ ", faint),
            Span::styled(format!("{label:<21} "), dim),
            Span::styled(action.to_string(), key),
        ])
    };
    // one nav hint: a highlighted key followed by its faint label
    let hint = |k: &str, label: &str| {
        vec![
            Span::styled(format!("{k} "), key),
            Span::styled(format!("{label}   "), dim),
        ]
    };

    let lines = vec![
        Line::from(Span::styled("♫  Welcome to lyrfin", title)),
        Line::from(""),
        Line::from(Span::styled(
            "Your library is empty — let's add some music.",
            dim,
        )),
        Line::from(""),
        Line::from(Span::styled("ADD LOCAL MUSIC", section)),
        row("Start with a folder", "lyrfin ~/Music"),
        row("In the app", ";  →  Library  →  ＋ Add folder"),
        row("Edit the config", "~/.config/lyrfin/config.toml"),
        Line::from(""),
        Line::from(Span::styled("STREAM FROM SPOTIFY", section)),
        row("Open Spotify", "press  7   (Premium account)"),
        Line::from(""),
        Line::from(Span::styled("FIND YOUR WAY", section)),
        Line::from(
            [
                hint("j/k", "move"),
                hint("/", "search"),
                hint("Tab", "panes"),
                hint("⏎", "play"),
            ]
            .concat(),
        ),
        Line::from(
            [
                hint("1–7", "views"),
                hint(";", "settings"),
                hint("?", "all keys"),
            ]
            .concat(),
        ),
        Line::from(""),
        Line::from(Span::styled("IF SYMBOLS LOOK WRONG", section)),
        // A terminal can't be asked which glyphs its font has, so this can't be
        // detected — point at the picker, which renders every set for comparison.
        Line::from(vec![
            Span::styled("  ▸ ", faint),
            Span::styled(format!("{:<21} ", "These should be icons"), dim),
            Span::styled(crate::icons::Icons::sample(&app.config.icon_set), key),
        ]),
        row("Boxes instead?", ";  →  Transport icons"),
    ];

    let block_w = lines.iter().map(Line::width).max().unwrap_or(0) as u16;
    let block_h = lines.len() as u16;
    let w = block_w.min(area.width);
    let h = block_h.min(area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    f.render_widget(Paragraph::new(lines), Rect::new(x, y, w, h));
}
