//! Overlays snapshot/behaviour tests, split out of snapshot.rs.

use super::*;

#[test]
fn radio_status_bar_hides_shuffle_and_repeat() {
    use crate::core::player::Repeat;
    let mut a = demo();
    a.player.repeat = Repeat::All;
    a.player.shuffle = true;
    // a normal view surfaces the repeat state in the status bar…
    let dash = render_layout(&mut a, Layout::Dashboard, 100, 20);
    assert!(
        dash.contains("all"),
        "dashboard status bar shows repeat=all"
    );
    // …but the Radio view drops shuffle/repeat (queue concepts, not for streams)
    let radio = render_layout(&mut a, Layout::Radio, 100, 20);
    assert!(!radio.contains("all"), "radio status bar omits repeat");
}

#[test]
fn spotify_view_shows_login_when_disconnected() {
    let mut a = demo();
    let s = render_layout(&mut a, Layout::Spotify, 100, 24);
    assert!(s.contains("SPOTIFY"), "panel title present");
    assert!(s.contains("Spotify"), "view name in the status bar");
    assert!(s.contains("Log in with Spotify"), "login prompt shown");
}

#[test]
fn key_7_opens_spotify() {
    use crate::event::{Key, KeyCode, Mods};
    let a = demo();
    let k = Key {
        code: KeyCode::Char('7'),
        mods: Mods::default(),
    };
    assert!(matches!(
        crate::keymap::map(&a, k),
        crate::action::Action::OpenSpotify
    ));
}

#[test]
fn z_fits_and_shift_z_resets_the_layout() {
    use crate::event::{Key, KeyCode, Mods};
    let a = demo();
    let z = Key {
        code: KeyCode::Char('z'),
        mods: Mods::default(),
    };
    assert!(matches!(
        crate::keymap::map(&a, z),
        crate::action::Action::FitLayout
    ));
    let shift_z = Key {
        code: KeyCode::Char('Z'),
        mods: Mods::default(),
    };
    assert!(matches!(
        crate::keymap::map(&a, shift_z),
        crate::action::Action::ResetLayout
    ));
}

#[test]
fn open_overlay_swallows_global_one_key_commands() {
    use crate::action::Action;
    use crate::event::{Key, KeyCode, Mods};
    let key = |c| Key {
        code: KeyCode::Char(c),
        mods: Mods::default(),
    };
    let mut a = demo();
    a.settings.popup = Some(0); // the per-view settings popup owns the screen
    // global one-key commands must NOT leak through while it's open
    assert!(matches!(crate::keymap::map(&a, key('v')), Action::Noop));
    assert!(matches!(crate::keymap::map(&a, key(' ')), Action::Noop));
    assert!(matches!(crate::keymap::map(&a, key('u')), Action::Noop)); // queue toggle
    // view switching (number keys) must NOT leak through either
    assert!(matches!(crate::keymap::map(&a, key('2')), Action::Noop));
    assert!(matches!(crate::keymap::map(&a, key('5')), Action::Noop));
    // …but the overlay's own navigation/adjust/cancel keys still work: j/k move
    // between rows, h/l step the selected row's value.
    assert!(matches!(crate::keymap::map(&a, key('j')), Action::Move(_)));
    assert!(matches!(
        crate::keymap::map(&a, key('l')),
        Action::SettingsAdjust(1)
    ));
    assert!(matches!(
        crate::keymap::map(&a, key('h')),
        Action::SettingsAdjust(-1)
    ));
    // seek transport (`,`/`.`) is NOT one of the overlay's keys — swallowed, so it
    // can't seek playback from behind the popup.
    assert!(matches!(crate::keymap::map(&a, key(',')), Action::Noop));
    assert!(matches!(crate::keymap::map(&a, key('.')), Action::Noop));
    // `f` is the overlay's own resize key (steps the size up), not a leaked global
    assert!(matches!(
        crate::keymap::map(&a, key('f')),
        Action::CycleOverlaySize
    ));
    // `q` is reserved for quit — it quits even with an overlay open (like Q / ctrl-c);
    // closing the overlay (going back) is esc + ctrl-o.
    assert!(matches!(crate::keymap::map(&a, key('q')), Action::Quit));
    let esc = Key {
        code: KeyCode::Esc,
        mods: Mods::default(),
    };
    assert!(matches!(crate::keymap::map(&a, esc), Action::Back));
    let ctrl_bracket = Key {
        code: KeyCode::Char('['),
        mods: Mods {
            ctrl: true,
            ..Default::default()
        },
    };
    assert!(matches!(crate::keymap::map(&a, ctrl_bracket), Action::Back));
    // closing the overlay restores the global commands
    a.settings.popup = None;
    assert!(matches!(
        crate::keymap::map(&a, key('u')),
        Action::ToggleQueue
    ));
}

#[test]
fn back_pops_a_drill_and_q_quits() {
    use crate::action::Action;
    use crate::core::model::TrackId;
    let mut a = demo();
    assert!(a.running);
    // drilled into a browsed album/playlist → Back (esc / ctrl-o) goes up one level
    // (exits the drill) and never quits the app
    a.browser.list = vec![TrackId::new(1)];
    a.browser.title = "Some Album".into();
    a.update(Action::Back);
    assert!(a.running, "Back exits the drill, it does not quit");
    assert!(a.browser.list.is_empty(), "the drill was exited");
    // Back at the top level (nothing to pop) is a no-op — it still does not quit
    a.update(Action::Back);
    assert!(a.running, "Back never quits");
    // `q` is reserved for quitting
    a.update(Action::Quit);
    assert!(!a.running, "q quits the app");
}

#[test]
fn stats_overlay_renders() {
    let mut app = demo();
    hide_panels(&mut app, Layout::Dashboard); // give the overlay the full main width
    app.toggle_info(crate::app::InfoTab::Stats);
    let s = render_layout(&mut app, Layout::Dashboard, 120, 40);
    assert!(s.contains("Stats"), "the Stats tab is shown");
    assert!(
        s.contains("Tracks") && s.contains("Artists"),
        "library totals"
    );
    // The overlay's own sections (the demo has no play data, so the TOP-N sections
    // are empty; the "LISTENING" header is unique to the stats overlay — with the
    // panes hidden it can't leak in from a base-view artist panel).
    assert!(
        s.contains("LIBRARY") && s.contains("LISTENING"),
        "the stats section headers render",
    );
}

#[test]
fn stats_overlay_scrolls_when_small() {
    use crate::action::{Action, Motion};
    let mut app = demo();
    app.toggle_info(crate::app::InfoTab::Stats);
    // small window → content overflows the box
    let top = render_layout(&mut app, Layout::Dashboard, 80, 14);
    // the first render publishes the max scroll; now jump to the bottom
    app.update(Action::Move(Motion::Bottom));
    assert!(
        app.info.as_ref().map(|i| i.stats_scroll).unwrap_or(0) > 0,
        "there is content to scroll to"
    );
    let bottom = render_layout(&mut app, Layout::Dashboard, 80, 14);
    assert_ne!(top, bottom, "scrolling reveals different stats");
}

#[test]
fn info_overlay_is_tabbed_and_switches() {
    use crate::app::InfoTab;
    let mut a = demo();
    a.player.current = Some(a.library.tracks.values().next().unwrap().id);
    // a roomy overlay so deep body rows (e.g. the Track tab's Path) show without
    // scrolling — the compact default would scroll them off (that's by design)
    a.config.overlay_size = crate::config::OVERLAY_SIZE_COUNT - 1;
    a.toggle_info(InfoTab::Keys);
    let keys = render_layout(&mut a, Layout::Dashboard, 100, 40);
    // all four tab names are on the bar
    for t in ["Keys", "Stats", "Health", "Track"] {
        assert!(keys.contains(t), "tab `{t}` shown:\n{keys}");
    }
    // Tab to Health reveals the health/recent-errors body
    a.toggle_info(InfoTab::Health);
    let health = render_layout(&mut a, Layout::Dashboard, 100, 40);
    assert!(
        health.contains("RECENT ERRORS"),
        "Health tab body:\n{health}"
    );
    // Tab (via OverlayTab) to Track shows the current track's tags
    a.toggle_info(InfoTab::Track);
    let track = render_layout(&mut a, Layout::Dashboard, 100, 40);
    assert!(
        track.contains("Bitrate") && track.contains("Path"),
        "Track tab body:\n{track}"
    );
}

#[test]
fn info_track_tab_follows_the_on_screen_source() {
    // Regression: while a Spotify track was on screen, the Info → Track tab showed
    // the frozen LOCAL library track (misleading). It must follow the source the
    // now-bar shows — the Spotify item, with its own fields, not the local file.
    use crate::app::InfoTab;
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.config.overlay_size = crate::config::OVERLAY_SIZE_COUNT - 1; // roomy: no scroll
    // a local track IS loaded (what the old code wrongly showed)
    a.player.current = Some(a.library.tracks.values().next().unwrap().id);
    // and a Spotify track is the on-screen now-playing item
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:dtmf01".into(),
        name: "DtMF".into(),
        subtitle: "Bad Bunny".into(),
        album: "DeBÍ TiRAR MáS FOToS".into(),
        year: Some(2025),
        duration_ms: 237_000,
        kind: Kind::Track,
        ..Default::default()
    });
    a.toggle_info(InfoTab::Track);
    let track = render_layout(&mut a, Layout::Spotify, 100, 40);
    assert!(
        track.contains("Spotify") && track.contains("DtMF") && track.contains("Bad Bunny"),
        "Track tab shows the Spotify item:\n{track}"
    );
    assert!(
        track.contains("spotify:track:dtmf01"),
        "the URI stands in for the (nonexistent) file path:\n{track}"
    );
    assert!(
        !track.contains("Bitrate") && !track.contains("Path"),
        "the local-file-only rows (Bitrate/Path) are NOT shown for a Spotify track:\n{track}"
    );
}

#[test]
fn info_overlay_keys_and_close() {
    use crate::action::{Action, Motion};
    use crate::app::InfoTab;
    use crate::event::{Key, KeyCode, Mods};
    let mut a = demo();
    let press = |a: &AppState, code: KeyCode| {
        crate::keymap::map(
            a,
            Key {
                code,
                mods: Mods::default(),
            },
        )
    };
    // `?` opens Info on the Keys tab
    let open = press(&a, KeyCode::Char('?'));
    a.update(open);
    assert!(matches!(
        a.info.as_ref().map(|i| i.tab),
        Some(InfoTab::Keys)
    ));
    // Tab steps the active tab; Esc closes the whole overlay
    assert!(matches!(press(&a, KeyCode::Tab), Action::OverlayTab(1)));
    a.update(Action::OverlayTab(1));
    assert!(matches!(
        a.info.as_ref().map(|i| i.tab),
        Some(InfoTab::Stats)
    ));
    // on a non-Keys tab, j scrolls
    assert!(matches!(
        press(&a, KeyCode::Char('j')),
        Action::Move(Motion::Down)
    ));
    a.update(Action::Back);
    assert!(a.info.is_none(), "Esc closes Info");
}

#[test]
fn info_tab_click_switches_tab() {
    use crate::app::InfoTab;
    let mut a = demo();
    a.toggle_info(InfoTab::Keys);
    // a tab click (MouseTarget::OverlayTab) routes through set_overlay_tab
    a.set_overlay_tab(2); // Keys, Stats, Health
    assert!(matches!(
        a.info.as_ref().map(|i| i.tab),
        Some(InfoTab::Health)
    ));
}

#[test]
fn help_overlay_is_scoped_to_the_view() {
    let mut a = demo();
    let has = |rows: &[(&str, &str)], needle: &str| rows.iter().any(|(_, d)| d.contains(needle));

    // local music view: playlist/library keys present; Spotify-only absent
    a.layout = Layout::Dashboard;
    let local = a.help_matches();
    assert!(
        has(&local, "switch view"),
        "global view-switch always shown"
    );
    assert!(
        has(&local, "settings for this view"),
        "global ; settings shown"
    );
    assert!(has(&local, "add to playlist"), "local shows playlist keys");
    assert!(
        has(&local, "library sidebar"),
        "local shows home/library keys"
    );
    assert!(
        !has(&local, "Liked Songs") && !has(&local, "browse by country"),
        "Spotify/Radio-only keys hidden in local views"
    );

    // Spotify view: global + Spotify keys only — no playlist/home keys
    a.layout = Layout::Spotify;
    let sp = a.help_matches();
    assert!(
        has(&sp, "switch view") && has(&sp, "settings for this view"),
        "globals shown"
    );
    assert!(has(&sp, "Liked Songs"), "Spotify like shown in Spotify");
    assert!(
        !has(&sp, "add to playlist") && !has(&sp, "library sidebar"),
        "playlist/home keys hidden in Spotify"
    );

    // Radio view: global + radio keys; no playlist keys
    a.layout = Layout::Radio;
    let rd = a.help_matches();
    assert!(has(&rd, "browse by country"), "radio keys shown in Radio");
    assert!(
        !has(&rd, "add to playlist"),
        "playlist keys hidden in Radio"
    );
}
