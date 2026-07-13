//! Mouse_session snapshot/behaviour tests, split out of snapshot.rs.

use super::*;

#[test]
fn radio_mouse_clicks_play_and_filter() {
    use crate::app::MouseTarget;
    use crate::radio::Station;
    let mk = |n: &str, u: &str| Station {
        name: n.into(),
        url: format!("http://x/{u}"),
        uuid: u.into(),
        ..Default::default()
    };
    let mut a = demo();
    a.config.mouse = true;
    a.layout = Layout::Radio;
    a.radio.stations = vec![mk("Alpha", "u1"), mk("Beta", "u2"), mk("Gamma", "u3")];
    // render to build the hit-map
    let _ = render_layout(&mut a, Layout::Radio, 100, 20);
    // double-click the 2nd station → it starts streaming
    let row = a
        .hit
        .borrow()
        .iter()
        .find(|(_, t)| matches!(t, MouseTarget::RadioRow(1)))
        .map(|(r, _)| *r)
        .expect("station row 1 is clickable");
    a.handle_click(row.x + 1, row.y, true);
    assert!(
        a.rnow
            .now_station
            .as_ref()
            .is_some_and(|s| s.name == "Beta"),
        "double-click plays the clicked station"
    );
    assert!(!a.rnow.radio_paused, "double-clicked station is streaming");

    // click the Country chip → the country picker opens
    let _ = render_layout(&mut a, Layout::Radio, 100, 20);
    let chip = a
        .hit
        .borrow()
        .iter()
        .find(|(_, t)| matches!(t, MouseTarget::RadioChip(1)))
        .map(|(r, _)| *r)
        .expect("country chip is clickable");
    a.handle_click(chip.x, chip.y, false);
    assert!(
        a.radio.picker.is_some(),
        "clicking Country opens the picker"
    );
}

#[test]
fn session_roundtrip_restores_state() {
    use crate::core::player::Repeat;
    use crate::core::player::Status;
    let mut a = demo();
    a.layout = Layout::LyricsFocus;
    a.selection = 3;
    a.views.viz_modes.insert(Layout::LyricsFocus, 2);
    a.player.elapsed = std::time::Duration::from_secs(42);
    a.player.shuffle = true;
    a.player.repeat = Repeat::All;
    a.player.speed = 1.25;
    let cur = a.player.current.unwrap();
    let saved = a.session();

    // a fresh app with the same (demo) library should restore by path
    let mut b = demo(); // throwaway dir (seeds the demo library); never the real config
    b.apply_session(saved);
    assert_eq!(b.layout, Layout::LyricsFocus);
    assert_eq!(b.selection, 3);
    assert_eq!(b.views.viz_modes.get(&Layout::LyricsFocus), Some(&2));
    assert_eq!(b.player.current, Some(cur));
    assert_eq!(b.player.elapsed.as_secs(), 42);
    assert_eq!(b.player.status, Status::Paused);
    assert!(b.player.shuffle);
    assert_eq!(b.player.repeat, Repeat::All);
    assert_eq!(b.player.speed, 1.25);
}

#[test]
fn panel_visibility_survives_a_session_round_trip() {
    // regression: panel_from_key dropped the "lyrics" key, so the Lyrics pane
    // reverted to its (hidden) default on reopen. Every panel key must restore.
    use crate::app::{Dock, Panel, PanelCfg};
    let mut a = demo();
    a.views.panels.insert(
        (Layout::Spotify, Panel::Lyrics),
        PanelCfg {
            shown: true,
            dock: Dock::Bottom,
            size: 24,
            len: 50,
        },
    );
    let saved = a.session();

    let mut b = demo(); // throwaway dir (seeds the demo library); never the real config
    b.apply_session(saved);
    let cfg = b.panel_in(Layout::Spotify, Panel::Lyrics);
    assert!(
        cfg.shown,
        "Lyrics pane visibility restored across a session"
    );
    assert_eq!(cfg.dock, Dock::Bottom, "dock restored");
    assert_eq!(cfg.size, 24, "size restored");
}

#[test]
fn mouse_click_selects_then_double_click_plays() {
    use crate::app::MouseTarget;
    use ratatui::layout::Rect;
    let mut app = demo(); // Home (Dashboard), AllTracks section loaded
    // the Dashboard main list is the drill-in `local.items`: a Track click moves
    // `local.sel`, a double-click plays via local_activate.
    app.register_click(Rect::new(0, 5, 40, 1), MouseTarget::Track(3));
    app.handle_click(10, 5, false); // single → select
    assert_eq!(app.local.sel, 3);
    let expected = match app.local.items.get(3) {
        Some(crate::app::LocalItem::Track(id)) => Some(*id),
        _ => None,
    };
    app.handle_click(10, 5, true); // double → play
    assert_eq!(app.player.current, expected);
}

#[test]
fn scroll_targets_box_under_pointer() {
    use crate::action::Motion;
    use crate::app::{Focus, LocalSection, MouseTarget, ScrollBox};
    use ratatui::layout::Rect;
    let mut app = demo();
    app.focus = Focus::Main;
    let sel0 = app.local.sel;
    assert_eq!(app.local.section, LocalSection::AllTracks);
    // pointer over the section-sidebar Tree region → scroll the section list,
    // not the main pane
    app.register_click(
        Rect::new(0, 2, 26, 10),
        MouseTarget::Scroll(ScrollBox::Tree),
    );
    app.handle_scroll(5, 5, Motion::Down);
    assert_eq!(app.focus, Focus::Sidebar);
    assert_eq!(app.local.section, LocalSection::ALL[1], "section advanced");
    assert_eq!(app.local.sel, sel0, "main-pane selection untouched");
}

#[test]
fn clicking_a_scrolled_row_does_not_jump() {
    use crate::action::Motion;
    use crate::app::{Focus, MouseTarget};
    let mut app = demo();
    app.focus = Focus::Main;
    // scroll to the bottom so the viewport offset is non-zero
    for _ in 0..app.display_ids().len() {
        app.update(crate::action::Action::Move(Motion::Down));
    }
    // short window → the 14-track demo list must scroll
    render_layout(&mut app, Layout::Dashboard, 80, 12);
    let off_before = app.scroll.list.get();
    assert!(off_before > 0, "the tracklist scrolled");

    // click the topmost *visible* row; it should select that row…
    let (rect, idx) = app
        .hit
        .borrow()
        .iter()
        .find_map(|(r, t)| match t {
            MouseTarget::Track(i) => Some((*r, *i)),
            _ => None,
        })
        .expect("a visible track row was registered");
    app.handle_click(rect.x + 1, rect.y, false);
    assert_eq!(app.local.sel, idx, "selects the row under the cursor");

    // …without the viewport jumping (the bug): the sticky offset holds.
    render_layout(&mut app, Layout::Dashboard, 80, 12);
    assert_eq!(
        app.scroll.list.get(),
        off_before,
        "viewport stayed put on click"
    );
}

#[test]
fn clicking_a_scrolled_queue_row_does_not_jump() {
    // The QUEUE pane used to recentre the anchor every frame, so clicking a visible
    // row snapped it to the pane's middle. It now shares the flat list's sticky
    // offset (`scroll.queue`), so a clicked row stays where it is. Covers the shared
    // `queue_pane` (local + Spotify queues render through it).
    use crate::app::{Focus, MouseTarget, Panel};
    let mut app = demo();
    app.config.mouse = true;
    app.layout = Layout::Dashboard;
    if !app.panel(Panel::Queue).shown {
        app.toggle_panel(Panel::Queue);
    }
    // enlarge the queue well past any pane height so it must scroll regardless of
    // the layout's split, then park the cursor on the last row.
    let base = app.player.queue.items.clone();
    for _ in 0..4 {
        app.player.queue.items.extend(base.iter().copied());
    }
    app.set_focus(Focus::Pane(Panel::Queue));
    app.queue_sel = app.player.queue.items.len() - 1;

    render_layout(&mut app, Layout::Dashboard, 120, 30);
    let off_before = app.scroll.queue.get();
    assert!(off_before > 0, "the queue scrolled");

    // click the topmost *visible* queue row; it selects that row in place…
    let (rect, idx) = app
        .hit
        .borrow()
        .iter()
        .find_map(|(r, t)| match t {
            MouseTarget::QueueRow(i) => Some((*r, *i)),
            _ => None,
        })
        .expect("a visible queue row was registered");
    app.handle_click(rect.x + 1, rect.y, false);
    assert_eq!(app.queue_sel, idx, "selects the queue row under the cursor");

    // …without the queue jumping to recentre the click.
    render_layout(&mut app, Layout::Dashboard, 120, 30);
    assert_eq!(
        app.scroll.queue.get(),
        off_before,
        "queue viewport stayed put on click"
    );
}

#[test]
fn clicking_a_scrolled_radio_row_does_not_jump() {
    // The radio station list had the same recentring bug; it now uses a sticky
    // offset (`radio.list_off`).
    use crate::app::MouseTarget;
    use crate::radio::Station;
    let mut app = demo();
    app.config.mouse = true;
    app.layout = Layout::Radio;
    app.radio.stations = (0..40)
        .map(|i| Station {
            name: format!("Station {i}"),
            url: format!("http://x/{i}"),
            uuid: i.to_string(),
            ..Default::default()
        })
        .collect();
    app.radio.sel = app.radio.stations.len() - 1; // park on the last row → scrolled

    render_layout(&mut app, Layout::Radio, 100, 20);
    let off_before = app.radio.list_off.get();
    assert!(off_before > 0, "the station list scrolled");

    let (rect, idx) = app
        .hit
        .borrow()
        .iter()
        .find_map(|(r, t)| match t {
            MouseTarget::RadioRow(i) => Some((*r, *i)),
            _ => None,
        })
        .expect("a visible station row was registered");
    app.handle_click(rect.x + 1, rect.y, false);
    assert_eq!(app.radio.sel, idx, "selects the station under the cursor");

    render_layout(&mut app, Layout::Radio, 100, 20);
    assert_eq!(
        app.radio.list_off.get(),
        off_before,
        "station list stayed put on click"
    );
}

#[test]
fn mouse_click_progress_bar_seeks() {
    use crate::app::MouseTarget;
    use ratatui::layout::Rect;
    let mut app = demo();
    app.player.duration = std::time::Duration::from_secs(100);
    app.register_click(Rect::new(10, 8, 20, 1), MouseTarget::Seek); // 20-wide bar at x=10
    app.handle_click(20, 8, false); // halfway → ~50s
    let secs = app.player.elapsed.as_secs() as i64;
    assert!((secs - 50).abs() <= 2, "seeked to ~50s, got {secs}");
}

#[test]
fn library_cache_roundtrips() {
    use crate::library::store::LibraryCache;
    let app = demo();
    let cache = LibraryCache::from_library(&app.library);
    assert_eq!(cache.tracks.len(), app.library.tracks.len());
    let json = serde_json::to_string(&cache).unwrap();
    let back: LibraryCache = serde_json::from_str(&json).unwrap();
    assert_eq!(back.tracks.len(), cache.tracks.len());
}

#[test]
fn fresh_build_restores_now_playing_despite_a_cache_miss() {
    // Reproduces the "every new build forgets the last track" bug: when the
    // on-disk library cache fails to load (e.g. a struct changed across a
    // rebuild), seed_demo() leaves a *placeholder* current track. The saved
    // session's now-playing isn't in the demo, so the initial restore misses —
    // and when the background scan lands the real library, set_library must NOT
    // treat the demo placeholder as a live track (which stranded it on the first
    // real track); it must restore the saved now-playing by path.
    use crate::core::model::{Track, TrackId};
    use crate::session::Session;
    let trk = |id: u32, path: &str, title: &str| Track {
        id: TrackId::new(id),
        path: std::path::PathBuf::from(path),
        title: title.into(),
        artist: title.into(),
        album: "X".into(),
        album_artist: "X".into(),
        album_id: None,
        artist_id: None,
        track_no: 0,
        disc_no: 0,
        track_total: 0,
        disc_total: 0,
        duration_ms: 200_000,
        year: None,
        genre: None,
        composer: String::new(),
        comment: String::new(),
        audio: None,
        rating: 0,
        favorite: false,
        play_count: 0,
        added_at: 0,
        last_played: 0,
    };
    let saved = "/music/real/Halsey - Without Me.flac";
    let mut a = demo(); // seed_demo() sets a placeholder current (demo track 1)
    let sess = Session {
        current_path: Some(saved.to_string()),
        elapsed_secs: Some(42),
        ..Session::default()
    };
    a.apply_session(sess); // restores against the demo library → misses the path
    // the real library lands (the saved track is NOT first alphabetically)
    a.set_library(vec![
        trk(1, "/music/real/Aaa.flac", "Aaa"),
        trk(2, saved, "Halsey"),
    ]);
    let cur = a
        .player
        .current
        .and_then(|id| a.library.track(id))
        .map(|t| t.path.to_string_lossy().into_owned());
    assert_eq!(
        cur.as_deref(),
        Some(saved),
        "the saved now-playing is restored, not reset to the first track"
    );
    assert_eq!(a.player.elapsed, std::time::Duration::from_secs(42));
}
