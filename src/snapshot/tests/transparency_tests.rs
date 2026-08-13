//! Transparency: what the *rendered cells* carry as a background.
//!
//! This is the one property that decides whether a transparent terminal window
//! stays transparent. Ghostty and kitty apply their window opacity only to cells
//! left at the terminal's DEFAULT background; a cell painted with an explicit
//! colour is composited opaque, whatever `background-opacity` says. So asserting
//! on the palette or on glyphs proves nothing here — only the cell's background
//! colour does. (iTerm2 blends the whole window instead, which is why the bug
//! was invisible there and reported as "works on iTerm, not Ghostty".)

use super::*;
use ratatui::style::Color;

/// Every layout, every cell: with transparency on, nothing may paint a
/// background except the highlights that are *meant* to be painted.
#[test]
fn transparent_mode_leaves_the_canvas_at_the_terminal_background() {
    let mut app = demo();
    app.config.transparent = true;
    app.reapply_transparency();

    for layout in [Layout::Dashboard, Layout::FullPlayer, Layout::LibraryFocus] {
        let buf = crate::snapshot::render_buffer(&mut app, layout, 100, 30);
        let painted = buf
            .content()
            .iter()
            .filter(|c| c.bg != Color::Reset)
            .count();
        let total = buf.content().len();
        // Highlights (the selection pill, the now-playing row, accent bars) still
        // paint, so this is a proportion rather than zero — but the canvas, the
        // panels and the status bar are the overwhelming majority of the screen.
        assert!(
            painted * 4 < total,
            "{layout:?}: {painted}/{total} cells still paint a background — a \
             transparent window would stay opaque"
        );
    }
}

/// The top-left cell is a surface — the canvas, or the panel that covers it —
/// never a highlight, so it is the sharpest single probe for the setting.
#[test]
fn the_base_canvas_paints_nothing_when_transparent() {
    let mut app = demo();
    app.config.transparent = true;
    app.reapply_transparency();
    let buf = crate::snapshot::render_buffer(&mut app, Layout::Dashboard, 80, 24);
    assert_eq!(buf[(0, 0)].bg, Color::Reset);
}

/// …and the default is unchanged: opaque, painted with a theme colour.
#[test]
fn opaque_by_default() {
    let mut app = demo();
    assert!(!app.config.transparent);
    let buf = crate::snapshot::render_buffer(&mut app, Layout::Dashboard, 80, 24);
    assert!(
        is_theme_surface(&app, buf[(0, 0)].bg),
        "the default must paint a surface colour, got {:?}",
        buf[(0, 0)].bg
    );
}

/// Whether `c` is one of the theme's two surface fills (canvas or panel) — which
/// one covers a given cell is a layout detail, and not what these tests are about.
fn is_theme_surface(app: &crate::app::AppState, c: Color) -> bool {
    c == Color::from(app.theme.bg) || c == Color::from(app.theme.panel)
}

/// The regression that would silently undo the feature: transparency lives on
/// the live `Theme`, so anything that re-resolves a theme (switching palette,
/// the follow-system light/dark swap, the album-art accent) has to stamp it back
/// on. It is a setting, not a palette value — a theme file can't carry it.
#[test]
fn a_theme_switch_keeps_transparency() {
    let mut app = demo();
    app.config.transparent = true;
    app.reapply_transparency();
    assert!(app.theme.transparent);

    app.apply_theme("cyberpunk");
    assert!(
        app.theme.transparent,
        "switching palette dropped the transparency setting"
    );
    let buf = crate::snapshot::render_buffer(&mut app, Layout::Dashboard, 80, 24);
    assert_eq!(buf[(0, 0)].bg, Color::Reset);
}

/// Toggling it off restores the painted background live, without a restart.
#[test]
fn turning_it_off_paints_again() {
    let mut app = demo();
    app.config.transparent = true;
    app.reapply_transparency();
    app.config.transparent = false;
    app.reapply_transparency();
    let buf = crate::snapshot::render_buffer(&mut app, Layout::Dashboard, 80, 24);
    assert!(
        is_theme_surface(&app, buf[(0, 0)].bg),
        "toggling transparency off must paint again, got {:?}",
        buf[(0, 0)].bg
    );
}

/// The setting has to be reachable without editing config.toml by hand — it is
/// the answer to "why isn't my transparent terminal transparent", so it must be
/// findable in the Theme group where the rest of the look lives.
#[test]
fn the_setting_is_offered_in_the_theme_group() {
    use crate::app::Setting;
    let app = demo();
    assert!(
        app.settings_items().contains(&Setting::Transparent),
        "the row is not offered anywhere in Settings"
    );
    assert_eq!(Setting::Transparent.group(), "Theme");
    let (label, _) =
        crate::ui::views::settings_rows::setting_label_value(&app, &Setting::Transparent);
    assert!(
        label.to_lowercase().contains("transparent"),
        "the row must name itself: {label}"
    );
}

/// Toggling the row flips the setting, persists it, AND re-resolves the theme —
/// without the last step the change wouldn't show until a restart.
#[test]
fn toggling_the_row_takes_effect_immediately() {
    use crate::app::Setting;
    let dir = std::env::temp_dir().join("lyrfin-transparency-toggle");
    let _ = std::fs::remove_dir_all(&dir);
    let mut a = crate::app::AppState::new(Config {
        dir,
        ..Default::default()
    });
    a.seed_demo();
    assert!(!a.theme.transparent);
    a.activate_setting(Setting::Transparent);
    assert!(a.config.transparent, "the setting flipped");
    assert!(a.theme.transparent, "…and the live theme picked it up");
}
