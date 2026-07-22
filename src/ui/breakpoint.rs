//! Terminal-size breakpoints.
//!
//! lyrfin's normal response to a shrinking window is to *drop* things: panes
//! collapse by `collapse_rank`, table columns by `drop_rank`, the now-bar hides
//! its art. That degrades well until it runs out of things to drop — at which
//! point the sidebar is gone, so no section can be reached, and the queue /
//! artist / lyrics panes are simply unavailable.
//!
//! Below [`MINI_W`] the UI switches strategy instead: one pane at a time, full
//! width, navigated as a stack of cards (see `ui::views::mini`). Nothing becomes
//! unreachable — it just takes a keypress instead of a glance.

use ratatui::layout::Rect;

/// Width at which the layout switches to the mini card stack.
///
/// Not a round number picked by feel: `Panel::Sidebar` defaults to `size: 22`
/// (a *percentage* of the window, not cells) and a pane collapses below
/// `MIN_PANE_W = 14` cells. `14 / 0.22 ≈ 63.6`, so below ~64 columns the sidebar
/// cannot survive as a percentage pane and the collapse loop drops it. That is
/// exactly where "hide things" should hand over to "card things".
pub const MINI_W: u16 = 64;

/// Whether `area` should render as the mini card stack rather than the docked
/// pane layout. The single source of truth — callers must not re-derive it from
/// the constant, so the threshold stays changeable in one place.
///
/// Deliberately **width-only**. Height barely constrains this layout: the panes
/// that matter dock left/right, so they're bounded by columns, and a short window
/// is already handled by the playback bar shrinking (`now_bar_height`) and the
/// per-pane height guards. Triggering cards on height would take working layouts
/// — Home at 100×12, a docked queue at 90×18 — and flatten them for no gain.
pub fn is_mini(area: Rect) -> bool {
    area.width < MINI_W
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    #[test]
    fn the_threshold_is_exclusive() {
        assert!(
            !is_mini(rect(MINI_W, 40)),
            "exactly at the threshold is not mini"
        );
        assert!(is_mini(rect(MINI_W - 1, 40)), "one column under is mini");
        assert!(!is_mini(rect(200, 60)), "a large window is never mini");
        assert!(is_mini(rect(30, 60)), "narrow and tall → mini");
    }

    #[test]
    fn height_alone_never_triggers_mini() {
        // a short-but-wide window still docks panes fine — they're bounded by
        // columns, not rows. Regression guard for Home at 100×12 and a docked
        // queue at 90×18, both of which render correctly with the full layout.
        assert!(
            !is_mini(rect(100, 12)),
            "short and wide keeps the full layout"
        );
        assert!(
            !is_mini(rect(90, 18)),
            "a docked queue survives a short window"
        );
        assert!(
            !is_mini(rect(200, 4)),
            "even absurdly short stays width-driven"
        );
    }
}
