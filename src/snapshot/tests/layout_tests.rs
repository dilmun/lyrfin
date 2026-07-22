//! Layout snapshot/behaviour tests, split out of snapshot.rs.

use super::*;

#[test]
fn dashboard_has_tracklist_and_chrome() {
    let mut app = demo();
    let s = render_layout(&mut app, Layout::Dashboard, 120, 36);
    // no top bar; the view name lives at the right of the status bar
    assert!(s.contains("Home"), "view name in the status bar");
    // the default layout is compact rows (no column header): the track name and
    // its artist · album · year meta render on one line
    assert!(
        s.contains("Midnight Protocol") && s.contains("Afterglow"),
        "track rows present in the default (rows) layout"
    );
    assert!(s.contains("ARTIST"), "artist panel present");
}

#[test]
fn empty_library_shows_onboarding_not_demo() {
    // A fresh install (empty cache, no seed_demo) must greet the user with the
    // onboarding panel — never fabricated demo tracks.
    let cfg = Config {
        dir: std::env::temp_dir().join("lyrfin-test-empty"),
        ..Config::default()
    };
    let mut app = AppState::new(cfg);
    app.load_cached_library(Vec::new()); // the real first-run path (empty cache)
    let s = render_layout(&mut app, Layout::Dashboard, 120, 40);
    assert!(
        s.contains("Welcome to lyrfin"),
        "onboarding heading present:\n{s}"
    );
    assert!(
        s.contains("ADD LOCAL MUSIC"),
        "local add-music guidance present"
    );
    assert!(
        !s.contains("Neon District") && !s.contains("Midnight Protocol"),
        "no fabricated demo data on a fresh install:\n{s}"
    );
}

#[test]
fn dashboard_track_layout_toggles_between_rows_and_columns() {
    // the shared `track_columns` toggle reshapes the local main list: the
    // TITLE/ARTIST/… column table (default) ↔ compact rows with no column header.
    let mut a = demo();
    hide_panels(&mut a, Layout::Dashboard); // about the tracklist, not the panes
    // rows: name + artist · album · year meta on one line, no header
    a.config.track_columns = false;
    let rows = render_layout(&mut a, Layout::Dashboard, 140, 30);
    let row = rows
        .lines()
        .find(|l| l.contains("Midnight Protocol"))
        .expect("the track row");
    assert!(
        row.contains("Neon District") && row.contains("Afterglow") && row.contains("2025"),
        "rows show artist · album · year on the track line: {row:?}"
    );
    assert!(
        !rows.contains("TITLE"),
        "the rows layout has no column header"
    );
    // flip to columns: the table header appears
    a.config.track_columns = true;
    let cols = render_layout(&mut a, Layout::Dashboard, 140, 30);
    assert!(
        cols.contains("TITLE") && cols.contains("ARTIST"),
        "the column table shows its header when track_columns is on"
    );
    assert!(
        cols.contains("Midnight Protocol"),
        "the track still renders"
    );
}

#[test]
fn rows_layout_honors_the_column_show_hide_toggles() {
    // a rows-layout list builds its meta from the same column toggles as the
    // table, so turning a column off drops it from the compact row too.
    let mut a = demo();
    hide_panels(&mut a, Layout::Dashboard);
    a.config.columns.album = false; // track_columns defaults to rows
    let s = render_layout(&mut a, Layout::Dashboard, 140, 30);
    let row = s
        .lines()
        .find(|l| l.contains("Midnight Protocol"))
        .expect("the track row");
    assert!(
        !row.contains("Afterglow"),
        "album dropped from the row meta when its column is off: {row:?}"
    );
    assert!(row.contains("Neon District"), "artist still shown");
}

#[test]
fn dashboard_stacks_queue_and_artist_on_the_same_edge() {
    let mut a = demo();
    a.layout = Layout::Dashboard;
    // drop the tracklist's ARTIST column so "ARTIST" can only come from the
    // artist panel (not the table header).
    a.config.columns.artist = false;
    // Force the artist + queue panes onto the same (right) edge so they must STACK
    // (queue above artist), both visible — Home docks Artist left by default now.
    use crate::app::{Dock, Panel};
    set_panel(&mut a, Layout::Dashboard, Panel::Queue, true, Dock::Right);
    set_panel(&mut a, Layout::Dashboard, Panel::Artist, true, Dock::Right);
    let s = render_layout(&mut a, Layout::Dashboard, 90, 36);
    assert!(s.contains("QUEUE"), "queue shows on the right");
    assert!(
        s.contains("ARTIST"),
        "artist panel shows (stacked under the queue, not squeezed out)"
    );
    assert!(s.contains("Midnight Protocol"), "tracklist still has room");
}

#[test]
fn library_view_is_a_three_column_browser() {
    use crate::action::{Action, Motion};
    use crate::app::Focus;
    let mut app = demo();
    app.focus = Focus::Main;

    // renders the three Miller columns on the shared shell
    let s = render_layout(&mut app, Layout::LibraryFocus, 120, 36);
    assert!(s.contains("Library"), "view name in the status bar");
    assert!(
        s.contains("ARTISTS") && s.contains("ALBUMS") && s.contains("TRACKS"),
        "all three column headers present:\n{s}"
    );

    // h/l (→ Move Left/Right) switch the active column, clamped to 0..=2
    assert_eq!(app.browser.col, 0);
    app.update(Action::Move(Motion::Right));
    assert_eq!(app.browser.col, 1, "→ advances the column");
    app.update(Action::Move(Motion::Right));
    app.update(Action::Move(Motion::Right));
    assert_eq!(app.browser.col, 2, "clamped at the TRACKS column");
    app.update(Action::Move(Motion::Left));
    assert_eq!(app.browser.col, 1, "← retreats");

    // on the ARTISTS column, j advances the artist and resets the dependent columns
    app.browser.col = 0;
    app.browser.album = 3;
    app.browser.track = 5;
    let (na, _, _) = app.browser_counts();
    if na > 1 {
        app.update(Action::Move(Motion::Down));
        assert_eq!(app.browser.artist, 1, "j advances the artist");
        assert_eq!(
            (app.browser.album, app.browser.track),
            (0, 0),
            "selecting a new artist resets albums + tracks"
        );
    }

    // Enter drills right column-by-column, then plays on the TRACKS column
    app.browser.col = 0;
    app.update(Action::Activate);
    assert_eq!(app.browser.col, 1, "Enter drills into ALBUMS");
    app.browser.col = 2;
    app.update(Action::Activate);
    assert!(
        app.player.current.is_some(),
        "Enter on the TRACKS column plays the selected track"
    );
}

#[test]
fn playback_bar_height_is_consistent_across_views() {
    // every view (#1–#4) sizes its playback bar with the same helper, so the
    // tall (9) / standard (6) / mini (2) choice is identical everywhere
    use crate::ui::components::{MINI_NOW_H, now_bar_height};
    assert_eq!(now_bar_height(true, 120, 36), 9, "tall window → full bar");
    assert_eq!(
        now_bar_height(true, 120, 24),
        6,
        "short window → standard bar"
    );
    assert_eq!(
        now_bar_height(false, 120, 36),
        6,
        "no bar-viz → standard bar"
    );
    // a mini-width frame collapses the bar regardless of the viz setting; height
    // alone never does (a short-but-wide window keeps the full layout)
    assert_eq!(
        now_bar_height(true, 50, 36),
        MINI_NOW_H,
        "narrow frame → compact 2-row bar"
    );
    assert_eq!(
        now_bar_height(true, 120, 18),
        6,
        "short but wide keeps the standard bar"
    );
}

#[test]
fn visualizer_includes_the_3d_waterfall() {
    let modes = crate::ui::components::VIZ_MODES;
    assert_eq!(modes.len(), 7);
    assert!(modes.contains(&"Waterfall"));
    assert!(!modes.contains(&"Globe"), "the globe was removed");
}

#[test]
fn waterfall_renders_nonempty() {
    use crate::action::Action;
    use crate::core::player::Status;
    let mode = 6u8; // Waterfall (needs frame history)
    let mut a = demo();
    a.player.status = Status::Playing;
    a.views.viz_modes.insert(Layout::FullPlayer, mode);
    for f in 0..50u32 {
        a.player.spectrum = (0..48)
            .map(|i| (((i as f32 * 0.4) + f as f32 * 0.3).sin().abs() * 0.8 + 0.1).min(1.0))
            .collect();
        a.update(Action::Tick);
    }
    let s = render_layout(&mut a, Layout::FullPlayer, 90, 22);
    assert!(
        s.chars().any(|c| ('\u{2801}'..='\u{28FF}').contains(&c)),
        "the waterfall renders a 3D Braille scene"
    );
    assert!(s.contains("Waterfall"), "panel title names the mode");
}

#[test]
fn side_panes_stack_or_sit_side_by_side() {
    use crate::app::{Dock, Panel};
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = crate::spotify::ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    let mk = |n: &str| Item {
        uri: format!("spotify:track:{n}"),
        name: n.into(),
        subtitle: "Artist".into(),
        kind: Kind::Track,
        duration_ms: 200_000,
        ..Default::default()
    };
    a.spov.now_spotify = Some(mk("Now"));
    a.spov.sp_queue = vec![mk("A"), mk("B")];
    set_panel(&mut a, Layout::Spotify, Panel::Queue, true, Dock::Right);
    set_panel(&mut a, Layout::Spotify, Panel::Artist, true, Dock::Right);
    let row_of = |s: &str, needle: &str| s.lines().position(|l| l.contains(needle));

    a.config.panes_horizontal = false;
    let v = render_layout(&mut a, Layout::Spotify, 140, 30);
    assert_ne!(
        row_of(&v, "QUEUE"),
        row_of(&v, "ARTIST"),
        "vertical: the panes stack on different rows"
    );

    a.config.panes_horizontal = true;
    let h = render_layout(&mut a, Layout::Spotify, 140, 30);
    assert_eq!(
        row_of(&h, "QUEUE"),
        row_of(&h, "ARTIST"),
        "horizontal: the panes sit side-by-side on the same row"
    );
}

fn connected_with_docked_queue(size: u16) -> AppState {
    use crate::app::{Dock, Panel};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = crate::spotify::ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    a.views.panels.insert(
        (Layout::Spotify, Panel::Queue),
        crate::app::PanelCfg {
            shown: true,
            dock: Dock::Right,
            size,
            len: 50,
        },
    );
    a
}

#[test]
fn narrow_terminal_collapses_to_single_pane() {
    // As the terminal shrinks, panes collapse (least-important first); on a very
    // narrow terminal every docked pane — sidebar included — is gone, leaving a
    // single full-width main pane.
    let mut a = connected_with_docked_queue(40);
    let s = render_layout(&mut a, Layout::Spotify, 36, 20);
    assert!(s.contains("SPOTIFY"), "the main result pane still renders");
    assert!(
        !s.contains("QUEUE"),
        "narrow terminal collapses → docked panes hidden"
    );
    assert!(!s.contains("LIBRARY"), "…and the sidebar is hidden too");
}

#[test]
fn dock_band_never_starves_main_content() {
    // A large pane (80) on a wide-but-finite body (120 cols): the MIN_MAIN clamp
    // bounds the band so the sidebar + result list keep room instead of being
    // blanked. (Narrow terminals collapse instead — see the test above.)
    let mut a = connected_with_docked_queue(80);
    let s = render_layout(&mut a, Layout::Spotify, 120, 20);
    assert!(s.contains("SPOTIFY"), "the main result pane still renders");
    assert!(s.contains("QUEUE"), "the docked pane renders");
    assert!(s.contains("LIBRARY"), "the sidebar still renders");
}

#[test]
fn queue_pane_marks_now_playing() {
    // The local queue and the Spotify queue pane now render through the same
    // shared queue_pane under one "QUEUE" title: the now-playing row is marked ▶.
    use crate::app::Panel;
    let mut a = demo();
    a.layout = Layout::Dashboard;
    if !a.panel(Panel::Queue).shown {
        a.toggle_panel(Panel::Queue);
    }
    let s = render_layout(&mut a, Layout::Dashboard, 120, 36);
    assert!(
        s.contains("QUEUE"),
        "the shared queue pane renders its title"
    );
    assert!(s.contains('▶'), "the now-playing row is marked ▶");
}

#[test]
fn cycling_the_big_visualizer_persists_immediately() {
    // Regression: the per-view visualizer mode lives in the session, which used to
    // be written only on a clean quit — so a Ctrl-C / crash / rebuild that killed
    // the process lost the change and the next launch fell back to Bars. Cycling
    // must now persist to disk on the spot, without any exit.
    use crate::action::Action;
    let dir = std::env::temp_dir().join("lyrfin_viz_persist_on_change");
    let _ = std::fs::remove_dir_all(&dir);
    let mut a = demo();
    a.config.dir = dir.clone();
    a.layout = Layout::FullPlayer; // a big-viz view
    assert_eq!(a.viz_mode(), 0, "starts at the default (Bars)");

    a.update(Action::CycleVisualizer); // → mode 1, and a save_session()
    assert_eq!(a.viz_mode(), 1, "cycled in memory");

    // it hit disk immediately — load a fresh session straight from the file
    let loaded = crate::session::Session::load(&dir);
    let restored = loaded
        .visualizer_modes
        .unwrap_or_default()
        .into_iter()
        .find(|(l, _)| l == "full_player")
        .map(|(_, m)| m);
    assert_eq!(
        restored,
        Some(1),
        "the cycled mode was written to session.json without a clean quit"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reset_layout_restores_view_defaults() {
    use crate::action::Action;
    use crate::app::Panel;
    let mut a = demo();
    a.layout = Layout::Dashboard;
    // mangle this view's layout: hide the sidebar, resize the queue, set a viz
    a.toggle_panel(Panel::Sidebar); // default is shown → now hidden
    a.resize_panel(Panel::Queue, 6); // creates a Queue size override
    a.views.viz_modes.insert(Layout::Dashboard, 4);
    assert!(
        a.views.panels.keys().any(|(l, _)| *l == Layout::Dashboard),
        "overrides exist before reset"
    );

    a.update(Action::ResetLayout);
    assert!(
        !a.views.panels.keys().any(|(l, _)| *l == Layout::Dashboard),
        "all Dashboard panel overrides cleared"
    );
    assert_eq!(a.viz_mode(), 0, "view visualizer mode reset");
    let side = a.panel(Panel::Sidebar);
    let def = Layout::Dashboard.default_panel(Panel::Sidebar);
    assert_eq!(side.shown, def.shown, "sidebar visibility back to default");
    assert_eq!(side.size, def.size, "sidebar width back to default");
    // only the current view is reset — other views keep their state
    a.layout = Layout::FullPlayer;
    a.resize_panel(Panel::Queue, 8);
    a.layout = Layout::Dashboard;
    a.update(Action::ResetLayout);
    assert!(
        a.views.panels.keys().any(|(l, _)| *l == Layout::FullPlayer),
        "resetting Dashboard leaves FullPlayer untouched"
    );
}

#[test]
fn comma_period_nudge_lyric_sync_only_when_lyrics_focused() {
    use crate::action::Action;
    use crate::app::{Focus, Layout, Panel};
    use crate::event::{Key, KeyCode, Mods};
    let mut a = demo();
    let k = |c| Key {
        code: KeyCode::Char(c),
        mods: Mods::default(),
    };

    // default (Dashboard, Main focus): `,`/`.` are the global seek keys
    assert!(matches!(crate::keymap::map(&a, k('.')), Action::Seek(5)));
    assert!(matches!(crate::keymap::map(&a, k(',')), Action::Seek(-5)));

    // Lyrics pane focused → they nudge the sync offset (later / earlier)
    a.focus = Focus::Pane(Panel::Lyrics);
    assert!(matches!(
        crate::keymap::map(&a, k('.')),
        Action::LyricsOffset(50)
    ));
    assert!(matches!(
        crate::keymap::map(&a, k(',')),
        Action::LyricsOffset(-50)
    ));

    // the dedicated Lyrics view nudges regardless of which sub-pane holds focus
    a.layout = Layout::LyricsFocus;
    a.focus = Focus::Main;
    assert!(matches!(
        crate::keymap::map(&a, k('.')),
        Action::LyricsOffset(50)
    ));
}

#[test]
fn ctrl_d_u_scroll_everywhere_and_shift_d_deletes_playlist() {
    use crate::action::{Action, Motion};
    use crate::app::{Focus, Layout, LocalSection, Panel};
    use crate::event::{Key, KeyCode, Mods};
    let ctrl = |c| Key {
        code: KeyCode::Char(c),
        mods: Mods {
            ctrl: true,
            ..Mods::default()
        },
    };
    let shift = |c| Key {
        code: KeyCode::Char(c),
        mods: Mods {
            shift: true,
            ..Mods::default()
        },
    };
    let mut a = demo();

    // ctrl-d / ctrl-u half-page scroll in a plain list,
    a.layout = Layout::Dashboard;
    a.focus = Focus::Main;
    assert!(matches!(
        crate::keymap::map(&a, ctrl('d')),
        Action::Move(Motion::PageDown)
    ));
    assert!(matches!(
        crate::keymap::map(&a, ctrl('u')),
        Action::Move(Motion::PageUp)
    ));
    // …the radio station list (the reported gap)…
    a.layout = Layout::Radio;
    assert!(matches!(
        crate::keymap::map(&a, ctrl('d')),
        Action::Move(Motion::PageDown)
    ));
    // …and past a focused pane whose plain `d` means remove: ctrl-d isn't shadowed.
    a.layout = Layout::Dashboard;
    a.focus = Focus::Pane(Panel::Queue);
    assert!(matches!(
        crate::keymap::map(&a, ctrl('d')),
        Action::Move(Motion::PageDown)
    ));

    // destructive delete-playlist moved off `d` to Shift+D (so plain `d` is free and
    // ctrl-d can never be misread as a delete).
    a.focus = Focus::Main;
    a.local.section = LocalSection::Playlists;
    assert!(matches!(
        crate::keymap::map(&a, shift('d')),
        Action::DeletePlaylist
    ));
}

#[test]
fn hjkl_navigate_seek_moves_to_comma_period_rate_to_parens() {
    use crate::action::{Action, Motion};
    use crate::app::{Focus, Layout};
    use crate::event::{Key, KeyCode, Mods};
    let mut a = demo();
    let k = |c| Key {
        code: KeyCode::Char(c),
        mods: Mods::default(),
    };
    let arrow = |code| Key {
        code,
        mods: Mods::default(),
    };

    // A plain player view (no 2-D main content): h/l and ←/→ shift focus through
    // the pane ring — they no longer seek.
    a.layout = Layout::FullPlayer;
    a.focus = Focus::Main;
    assert!(matches!(
        crate::keymap::map(&a, k('h')),
        Action::FocusDir(-1)
    ));
    assert!(matches!(
        crate::keymap::map(&a, k('l')),
        Action::FocusDir(1)
    ));
    assert!(matches!(
        crate::keymap::map(&a, arrow(KeyCode::Left)),
        Action::FocusDir(-1)
    ));
    assert!(matches!(
        crate::keymap::map(&a, arrow(KeyCode::Right)),
        Action::FocusDir(1)
    ));

    // seek is now the `,`/`.` transport pair (universal), and rate moved to the parens
    assert!(matches!(crate::keymap::map(&a, k(',')), Action::Seek(-5)));
    assert!(matches!(crate::keymap::map(&a, k('.')), Action::Seek(5)));
    assert!(matches!(crate::keymap::map(&a, k(')')), Action::Rate(_, _)));
    assert!(matches!(crate::keymap::map(&a, k('(')), Action::Rate(_, _)));

    // The 2-D views keep their column/card meaning for h/l: the Library browser
    // still switches the active column rather than shifting focus.
    a.layout = Layout::LibraryFocus;
    a.focus = Focus::Main;
    assert!(matches!(
        crate::keymap::map(&a, k('h')),
        Action::Move(Motion::Left)
    ));
    assert!(matches!(
        crate::keymap::map(&a, k('l')),
        Action::Move(Motion::Right)
    ));
}

#[test]
fn angle_brackets_resize_the_focused_pane() {
    use crate::action::Action;
    use crate::event::{Key, KeyCode, Mods};
    let a = demo();
    let k = |c| Key {
        code: KeyCode::Char(c),
        mods: Mods::default(),
    };
    assert!(matches!(
        crate::keymap::map(&a, k('>')),
        Action::ResizeFocusedPane(1)
    ));
    assert!(matches!(
        crate::keymap::map(&a, k('<')),
        Action::ResizeFocusedPane(-1)
    ));
}

#[test]
fn resize_grows_then_shrinks_the_focused_spotify_pane() {
    use crate::app::{Focus, Layout, Panel};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.focus = Focus::Pane(Panel::Queue);
    let before = a.panel(Panel::Queue).size;
    // the queue defaults to the RIGHT edge, where the direction flips so `<`/`>`
    // move the boundary in their on-screen direction: `<` grows a right pane.
    a.update(crate::action::Action::ResizeFocusedPane(-1));
    assert!(
        a.panel(Panel::Queue).size > before,
        "< grows a right-docked pane"
    );
    a.update(crate::action::Action::ResizeFocusedPane(1));
    assert_eq!(
        a.panel(Panel::Queue).size,
        before,
        "> undoes the < step exactly"
    );
}

#[test]
fn resizing_a_stacked_pane_resizes_its_whole_column() {
    use crate::action::Action;
    use crate::app::{Dock, Focus, Panel};
    let mut a = demo();
    a.layout = Layout::Dashboard;
    // queue + lyrics stacked on the right edge share one column width (the band is
    // the larger of them), so resizing one must resize the whole column — else a
    // pane that isn't the largest can't be grown at all.
    set_panel(&mut a, Layout::Dashboard, Panel::Queue, true, Dock::Right);
    set_panel(&mut a, Layout::Dashboard, Panel::Lyrics, true, Dock::Right);
    a.focus = Focus::Pane(Panel::Queue);
    let (q0, l0) = (a.panel(Panel::Queue).size, a.panel(Panel::Lyrics).size);
    a.update(Action::ResizeFocusedPane(-1)); // `<` grows a right-docked column
    assert!(a.panel(Panel::Queue).size > q0, "the focused queue grows");
    assert!(
        a.panel(Panel::Lyrics).size > l0,
        "its stacked column-mate grows with it, so the column actually widens"
    );
}

#[test]
fn pane_height_share_adjusts_for_stacked_panes() {
    use crate::action::Action;
    use crate::app::{Dock, Focus, Panel};
    let mut a = demo();
    a.layout = Layout::Dashboard;
    // queue + lyrics stacked on the right split the column height 50/50 by default
    set_panel(&mut a, Layout::Dashboard, Panel::Queue, true, Dock::Right);
    set_panel(&mut a, Layout::Dashboard, Panel::Lyrics, true, Dock::Right);
    a.focus = Focus::Pane(Panel::Queue);
    let before = a.panel(Panel::Queue).len;
    a.update(Action::ResizePaneHeight(1)); // `}` — taller
    assert!(
        a.panel(Panel::Queue).len > before,
        "the focused pane's height share grows"
    );
    assert_eq!(
        a.panel(Panel::Lyrics).len,
        50,
        "the column-mate keeps its weight, so only the split shifts"
    );

    // a pane alone on its edge has no column-mate, so height share is a no-op
    let mut b = demo();
    b.layout = Layout::Dashboard;
    // Home stacks Artist under the sidebar by default; hide it so the sidebar is
    // genuinely alone on the left edge for this no-op check.
    set_panel(&mut b, Layout::Dashboard, Panel::Artist, false, Dock::Left);
    b.focus = Focus::Sidebar; // the sidebar is alone on the left edge
    let s = b.panel(Panel::Sidebar).len;
    b.update(Action::ResizePaneHeight(1));
    assert_eq!(
        b.panel(Panel::Sidebar).len,
        s,
        "no height share to adjust when alone on the edge"
    );
}

#[test]
fn fit_layout_resets_sizes_but_keeps_the_panes() {
    use crate::action::Action;
    use crate::app::{Dock, Panel};
    let mut a = demo();
    a.layout = Layout::Dashboard;
    // user shows + moves the queue, then drifts its size off the default
    set_panel(&mut a, Layout::Dashboard, Panel::Queue, true, Dock::Left);
    a.resize_panel(Panel::Queue, 1);
    let def = Layout::Dashboard.default_panel(Panel::Queue).size;
    assert_ne!(a.panel(Panel::Queue).size, def, "size drifted off default");

    a.update(Action::FitLayout);
    assert_eq!(
        a.panel(Panel::Queue).size,
        def,
        "fit restores the default percentage size"
    );
    assert!(a.panel(Panel::Queue).shown, "fit keeps the pane shown");
    assert_eq!(
        a.panel(Panel::Queue).dock,
        Dock::Left,
        "fit keeps where the user docked it"
    );
}

#[test]
fn every_layout_renders_nonempty() {
    let mut app = demo();
    for layout in [Layout::Dashboard, Layout::FullPlayer, Layout::LyricsFocus] {
        let s = render_layout(&mut app, layout, 110, 30);
        assert!(s.trim().lines().count() > 10, "{layout:?} renders rows");
    }
}

#[test]
fn concert_mode_renders_fullscreen() {
    let mut app = demo();
    let s = render_layout(&mut app, Layout::Concert, 100, 32);
    // the now-playing title + a progress time, and NO shared chrome tabs
    assert!(s.contains("Midnight Protocol"), "title centered");
    assert!(s.contains(":"), "progress time shown");
    assert!(
        !s.contains("Now Playing"),
        "no top-bar tab strip (fullscreen)"
    );
}

#[test]
fn queue_docks_to_each_edge() {
    use crate::app::{Dock, Panel};
    let mut app = demo();
    // Right (default): queue column on the right half of the Home view
    set_panel(&mut app, Layout::Dashboard, Panel::Queue, true, Dock::Right);
    let r = render_layout(&mut app, Layout::Dashboard, 90, 18);
    let qcol = r.lines().find(|l| l.contains("QUEUE")).unwrap();
    assert!(qcol.find("QUEUE").unwrap() > 30, "queue on the right");
    // Top: queue spans the first body row, above the MUSIC main pane
    set_panel(&mut app, Layout::Dashboard, Panel::Queue, true, Dock::Top);
    let t: Vec<String> = render_layout(&mut app, Layout::Dashboard, 90, 18)
        .lines()
        .map(|s| s.to_string())
        .collect();
    let qrow = t.iter().position(|l| l.contains("QUEUE")).unwrap();
    let mrow = t.iter().position(|l| l.contains("MUSIC")).unwrap();
    assert!(qrow < mrow, "queue above the main pane");
    // cycle order L→T→R→B
    assert_eq!(Dock::Left.cycle(), Dock::Top);
    assert_eq!(Dock::Bottom.cycle(), Dock::Left);
}

#[test]
fn per_view_panels_are_independent() {
    use crate::action::Action;
    use crate::app::{Dock, Panel};
    let mut app = demo();
    // start from a known-hidden queue on Home (it's shown by default now)
    set_panel(
        &mut app,
        Layout::Dashboard,
        Panel::Queue,
        false,
        Dock::Right,
    );
    // show the queue on Home only
    app.layout = Layout::Dashboard;
    app.update(Action::ToggleQueue);
    assert!(app.panel_in(Layout::Dashboard, Panel::Queue).shown);
    assert!(
        !app.panel_in(Layout::FullPlayer, Panel::Queue).shown,
        "toggling queue on Home does not affect Now Playing"
    );
    // moving it on Home doesn't move it elsewhere (default stays Right)
    app.update(Action::ToggleQueueSide);
    assert_ne!(
        app.panel_in(Layout::Dashboard, Panel::Queue).dock,
        Dock::Right
    );
    assert_eq!(
        app.panel_in(Layout::FullPlayer, Panel::Queue).dock,
        Dock::Right
    );
}

#[test]
fn focused_dock_pane_moves_and_resizes_in_the_dashboard() {
    use crate::action::Action;
    use crate::app::{Focus, Panel};
    let mut a = demo();
    a.layout = Layout::Dashboard;
    // the Artist pane is a movable dock pane on Home; show it on a known edge
    // (Right) so the move → Bottom and right-docked-grows assertions are stable
    // (Home docks Artist left by default now).
    set_panel(
        &mut a,
        Layout::Dashboard,
        Panel::Artist,
        true,
        crate::app::Dock::Right,
    );
    a.focus = Focus::Pane(Panel::Artist);
    assert_eq!(
        a.focused_panel(),
        Some(Panel::Artist),
        "focus resolves to the Artist pane (was stale before)"
    );

    // `m` cycles the focused pane's edge → Right → Bottom (under the library)
    let before = a.panel(Panel::Artist).dock;
    a.update(Action::MoveFocusedPane);
    assert_eq!(
        a.panel(Panel::Artist).dock,
        before.cycle(),
        "Artist pane moved to the next edge"
    );

    // resize the focused Artist pane (previously a no-op — focused_panel didn't
    // recognise Artist/Lyrics focus). It's right-docked, so `<` grows it.
    let s0 = a.panel(Panel::Artist).size;
    a.update(Action::ResizeFocusedPane(-1));
    assert!(
        a.panel(Panel::Artist).size > s0,
        "focused Artist pane grows on resize"
    );
}

#[test]
fn panes_scale_with_window_and_collapse_progressively() {
    use crate::app::{Dock, Panel};
    let mut a = demo();
    a.layout = Layout::Dashboard;
    set_panel(&mut a, Layout::Dashboard, Panel::Queue, true, Dock::Right);
    set_panel(&mut a, Layout::Dashboard, Panel::Artist, true, Dock::Right);

    // wide terminal: the sidebar + the right-edge panes all fit (percentage-sized)
    let wide = render_layout(&mut a, Layout::Dashboard, 160, 40);
    assert!(wide.contains("LIBRARY"), "sidebar shows when there's room");
    assert!(
        wide.contains("QUEUE") && wide.contains("ARTIST"),
        "the docked panes show when there's room"
    );

    // very narrow terminal: everything collapses to the single main pane, but the
    // main content is never blanked — that's the smooth, importance-ordered drop.
    let tiny = render_layout(&mut a, Layout::Dashboard, 34, 40);
    assert!(
        !tiny.contains("LIBRARY") && !tiny.contains("QUEUE") && !tiny.contains("ARTIST"),
        "a tiny terminal collapses every pane"
    );
    assert!(tiny.contains("MUSIC"), "the main content always survives");
}

#[test]
fn library_sidebar_is_a_movable_dock_pane() {
    use crate::action::Action;
    use crate::app::{Dock, Focus, Panel};
    let mut a = demo();
    a.layout = Layout::Dashboard;
    // the LIBRARY sidebar is a first-class movable pane, shown left by default
    assert!(Layout::Dashboard.panels().contains(&Panel::Sidebar));
    assert!(a.panel(Panel::Sidebar).shown);
    assert_eq!(a.panel(Panel::Sidebar).dock, Dock::Left);

    // focus it and move it like any other pane (Left → Top, the next edge)
    a.focus = Focus::Sidebar;
    assert_eq!(a.focused_panel(), Some(Panel::Sidebar));
    a.update(Action::MoveFocusedPane);
    assert_eq!(a.panel(Panel::Sidebar).dock, Dock::Left.cycle());

    // it still renders with its LIBRARY title once moved
    let s = render_layout(&mut a, Layout::Dashboard, 120, 36);
    assert!(
        s.contains("LIBRARY"),
        "the sidebar keeps its title when docked"
    );
}

#[test]
fn grid_layout_scales_columns_with_width() {
    use crate::ui::components::grid_cells;
    use ratatui::layout::Rect;
    // a FIXED card width (16): a wider pane fits more columns (cards keep their size)
    let off = std::cell::Cell::new(0);
    let (narrow, cols_narrow) = grid_cells(Rect::new(0, 0, 40, 30), 12, 0, 16, (10, 20), &off);
    let (wide, cols_wide) = grid_cells(Rect::new(0, 0, 120, 30), 12, 0, 16, (10, 20), &off);
    assert!(cols_wide > cols_narrow, "a wider window gives more columns");
    assert!(!narrow.is_empty() && !wide.is_empty(), "cards are laid out");
    // the card size is identical regardless of pane width (no stretch-to-fill)
    assert_eq!(
        narrow[0].1.width, wide[0].1.width,
        "card width is fixed, not stretched to the pane"
    );
    for (_, r) in &wide {
        assert!(
            r.x + r.width <= 120 && r.y + r.height <= 30,
            "card rect stays within the area"
        );
    }
}

#[test]
fn grid_cover_stays_square_so_the_photo_covers_the_placeholder() {
    use crate::ui::components::grid_cells;
    use ratatui::layout::Rect;
    // The card's cover must stay ~square in PIXELS (cells are ~2:1, so cover_h ≈
    // card_w/2) — otherwise the round photo can't cover the tinted placeholder disc
    // and the tint shows through as wings. Guard that the height tracks the width
    // across a range of pane sizes (it must NOT shrink independently to pack rows).
    for h in [24u16, 35, 48, 60] {
        let (cells, _) = grid_cells(
            Rect::new(0, 0, 96, h),
            30,
            0,
            22,
            (10, 20),
            &std::cell::Cell::new(0),
        );
        let card_w = cells[0].1.width as f32;
        let cover_h = (cells[0].1.height - 3) as f32; // 3 rows reserved for labels
        let ratio = card_w / (cover_h * 2.0); // width_px / height_px of the cover
        assert!(
            (0.7..=1.45).contains(&ratio),
            "cover stays ~square in pixels (ratio {ratio:.2} at height {h}) so the photo covers the disc"
        );
    }
}

#[test]
fn grid_card_height_uses_real_cell_aspect_so_rows_arent_wasted() {
    use crate::ui::components::grid_cells;
    use ratatui::layout::Rect;
    use std::collections::BTreeSet;
    // card_w 22, 2:1 cells: the cover is (22 − 2·pad)=20 cells wide → 10 cells tall
    // (square in px), + 3 label rows = 13-tall cards. A 41-row pane therefore fits 3
    // rows (3·13 = 39 ≤ 41) — the old `card_w/2 + 3 = 14` (ignoring padding) would
    // have over-reserved and shown only 2.
    let (cells, _) = grid_cells(
        Rect::new(0, 0, 120, 41),
        30,
        0,
        22,
        (10, 20),
        &std::cell::Cell::new(0),
    );
    let rows: BTreeSet<u16> = cells.iter().map(|(_, r)| r.y).collect();
    assert_eq!(
        rows.len(),
        3,
        "the corrected card height fits the third row instead of wasting it"
    );
}

#[test]
fn carousel_scroll_is_sticky_and_resets_between_carousels() {
    use crate::app::ReleaseRow;
    use crate::ui::components::carousel_off;
    // cols = 3. Carousel A has 6 cards (scrolls), B has 3 (fits).
    let rows = vec![
        ReleaseRow::Cards(vec![10, 11, 12, 13, 14, 15]),
        ReleaseRow::Cards(vec![20, 21, 22]),
    ];
    let cols = 3;
    let off = std::cell::Cell::new(0);
    let key = std::cell::Cell::new(0);

    // select A[0] → window starts at 0 (shows 10,11,12)
    assert_eq!(carousel_off(&rows, 10, cols, &off, &key), 0);
    // select A[2], already visible → the carousel does NOT scroll (clicked card stays
    // put, so a double-click's second click hits the same card)
    assert_eq!(
        carousel_off(&rows, 12, cols, &off, &key),
        0,
        "visible card: no scroll"
    );
    // select A[5], off the right edge → scroll the MINIMUM to reveal it (offset 3,
    // showing 13,14,15), NOT centre it
    assert_eq!(
        carousel_off(&rows, 15, cols, &off, &key),
        3,
        "sticky reveal at the edge"
    );
    // click into carousel B → the offset resets, so B does NOT inherit A's scroll of 3
    // (which would have shifted B's cards and mis-targeted the click)
    assert_eq!(
        carousel_off(&rows, 20, cols, &off, &key),
        0,
        "new carousel starts fresh"
    );
}

#[test]
fn grid_viewport_is_sticky_not_recentred_on_selection() {
    use crate::ui::components::grid_cells;
    use ratatui::layout::Rect;
    // 30 cards, card_w 22 → cols 5, card_h 13 → rows_visible 3 (rows_total 6).
    let area = Rect::new(0, 0, 120, 41);
    let font = (10, 20);
    let off = std::cell::Cell::new(0);
    let rect_of =
        |cells: &[(usize, Rect)], i: usize| cells.iter().find(|(x, _)| *x == i).map(|(_, r)| *r);

    // top of the list: rows 0..3 (items 0..15) visible, offset 0
    let (a, _) = grid_cells(area, 30, 0, 22, font, &off);
    assert_eq!(off.get(), 0);
    let item5_before = rect_of(&a, 5).expect("item 5 (row 1) visible");

    // select a card that is ALREADY visible (item 5) → the viewport must NOT move,
    // so the card the user clicked stays exactly where it was (the core of the fix:
    // no re-centring under the cursor, so a second click lands on the same card).
    let (b, _) = grid_cells(area, 30, 5, 22, font, &off);
    assert_eq!(
        off.get(),
        0,
        "selecting a visible card does not scroll the grid"
    );
    assert_eq!(
        rect_of(&b, 5),
        Some(item5_before),
        "the clicked card stays in place"
    );

    // select a card off the bottom (item 20, row 4) → scroll the MINIMUM to reveal it
    // (row 4 becomes the last visible row: off = 4 + 1 − 3 = 2), NOT re-centre it.
    let (c, _) = grid_cells(area, 30, 20, 22, font, &off);
    assert_eq!(
        off.get(),
        2,
        "sticky scroll reveals the row at the edge, not centred"
    );
    assert!(
        rect_of(&c, 20).is_some(),
        "the newly-selected card is now visible"
    );
}

#[test]
fn grid_block_centers_in_the_pane() {
    use crate::ui::components::grid_cells;
    use ratatui::layout::Rect;
    // The card block centres in the available area on BOTH axes (margins derived
    // from the live pane size, not hardcoded): few items in a wide/tall pane → equal
    // left/right and top/bottom margins around the block.
    let inner = Rect::new(0, 0, 100, 40);
    let (cells, cols) = grid_cells(inner, 6, 0, 22, (10, 20), &std::cell::Cell::new(0)); // fixed card width 22
    let left = cells.iter().map(|(_, r)| r.x).min().unwrap() - inner.x;
    let right = (inner.x + inner.width) - cells.iter().map(|(_, r)| r.x + r.width).max().unwrap();
    let top = cells.iter().map(|(_, r)| r.y).min().unwrap() - inner.y;
    let bottom =
        (inner.y + inner.height) - cells.iter().map(|(_, r)| r.y + r.height).max().unwrap();
    assert!(
        top > 0,
        "a short grid in a tall pane is pushed down (centred)"
    );
    assert!(
        (left as i32 - right as i32).abs() <= 1,
        "left/right margins are ~equal (centred horizontally): {left} vs {right}"
    );
    assert!(
        (top as i32 - bottom as i32).abs() <= 1,
        "top/bottom margins are ~equal (centred vertically): {top} vs {bottom}"
    );
    assert!(cols >= 1);
}

#[test]
fn grid_move_navigates_two_dimensionally() {
    use crate::action::Action;
    use crate::app::{Focus, LocalItem};
    use crate::core::model::AlbumId;
    let mut a = demo();
    a.layout = Layout::Dashboard;
    a.focus = Focus::Main;
    a.local.items = (1..=6u32)
        .map(|i| LocalItem::Album(AlbumId::new(i)))
        .collect();
    a.local.cols.set(3); // last rendered column count
    a.local.sel = 0;
    a.update(Action::GridMove(1, 0)); // → one card right
    assert_eq!(a.local.sel, 1);
    a.update(Action::GridMove(0, 1)); // → down a row (stride = 3)
    assert_eq!(a.local.sel, 4);
    a.update(Action::GridMove(0, 1)); // clamps at the last card
    assert_eq!(a.local.sel, 5);
}

#[test]
fn touchpad_scroll_navigates_the_grid_in_2d() {
    use crate::action::Motion;
    use crate::app::{Focus, LocalItem, LocalSection, MouseTarget};
    use crate::core::model::AlbumId;
    use ratatui::layout::Rect;
    let mut a = demo();
    a.layout = Layout::Dashboard;
    a.focus = Focus::Main;
    a.local.section = LocalSection::Albums;
    a.local.grid = true;
    a.local.crumb = None;
    a.local.items = (1..=6u32)
        .map(|i| LocalItem::Album(AlbumId::new(i)))
        .collect();
    a.local.cols.set(3); // last rendered column count
    a.local.sel = 0;
    assert!(a.grid_nav_active(), "the Albums cover grid uses 2-D nav");
    // a hit region so the scroll resolves to the Main grid
    a.register_click(Rect::new(0, 2, 60, 20), MouseTarget::Track(0));

    // touchpad scroll is throttled: it takes two events to commit one step (a swipe
    // fires many events, so one-per-event ran away). RIGHT → one card sideways (dx).
    a.handle_scroll(5, 5, Motion::Right);
    assert_eq!(a.local.sel, 0, "one event is below the step threshold");
    a.handle_scroll(5, 5, Motion::Right);
    assert_eq!(a.local.sel, 1, "the second event steps a card sideways");
    // two-finger DOWN → down a whole row (stride = cols = 3)
    a.handle_scroll(5, 5, Motion::Down);
    a.handle_scroll(5, 5, Motion::Down);
    assert_eq!(a.local.sel, 4, "vertical scroll steps a whole row");
    // two-finger LEFT → back a card
    a.handle_scroll(5, 5, Motion::Left);
    a.handle_scroll(5, 5, Motion::Left);
    assert_eq!(a.local.sel, 3);
}

#[test]
fn touchpad_horizontal_grid_scroll_is_row_locked() {
    // A sideways two-finger swipe stays on the row it started on: at the row's end it
    // clamps instead of wrapping onto the next row. Row changes need a vertical
    // gesture. (Keyboard h/l keeps its wrap behaviour — this is touchpad-only.)
    use crate::action::Motion;
    use crate::app::{Focus, LocalItem, LocalSection, MouseTarget};
    use crate::core::model::AlbumId;
    use ratatui::layout::Rect;
    let mut a = demo();
    a.layout = Layout::Dashboard;
    a.focus = Focus::Main;
    a.local.section = LocalSection::Albums;
    a.local.grid = true;
    a.local.items = (1..=6u32)
        .map(|i| LocalItem::Album(AlbumId::new(i)))
        .collect();
    a.local.cols.set(3); // rows: [0,1,2] and [3,4,5]
    a.local.sel = 2; // last card of row 0
    a.register_click(Rect::new(0, 2, 60, 20), MouseTarget::Track(0));

    // two RIGHT events (one throttled step) at the row's end → clamps, no wrap to 3
    a.handle_scroll(5, 5, Motion::Right);
    a.handle_scroll(5, 5, Motion::Right);
    assert_eq!(
        a.local.sel, 2,
        "horizontal scroll clamps at the row end (no wrap)"
    );

    // a deliberate vertical gesture is what crosses to the next row
    a.handle_scroll(5, 5, Motion::Down);
    a.handle_scroll(5, 5, Motion::Down);
    assert_eq!(a.local.sel, 5, "vertical scroll frees the row (2 → 5)");
}

#[test]
fn touchpad_horizontal_swipe_absorbs_vertical_jitter() {
    // While swiping sideways, stray vertical events must not leap to another row —
    // each committed horizontal step clears the pending vertical count, so the row
    // stays "locked" until a deliberate up/down gesture.
    use crate::action::Motion;
    use crate::app::{Focus, LocalItem, LocalSection, MouseTarget};
    use crate::core::model::AlbumId;
    use ratatui::layout::Rect;
    let mut a = demo();
    a.layout = Layout::Dashboard;
    a.focus = Focus::Main;
    a.local.section = LocalSection::Albums;
    a.local.grid = true;
    a.local.items = (1..=9u32)
        .map(|i| LocalItem::Album(AlbumId::new(i)))
        .collect();
    a.local.cols.set(3); // rows of 3
    a.local.sel = 0;
    a.register_click(Rect::new(0, 2, 60, 20), MouseTarget::Track(0));

    // a mostly-sideways swipe with a single stray vertical event interleaved
    a.handle_scroll(5, 5, Motion::Right);
    a.handle_scroll(5, 5, Motion::Down); // stray jitter
    a.handle_scroll(5, 5, Motion::Right); // commits a horizontal step, clears vertical
    a.handle_scroll(5, 5, Motion::Right);
    a.handle_scroll(5, 5, Motion::Right);
    // stepped sideways within row 0 only; never dropped to row 1 (would be sel >= 3)
    assert!(
        a.local.sel < 3,
        "stray vertical jitter didn't change rows (sel={})",
        a.local.sel
    );
    assert!(a.local.sel > 0, "the sideways swipe still moved");
}

#[test]
fn carousel_arrow_click_scrolls_the_row() {
    use crate::app::{Focus, LocalItem, MouseTarget};
    use crate::core::model::{AlbumId, TrackId};
    let mut a = demo();
    a.config.mouse = true;
    a.layout = Layout::Dashboard;
    a.focus = Focus::Main;
    // a grouped artist page with one ALBUMS carousel of 8 covers (more than fit),
    // so its ❯ arrow is drawn (and now clickable). The crumb marks the drill-in.
    a.local.crumb = Some("☻ Artist".into());
    let mut items = vec![
        LocalItem::Header("POPULAR"),
        LocalItem::Track(TrackId::new(1)),
        LocalItem::Header("ALBUMS"),
    ];
    for i in 1..=8u32 {
        items.push(LocalItem::Album(AlbumId::new(i)));
    }
    a.local.items = items;
    a.local.sel = 3; // first album — the carousel's left edge, so only ❯ shows

    // render so the carousel + its ❯ arrow register their click targets
    render_layout(&mut a, Layout::Dashboard, 80, 30);
    let (rect, target) = a
        .hit
        .borrow()
        .iter()
        .find_map(|(r, t)| match t {
            MouseTarget::GridScroll(i) => Some((*r, *i)),
            _ => None,
        })
        .expect("a carousel scroll arrow registered a click target");
    assert!(
        target > a.local.sel,
        "the ❯ arrow reveals a card further right"
    );
    a.handle_click(rect.x, rect.y, false);
    assert_eq!(
        a.local.sel, target,
        "clicking the arrow selects (reveals) that card — sliding the row"
    );
}

#[test]
fn touchpad_horizontal_scroll_is_a_noop_on_a_plain_list() {
    use crate::action::Motion;
    use crate::app::{Focus, MouseTarget};
    use ratatui::layout::Rect;
    let mut a = demo(); // AllTracks — a flat list, not a grid
    a.focus = Focus::Main;
    assert!(!a.grid_nav_active(), "a flat tracklist is not grid nav");
    let sel0 = a.local.sel;
    a.register_click(Rect::new(0, 2, 60, 20), MouseTarget::Track(0));
    a.handle_scroll(5, 5, Motion::Right); // sideways over a list → nothing
    assert_eq!(
        a.local.sel, sel0,
        "horizontal scroll does not move a plain list"
    );
    a.handle_scroll(5, 5, Motion::Down); // vertical still steps it
    assert_ne!(a.local.sel, sel0, "vertical scroll still moves the list");
}

#[test]
fn artist_page_albums_are_a_grid_region_with_boundary_nav() {
    use crate::action::Action;
    use crate::app::{Focus, LocalItem};
    use crate::core::model::{AlbumId, TrackId};
    let mut a = demo();
    a.layout = Layout::Dashboard;
    a.focus = Focus::Main;
    // a grouped artist page: POPULAR tracks (list) + ALBUMS (grid)
    a.local.crumb = Some("☻ Artist".into());
    a.local.items = vec![
        LocalItem::Header("POPULAR"),
        LocalItem::Track(TrackId::new(1)),
        LocalItem::Track(TrackId::new(2)),
        LocalItem::Header("ALBUMS"),
        LocalItem::Album(AlbumId::new(1)),
        LocalItem::Album(AlbumId::new(2)),
        LocalItem::Album(AlbumId::new(3)),
        LocalItem::Album(AlbumId::new(4)),
    ];
    // the release region begins at the first release header (ALBUMS, index 3)
    assert_eq!(a.artist_releases_from(), Some(3));
    a.local.cols.set(3);

    // ALBUMS is one horizontal carousel (idx 4..=7); h/l scroll/select within it
    a.local.sel = 4; // first album
    a.update(Action::GridMove(1, 0)); // → next album in the carousel
    assert_eq!(a.local.sel, 5);
    a.update(Action::GridMove(1, 0));
    a.update(Action::GridMove(1, 0));
    assert_eq!(a.local.sel, 7, "l scrolls to the carousel end");
    a.update(Action::GridMove(1, 0));
    assert_eq!(a.local.sel, 7, "l clamps at the carousel end");

    // moving up off the (only) carousel jumps back to the last POPULAR track
    a.local.sel = 4;
    a.update(Action::GridMove(0, -1));
    assert_eq!(
        a.local.sel, 2,
        "k off the album carousel → last POPULAR track"
    );

    // a plain drilled-in track list is NOT an album grid
    a.local.items = vec![
        LocalItem::Track(TrackId::new(1)),
        LocalItem::Track(TrackId::new(2)),
    ];
    assert_eq!(a.artist_releases_from(), None);
    // and neither is a top-level list (no crumb)
    a.local.crumb = None;
    a.local.items = vec![LocalItem::Album(AlbumId::new(1))];
    assert_eq!(a.artist_releases_from(), None);
}

#[test]
fn artist_page_renders_popular_list_and_album_grid() {
    use crate::app::LocalItem;
    use crate::core::model::{AlbumId, TrackId};
    let mut a = demo();
    a.local.crumb = Some("☻ Artist".into());
    a.local.items = vec![
        LocalItem::Header("POPULAR"),
        LocalItem::Track(TrackId::new(1)),
        LocalItem::Header("ALBUMS"),
        LocalItem::Album(AlbumId::new(1)),
        LocalItem::Album(AlbumId::new(2)),
    ];
    // the mixed render reaches both regions without panicking; both headers show
    let s = render_layout(&mut a, Layout::Dashboard, 120, 40);
    assert!(s.contains("POPULAR"), "the POPULAR track list renders");
    assert!(s.contains("ALBUMS"), "the ALBUMS grid section renders");
}

#[test]
fn release_carousel_shows_more_cues_at_the_edges() {
    use crate::app::LocalItem;
    use crate::core::model::AlbumId;
    let mut a = demo();
    a.layout = Layout::Dashboard;
    // an artist page with one big ALBUMS group (more covers than fit) → it's a
    // horizontal carousel; ❮/❯ scroll arrows show when more covers are off an edge.
    a.local.crumb = Some("☻ Artist".into());
    let mut items = vec![LocalItem::Header("ALBUMS")];
    items.extend((1..=20u32).map(|i| LocalItem::Album(AlbumId::new(i))));
    a.local.items = items;

    // first card selected → window at the start → more to the right only
    a.local.sel = 1;
    let s = render_layout(&mut a, Layout::Dashboard, 120, 30);
    assert!(
        s.contains('❯'),
        "a ❯ arrow shows — more covers off the right edge"
    );
    assert!(!s.contains('❮'), "no ❮ arrow at the carousel start");

    // last card selected → window at the end → more to the left only
    a.local.sel = 20;
    let s = render_layout(&mut a, Layout::Dashboard, 120, 30);
    assert!(
        s.contains('❮'),
        "a ❮ arrow shows — more covers off the left edge"
    );
    assert!(!s.contains('❯'), "no ❯ arrow at the carousel end");
}

#[test]
fn artist_page_grid_navigates_across_release_groups() {
    use crate::action::Action;
    use crate::app::{Focus, LocalItem};
    use crate::core::model::{AlbumId, TrackId};
    let mut a = demo();
    a.layout = Layout::Dashboard;
    a.focus = Focus::Main;
    // POPULAR + two release groups (ALBUMS / SINGLES & EPs), 2 cards each, cols = 2
    a.local.crumb = Some("☻ Artist".into());
    a.local.items = vec![
        LocalItem::Header("POPULAR"),
        LocalItem::Track(TrackId::new(1)),
        LocalItem::Header("ALBUMS"),
        LocalItem::Album(AlbumId::new(1)), // idx 3
        LocalItem::Album(AlbumId::new(2)), // idx 4
        LocalItem::Header("SINGLES & EPs"),
        LocalItem::Album(AlbumId::new(3)), // idx 6
        LocalItem::Album(AlbumId::new(4)), // idx 7
    ];
    assert_eq!(a.artist_releases_from(), Some(2)); // first release header (ALBUMS)
    a.local.cols.set(2);

    // each group is a horizontal carousel: j/k move BETWEEN carousels (keeping the
    // column), h/l scroll WITHIN one (clamped — no group crossing).
    a.local.sel = 3; // ALBUMS carousel, first card
    a.update(Action::GridMove(0, 1));
    assert_eq!(
        a.local.sel, 6,
        "down moves to the next group's carousel at the same column"
    );
    a.update(Action::GridMove(0, -1));
    assert_eq!(a.local.sel, 3, "up returns to the previous carousel");
    // l scrolls within ALBUMS, then clamps at its end (does NOT cross to SINGLES & EPs)
    a.update(Action::GridMove(1, 0));
    assert_eq!(a.local.sel, 4, "l moves within the ALBUMS carousel");
    a.update(Action::GridMove(1, 0));
    assert_eq!(
        a.local.sel, 4,
        "l clamps at the carousel end (no group crossing)"
    );
    // k off the very top carousel drops back into the POPULAR track list
    a.local.sel = 3;
    a.update(Action::GridMove(0, -1));
    assert_eq!(a.local.sel, 1, "k off the top carousel → the POPULAR track");
}

#[test]
fn local_grid_cards_show_the_artist_only_not_counts() {
    use crate::app::LocalItem;
    use crate::core::model::{AlbumId, ArtistId};
    use crate::ui::components::local_grid_subtitle;
    let a = demo();
    // the seeded library has album 1 ("Afterglow" by "Neon District")
    assert_eq!(
        local_grid_subtitle(&a, &LocalItem::Album(AlbumId::new(1))),
        "Neon District",
        "album card subtitle is the artist only — no year / track count"
    );
    assert!(
        local_grid_subtitle(&a, &LocalItem::Artist(ArtistId::new(1))).is_empty(),
        "artist card shows just the name — no 'N albums' count (matches Spotify)"
    );
}

#[test]
fn grid_card_title_centres_title_and_year_and_truncates_the_title() {
    use crate::ui::components::card_title_parts;
    // short title: title + " <year>" render as one centred unit (space-separated),
    // fitting within the width — the caller centres the pair, so no forced padding
    let (title, suffix) = card_title_parts("ANTI", Some(2016), 16).expect("year shown");
    assert_eq!(title, "ANTI");
    assert_eq!(suffix, " 2016", "year follows the title, space-separated");
    assert!(
        title.chars().count() + suffix.chars().count() <= 16,
        "fits the title area"
    );
    // long title: the TITLE truncates (…) — the year survives
    let (title2, suffix2) =
        card_title_parts("A Very Long Album Name Indeed", Some(2020), 16).expect("year shown");
    assert!(title2.contains('…'), "long title truncates");
    assert_eq!(suffix2, " 2020", "year kept even when the title overflows");
    assert!(
        title2.chars().count() + suffix2.chars().count() <= 16,
        "never overflows the title area"
    );
    // no year → None (the bare title is centred)
    assert!(card_title_parts("Untitled", None, 16).is_none());
    // no room for a year → None (degrade gracefully on a tiny card)
    assert!(card_title_parts("X", Some(1999), 3).is_none());
}

#[test]
fn carousel_packs_from_top_and_keeps_the_selected_shelf_visible() {
    use crate::ui::components::pack_top_row;
    // 3 shelves, each Header(h2) + Cards(h11). Row tops (rows: H,C,H,C,H,C):
    let tops = [0u16, 2, 13, 15, 26, 28];
    // everything fits → no scroll, pack from the very top
    assert_eq!(pack_top_row(&tops, 5, 39, 40), 0, "fits → top-anchored");
    // selection in the first shelf → still top-anchored (no centred gap)
    assert_eq!(pack_top_row(&tops, 1, 13, 30), 0, "first shelf → no scroll");
    // last shelf, viewport too short → scroll down by whole rows just enough to
    // reveal it (lands on shelf 1's header row, index 2)
    assert_eq!(
        pack_top_row(&tops, 5, 39, 30),
        2,
        "scrolls to reveal, whole rows"
    );
    // never scroll past the selected shelf's own header (row sel-1), even when the
    // pane is shorter than a single shelf — its title always stays on screen
    assert_eq!(
        pack_top_row(&tops, 5, 39, 12),
        4,
        "keeps the selected shelf's header"
    );
}

#[test]
fn local_grid_album_cards_carry_their_year() {
    use crate::app::LocalItem;
    use crate::core::model::{AlbumId, ArtistId};
    use crate::ui::components::local_grid_year;
    let a = demo();
    // the seeded album 1 ("Afterglow") is from 2025
    assert_eq!(
        local_grid_year(&a, &LocalItem::Album(AlbumId::new(1))),
        Some(2025),
        "album card carries its release year"
    );
    assert_eq!(
        local_grid_year(&a, &LocalItem::Artist(ArtistId::new(1))),
        None,
        "artist cards have no year"
    );
}

#[test]
fn popular_track_meta_is_artist_album_year() {
    use crate::ui::components::track_meta;
    assert_eq!(
        track_meta("Adele", "21", Some(2011)),
        "Adele  ·  21  ·  2011"
    );
    assert_eq!(
        track_meta("Adele", "", None),
        "Adele",
        "empty album/year skipped"
    );
    assert_eq!(
        track_meta("", "Nevermind", Some(1991)),
        "Nevermind  ·  1991"
    );
    assert_eq!(track_meta("", "", None), "");
}

#[test]
fn circle_crop_makes_corners_transparent() {
    // circle art masks its corners to transparent (not a baked bg colour) so the
    // panel behind shows through. Under Kitty that also makes a theme change free:
    // the same unchanged image simply shows the new panel, no re-crop. Other
    // protocols composite against the *terminal* background rather than the cell,
    // so there the picker underlays the panel colour at encode time instead — see
    // `AppState::art_needs_opaque_bg`.
    use image::{DynamicImage, Rgba, RgbaImage};
    let src = DynamicImage::ImageRgba8(RgbaImage::from_pixel(20, 20, Rgba([200, 100, 50, 255])));
    let out = crate::spotify::artwork::circle_crop(src).to_rgba8();
    assert_eq!(
        out.get_pixel(0, 0).0[3],
        0,
        "the corner is fully transparent"
    );
    assert_eq!(out.get_pixel(10, 10).0[3], 255, "the centre stays opaque");
}

#[test]
fn error_toasts_linger_and_the_last_error_is_copyable() {
    let mut a = demo();
    a.notify_error("Couldn't update Liked Songs: Spotify error 411".into());
    // stashed for copying, and the toast lingers far longer than a normal one
    assert_eq!(
        a.last_error.as_deref(),
        Some("Couldn't update Liked Songs: Spotify error 411")
    );
    let n = a.notification.as_ref().expect("an error toast is shown");
    assert!(
        n.ttl_ticks > 180,
        "an error toast lingers longer than a normal notification"
    );
    assert!(n.text.contains("411"));
    // copying with no error gives a graceful message (and no clipboard write)
    let mut b = demo();
    b.copy_last_error();
    assert_eq!(b.notification.as_ref().unwrap().text, "No error to copy");
}

#[test]
fn errors_are_logged_and_the_overlay_toggles() {
    use crate::action::Action;
    let mut a = demo();
    assert!(a.error_log.is_empty());
    a.notify_error("first".into()); // toast + log
    a.log_error("second".into()); // log only
    assert_eq!(a.error_log.len(), 2);
    assert_eq!(a.error_log.back().unwrap().msg, "second");
    assert_eq!(
        a.last_error.as_deref(),
        Some("second"),
        "latest is copyable"
    );
    // the ring is bounded
    for i in 0..crate::app::ERROR_LOG_CAP + 25 {
        a.log_error(format!("e{i}"));
    }
    assert_eq!(
        a.error_log.len(),
        crate::app::ERROR_LOG_CAP,
        "the error log is a bounded ring"
    );
    // the Info overlay toggles open (on the Health tab) / closed
    a.update(Action::ToggleErrorLog);
    assert!(
        matches!(
            a.info.as_ref().map(|i| i.tab),
            Some(crate::app::InfoTab::Health)
        ),
        "toggled open on the Health tab"
    );
    a.update(Action::ToggleErrorLog);
    assert!(a.info.is_none(), "toggled closed");
}

#[test]
fn switching_theme_keeps_the_art_cache() {
    // circle covers have transparent corners, so the new panel just shows through the
    // same images — a theme switch must KEEP the cache (no wasteful re-decode/re-fetch)
    // and let the covers recolor on the next render.
    use crate::artwork::{ArtKey, ArtSource};
    let mut a = demo();
    a.request_art(ArtKey::Remote(1), ArtSource::Url("u".into()), true);
    assert!(!a.grid_art.borrow().is_empty(), "art was cached");
    a.set_theme("aurora");
    assert!(
        !a.grid_art.borrow().is_empty(),
        "a theme switch keeps the cached art (transparent corners recolor for free)"
    );
}

#[test]
fn hash_toggles_grid_and_list() {
    use crate::action::Action;
    use crate::app::LocalSection;
    let mut a = demo();
    a.layout = Layout::Dashboard;
    a.local.section = LocalSection::Albums;
    a.local_load_section();
    assert!(a.local.grid, "Albums default to the cover grid");
    assert!(
        a.local_grid_active(),
        "the grid is active on the Albums section"
    );
    a.update(Action::ToggleGridView);
    assert!(
        !a.local.grid && !a.local_grid_active(),
        "# switches to a list"
    );
}

#[test]
fn grid_art_cache_bounds_and_evicts_least_recently_used() {
    use crate::artwork::{ArtKey, ArtSource};
    const CAP: usize = 256; // GRID_ART_CAP
    let a = demo();
    let src = || ArtSource::Url("u".into());
    // fill the cache to its cap with distinct remote keys
    for i in 0..CAP as u64 {
        a.request_art(ArtKey::Remote(i), src(), false);
    }
    assert_eq!(a.grid_art.borrow().len(), CAP);
    // touch key 0 (now most-recent), then insert a new key → eviction runs
    a.request_art(ArtKey::Remote(0), src(), false); // bump recency
    a.request_art(ArtKey::Remote(9999), src(), false); // new → triggers eviction
    let cache = a.grid_art.borrow();
    assert_eq!(cache.len(), CAP, "the cache stays bounded at the cap");
    assert!(
        cache.contains_key(&ArtKey::Remote(0)),
        "a recently-used key survives eviction"
    );
    assert!(
        !cache.contains_key(&ArtKey::Remote(1)),
        "the least-recently-used key is evicted first"
    );
    assert!(
        cache.contains_key(&ArtKey::Remote(9999)),
        "the newly requested key is inserted"
    );
}

#[test]
fn grid_art_requests_per_item_and_coalesces() {
    use crate::app::{ArtThumb, LocalSection};
    use crate::artwork::ArtKey;
    let mut a = demo();
    a.layout = Layout::Dashboard;

    // an Album card → an Album art key; shape follows the global grid_circle
    a.local.section = LocalSection::Albums;
    a.local_load_section();
    let album = a.local.items[0].clone();
    let (key, src, circle) = a.item_art(&album).expect("album has an art request");
    assert!(matches!(key, ArtKey::Album(_)));
    assert!(circle, "default shape is circle (grid_circle on)");
    a.request_art(key, src, circle);
    assert!(
        matches!(a.grid_art.borrow().get(&key), Some((_, ArtThumb::Pending))),
        "requesting marks the thumbnail pending"
    );
    // a second request for the same item is coalesced (no duplicate entry)
    let (k2, s2, c2) = a.item_art(&album).unwrap();
    a.request_art(k2, s2, c2);
    assert_eq!(a.grid_art.borrow().len(), 1, "duplicate request coalesced");

    // the shape toggle applies UNIFORMLY to every card (album + artist) and clears
    // the cache so thumbnails re-decode in the new shape
    a.toggle_grid_shape();
    assert!(!a.config.grid_circle, "toggle switches to rounded squares");
    assert!(
        a.grid_art.borrow().is_empty(),
        "toggling shape clears the cache"
    );
    let (_, _, album_circle) = a.item_art(&a.local.items[0].clone()).unwrap();
    assert!(!album_circle, "albums are rounded too once toggled off");
    a.local.section = LocalSection::Artists;
    a.local_load_section();
    let artist = a.local.items[0].clone();
    let (akey, _, acircle) = a.item_art(&artist).expect("artist has an art request");
    assert!(matches!(akey, ArtKey::Artist(_)));
    assert!(!acircle, "artists follow the same global shape");
}

#[test]
fn grid_choice_is_remembered_per_section() {
    use crate::action::Action;
    use crate::app::LocalSection;
    let mut a = demo();
    a.layout = Layout::Dashboard;
    a.local.section = LocalSection::Albums;
    a.local_load_section();
    assert!(a.local.grid, "Albums default to the grid");
    a.update(Action::ToggleGridView); // → list, remembered for Albums
    assert!(!a.local.grid);
    assert_eq!(a.views.grid.get(&LocalSection::Albums), Some(&false));

    // Artists keep their own default; switching back to Albums restores the override
    a.local.section = LocalSection::Artists;
    a.local_load_section();
    assert!(a.local.grid, "Artists still default to the grid");
    a.local.section = LocalSection::Albums;
    a.local_load_section();
    assert!(!a.local.grid, "Albums kept the per-section list choice");
}

#[test]
fn artist_pane_photo_uses_a_distinct_key_from_the_grid_card() {
    // the now-playing artist can be an on-screen grid card too; the pane photo
    // (large) and the card (small) must own SEPARATE image protocols or they thrash
    // one shared resize cache (small image + leftover placeholder + jitter).
    use crate::app::LocalSection;
    use crate::artwork::ArtKey;
    let mut a = demo();
    a.local.section = LocalSection::Artists;
    a.local_load_section();
    let card = a.local.items[0].clone();
    let (grid_key, _, _) = a.item_art(&card).expect("artist card has art");
    let ArtKey::Artist(id) = grid_key else {
        panic!("an artist card keys as ArtKey::Artist");
    };
    let (pane_key, _, _) = a.artist_pane_art(id).expect("artist pane has art");
    assert!(matches!(pane_key, ArtKey::ArtistPhoto(_)));
    assert_ne!(
        grid_key, pane_key,
        "pane photo + grid card own separate protocols"
    );
}

#[test]
fn spotify_grid_move_navigates_two_dimensionally() {
    use crate::action::Action;
    use crate::app::Focus;
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.focus = Focus::Main;
    a.spotify.items = (1..=6)
        .map(|i| Item {
            name: format!("Album {i}"),
            kind: Kind::Album,
            ..Default::default()
        })
        .collect();
    a.spotify.cols.set(3); // last rendered column count
    a.spotify.sel = 0;
    a.update(Action::GridMove(1, 0)); // → one card right
    assert_eq!(a.spotify.sel, 1);
    a.update(Action::GridMove(0, 1)); // → down a row (stride = 3)
    assert_eq!(a.spotify.sel, 4);
    a.update(Action::GridMove(0, 1)); // clamps at the last card
    assert_eq!(a.spotify.sel, 5);
}

#[test]
fn spotify_hash_toggles_grid_and_list() {
    use crate::action::Action;
    use crate::spotify::api::{Item, Kind, Section};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.section = Section::Albums;
    a.spotify_load_section(); // sets grid from the per-section default
    assert!(a.spotify.grid, "Albums default to the cover grid");
    a.spotify.items = vec![Item {
        name: "An Album".into(),
        kind: Kind::Album,
        ..Default::default()
    }];
    assert!(
        a.spotify_grid_active(),
        "the grid is active on the Albums section with items"
    );
    a.update(Action::ToggleGridView);
    assert!(
        !a.spotify.grid && !a.spotify_grid_active(),
        "# switches to a list"
    );
    assert_eq!(a.views.spotify_grid.get(&Section::Albums), Some(&false));
}

#[test]
fn spotify_playlists_section_defaults_to_a_cover_grid() {
    use crate::action::Action;
    use crate::spotify::api::{Item, Kind, Section};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.section = Section::Playlists;
    a.spotify_load_section(); // sets grid from the per-section default
    assert!(
        a.spotify.grid,
        "Playlists default to the cover grid, like Albums"
    );
    // playlists carry real cover art (owner as the subtitle), so grid cards render
    a.spotify.items = vec![Item {
        name: "My Mix".into(),
        subtitle: "me".into(),
        image: Some("https://i.scdn.co/image/cover".into()),
        kind: Kind::Playlist,
        ..Default::default()
    }];
    assert!(
        a.spotify_grid_active(),
        "the grid is active on the Playlists section with items"
    );
    a.update(Action::ToggleGridView);
    assert!(
        !a.spotify_grid_active(),
        "# switches the Playlists section to a list too"
    );
    assert_eq!(a.views.spotify_grid.get(&Section::Playlists), Some(&false));
}

#[test]
fn spotify_grid_choice_is_remembered_per_section() {
    use crate::action::Action;
    use crate::spotify::api::Section;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.section = Section::Albums;
    a.spotify_load_section();
    assert!(a.spotify.grid, "Albums default to the grid");
    a.update(Action::ToggleGridView); // → list, remembered for Albums
    assert!(!a.spotify.grid);

    // Artists keep their own default; switching back to Albums restores the override
    a.spotify.section = Section::Artists;
    a.spotify_load_section();
    assert!(a.spotify.grid, "Artists still default to the grid");
    a.spotify.section = Section::Albums;
    a.spotify_load_section();
    assert!(!a.spotify.grid, "Albums kept the per-section list choice");

    // a non-container section is never a grid
    a.spotify.section = Section::LikedSongs;
    a.spotify_load_section();
    assert!(!a.spotify.grid, "Liked Songs is a list, not a grid");
}

#[test]
fn home_uses_the_standard_shell() {
    // Home now renders through the shared browser_shell: a FIXED bordered
    // LIBRARY sidebar + a titled MUSIC main pane (like the Spotify view).
    let mut app = demo();
    let s = render_layout(&mut app, Layout::Dashboard, 100, 12);
    assert!(
        s.contains("LIBRARY"),
        "the LIBRARY sidebar is a fixed left column on Home"
    );
    assert!(
        s.contains("MUSIC"),
        "the main pane shows the standardized MUSIC · … title"
    );
    assert!(
        s.contains("Midnight Protocol"),
        "the tracklist renders in the main pane"
    );
}

#[test]
fn panel_size_adjustable_and_per_view() {
    use crate::app::Panel;
    let mut app = demo();
    app.layout = Layout::FullPlayer;
    let before = app.panel(Panel::Queue).size;
    app.resize_panel(Panel::Queue, 1);
    assert!(app.panel(Panel::Queue).size > before, "queue size grows");
    // other views keep their own (default) size
    assert_eq!(
        app.panel_in(Layout::Dashboard, Panel::Queue).size,
        Layout::Dashboard.default_panel(Panel::Queue).size
    );
}

#[test]
fn visualizer_mode_is_per_view() {
    use crate::action::Action;
    let mut app = demo();
    app.layout = Layout::FullPlayer;
    app.update(Action::CycleVisualizer); // Playing → mode 1
    app.update(Action::CycleVisualizer); // Playing → mode 2
    assert_eq!(app.viz_mode(), 2);
    // a different view keeps its own (default) mode
    app.layout = Layout::LyricsFocus;
    assert_eq!(app.viz_mode(), 0, "Lyrics unaffected by Playing");
    app.update(Action::CycleVisualizer); // Lyrics → 1
    assert_eq!(app.viz_mode(), 1);
    // Playing still on its own mode
    app.layout = Layout::FullPlayer;
    assert_eq!(app.viz_mode(), 2);
}

#[test]
fn short_album_browse_shows_index_numbers() {
    let mut a = demo();
    hide_panels(&mut a, Layout::Dashboard); // title must appear only in the tracklist
    a.config.track_columns = true; // the index column only exists in the column layout
    a.player.current = None; // so the first row shows its index, not the ▶ marker
    let known = a
        .library
        .tracks
        .values()
        .find(|t| t.title == "Midnight Protocol")
        .map(|t| t.id)
        .expect("demo track");
    // a short (3-track) album → index_w == 1; the index must still render.
    let others: Vec<_> = a
        .library
        .tracks
        .values()
        .filter(|t| t.id != known)
        .take(2)
        .map(|t| t.id)
        .collect();
    let mut ids = vec![known];
    ids.extend(others);
    a.browser.list = ids; // a browsed list of 3 → the tracklist shows it

    // Dashboard (queue hidden by default) so the title appears only in the
    // tracklist — Split would also show it in the queue pane (with a ▶). Wide
    // enough that the title isn't truncated.
    let s = render_layout(&mut a, Layout::Dashboard, 160, 30);
    let line = s
        .lines()
        .find(|l| l.contains("Midnight Protocol"))
        .expect("the browsed track's row");
    let before = &line[..line.find("Midnight Protocol").unwrap()];
    assert!(
        before.chars().any(|c| c.is_ascii_digit()),
        "the index column renders a number, not an empty column: {line:?}"
    );
}

#[test]
fn each_view_remembers_its_cursor() {
    use crate::action::Action;
    let mut a = demo();
    a.layout = Layout::Dashboard;
    a.selection = 7;
    // move to another view and put its highlight elsewhere
    a.update(Action::SwitchLayout(Layout::FullPlayer));
    a.selection = 2;
    // back to Dashboard → its own highlight (7) is restored
    a.update(Action::SwitchLayout(Layout::Dashboard));
    assert_eq!(a.selection, 7, "Dashboard restored its highlight");
    // …and the other view still remembers its own (2)
    a.update(Action::SwitchLayout(Layout::FullPlayer));
    assert_eq!(a.selection, 2, "the other view kept its own highlight");
}

#[test]
fn panel_toggle_keys_are_unshifted() {
    use crate::action::Action;
    use crate::event::{Key, KeyCode, Mods};
    let app = demo();
    let k = |c: char| {
        crate::keymap::map(
            &app,
            Key {
                code: KeyCode::Char(c),
                mods: Mods::default(),
            },
        )
    };
    assert!(matches!(k('u'), Action::ToggleQueue), "u toggles queue");
    assert!(
        matches!(k('q'), Action::Quit),
        "q is quit-only (back / up-one-level is esc + ctrl-o)"
    );
    assert!(
        matches!(k('i'), Action::ToggleArtistInfo),
        "i toggles artist"
    );
    assert!(matches!(k('Q'), Action::Quit), "shift-Q force-quits");
    assert!(matches!(k('I'), Action::ToggleStats), "shift-I = stats");
}

#[test]
fn switching_views_clamps_focus_to_a_valid_region() {
    use crate::action::Action;
    use crate::app::{Focus, Panel};
    let mut a = demo();
    // park focus on a pane the target view never exposes (the Dashboard has no
    // Visualizer pane). Focus is one shared field now, so the switch must clamp it.
    a.layout = Layout::Spotify;
    a.focus = Focus::Pane(Panel::Visualizer);
    a.update(Action::SwitchLayout(Layout::Dashboard));
    assert!(
        !matches!(a.focus, Focus::Pane(Panel::Visualizer)),
        "focus clamps off a region the Dashboard doesn't expose, got {:?}",
        a.focus
    );
    // a focus the target view DOES expose is preserved across the switch
    a.focus = Focus::Main;
    a.update(Action::SwitchLayout(Layout::Spotify));
    assert_eq!(a.focus, Focus::Main, "a valid focus survives the switch");
}

/// A modal overlay gates mouse clicks on the view behind it: base-view regions
/// (registered before the overlay boundary) are ignored while the modal is open,
/// but the overlay's own controls (registered after) stay clickable.
#[test]
fn a_modal_gates_base_view_mouse_clicks() {
    use crate::app::MouseTarget;
    use ratatui::layout::Rect;
    let mut a = demo();

    // no modal → a base-view control reacts to a click
    let vol = Rect::new(10, 5, 10, 1);
    a.player.volume = 50;
    a.register_click(vol, MouseTarget::Volume);
    a.handle_click(15, 5, false);
    assert_ne!(
        a.player.volume, 50,
        "base control is clickable with no modal"
    );

    // simulate a frame that draws a modal on top of the base view
    a.clear_hits();
    a.player.volume = 50;
    a.register_click(vol, MouseTarget::Volume); // base region (below the boundary)
    a.mark_overlay_hits(); // overlay regions start here
    let row = Rect::new(0, 0, 20, 1);
    a.register_click(row, MouseTarget::SettingRow(3)); // an overlay control
    a.settings.overlay = true; // modal_open() is now true

    // a click on the base region is ignored…
    a.handle_click(15, 5, false);
    assert_eq!(
        a.player.volume, 50,
        "base-view clicks are ignored while a modal is open"
    );
    // …but the overlay's own control still works
    a.handle_click(5, 0, false);
    assert_eq!(
        a.settings.sel, 3,
        "overlay controls stay clickable under a modal"
    );
}

/// The command palette's rows are clickable: a click selects the row (double-click
/// runs it). Also proves overlay controls work while the palette modal is open.
#[test]
fn command_palette_rows_are_clickable() {
    use crate::action::Action;
    use crate::app::MouseTarget;
    use ratatui::layout::Rect;
    let mut a = demo();
    a.update(Action::OpenPalette);
    assert!(a.palette.is_some(), "palette is open");

    // simulate the palette drawn as an overlay with a clickable command row
    a.clear_hits();
    a.mark_overlay_hits(); // overlay boundary
    a.register_click(Rect::new(0, 3, 30, 1), MouseTarget::PaletteRow(4));
    a.handle_click(5, 3, false);
    assert_eq!(
        a.palette.as_ref().unwrap().sel,
        4,
        "clicking a palette row selects it"
    );
}

/// A mouse interaction that hits a target must mark the frame dirty so the screen
/// repaints — otherwise the selection/scroll change is invisible until something
/// else forces a redraw. (Regression: mouse handlers didn't set `dirty`, unlike the
/// keyboard `update` path, so clicks looked dead while nothing was animating.)
#[test]
fn mouse_interactions_request_a_repaint() {
    use crate::action::Motion;

    // click a tracklist row → dirty
    let mut a = demo();
    let _ = render_layout(&mut a, Layout::Dashboard, 120, 36);
    let _ = a.take_dirty(); // clear setup dirt
    a.handle_click(50, 3, false); // Track row lives around x27..88
    assert!(a.take_dirty(), "a click on a row requests a repaint");

    // scroll over the list → dirty
    let _ = render_layout(&mut a, Layout::Dashboard, 120, 36);
    let _ = a.take_dirty();
    a.handle_scroll(50, 3, Motion::Down);
    assert!(a.take_dirty(), "a scroll over a list requests a repaint");

    // a click that hits no target does NOT force a needless repaint
    a.clear_hits(); // empty hit map → the click resolves to nothing
    let _ = a.take_dirty();
    a.handle_click(50, 3, false);
    assert!(!a.take_dirty(), "a miss doesn't request a needless repaint");
}

#[test]
fn lyrics_pane_focus_advertises_the_sync_shortcut() {
    use crate::app::{Focus, Panel};
    let mut a = demo();
    a.notification = None; // demo's startup toast otherwise owns the left zone

    // the dedicated Lyrics view carries the sync keys in its base hint
    let s = render_layout(&mut a, Layout::LyricsFocus, 140, 36);
    assert!(s.contains("sync"), "the Lyrics view advertises , . sync");

    // Dashboard: shown only while the Lyrics pane holds focus (via browse_hints)
    a.focus = Focus::Pane(Panel::Lyrics);
    let s = render_layout(&mut a, Layout::Dashboard, 140, 36);
    assert!(
        s.contains("sync"),
        "a focused Lyrics pane shows the sync hint"
    );

    // a different focused pane must not show it (no false positives)
    a.focus = Focus::Pane(Panel::Queue);
    let s = render_layout(&mut a, Layout::Dashboard, 140, 36);
    assert!(!s.contains("sync"), "the Queue pane hint has no sync key");

    // the Library view's per-view hint isn't focus-aware, but we still surface it
    a.focus = Focus::Pane(Panel::Lyrics);
    let s = render_layout(&mut a, Layout::LibraryFocus, 140, 36);
    assert!(
        s.contains("sync"),
        "the Library view's Lyrics pane shows the sync hint"
    );
}
