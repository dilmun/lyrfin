//! The standardized "browser shell" shared by every source view (Dashboard,
//! Spotify, Radio, future Apple Music): movable docked panes wrapping a
//! `[ bordered sidebar | bordered titled main ]` core, so they all share one
//! chrome. Presentation only — each view supplies its own titles and body-render
//! closures; the shell owns the docking, borders, and the two-column split.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::app::{AppState, Panel};

/// One bordered column of the shell core. `render` draws into the *inner* rect
/// (inside the border); `focused` highlights the whole border.
pub struct ShellPane<'a> {
    pub title: &'a str,
    /// Optional secondary label floated to the RIGHT end of the title border (e.g.
    /// the connected Spotify account). Dropped automatically when the pane is too
    /// narrow to fit it beside `title` — see [`super::panel_titled`].
    pub title_right: Option<Line<'a>>,
    pub focused: bool,
    pub render: &'a dyn Fn(&mut Frame, Rect, &AppState),
}

/// Render the standard shell. Every movable pane — including the library/section
/// `Sidebar` — docks around `area` via [`super::dock_panels`]; the bordered
/// `main` pane fills the remaining core. Views that have a sidebar simply list
/// `Panel::Sidebar` in `panels` and draw it in their `dock` closure (Dashboard,
/// Spotify, and Radio all do). `dock_panels` owns the responsive collapse: as the terminal shrinks it
/// drops the least-important panes (percentage-sized) one by one, down to a
/// single main pane — so a narrow terminal stays usable.
pub fn browser_shell(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    panels: &[Panel],
    dock: &dyn Fn(&mut Frame, Rect, &AppState, Panel),
    main: ShellPane,
) {
    let core = super::dock_panels(f, area, app, panels, dock);
    // the main pane's slot, for directional focus movement (ctrl+h/j/k/l); the
    // docked panes register themselves inside `dock_panels`
    app.register_focus(core, crate::app::Focus::Main);
    let inner = super::panel_titled(f, core, app, main.title, main.title_right, main.focused);
    (main.render)(f, inner, app);
}

#[cfg(test)]
mod tests {
    use super::super::pane_span;
    use crate::app::Dock;
    use ratatui::layout::Rect;

    #[test]
    fn pane_span_is_a_percentage_of_the_window() {
        let area = Rect::new(0, 0, 200, 50);
        // left/right docks take a % of width; top/bottom a % of height
        assert_eq!(pane_span(area, Dock::Left, 25), 50); // 25% of 200
        assert_eq!(pane_span(area, Dock::Right, 30), 60);
        assert_eq!(pane_span(area, Dock::Top, 20), 10); // 20% of 50
        assert_eq!(pane_span(area, Dock::Bottom, 40), 20);
        // scales with the window: half the width → half the cells
        assert_eq!(pane_span(Rect::new(0, 0, 100, 50), Dock::Left, 25), 25);
    }
}
