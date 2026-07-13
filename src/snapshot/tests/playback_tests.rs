//! Playback snapshot/behaviour tests, split out of snapshot.rs.

use super::*;

#[test]
fn play_current_album_and_artist_scope_the_queue() {
    use crate::action::Action;
    let mut app = demo();
    let cur = app.player.current.unwrap();
    let artist = app.library.track(cur).unwrap().artist_id;
    let album = app.library.track(cur).unwrap().album_id;

    // "Play this artist" → the queue becomes exactly that artist's tracks,
    // and the queue is the truth (every queued track shares the artist).
    app.update(Action::PlayCurrentArtist);
    assert!(!app.player.queue.items.is_empty());
    assert!(
        app.player
            .queue
            .items
            .iter()
            .all(|id| app.library.track(*id).unwrap().artist_id == artist),
        "the whole queue is the one artist"
    );
    assert_eq!(
        app.library
            .track(app.player.current.unwrap())
            .unwrap()
            .artist_id,
        artist,
        "still playing within the artist"
    );

    // "Play this album" → the queue becomes exactly the current album.
    app.update(Action::PlayCurrentAlbum);
    assert!(!app.player.queue.items.is_empty());
    assert!(
        app.player
            .queue
            .items
            .iter()
            .all(|id| app.library.track(*id).unwrap().album_id == album),
        "the whole queue is the one album"
    );
}

#[test]
fn queue_pane_follows_the_now_playing_track_on_advance() {
    use crate::core::player::Repeat;
    let mut app = demo();
    let ids: Vec<_> = app.library.tracks.values().map(|t| t.id).take(5).collect();
    assert!(ids.len() >= 3, "demo has tracks");
    app.player.queue.items = ids.clone();
    app.player.queue.position = ids.len() - 1; // sitting on the last track
    app.player.current = Some(ids[ids.len() - 1]);
    app.player.repeat = Repeat::All;
    app.player.shuffle = false;
    app.queue_sel = ids.len() - 1; // QUEUE-pane cursor parked on the last row
    // the last track ends → repeat-all wraps back to the first and keeps playing
    app.advance_after_finish();
    assert_eq!(
        app.player.queue.position, 0,
        "repeat-all wraps to the first track"
    );
    assert_eq!(
        app.queue_sel, 0,
        "the QUEUE pane cursor follows the wrap so the now-playing row stays visible"
    );
}

#[test]
fn next_track_derives_from_queue() {
    use crate::core::player::Repeat;
    let mut app = demo();
    // build a 3-track queue from the library, sitting on the first
    let ids: Vec<_> = app.library.tracks.values().map(|t| t.id).take(3).collect();
    assert!(ids.len() >= 2, "demo has tracks");
    app.player.queue.items = ids.clone();
    app.player.queue.position = 0;
    app.player.shuffle = false;
    let want = app.library.track(ids[1]).unwrap().title.clone();
    assert_eq!(app.next_queue_title(), Some(want));
    // at the end, no repeat → no next
    app.player.queue.position = ids.len() - 1;
    assert_eq!(app.next_queue_title(), None);
    // shuffle pre-reorders the queue (toggle_shuffle) rather than picking at random,
    // so the hint keeps showing the deterministic next instead of blanking.
    app.player.queue.position = 0;
    app.player.shuffle = true;
    assert_eq!(
        app.next_queue_title(),
        Some(app.library.track(ids[1]).unwrap().title.clone()),
        "shuffle no longer blanks the Next hint"
    );
    // repeat-one replays the current track — and it takes precedence over shuffle,
    // exactly as advance_after_finish does (repeat-one is checked before next()).
    let current = app.library.track(ids[0]).unwrap().title.clone();
    app.player.repeat = Repeat::One;
    assert_eq!(
        app.next_queue_title(),
        Some(current),
        "repeat-one shows the current track even under shuffle"
    );
}

#[test]
fn playback_viz_mode_is_independent_of_the_view_visualizer() {
    use crate::action::Action;
    let mut a = demo();

    // FullPlayer has a big visualizer → `v` cycles it, leaving the playback
    // bar's own visualizer mode untouched.
    a.layout = Layout::FullPlayer;
    let pv = a.config.player_viz_mode;
    let big0 = a.viz_mode();
    a.update(Action::CycleVisualizer);
    assert_ne!(a.viz_mode(), big0, "the big visualizer cycled");
    assert_eq!(a.config.player_viz_mode, pv, "playback viz mode untouched");

    // Dashboard has no big visualizer → `v` cycles the playback-bar viz.
    a.layout = Layout::Dashboard;
    let pv0 = a.config.player_viz_mode;
    a.update(Action::CycleVisualizer);
    assert_eq!(
        a.config.player_viz_mode,
        (pv0 + 1) % crate::ui::components::VIZ_MODES.len() as u8,
        "playback viz cycled where there's no big one"
    );
}

#[test]
fn l_cycles_lyrics_format_in_the_lyrics_view() {
    use crate::action::Action;
    let mut a = demo();
    a.layout = Layout::LyricsFocus;
    // start from a known format (config may be persisted in a shared test dir)
    a.config.lyrics_karaoke = false;
    a.config.lyrics_teleprompter = false;
    a.update(Action::ToggleLyrics);
    assert!(a.config.lyrics_karaoke, "L → karaoke");
    a.update(Action::ToggleLyrics);
    assert!(a.config.lyrics_teleprompter, "L → teleprompter");
    a.update(Action::ToggleLyrics);
    assert!(
        !a.config.lyrics_karaoke && !a.config.lyrics_teleprompter,
        "L → back to plain"
    );

    // outside the lyrics view, L toggles the movable Lyrics pane (like `i` toggles
    // Artist) and leaves the format alone.
    a.layout = Layout::Dashboard;
    let before = a.panel(crate::app::Panel::Lyrics).shown;
    a.update(Action::ToggleLyrics);
    assert!(
        !a.config.lyrics_karaoke,
        "L elsewhere leaves the lyrics format alone"
    );
    assert_ne!(
        a.panel(crate::app::Panel::Lyrics).shown,
        before,
        "L toggles the Lyrics pane on a view that hosts it"
    );
}

#[test]
fn local_play_keeps_overlays_paused_not_cleared() {
    use crate::core::model::TrackId;
    use crate::radio::Station;
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    // a Spotify track "streaming"
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:x".into(),
        name: "X".into(),
        subtitle: "Y".into(),
        album: "Z".into(),
        image: None,
        kind: Kind::Track,
        duration_ms: 1000,
        artist_uri: None,
        ..Default::default()
    });
    a.spov.spotify_paused = false;
    a.spov.sp_started = true;
    // a radio station "streaming"
    a.rnow.now_station = Some(Station {
        name: "St".into(),
        ..Default::default()
    });
    a.rnow.radio_paused = false;

    // play a local track — both overlays pause but are KEPT (so switching back
    // shows them, resumable, and they persist on close)
    a.player.current = Some(TrackId::new(1));
    a.play_current();
    assert!(
        a.spov.now_spotify.is_some() && a.spov.spotify_paused && !a.spov.sp_started,
        "Spotify overlay retained + paused (not cleared)"
    );
    assert!(
        a.rnow.now_station.is_some() && a.rnow.radio_paused,
        "radio overlay retained + paused (not cleared)"
    );
}

#[test]
fn now_bar_marks_a_favorite_with_a_heart() {
    let mut a = demo();
    // the demo's track 1 is the only favorite — make it the now-playing track
    let fav = a
        .library
        .tracks
        .values()
        .find(|t| t.favorite)
        .map(|t| t.id)
        .expect("demo has a favorite");
    a.player.current = Some(fav);
    // (the sidebar's Favorites row also uses ♥, so isolate the now-bar's by
    // counting and toggling the *same* track's favorite flag)
    let hearts = |s: &str| s.matches('♥').count();
    let with = render_layout(&mut a, Layout::Dashboard, 120, 40);

    a.library.track_mut(fav).unwrap().favorite = false;
    let without = render_layout(&mut a, Layout::Dashboard, 120, 40);

    assert_eq!(
        hearts(&with),
        hearts(&without) + 1,
        "the now-bar adds exactly one ♥ for a favorite playing track"
    );
}

#[test]
fn now_bar_shows_elapsed_and_total_time() {
    let mut a = demo();
    a.player.current = a.library.tracks.values().next().map(|t| t.id);
    a.player.elapsed = std::time::Duration::from_secs(75); // 1:15
    a.player.duration = std::time::Duration::from_secs(220); // 3:40
    let s = render_layout(&mut a, Layout::Dashboard, 100, 32);
    assert!(s.contains("1:15"), "elapsed time renders in the now-bar");
    assert!(s.contains("3:40"), "total time renders in the now-bar");
}

#[test]
fn redraw_only_when_dirty_or_animating() {
    use crate::action::{Action, Motion};
    let mut a = demo();
    assert!(a.take_dirty(), "starts dirty so the first frame paints");
    assert!(!a.take_dirty(), "dirty is consumed");
    // an idle tick does not force a repaint (notification hasn't expired yet)
    a.update(Action::Tick);
    assert!(!a.dirty, "an idle tick stays clean");
    // any real action requests a repaint
    a.update(Action::Move(Motion::Down));
    assert!(a.dirty, "navigation marks dirty");
    // the demo's startup notification keeps the loop animating until it expires
    assert!(a.is_animating(), "a visible notification animates");
}

#[test]
fn transport_and_rating_update_state() {
    let mut app = demo();
    let id = app.player.current.unwrap();
    app.update(crate::action::Action::Rate(id, 5));
    assert_eq!(app.library.track(id).unwrap().rating, 5);

    use crate::core::player::Status;
    // demo starts paused; toggling plays, toggling again pauses
    assert_eq!(app.player.status, Status::Paused);
    app.update(crate::action::Action::TogglePlay);
    assert_eq!(app.player.status, Status::Playing);
    app.update(crate::action::Action::TogglePlay);
    assert_eq!(app.player.status, Status::Paused);
}

#[test]
fn shuffle_next_is_nonlinear() {
    let mut app = demo();
    use crate::action::Action;
    let qlen = app.player.queue.items.len();
    assert!(qlen > 3, "demo queue should have several tracks");
    app.player.queue.position = 0;
    app.player.current = app.player.queue.items.first().copied();
    // the original (linear) track order before shuffling
    let linear = app.player.queue.items.clone();
    // enable shuffle: toggle_shuffle reorders the upcoming tail in place, so the
    // player walks a shuffled *track* order (positions still advance 0→1→2…, but the
    // track sitting at each position is permuted). The current track stays put.
    app.player.toggle_shuffle();
    let steps = (qlen - 1).min(6);
    let mut visited = Vec::new();
    for _ in 0..steps {
        app.update(Action::Next);
        visited.push(app.player.current.expect("a track is playing"));
    }
    assert_eq!(visited.len(), steps, "advanced through the shuffled tail");
    assert!(!app.player.queue.history.is_empty(), "history recorded");
    assert_ne!(
        visited,
        linear[1..=steps].to_vec(),
        "playback order is shuffled, not linear"
    );
}

#[test]
fn speed_steps_snap_to_the_quarter_grid() {
    use crate::action::Action;
    use crate::event::{Key, KeyCode, Mods};
    let step = |a: &AppState, c: char| match crate::keymap::map(
        a,
        Key {
            code: KeyCode::Char(c),
            mods: Mods::default(),
        },
    ) {
        Action::SetSpeed(s) => s,
        other => panic!("expected SetSpeed, got {other:?}"),
    };
    let mut a = demo();
    a.player.speed = 1.0;
    assert!((step(&a, ']') - 1.25).abs() < 1e-4, "1.0 → 1.25");
    assert!((step(&a, '[') - 0.75).abs() < 1e-4, "1.0 → 0.75");
    a.player.speed = 2.0;
    assert!((step(&a, ']') - 2.0).abs() < 1e-4, "clamps at 2.0×");
    a.player.speed = 0.25;
    assert!((step(&a, '[') - 0.25).abs() < 1e-4, "clamps at 0.25×");
    a.player.speed = 0.7; // off-grid (e.g. a restored older speed) snaps first
    assert!(
        (step(&a, ']') - 1.0).abs() < 1e-4,
        "0.7 snaps then steps to 1.0"
    );
}

#[test]
fn now_bar_shows_the_speed_badge_off_normal() {
    // The shared playback_bar renders the local adapter's under-bar time label,
    // which carries the speed badge only when playback is off 1.0×.
    let mut a = demo();
    a.config.player_viz = true; // tall bar → the time row (with the badge) is shown
    a.player.speed = 1.0;
    let s = render_layout(&mut a, Layout::Dashboard, 120, 40);
    assert!(!s.contains('×'), "no speed badge at normal speed");
    a.player.speed = 1.5;
    let s = render_layout(&mut a, Layout::Dashboard, 120, 40);
    assert!(s.contains("1.5×"), "off-normal speed shows the badge");
}

#[test]
fn now_bar_drops_the_volume_meter_when_too_narrow_for_the_transport() {
    // On a wide bar the volume meter sits right of the centred transport; on a
    // narrow bar it would overdraw the buttons, so it's dropped (no messy overlap).
    let mut a = demo();
    a.config.player_viz = true;
    let wide = render_layout(&mut a, Layout::Dashboard, 120, 40);
    assert!(wide.contains('%'), "wide bar shows the volume meter");
    let narrow = render_layout(&mut a, Layout::Dashboard, 44, 40);
    assert!(
        !narrow.contains('%'),
        "narrow bar drops the volume meter rather than clobbering the transport"
    );
}
