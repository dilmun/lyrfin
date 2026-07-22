//! Headless frame rendering via ratatui's `TestBackend`. Used to verify layouts
//! without a TTY and as the basis for snapshot tests (M5).

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use crate::app::{AppState, Layout};
use crate::ui;

/// Render one layout to a plain-text grid (symbols only, no color).
pub fn render_layout(app: &mut AppState, layout: Layout, w: u16, h: u16) -> String {
    app.layout = layout;
    let mut term = Terminal::new(TestBackend::new(w, h)).expect("test backend");
    term.draw(|f| ui::render(f, app)).expect("draw");
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

/// Print every layout (used by `lyrfin --snapshot [WxH]`).
pub fn dump_all(app: &mut AppState, w: u16, h: u16) {
    let layouts = [
        ("Dashboard", Layout::Dashboard),
        ("FullPlayer", Layout::FullPlayer),
        ("LyricsFocus", Layout::LyricsFocus),
    ];
    // warm up animated state (visualizer smoothing) so the dump isn't blank
    for _ in 0..50 {
        app.update(crate::action::Action::Tick);
    }
    for (name, layout) in layouts {
        println!("\n══════════ {name} ({w}x{h}) ══════════");
        print!("{}", render_layout(app, layout, w, h));
    }
    // each visualizer mode, shown in the Now Playing view (which hosts the viz)
    for (i, name) in crate::ui::components::VIZ_MODES.iter().enumerate() {
        app.views.viz_modes.insert(Layout::FullPlayer, i as u8);
        for _ in 0..20 {
            app.update(crate::action::Action::Tick);
        }
        println!("\n──────── Visualizer · {name} ────────");
        print!("{}", render_layout(app, Layout::FullPlayer, w, h));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppState;
    use crate::config::Config;

    // Domain suites split out of this file — each is its own
    // snapshot/tests/<name>_tests.rs reaching the shared demo()/set_panel() via
    // `use super::*` (alongside the existing spotify_tests / radio_tests).
    mod equalizer_tests;
    mod layout_tests;
    mod local_browse_tests;
    mod mini_tests;
    mod mouse_session_tests;
    mod nowplaying_tests;
    mod overlays_tests;
    mod palette_tests;
    mod playback_tests;
    mod settings_tests;
    mod tags_search_tests;
    mod unified_views_tests;

    fn demo() -> AppState {
        // point persistence at a throwaway dir so `cargo test` never touches the
        // user's real ~/.config/lyrfin (playlists / config / cache).
        let cfg = Config {
            dir: std::env::temp_dir().join("lyrfin-test"),
            ..Config::default()
        };
        let mut app = AppState::new(cfg);
        app.seed_demo();
        app
    }

    // Spotify tests, split by area into their own snapshot/tests/spotify_*_tests.rs.
    mod spotify_auth_tests;
    mod spotify_playback_tests;
    mod spotify_view_tests;

    fn set_panel(
        app: &mut AppState,
        layout: Layout,
        panel: crate::app::Panel,
        shown: bool,
        dock: crate::app::Dock,
    ) {
        app.views.panels.insert(
            (layout, panel),
            crate::app::PanelCfg {
                shown,
                dock,
                size: 30,
                len: 50,
            },
        );
    }

    /// Hide every docked pane on `layout` so a content test gets the full-width
    /// main pane regardless of the view's default arrangement (Home now defaults
    /// to the four-pane "hero" layout).
    fn hide_panels(app: &mut AppState, layout: Layout) {
        use crate::app::{Dock, Panel};
        for p in [
            Panel::Sidebar,
            Panel::Queue,
            Panel::Artist,
            Panel::Lyrics,
            Panel::Visualizer,
        ] {
            set_panel(app, layout, p, false, Dock::Right);
        }
    }

    // Radio-view tests live in their own file (snapshot/tests/radio_tests.rs) to
    // keep this module navigable.
    mod radio_tests;

    #[test]
    fn focus_routes_navigation() {
        let mut app = demo();
        use crate::action::{Action, Motion};
        use crate::app::{Focus, LocalSection, Panel};
        // focus the section sidebar, move down — the selected section advances
        // (and loads), not the tracklist
        app.focus = Focus::Sidebar;
        assert_eq!(app.local.section, LocalSection::AllTracks);
        app.update(Action::Move(Motion::Down));
        assert_eq!(app.local.section, LocalSection::ALL[1]);
        assert_eq!(app.selection, 0, "the tracklist cursor didn't move");
        // focus the queue, move down — queue selection advances
        app.focus = Focus::Pane(Panel::Queue);
        app.update(Action::Move(Motion::Down));
        assert_eq!(app.queue_sel, 1);
    }

    #[test]
    fn add_to_playlist_picker_adds_track() {
        let mut app = demo();
        use crate::action::Action;
        let pid = app.library.create_playlist("Test".into());
        let first = app.display_ids()[0];
        app.selection = 0;
        // open the picker for the selected tracklist row
        app.update(Action::AddToPlaylistPrompt);
        assert_eq!(app.input.add_targets, vec![first]);
        // point the picker at "Test" and confirm
        let idx = app
            .library
            .playlists_sorted()
            .iter()
            .position(|p| p.id == pid)
            .unwrap();
        app.input.add_sel = idx;
        app.update(Action::Activate);
        assert!(app.library.playlists[&pid].track_ids.contains(&first));
        assert!(app.input.add_targets.is_empty());
    }

    #[test]
    fn playlists_section_creates_via_naming_flow() {
        let mut app = demo();
        use crate::action::Action;
        use crate::app::{Focus, LocalSection};
        // browse the Playlists section (the gate for the playlist actions)
        app.focus = Focus::Main;
        app.local.section = LocalSection::Playlists;
        app.local_load_section();
        // create a playlist via the naming flow; the section list refreshes
        let before = app.library.playlists.len();
        let rows_before = app.local.items.len();
        app.update(Action::BeginNewPlaylist);
        app.update(Action::NameInput("Road Trip".into()));
        app.update(Action::Activate);
        assert_eq!(app.library.playlists.len(), before + 1);
        assert_eq!(app.local.items.len(), rows_before + 1);
        assert!(
            app.library
                .playlists
                .values()
                .any(|p| p.name == "Road Trip")
        );
    }
}

#[cfg(test)]
mod tcqedit {

    use crate::action::{Action, Caret};
    fn app() -> crate::app::AppState {
        let cfg = crate::config::Config {
            dir: std::env::temp_dir().join("lyrfin-tcq"),
            ..Default::default()
        };
        let mut a = crate::app::AppState::new(cfg);
        a.seed_demo();
        a.player.current = Some(a.library.tracks.values().next().unwrap().id);
        a
    }
    #[test]
    fn query_caret_edits() {
        let mut a = app();
        a.update(Action::OpenTagSearch);
        a.update(Action::TagInput("Assala".into())); // focus, caret=end
        assert_eq!(a.tags.search.as_ref().unwrap().qcaret, 6);
        a.update(Action::QueryCaret(Caret::Home));
        a.update(Action::QueryInsert('X'));
        assert_eq!(a.tags.search.as_ref().unwrap().query, "XAssala");
        assert_eq!(a.tags.search.as_ref().unwrap().qcaret, 1);
        a.update(Action::QueryDelete); // delete 'A' at caret
        assert_eq!(a.tags.search.as_ref().unwrap().query, "Xssala");
        a.update(Action::QueryCaret(Caret::End));
        a.update(Action::QueryBackspace);
        assert_eq!(a.tags.search.as_ref().unwrap().query, "Xssal");
    }
    #[test]
    fn arabic_caret_ok() {
        let mut a = app();
        a.update(Action::OpenTagSearch);
        a.update(Action::TagInput("أصالة".into()));
        let n = "أصالة".chars().count();
        assert_eq!(a.tags.search.as_ref().unwrap().qcaret, n);
        a.update(Action::QueryCaret(Caret::Home));
        a.update(Action::QueryDelete); // remove first Arabic char
        assert_eq!(a.tags.search.as_ref().unwrap().query.chars().count(), n - 1);
    }
}

#[cfg(test)]
mod tccover {

    use crate::action::Action;
    #[test]
    fn scope_toggle() {
        let cfg = crate::config::Config {
            dir: std::env::temp_dir().join("lyrfin-tcc"),
            ..Default::default()
        };
        let mut app = crate::app::AppState::new(cfg);
        app.seed_demo();
        app.player.current = Some(app.library.tracks.values().next().unwrap().id);
        app.update(Action::OpenCoverSearch);
        let cs = app.tags.cover.as_ref().unwrap();
        let multi = cs.paths.len() > 1;
        assert_eq!(
            cs.album_wide, multi,
            "multi-track album defaults to album-wide"
        );
        app.update(Action::CoverToggleScope);
        assert_eq!(
            app.tags.cover.as_ref().unwrap().album_wide,
            !multi,
            "s toggles scope"
        );
    }
}
