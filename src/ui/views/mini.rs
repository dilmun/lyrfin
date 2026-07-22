//! The mini layout: one pane at a time as a full-width card, for terminals too
//! narrow to dock panes side by side — see [`crate::ui::breakpoint`].
//!
//! The wide layout has two independent axes: the *pane* axis (`focus` — sidebar,
//! main, queue/artist/lyrics) and the *drill* axis (the browse history in
//! `app::nav`). With only one pane on screen those flatten into a single sequence
//! of cards that Back and Forward walk:
//!
//! ```text
//! [Sidebar] ←→ [Main depth 0] ←→ [Main depth 1] ←→ …
//!                  Albums            Album tracks
//! ```
//!
//! Card identity needs no new state: `app.focus` already names the region on
//! screen, so this module renders whatever it points at and the input layer moves
//! it. Dock panes (Queue / Artist / Lyrics) are *lateral* rather than deeper —
//! their existing toggle keys make them the current card, and Esc returns to Main.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{AppState, Focus, Layout as AppLayout, Panel};
use crate::ui::breakpoint;
use crate::ui::components;

/// Whether the mini card layout is in effect, from the last rendered frame size.
///
/// The single source of truth, shared by the render layer and the input layer —
/// `h`/`l` mean "walk the cards" only when cards are actually on screen, so both
/// must agree. Reads `app.frame` rather than taking a rect so the keymap, which
/// has no frame, can ask the same question.
///
/// Only the pane-based browse views take part. Now Playing, Lyrics and Concert are
/// already single-surface views that own their whole body and degrade on their own
/// — there is no pane stack to flatten and no browse history to walk, so putting
/// them behind a card shell would add chrome without adding reach.
pub fn active(app: &AppState) -> bool {
    breakpoint::is_mini(app.frame.get())
        && matches!(
            app.layout,
            AppLayout::Dashboard | AppLayout::LibraryFocus | AppLayout::Radio | AppLayout::Spotify
        )
}

/// Draw the current card plus the trail line above it.
pub fn mini(f: &mut Frame, area: Rect, app: &AppState) {
    if area.height < 3 || area.width < 4 {
        return; // no room for a bordered card — draw nothing rather than garbage
    }
    let [trail_row, body] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(2)]).areas(area);
    trail(f, trail_row, app);
    match app.focus {
        Focus::Pane(p) => pane_card(f, body, app, p),
        Focus::Sidebar => sidebar_card(f, body, app),
        // Search is a mode *within* the main card, not a card of its own
        Focus::Main | Focus::Search => main_card(f, body, app),
    }
}

/// The trail line: where this card sits, and which way it can move.
///
/// `‹` / `›` are drawn only when that direction actually goes somewhere, so the
/// line doubles as the affordance for keys that have no visible chrome otherwise.
fn trail(f: &mut Frame, area: Rect, app: &AppState) {
    let th = &app.theme;
    let dim = Style::default().fg(th.text_faint.into());
    let lit = Style::default().fg(th.text_dim.into());

    let back = can_go_back(app);
    let mut spans = vec![Span::styled(
        if back { "‹ " } else { "  " },
        if back { lit } else { dim },
    )];
    spans.push(Span::styled(card_label(app), lit));

    let fwd = can_go_forward(app);
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let arrow = if fwd { "› " } else { "" };
    // right-align the forward arrow; drop it rather than wrap on a tiny window
    let pad = (area.width as usize).saturating_sub(used + arrow.chars().count());
    if pad > 0 || arrow.is_empty() {
        spans.push(Span::raw(" ".repeat(pad)));
        if fwd {
            spans.push(Span::styled(arrow, lit));
        }
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// A short label for the card on screen — the pane name, or the browse position.
fn card_label(app: &AppState) -> String {
    match app.focus {
        Focus::Pane(p) => p.label().to_string(),
        Focus::Sidebar => match app.layout {
            AppLayout::Radio => "Radio".into(),
            AppLayout::Spotify => "Spotify".into(),
            _ => "Library".into(),
        },
        _ => match app.layout {
            // the Miller columns are a fixed hierarchy — name the current level
            AppLayout::LibraryFocus => match app.browser.col {
                0 => "Artists".into(),
                1 => "Albums".into(),
                _ => "Tracks".into(),
            },
            AppLayout::Radio => app.radio.section.label().to_string(),
            AppLayout::Spotify => app
                .spotify
                .crumb
                .clone()
                .unwrap_or_else(|| app.spotify.section.label().to_string()),
            _ => app
                .local
                .crumb
                .clone()
                .unwrap_or_else(|| app.local.section.label().to_string()),
        },
    }
}

/// Whether Back goes anywhere from the current card: out of a dock pane, up a
/// drill level, or from the main list to the sidebar.
fn can_go_back(app: &AppState) -> bool {
    match app.focus {
        Focus::Pane(_) => true,  // Esc drops back to Main
        Focus::Sidebar => false, // the first card in the sequence
        // The Miller columns are their own card sequence with no sidebar behind
        // them, so column 0 is its first card. Every other view can always retreat
        // to its sidebar, whether or not there's also a drill level to pop.
        _ => app.layout != AppLayout::LibraryFocus || app.browser.col > 0,
    }
}

/// Whether Forward goes anywhere: deeper into the fixed Miller hierarchy, into the
/// main list from the sidebar, or back down a drill level a Back stepped out of.
fn can_go_forward(app: &AppState) -> bool {
    match app.focus {
        Focus::Pane(_) => false,
        Focus::Sidebar => true, // → the main list
        _ => match app.layout {
            AppLayout::LibraryFocus => app.browser.col < 2,
            AppLayout::Spotify => app.spotify.can_forward(),
            AppLayout::Dashboard => app.local.nav.can_forward(),
            _ => false,
        },
    }
}

/// A dock pane as a full-width card. Every pane renderer already takes an arbitrary
/// rect and draws its own border, so they need no mini-specific variant.
fn pane_card(f: &mut Frame, area: Rect, app: &AppState, panel: Panel) {
    let spotify = app.layout == AppLayout::Spotify;
    match panel {
        Panel::Queue if spotify => components::spotify_queue(f, area, app, true),
        Panel::Queue => components::queue(f, area, app),
        Panel::Artist if spotify => components::spotify_artist_panel(f, area, app, true),
        Panel::Artist => components::artist_panel(f, area, app, true),
        Panel::Lyrics => {
            let src = if spotify {
                crate::app::LyricsPane::Spotify
            } else {
                crate::app::LyricsPane::Local
            };
            components::lyrics_panel(f, area, app, true, src);
        }
        // the sidebar reaches here only if focus was left on it as a dock pane
        Panel::Sidebar => sidebar_card(f, area, app),
        // Visualizer is a pane only in Now Playing / Lyrics, which the mini layout
        // doesn't cover — handled for exhaustiveness, not reachability.
        Panel::Visualizer => {
            let inner = components::panel(f, area, app, "VISUALIZER", true);
            components::spectrum_bare(f, inner, app, app.config.player_viz_mode);
        }
    }
}

/// The section list as a full-width card.
fn sidebar_card(f: &mut Frame, area: Rect, app: &AppState) {
    match app.layout {
        AppLayout::Spotify => {
            let inner = components::panel(f, area, app, "LIBRARY", true);
            super::spotify_view::spotify_sidebar_body(f, inner, app);
        }
        AppLayout::Radio => {
            let inner = components::panel(f, area, app, "RADIO", true);
            super::radio_view::radio_sidebar(f, inner, app);
        }
        _ => {
            let inner = components::panel(f, area, app, components::sidebar_title(app), true);
            components::sidebar_body(f, inner, app);
        }
    }
}

/// The view's primary content as a full-width card.
fn main_card(f: &mut Frame, area: Rect, app: &AppState) {
    match app.layout {
        // the Miller browser shows exactly one of its three columns
        AppLayout::LibraryFocus => {
            if app.library.track_count() == 0 {
                components::welcome(f, area, app);
            } else {
                super::browse_column(f, area, app, app.browser.col, true);
            }
        }
        AppLayout::Spotify => {
            if matches!(
                app.spotify.conn,
                crate::spotify::ConnState::Connected { .. }
            ) {
                let title = super::spotify_view::spotify_main_title(app);
                let inner = components::panel(f, area, app, &title, true);
                super::spotify_view::spotify_main_body(f, inner, app);
            } else {
                super::spotify_view::spotify_auth(f, area, app);
            }
        }
        AppLayout::Radio => {
            let title = super::radio_view::radio_main_title(&app.radio);
            let inner = components::panel(f, area, app, &title, true);
            super::radio_view::radio_body(f, inner, app);
        }
        _ => {
            let title = components::tracklist_title(app);
            let inner = components::panel(f, area, app, &title, true);
            components::local_main_body(f, inner, app);
        }
    }
}
