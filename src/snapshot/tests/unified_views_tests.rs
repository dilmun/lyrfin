//! The player views (Now Playing / Lyrics / Concert) follow whatever is playing,
//! while the source views (Home / Library / Spotify / Radio) each stay on their own.

use super::*;
use crate::app::{Layout as L, NpSource};

/// Put a Spotify track on the overlay as if it were streaming.
fn spotify_playing(app: &mut AppState) {
    app.spov.now_spotify = Some(crate::spotify::api::Item {
        name: "Till I Collapse".into(),
        album: "The Eminem Show".into(),
        duration_ms: 297_000,
        ..Default::default()
    });
    app.spov.spotify_paused = false;
    app.spov.sp_pos = 30.0;
    app.spov.sp_dur = 297.0;
    // the local player must be stopped — only one source drives the engine
    app.player.status = crate::core::player::Status::Paused;
}

/// Tune a station as if it were streaming.
fn radio_playing(app: &mut AppState) {
    app.rnow.now_station = Some(crate::radio::Station {
        name: "Cairo Jazz".into(),
        url: "u".into(),
        uuid: "u".into(),
        ..Default::default()
    });
    app.rnow.radio_paused = false;
    app.player.status = crate::core::player::Status::Paused;
}

#[test]
fn only_the_player_views_are_source_neutral() {
    // the boundary the whole change rests on
    for l in [L::FullPlayer, L::LyricsFocus, L::Concert] {
        assert!(l.is_player_view(), "{l:?} belongs to no source");
    }
    for l in [L::Dashboard, L::LibraryFocus, L::Spotify, L::Radio] {
        assert!(!l.is_player_view(), "{l:?} is a source view");
    }
}

#[test]
fn the_audible_source_wins_over_a_paused_one() {
    let mut a = demo();
    // the fixture loads a track but leaves it paused, so nothing is audible yet
    assert_eq!(
        a.audible_source(),
        None,
        "a loaded-but-paused track is not audible"
    );
    a.player.status = crate::core::player::Status::Playing;
    assert_eq!(a.audible_source(), Some(NpSource::Local));

    spotify_playing(&mut a);
    assert_eq!(a.audible_source(), Some(NpSource::Spotify));
    assert_eq!(a.now_playing_source(), Some(NpSource::Spotify));

    radio_playing(&mut a);
    assert_eq!(
        a.audible_source(),
        Some(NpSource::Radio),
        "radio outranks a paused Spotify"
    );
}

#[test]
fn a_paused_source_is_remembered_so_the_views_dont_snap_back_to_local() {
    // Pausing Spotify and opening Lyrics should still show Spotify. Without the
    // memory, every source is merely "loaded" and local would win by position.
    let mut a = demo();
    spotify_playing(&mut a);
    a.update(crate::action::Action::Tick); // records the audible source
    a.spov.spotify_paused = true;
    a.layout = L::LyricsFocus;

    assert_eq!(a.audible_source(), None, "nothing is playing now");
    assert_eq!(
        a.now_playing_source(),
        Some(NpSource::Spotify),
        "the paused Spotify track is still what the player views are about"
    );
}

#[test]
fn now_playing_shows_the_streaming_spotify_track() {
    let mut a = demo();
    let local = a
        .current_track()
        .map(|t| t.title.clone())
        .expect("demo has a local track");
    spotify_playing(&mut a);

    let s = render_layout(&mut a, L::FullPlayer, 120, 30);
    assert!(
        s.contains("Till I Collapse"),
        "the Spotify track is on the bar"
    );
    assert!(
        !s.contains(&local),
        "the idle local track is not shown while Spotify streams"
    );
}

#[test]
fn concert_renders_every_source() {
    let mut a = demo();
    // local
    let s = render_layout(&mut a, L::Concert, 100, 32);
    assert!(s.contains("Midnight Protocol"), "local track in Concert");

    // spotify — title + artist, and a real progress beam (it has a duration)
    spotify_playing(&mut a);
    let s = render_layout(&mut a, L::Concert, 100, 32);
    assert!(s.contains("Till I Collapse"), "Spotify track in Concert");
    assert!(
        s.contains('●'),
        "a track with a duration gets a progress beam"
    );

    // radio — a live stream has no total, so it gets a LIVE badge instead
    a.spov.now_spotify = None;
    radio_playing(&mut a);
    let s = render_layout(&mut a, L::Concert, 100, 32);
    assert!(s.contains("Cairo Jazz"), "the station in Concert");
    assert!(
        s.contains("LIVE"),
        "a live stream shows LIVE, not a progress beam"
    );
}

#[test]
fn a_source_view_never_shows_another_sources_playback() {
    // The counterpart to the unification: Home browses the local library, so a
    // streaming station must not appear there just because it is audible.
    let mut a = demo();
    let local = a
        .current_track()
        .map(|t| t.title.clone())
        .expect("demo has a local track");
    radio_playing(&mut a);
    a.notification = None; // the "Tuning in…" toast isn't a playback display

    let s = render_layout(&mut a, L::Dashboard, 120, 24);
    assert!(s.contains(&local), "Home still shows the local track");
    assert!(
        !s.contains("Cairo Jazz"),
        "no radio bleeds into the local browser"
    );
}

#[test]
fn lyrics_follow_the_playing_source_and_dont_clobber_its_slot() {
    use crate::app::LyricsPane;
    let mut a = demo();
    spotify_playing(&mut a);

    a.layout = L::LyricsFocus;
    assert_eq!(
        a.active_lyrics_pane(),
        LyricsPane::Spotify,
        "the Lyrics view targets the streaming Spotify track"
    );

    // a source view keeps its own pane regardless of what is audible — Home docks
    // a Local lyrics pane, so fetching Spotify's words would fill the wrong slot
    a.layout = L::Dashboard;
    assert_eq!(a.active_lyrics_pane(), LyricsPane::Local);
}

#[test]
fn the_visualizer_runs_for_every_source_in_a_player_view() {
    // All three sources feed the same audio ring, so the FFT already sees them;
    // the only question is whether a view lets the bars move.
    for (label, setup) in [
        ("spotify", spotify_playing as fn(&mut AppState)),
        ("radio", radio_playing as fn(&mut AppState)),
    ] {
        let mut app = demo();
        app.player.spectrum = vec![0.5; crate::app::VIZ_BANDS];
        setup(&mut app);
        app.layout = L::Concert;
        for _ in 0..12 {
            app.update(crate::action::Action::Tick);
        }
        assert!(
            app.viz.levels.iter().any(|&v| v > 0.01),
            "{label}: the visualizer moves in a player view while it streams"
        );
    }

    // ...but a *source* view stays flat for someone else's audio
    let mut app = demo();
    app.player.spectrum = vec![0.5; crate::app::VIZ_BANDS];
    radio_playing(&mut app);
    app.layout = L::Dashboard;
    for _ in 0..40 {
        app.update(crate::action::Action::Tick);
    }
    assert!(
        app.viz.levels.iter().all(|&v| v < 0.05),
        "the local browser does not animate to radio audio"
    );
}

#[test]
fn add_to_playlist_targets_the_playing_source() {
    use crate::action::Action;
    // In a player view `a` adds what is *playing*, not whatever local track was
    // last loaded. Before this it always took `player.current`, so adding while
    // Spotify streamed filed a stale local track into a local playlist.
    let mut a = demo();
    spotify_playing(&mut a);
    a.layout = L::Concert;
    a.update(Action::AddToPlaylistPrompt);
    assert!(
        a.input.add_targets.is_empty(),
        "no LOCAL picker opens for a Spotify track"
    );
    assert!(
        a.spotify.pl_modal.is_some(),
        "the Spotify playlist modal opens instead"
    );

    // radio routes to the station playlists, again not the local picker
    let mut a = demo();
    radio_playing(&mut a);
    a.layout = L::Concert;
    a.update(Action::AddToPlaylistPrompt);
    assert!(
        a.input.add_targets.is_empty(),
        "no local picker for a station"
    );

    // ...and a local track still opens the local picker
    let mut a = demo();
    a.player.status = crate::core::player::Status::Playing;
    a.layout = L::Concert;
    a.update(Action::AddToPlaylistPrompt);
    assert!(
        !a.input.add_targets.is_empty(),
        "a local track still uses the local picker"
    );
}

#[test]
fn a_source_view_keeps_its_own_add_destination() {
    use crate::action::Action;
    // Home browses the local library: `a` there files the *selected local row*,
    // even while Spotify streams. Only the player views follow the audio.
    let mut a = demo();
    spotify_playing(&mut a);
    a.layout = L::Dashboard;
    a.update(Action::AddToPlaylistPrompt);
    assert!(
        !a.input.add_targets.is_empty(),
        "the local browser still opens the local picker"
    );
    assert!(a.spotify.pl_modal.is_none());
}

#[test]
fn the_session_remembers_which_source_was_playing() {
    // Reopening after a Spotify session used to land on the last *local* track:
    // nothing streams on launch, so with no memory the fallback picked local by
    // priority order even though Spotify's paused track had been restored too.
    let mut a = demo();
    spotify_playing(&mut a);
    a.update(crate::action::Action::Tick); // records the audible source
    let saved = a.session();
    assert_eq!(
        saved.last_source.as_deref(),
        Some("spotify"),
        "the source is saved as a stable key"
    );

    // `apply_session` is the launch path for non-library state (the Spotify
    // overlay, panels, focus); `restore_library_state` only covers what needs the
    // scanned library first.
    let mut b = demo();
    b.apply_session(saved);
    b.layout = L::LyricsFocus;
    assert_eq!(b.audible_source(), None, "nothing streams on launch");
    assert_eq!(
        b.now_playing_source(),
        Some(NpSource::Spotify),
        "the player views reopen on the Spotify track, not the local one"
    );
}

#[test]
fn transport_keys_drive_the_source_the_player_view_shows() {
    use crate::action::Action;
    // The view showed the restored Spotify track, but `space` started the local
    // one underneath it — display followed the source while transport did not.
    let mut a = demo();
    spotify_playing(&mut a);
    a.update(Action::Tick); // it was audible — that is what gets recorded
    a.spov.spotify_paused = true; // then paused, as after a relaunch
    a.layout = L::LyricsFocus;
    assert_eq!(
        a.now_playing_source(),
        Some(NpSource::Spotify),
        "the view is about the Spotify track"
    );

    a.update(Action::TogglePlay);
    assert_ne!(
        a.player.status,
        crate::core::player::Status::Playing,
        "space must NOT start the local player in a player view showing Spotify"
    );

    // a source view is unchanged: `space` in Home still means "play my music"
    let mut b = demo();
    spotify_playing(&mut b);
    b.layout = L::Dashboard;
    b.update(Action::TogglePlay);
    assert_eq!(
        b.player.status,
        crate::core::player::Status::Playing,
        "Home still plays the local library"
    );
}

#[test]
fn next_and_previous_follow_the_player_views_source() {
    use crate::action::Action;
    let mut a = demo();
    let local_before = a.player.queue.position;
    spotify_playing(&mut a);
    a.layout = L::Concert;

    a.update(Action::Next);
    assert_eq!(
        a.player.queue.position, local_before,
        "n steps the Spotify track, not the local queue"
    );
    a.update(Action::Previous);
    assert_eq!(a.player.queue.position, local_before, "same for p");
}

#[test]
fn enter_acts_on_the_playing_source_not_the_local_list() {
    use crate::action::Action;
    // Same bug as space: a player view has no list, so Enter fell through to the
    // local tracklist selection and started a local track under a Spotify one.
    let mut a = demo();
    spotify_playing(&mut a);
    a.update(Action::Tick);
    a.spov.spotify_paused = true;
    a.layout = L::Concert;

    a.update(Action::Activate);
    assert_ne!(
        a.player.status,
        crate::core::player::Status::Playing,
        "Enter must not start the local player in a player view showing Spotify"
    );
}

#[test]
fn favourite_stars_the_playing_source() {
    use crate::action::Action;
    let mut a = demo();
    let local_fav = |app: &AppState| {
        app.player
            .current
            .and_then(|id| app.library.track(id))
            .is_some_and(|t| t.favorite)
    };
    let before = local_fav(&a);
    spotify_playing(&mut a);
    a.layout = L::LyricsFocus;

    a.update(Action::ToggleFavoriteSel);
    assert_eq!(
        local_fav(&a),
        before,
        "f must not favourite the local track while the view shows Spotify"
    );
}

#[test]
fn the_player_views_host_no_dock_panes() {
    // Each does one thing. A docked queue beside Now Playing showed the *local*
    // queue next to a possibly-Spotify track, and a second visualizer competed
    // with the one the view already draws.
    for l in [L::FullPlayer, L::LyricsFocus, L::Concert] {
        assert!(l.panels().is_empty(), "{l:?} hosts no dock panes");
        let mut a = demo();
        a.layout = l;
        assert!(
            !a.popup_tab_names().contains(&"Panes"),
            "{l:?} settings offer no Panes tab, since it has none"
        );
    }
    // the browsing views keep theirs
    assert!(!L::Dashboard.panels().is_empty());
}
