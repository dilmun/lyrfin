//! Playback behaviour tests (split from tag_editor_tests). `use super::*`
//! reaches the shared app() fixture + AppState privates.

use super::*;

#[test]
fn settings_crossfade_changes_via_enter_and_adjust() {
    let mut a = app();
    a.config.dir = std::env::temp_dir().join("lyrfin_xfade_test");
    a.open_settings_group("Audio");
    a.settings.sel = a
        .settings_group_items()
        .iter()
        .position(|s| matches!(s, Setting::Crossfade))
        .unwrap();
    assert_eq!(a.config.crossfade_ms, 0, "off by default");

    // Enter cycles presets off → 2s → 4s → 8s → off
    a.update(Action::Activate);
    assert_eq!(a.config.crossfade_ms, 2000);
    a.update(Action::Activate);
    assert_eq!(a.config.crossfade_ms, 4000);

    // h/l (Seek) fine-adjust by 250 ms
    a.update(Action::Seek(5));
    assert_eq!(a.config.crossfade_ms, 4250);
    a.update(Action::Seek(-5));
    assert_eq!(a.config.crossfade_ms, 4000);

    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("lyrfin_xfade_test"));
}

#[test]
fn seek_is_a_noop_while_a_live_station_plays() {
    let mut a = app();
    a.player.status = Status::Paused; // keep the demo clock from advancing
    a.player.duration = Duration::from_secs(300);
    a.player.elapsed = Duration::from_secs(42);

    // A live radio stream is forward-only. While one plays, the shown position is
    // the preserved local track's — a seek must not touch it (or the live stream).
    a.rnow.now_station = Some(crate::radio::Station::default());
    a.rnow.radio_paused = false;
    assert!(a.rnow.is_live());
    a.update(Action::Seek(5));
    assert_eq!(
        a.player.elapsed,
        Duration::from_secs(42),
        "seek must be a no-op on a live stream"
    );

    // Pausing the station hands the engine back to local playback → seek applies.
    a.rnow.radio_paused = true;
    assert!(!a.rnow.is_live());
    a.update(Action::Seek(5));
    assert_eq!(
        a.player.elapsed,
        Duration::from_secs(47),
        "with the station paused, local seek works again"
    );
}

#[test]
fn dvr_seek_is_scoped_to_the_radio_view() {
    use crate::app::radio::DvrState;
    let mut a = app();
    a.layout = Layout::Radio; // the DVR window is only seekable while showing radio
    a.player.status = Status::Paused;
    a.player.elapsed = Duration::from_secs(42); // the preserved local position
    a.rnow.now_station = Some(crate::radio::Station::default());
    a.rnow.radio_paused = false;
    // a timeshift buffer: rewound to 100 s within the window [0, 200]
    a.rnow.dvr = Some(DvrState {
        pos: 100.0,
        start: 0.0,
        live: 200.0,
        following: false,
    });

    // in the Radio view, seek moves within the DVR window (not the local track)
    a.update(Action::Seek(-5));
    assert_eq!(
        a.rnow.dvr.unwrap().pos,
        95.0,
        "DVR seek moves within the window"
    );
    assert_eq!(
        a.player.elapsed,
        Duration::from_secs(42),
        "the preserved local track position is untouched"
    );
    // rewinding past the window start clamps to it (can't go before the buffer)
    a.rnow.dvr.as_mut().unwrap().pos = 3.0;
    a.update(Action::Seek(-5));
    assert_eq!(a.rnow.dvr.unwrap().pos, 0.0, "clamped to the buffer start");

    // leaving the Radio view: seek must NOT reach the background radio's window from
    // any other view (Dashboard, Spotify, …) — the leak we're fixing.
    a.rnow.dvr.as_mut().unwrap().pos = 100.0;
    for view in [Layout::Dashboard, Layout::Spotify, Layout::FullPlayer] {
        a.layout = view;
        a.update(Action::Seek(-5));
        assert_eq!(
            a.rnow.dvr.unwrap().pos,
            100.0,
            "{view:?} must not seek the background radio"
        );
    }
}

#[test]
fn ab_loop_marks_repeats_and_clears() {
    let mut a = app();
    a.player.status = Status::Paused; // keep the demo clock from advancing

    a.player.elapsed = Duration::from_secs(10);
    a.update(Action::AbLoopCycle); // set A
    assert_eq!(a.fx.ab_a, Some(Duration::from_secs(10)));
    assert!(a.fx.ab_loop.is_none());

    a.player.elapsed = Duration::from_secs(30);
    a.update(Action::AbLoopCycle); // set B → active loop
    assert_eq!(
        a.fx.ab_loop,
        Some((Duration::from_secs(10), Duration::from_secs(30)))
    );
    assert!(a.fx.ab_a.is_none());

    // passing B jumps back to A
    a.player.elapsed = Duration::from_secs(35);
    a.update(Action::Tick);
    assert_eq!(a.player.elapsed, Duration::from_secs(10));

    // cycling again clears the loop
    a.update(Action::AbLoopCycle);
    assert!(a.fx.ab_loop.is_none() && a.fx.ab_a.is_none());
}

#[test]
fn sleep_timer_sets_cycles_and_expires() {
    let mut a = app();
    assert!(a.sleep_remaining_secs().is_none());

    a.update(Action::SetSleepTimer(30));
    let left = a.sleep_remaining_secs().expect("armed");
    assert!(left > 1700 && left <= 1800, "≈30 min remaining");

    a.update(Action::SetSleepTimer(0));
    assert!(a.sleep_remaining_secs().is_none(), "0 cancels");

    // cycle: off → 15
    a.update(Action::CycleSleepTimer);
    let m = a.sleep_remaining_secs().unwrap().div_ceil(60);
    assert_eq!(m, 15);

    // expiry pauses playback and clears the timer
    a.player.status = Status::Playing;
    a.fx.sleep_until = Some(crate::datetime::now_unix().saturating_sub(1));
    a.update(Action::Tick);
    assert_eq!(a.player.status, Status::Paused);
    assert!(a.fx.sleep_until.is_none());
}

#[test]
fn gapless_next_path_respects_mode() {
    let mut a = app(); // sequential by default, multi-track queue at pos 0
    a.player.queue.position = 0;
    let want = a
        .library
        .track(a.player.queue.items[1])
        .unwrap()
        .path
        .clone();
    assert_eq!(
        a.gapless_next_path(),
        Some(want.clone()),
        "sequential preloads next"
    );

    // shuffle pre-reorders the queue tail, so the sequential next is still correct —
    // gapless now preloads under shuffle instead of bailing out.
    a.player.shuffle = true;
    assert_eq!(
        a.gapless_next_path(),
        Some(want),
        "shuffle still preloads the deterministic next"
    );
    a.player.shuffle = false;

    a.player.repeat = Repeat::One;
    assert_eq!(a.gapless_next_path(), None, "repeat-one disables gapless");
    a.player.repeat = Repeat::Off;

    a.config.gapless = false;
    assert_eq!(a.gapless_next_path(), None, "gapless off → no preload");
    a.config.gapless = true;

    // last track, repeat off → nothing; repeat all → wrap to first
    a.player.queue.position = a.player.queue.items.len() - 1;
    assert_eq!(a.gapless_next_path(), None);
    a.player.repeat = Repeat::All;
    let first = a
        .library
        .track(a.player.queue.items[0])
        .unwrap()
        .path
        .clone();
    assert_eq!(a.gapless_next_path(), Some(first), "repeat-all wraps");
}

#[test]
fn soft_advance_syncs_to_next_track() {
    let mut a = app();
    a.player.queue.position = 0;
    a.player.current = a.player.queue.items.first().copied();
    let from = a.player.queue.items[0];
    let to = a.player.queue.items[1];

    a.soft_advance(); // engine flipped tracks gaplessly; app must follow
    assert_eq!(a.player.queue.position, 1);
    assert_eq!(a.player.current, Some(to));
    assert_eq!(
        a.loaded_track,
        Some(to),
        "no reload, but now-playing synced"
    );
    assert!(
        a.player.queue.history.contains(&from),
        "outgoing track recorded for Previous"
    );
}

#[test]
fn next_follows_now_playing_in_tracklist() {
    let mut a = app();
    // tracklist shows the queue (not searching / browsing a sub-list)
    a.search.active = false;
    a.search.query.clear();
    a.browser.list.clear();
    a.player.repeat = crate::core::player::Repeat::Off;
    a.player.queue.position = 0;
    a.player.current = a.player.queue.items.first().copied();
    a.selection = 999; // cursor parked far from the now-playing row

    a.advance_next();
    assert_eq!(a.player.queue.position, 1, "advanced to the next track");
    assert_eq!(
        a.selection, 1,
        "the tracklist cursor follows the now-playing track"
    );
}

#[test]
fn tab_reaches_queue_in_dashboard_when_shown() {
    let mut a = app();
    a.layout = Layout::Dashboard;
    // isolate the sidebar/main/queue ring: hide the display panes (Artist/Lyrics)
    // that the Dashboard now also exposes to Tab.
    for p in [Panel::Artist, Panel::Lyrics] {
        if a.panel(p).shown {
            a.toggle_panel(p);
        }
    }
    // hidden queue → Tab skips it (Tracklist → Sidebar)
    if a.panel(Panel::Queue).shown {
        a.toggle_panel(Panel::Queue);
    }
    a.focus = Focus::Main;
    a.update(Action::CyclePane);
    assert_eq!(a.focus, Focus::Sidebar, "a hidden queue is skipped");

    // shown queue → Tab now reaches it (the reported bug)
    a.toggle_panel(Panel::Queue);
    a.focus = Focus::Main;
    // park the now-playing cursor away from the queue selection so a focus
    // re-anchor would visibly jump the list
    a.player.queue.position = 3;
    a.queue_sel = 0;
    a.update(Action::CyclePane);
    assert_eq!(
        a.focus,
        Focus::Pane(Panel::Queue),
        "the shown queue is focusable in the dashboard"
    );
    assert_eq!(
        a.queue_sel, 3,
        "focusing the queue parks the cursor on the now-playing row (no scroll jump)"
    );

    // hiding the queue while focused doesn't strand focus on it
    a.toggle_panel(Panel::Queue);
    assert_eq!(a.focus, Focus::Main, "focus leaves a just-hidden queue");
}

#[test]
fn tab_cycles_only_the_views_active_panes() {
    let mut a = app();

    // FullPlayer with the queue shown → exactly two active panes (queue +
    // now-playing/viz), so two Tabs return to the start — no dead stops.
    a.layout = Layout::FullPlayer;
    if !a.panel(Panel::Queue).shown {
        a.toggle_panel(Panel::Queue);
    }
    a.focus = Focus::Main;
    a.update(Action::CyclePane);
    assert_eq!(a.focus, Focus::Pane(Panel::Queue), "tab 1 → queue");
    a.update(Action::CyclePane);
    assert_eq!(a.focus, Focus::Main, "tab 2 → back (2 panes = 2 tabs)");

    // hide the queue → only the now-playing pane is active → Tab is a no-op.
    a.toggle_panel(Panel::Queue);
    a.focus = Focus::Main;
    a.update(Action::CyclePane);
    assert_eq!(a.focus, Focus::Main, "a single active pane → Tab stays put");
}

#[test]
fn ctrl_n_p_navigate_the_focused_list() {
    let mut a = app(); // Home (Dashboard), AllTracks section loaded
    // Dashboard + Focus::Main → moves the drill-in main list cursor (`local.sel`)
    a.focus = Focus::Main;
    a.local.sel = 0;
    a.update(Action::NavDown);
    assert_eq!(a.local.sel, 1, "NavDown moves down the main list");
    a.update(Action::NavUp);
    assert_eq!(a.local.sel, 0, "NavUp moves back up");

    // Focus::Sidebar → moves (and loads) the selected section instead
    a.focus = Focus::Sidebar;
    let sec0 = a.local.section;
    a.update(Action::NavDown);
    assert_ne!(a.local.section, sec0, "NavDown advances the section");

    // command palette open → moves the palette selection instead
    a.focus = Focus::Main;
    a.local.sel = 0;
    a.update(Action::OpenPalette);
    assert!(a.palette.is_some(), "palette open");
    a.update(Action::NavDown);
    assert_eq!(
        a.palette.as_ref().unwrap().sel,
        1,
        "NavDown moves the palette, not the list behind it"
    );
    assert_eq!(a.local.sel, 0, "the list behind the palette is untouched");
}

#[test]
fn pane_focus_steps_forward_and_back() {
    let mut a = app();
    a.layout = Layout::Dashboard;
    // isolate the basic ring by hiding the display panes the Dashboard also exposes
    for p in [Panel::Artist, Panel::Lyrics] {
        if a.panel(p).shown {
            a.toggle_panel(p);
        }
    }
    if !a.panel(Panel::Queue).shown {
        a.toggle_panel(Panel::Queue);
    }
    // ring = [Sidebar, Tracklist, Queue]
    a.focus = Focus::Main;
    a.update(Action::CyclePane); // → Queue
    assert_eq!(a.focus, Focus::Pane(Panel::Queue));
    a.update(Action::CyclePaneRev); // ← Tracklist
    assert_eq!(a.focus, Focus::Main);
    a.update(Action::CyclePaneRev); // ← Sidebar
    assert_eq!(a.focus, Focus::Sidebar);
    a.update(Action::CyclePaneRev); // ← wraps to Queue
    assert_eq!(a.focus, Focus::Pane(Panel::Queue));
}

#[test]
fn tab_reaches_the_artist_pane_on_the_dashboard() {
    let mut a = app();
    a.layout = Layout::Dashboard;
    // queue + lyrics hidden, artist shown → ring = [Sidebar, Main, Artist]. The
    // Dashboard now exposes its shown movable panes to Tab, like the Spotify view.
    for p in [Panel::Queue, Panel::Lyrics] {
        if a.panel(p).shown {
            a.toggle_panel(p);
        }
    }
    if !a.panel(Panel::Artist).shown {
        a.toggle_panel(Panel::Artist);
    }
    a.focus = Focus::Main;
    a.update(Action::CyclePane);
    assert_eq!(
        a.focus,
        Focus::Pane(Panel::Artist),
        "Tab reaches the Artist pane on the Dashboard"
    );
}

#[test]
fn queue_reorder_remove_and_clear() {
    let mut a = app();
    a.focus = Focus::Pane(Panel::Queue);
    let items0 = a.player.queue.items.clone();
    let n = items0.len();
    assert!(n > 3, "demo seeds a multi-track queue");
    let pos = a.player.queue.position;
    // select the first upcoming track (absolute index into the full queue)
    a.queue_sel = pos + 1;

    // 'J' down-swaps with the next item
    assert!(matches!(
        crate::keymap::map(
            &a,
            Key {
                code: KeyCode::Char('J'),
                mods: Mods::default()
            }
        ),
        Action::QueueMove(Motion::Down)
    ));
    a.update(Action::QueueMove(Motion::Down));
    assert_eq!(a.player.queue.items[pos + 1], items0[pos + 2]);
    assert_eq!(a.player.queue.items[pos + 2], items0[pos + 1]);
    assert_eq!(a.queue_sel, pos + 2);

    // move back up restores order
    a.update(Action::QueueMove(Motion::Up));
    assert_eq!(a.player.queue.items, items0);
    assert_eq!(a.queue_sel, pos + 1);

    // remove the selected (non-playing) track
    a.update(Action::QueueRemove);
    assert_eq!(a.player.queue.items.len(), n - 1);
    assert_eq!(
        a.player.queue.items[pos + 1],
        items0[pos + 2],
        "later items shift up"
    );

    // the currently-playing track can't be removed
    a.queue_sel = a.player.queue.position;
    let len = a.player.queue.items.len();
    a.update(Action::QueueRemove);
    assert_eq!(a.player.queue.items.len(), len, "now-playing track is kept");

    // clear upcoming leaves the played history + the current track
    a.update(Action::QueueClearUpcoming);
    assert_eq!(a.player.queue.position + 1, a.player.queue.items.len());
}

#[test]
fn playback_speed_scales_the_progress_clock() {
    // A playing local track, with the demo clock free to run (no recent engine
    // Progress event), advances `elapsed` proportionally to the playback speed —
    // so a sped-up track's timer moves faster, matching the engine's clock.
    let tick_once = |speed: f32| {
        let mut a = app();
        a.player.status = Status::Playing;
        a.player.duration = Duration::from_secs(600);
        a.player.elapsed = Duration::ZERO;
        a.player.speed = speed;
        a.tick = 1000;
        a.last_audio_progress = 0; // engine not "driving" → the demo clock runs
        a.update(Action::Tick);
        a.player.elapsed
    };
    let at_1x = tick_once(1.0);
    let at_2x = tick_once(2.0);
    assert!(at_1x > Duration::ZERO, "1x advances the clock");
    assert!(
        at_2x > at_1x,
        "2x advances the clock faster: {at_2x:?} vs {at_1x:?}"
    );
}

#[test]
fn local_shuffle_repeat_toggle_shows_a_status_toast() {
    // Regression: only Spotify used to toast on shuffle/repeat; local was silent, so
    // the feedback differed between views. Now the shared active-source helpers toast
    // everywhere.
    let mut a = app();
    a.layout = Layout::Dashboard; // a local view
    a.cycle_repeat_active();
    assert!(
        a.notification
            .as_ref()
            .is_some_and(|n| n.text.contains("Repeat")),
        "cycling repeat toasts the mode in local too"
    );
    a.toggle_shuffle_active();
    assert!(
        a.notification
            .as_ref()
            .is_some_and(|n| n.text.contains("Shuffle")),
        "toggling shuffle toasts in local too"
    );
}

#[test]
fn next_hint_follows_the_view_not_play_state() {
    // Regression: the "Next:" hint used to fall back to the LOCAL queue whenever
    // Spotify was paused, disagreeing with the (view-driven) now-playing bar. It now
    // follows the view, exactly like `now_bar`.
    let mut a = app();

    // a local view → the local queue's next, regardless of any Spotify state
    a.layout = Layout::Dashboard;
    a.spov.spotify_paused = true;
    assert_eq!(a.status_next_title(), a.next_queue_title());

    // the Spotify view → Spotify's up-next, even when paused (the reported bug)
    a.layout = Layout::Spotify;
    assert_eq!(a.status_next_title(), a.spotify_next_title());

    // radio → no queue
    a.layout = Layout::Radio;
    assert_eq!(a.status_next_title(), None);
}
