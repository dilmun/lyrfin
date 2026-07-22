//! Presentation layer. Pure render: takes `&AppState`, draws widgets into the
//! ratatui `Frame` — never mutates state.
//!
//! `render` draws the shared chrome (top bar / now-playing bar / status bar)
//! then dispatches the body to the active layout's view. Layouts map 1:1 to
//! `design/mockups/`.

pub mod breakpoint;
pub mod components;
pub mod theme;
pub mod views;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::Block;

use crate::app::{AppState, Layout as AppLayout};

pub fn render(f: &mut Frame, app: &AppState) {
    app.clear_hits(); // rebuild the mouse hit-map for this frame
    let area = f.area();
    // record the frame so the input layer can ask which layout is on screen
    app.frame.set(area);
    // record the modal overlay's rect (if any) *before* the base view renders, so
    // its content-art sites suppress only the inline covers that fall under the
    // overlay — the art beside it stays visible. The overlays drawn at the bottom
    // of this fn size themselves the same way.
    app.overlay_rect.set(active_overlay_rect(app, area));
    // base background
    f.render_widget(
        Block::default().style(Style::default().bg(app.theme.bg.into())),
        area,
    );

    // Now Playing & Lyrics draw their own transport card, and Settings has no
    // need for the playback bar — all three skip the shared now-bar and give the
    // freed space to the body.
    // Mini: too narrow/short to dock panes, so the view becomes a stack of
    // full-width cards under a compact 2-row playback bar. Bypasses the pane
    // docking entirely — `MIN_MAIN_W` structurally reserves a main rect, so a
    // full-width single pane isn't expressible through `PanelCfg`.
    let mini = views::mini::active(app);
    let body = if mini {
        let now_h = components::now_bar_height(app.config.player_viz, area.width, area.height);
        let [body, now, status] = Layout::vertical([
            Constraint::Min(3),
            Constraint::Length(now_h),
            Constraint::Length(1),
        ])
        .areas(area);
        components::now_bar(f, now, app);
        components::status_bar(f, status, app);
        body
    } else if app.layout == AppLayout::Concert {
        // Concert: fullscreen, no shared chrome — the view owns the whole frame
        area
    } else if matches!(
        app.layout,
        // these draw their own playback box (now-playing card / own bar), so they
        // skip the shared now-bar — otherwise the playback box appears twice.
        AppLayout::FullPlayer | AppLayout::LyricsFocus
    ) {
        let [body, status] =
            Layout::vertical([Constraint::Min(6), Constraint::Length(1)]).areas(area);
        components::status_bar(f, status, app);
        body
    } else {
        // playback box: full height (with the visualizer) on tall windows, else
        // shrink to fit so the body keeps its space. The viz auto-collapses when
        // the box is short (see now_bar's vertical split).
        let now_h = components::now_bar_height(app.config.player_viz, area.width, area.height);
        let [body, now, status] = Layout::vertical([
            Constraint::Min(6),        // body
            Constraint::Length(now_h), // now-playing bar
            Constraint::Length(1),     // status bar
        ])
        .areas(area);
        components::now_bar(f, now, app);
        components::status_bar(f, status, app);
        body
    };

    // Global queue panel: dock it on every view (right by default, left if asked)
    // when toggled on — skipped in fullscreen Concert. The tag editor floats over
    // everything (see the overlays below), so it no longer carves the layout.
    let queue_p = app.panel(crate::app::Panel::Queue);
    let body = if !mini
        && queue_p.shown
        // these views dock the queue *inside* their content (above their own
        // playback bar), so the chrome leaves their body intact:
        && !matches!(
            app.layout,
            AppLayout::Concert
                | AppLayout::Dashboard
                | AppLayout::LibraryFocus
                | AppLayout::FullPlayer
                | AppLayout::LyricsFocus
                | AppLayout::Radio
                | AppLayout::Spotify
        )
        && body.width >= 40
    {
        // queue_p.size is a percentage of the body along its dock axis
        let span = components::pane_span(body, queue_p.dock, queue_p.size);
        let (q, main) = components::dock_split(body, queue_p.dock, span, span.min(body.height / 2));
        components::queue(f, q, app);
        main
    } else {
        body
    };

    if mini {
        views::mini::mini(f, body, app);
    } else {
        match app.layout {
            AppLayout::LyricsFocus => views::lyrics(f, body, app),
            AppLayout::FullPlayer => views::nowplaying(f, body, app),
            AppLayout::Dashboard => views::dashboard(f, body, app),
            AppLayout::LibraryFocus => views::library(f, body, app),
            AppLayout::Concert => views::concert(f, body, app),
            AppLayout::Radio => views::radio(f, body, app),
            AppLayout::Spotify => views::spotify(f, body, app),
        }
    }

    // everything below is an overlay drawn on top of the base view — mark the
    // hit-map boundary so base-view clicks are ignored while a modal is open.
    app.mark_overlay_hits();
    // the full Settings, as a centered overlay (command-palette only)
    if app.settings.overlay {
        views::settings_overlay(f, area, app);
    }
    // per-view settings popup overlay (the `;` shortcut) — under the modals below
    if app.settings.popup.is_some() {
        views::settings_popup(f, area, app);
    }
    // the Equalizer overlay (the `e` shortcut / palette) — self-contained modal
    if app.eq_open() {
        views::equalizer_overlay(f, area, app);
    }
    if !app.input.add_targets.is_empty() {
        components::add_playlist_overlay(f, area, app);
    } else if app.input.naming.is_some() {
        // text-entry dialog (new/rename playlist, add folder, bookmark, …) — the
        // add-to-playlist overlay above already hosts its own name field
        components::name_overlay(f, area, app);
    }
    if app.input.confirm_delete.is_some() {
        components::confirm_delete_overlay(f, area, app);
    }
    // Spotify playlist management modals (add/create/rename picker + unfollow)
    if app.spotify.pl_modal.is_some() {
        components::spotify_playlist_overlay(f, area, app);
    }
    if app.spotify.pl_confirm_delete.is_some() {
        components::spotify_confirm_delete_overlay(f, area, app);
    }
    // the unified read-only Info overlay (Keys / Stats / Health / Track tabs)
    if let Some(info) = &app.info {
        components::info_overlay(f, area, app, info);
    }
    // unified Tag Edit modal (Edit / Auto Tag / Cover tabs)
    if app.tags_open() {
        components::tags_overlay(f, area, app);
    }
    if app.palette.is_some() {
        components::command_palette(f, area, app);
    }
}

/// The rect the open modal overlay(s) occupy this frame, or `None` when none is
/// open. Mirrors the draw dispatch at the bottom of [`render`] and each overlay's
/// own sizing so content-art suppression lines up with what's actually drawn. When
/// several stack, returns their bounding union — a safe superset (it can only
/// over-suppress the gap between them, never let an image bleed).
fn active_overlay_rect(app: &AppState, area: Rect) -> Option<Rect> {
    use components::{centered, overlay_dims};
    let mut acc: Option<Rect> = None;
    let mut cover = |r: Rect| acc = Some(acc.map_or(r, |a: ratatui::layout::Rect| a.union(r)));

    // Big centred content overlays (Settings / Info / Tag editor) all share
    // `overlay_dims`; each renders through `overlay_frame` → `centered`.
    if app.settings.overlay || app.info.is_some() || app.tags_open() {
        let (w, h) = overlay_dims(app, area);
        cover(centered(area, w, h));
    }
    // Per-view quick-settings popup (`;`) — the compact tabbed sibling.
    if app.settings.popup.is_some() {
        let (w, h) = views::popup_dims(app, area);
        cover(centered(area, w, h));
    }
    // Equalizer overlay.
    if app.eq_open() {
        let (w, h) = views::eq_dims(app, area);
        cover(centered(area, w, h));
    }
    // Add-to-playlist is content-sized (it can grow tall with many playlists), so
    // it needs its own dims; the naming dialog it hosts is the fixed small size.
    if !app.input.add_targets.is_empty() {
        let (w, h) = components::add_playlist_dims(app, area);
        cover(centered(area, w, h));
    } else if app.input.naming.is_some() {
        let (w, h) = components::dialog_dims(area, 52, 7);
        cover(centered(area, w, h));
    }
    // The fixed 52×7 confirm / picker dialogs.
    if app.input.confirm_delete.is_some()
        || app.spotify.pl_modal.is_some()
        || app.spotify.pl_confirm_delete.is_some()
    {
        let (w, h) = components::dialog_dims(area, 52, 7);
        cover(centered(area, w, h));
    }
    // Command palette — top-anchored (not centred), narrow. Use its max height as a
    // safe superset so a tile just below it can't bleed.
    if app.palette.is_some() {
        let w = 58u16.min(area.width.saturating_sub(4));
        let h = 23u16.min(area.height.saturating_sub(2));
        let x = area.x + area.width.saturating_sub(w) / 2;
        cover(Rect::new(x, area.y + 2, w, h));
    }
    acc
}
