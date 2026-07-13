//! Spotify playback snapshot/behaviour tests, split out of spotify_tests.rs.
//! A child of `tests`, so `use super::*` pulls in the shared demo()/render_layout.

use super::*;

#[test]
fn spotify_drill_in_pushes_and_back_restores_the_parent_list() {
    use crate::spotify::Tokens;
    use crate::spotify::api::{Item, Kind};
    let mk = |name: &str, kind: Kind| Item {
        uri: format!("spotify:{}:{name}", "x"),
        name: name.into(),
        kind,
        ..Default::default()
    };
    let mut a = demo();
    a.layout = Layout::Spotify;
    // a session token is required to drill in (only `is_some` is checked)
    a.spotify.tokens = Some(Tokens {
        access_token: "t".into(),
        refresh_token: "r".into(),
        expires_at: 0,
        scopes: String::new(),
    });
    // parent list: two albums, cursor on the second
    a.spotify.items = vec![mk("Album One", Kind::Album), mk("Album Two", Kind::Album)];
    a.spotify.sel = 1;
    a.spotify.crumb = None;

    // drill into the selected album (Album = Web-API path, no live session)
    let item = a.spotify.items[1].clone();
    a.spotify_open(item);
    assert_eq!(a.spotify.nav_stack_len(), 1, "one frame pushed");
    assert!(
        a.spotify.crumb.as_deref() == Some("◉ Album Two"),
        "breadcrumb names the container"
    );
    assert_eq!(a.spotify.sel, 0, "cursor resets at the child level");
    assert!(
        a.spotify.items.is_empty(),
        "parent list moved into the frame"
    );

    // the fetched child tracks land; the user moves the cursor
    a.spotify.items = vec![mk("Track A", Kind::Track), mk("Track B", Kind::Track)];
    a.spotify.sel = 1;

    // Esc pops back: the parent list + cursor + crumb are restored verbatim
    a.spotify_cancel();
    assert_eq!(a.spotify.nav_stack_len(), 0, "frame popped");
    assert_eq!(a.spotify.items.len(), 2, "parent albums restored");
    assert_eq!(a.spotify.items[1].name, "Album Two");
    assert_eq!(a.spotify.sel, 1, "parent cursor restored");
    assert!(
        a.spotify.crumb.is_none(),
        "back at the top level (no crumb)"
    );
}

#[test]
fn q_is_quit_only_and_back_keys_pop_a_spotify_drill_in() {
    use crate::action::Action;
    use crate::event::{Key, KeyCode, Mods};
    use crate::spotify::Tokens;
    use crate::spotify::api::{Item, Kind};
    let mk = |name: &str, kind: Kind| Item {
        uri: format!("spotify:x:{name}"),
        name: name.into(),
        kind,
        ..Default::default()
    };
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.tokens = Some(Tokens {
        access_token: "t".into(),
        refresh_token: "r".into(),
        expires_at: 0,
        scopes: String::new(),
    });
    a.spotify.items = vec![mk("Album One", Kind::Album), mk("Album Two", Kind::Album)];
    a.spotify.sel = 1;

    // q is quit-only now (never drills out); back / up-one-level is esc + ctrl-o.
    let q = Key {
        code: KeyCode::Char('q'),
        mods: Mods::default(),
    };
    assert_eq!(crate::keymap::map(&a, q), Action::Quit, "q maps to Quit");
    let ctrl_o = Key {
        code: KeyCode::Char('o'),
        mods: Mods {
            ctrl: true,
            ..Default::default()
        },
    };
    assert_eq!(
        crate::keymap::map(&a, ctrl_o),
        Action::Back,
        "ctrl-o maps to Back (vim-style up one level)"
    );

    // drill into an album, then Back (esc/ctrl-o) pops the level instead of quitting
    let item = a.spotify.items[1].clone();
    a.spotify_open(item);
    assert_eq!(a.spotify.nav_stack_len(), 1, "drilled in");
    a.running = true;
    a.update(Action::Back);
    assert!(
        a.running,
        "Back on a Spotify drill-in pops the level — does NOT quit"
    );
    assert_eq!(
        a.spotify.nav_stack_len(),
        0,
        "Back popped the Spotify drill-in"
    );

    // at the top level (nothing to back out of), q quits the app
    a.update(Action::Quit);
    assert!(!a.running, "q/Quit exits the app");
}

#[test]
fn spotify_like_toggle_is_optimistic_and_sends_set_saved() {
    use crate::spotify::Tokens;
    use crate::spotify::api::{Item, Kind, SpRequest};
    let (tx, rx) = crossbeam_channel::unbounded::<SpRequest>();
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.set_spotify_sender(tx); // tokens still None → no auto-resume spawn
    a.spotify.tokens = Some(Tokens {
        access_token: "t".into(),
        refresh_token: "r".into(),
        expires_at: 0,
        scopes: String::new(),
    });

    // nothing playing → the toggle is a no-op (no request, no state change)
    a.spov.sp_saved = false;
    a.spotify_toggle_saved();
    assert!(!a.spov.sp_saved);
    assert!(
        rx.try_recv().is_err(),
        "no request without a now-playing track"
    );

    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:abc".into(),
        name: "Song".into(),
        kind: Kind::Track,
        ..Default::default()
    });
    // like: flips optimistically + sends SetSaved{saved:true}
    a.spotify_toggle_saved();
    assert!(a.spov.sp_saved, "optimistically marked saved");
    assert!(
        matches!(rx.try_recv(), Ok(SpRequest::SetSaved { saved: true, .. })),
        "a SetSaved(true) request is queued"
    );
    // unlike: flips back + sends SetSaved{saved:false}
    a.spotify_toggle_saved();
    assert!(!a.spov.sp_saved, "optimistically un-saved");
    assert!(
        matches!(rx.try_recv(), Ok(SpRequest::SetSaved { saved: false, .. })),
        "a SetSaved(false) request is queued"
    );
}

#[test]
fn a_failed_like_reverts_the_optimistic_heart() {
    use crate::spotify::Tokens;
    use crate::spotify::api::{Item, Kind, SpRequest, SpResult};
    let (tx, rx) = crossbeam_channel::unbounded::<SpRequest>();
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.set_spotify_sender(tx);
    a.spotify.tokens = Some(Tokens {
        access_token: "t".into(),
        refresh_token: "r".into(),
        expires_at: 0,
        scopes: String::new(),
    });
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:abc".into(),
        name: "Song".into(),
        kind: Kind::Track,
        ..Default::default()
    });

    // like → optimistically marked saved (heart fills) + a SetSaved is queued
    a.spotify_toggle_saved();
    assert!(a.spov.sp_saved, "optimistically saved");
    assert!(matches!(rx.try_recv(), Ok(SpRequest::SetSaved { .. })));

    // the worker bounces it (e.g. 403) — the optimistic heart must not keep lying
    a.on_spotify_result(SpResult::Error {
        key: String::new(),
        msg: "Couldn't update Liked Songs: 403".into(),
    });
    assert!(
        !a.spov.sp_saved,
        "a failed like clears the optimistic heart instead of leaving it filled"
    );
    assert!(
        matches!(rx.try_recv(), Ok(SpRequest::CheckSaved { .. })),
        "it reconciles with Spotify's real saved state after the failure"
    );
}

#[test]
fn spotify_playback_state_survives_a_session_round_trip() {
    use crate::spotify::api::{Item, Kind};
    let mk = |name: &str| Item {
        uri: format!("spotify:track:{name}"),
        name: name.into(),
        subtitle: "Ed Sheeran".into(),
        album: "Divide".into(),
        image: None,
        kind: Kind::Track,
        duration_ms: 200_000,
        artist_uri: None,
        ..Default::default()
    };
    let mut a = demo();
    a.spov.now_spotify = Some(mk("Perfect"));
    a.spov.sp_queue = vec![mk("Shape of You"), mk("Perfect"), mk("Castle on the Hill")];
    a.spov.sp_idx = 1;
    a.spov.sp_pos = 42.0;
    a.spov.sp_shuffle = true;
    a.spov.sp_repeat = crate::core::player::Repeat::All;

    // capture + reapply into a fresh instance
    let sess = a.session();
    let mut b = demo();
    b.apply_session(sess);
    assert_eq!(
        b.spov.now_spotify.as_ref().map(|t| t.name.as_str()),
        Some("Perfect"),
        "now-playing track restored"
    );
    assert_eq!(b.spov.sp_queue.len(), 3, "queue restored");
    assert_eq!(b.spov.sp_idx, 1, "queue position restored");
    assert_eq!(b.spov.sp_pos, 42.0, "elapsed position restored");
    assert_eq!(b.spov.sp_dur, 200.0, "duration rebuilt from the track");
    assert!(
        b.spov.spotify_paused && !b.spov.sp_started,
        "restored paused, not auto-streamed"
    );
    assert!(b.spov.sp_shuffle, "shuffle mode restored");
    assert_eq!(
        b.spov.sp_repeat,
        crate::core::player::Repeat::All,
        "repeat mode restored"
    );
    // restoring shuffle must NOT re-shuffle the persisted (already-shuffled) queue —
    // the order and next-up track are exactly what was saved
    assert_eq!(
        b.spov
            .sp_queue
            .iter()
            .map(|t| t.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Shape of You", "Perfect", "Castle on the Hill"],
        "the saved queue order is preserved (no re-shuffle on restore)"
    );
}

#[test]
fn space_pauses_a_buffering_spotify_track() {
    // regression: while a track buffered (loaded, autoplay, no Playing yet),
    // space called spotify_resume() — re-loading instead of pausing. Now space
    // pauses, cancelling the pending play so it won't start after buffering.
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::session::SessionCommand;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:x".into(),
        name: "Song".into(),
        kind: Kind::Track,
        duration_ms: 200_000,
        ..Default::default()
    });
    a.spov.sp_started = false; // buffering: not yet Playing…
    a.spov.spotify_paused = false; // …and not paused
    let (tx, rx) = crossbeam_channel::unbounded::<SessionCommand>();
    a.spov.session_cmd = Some(tx);
    assert!(a.sp_buffering(), "precondition: track is buffering");

    a.toggle_play();
    assert!(a.spov.spotify_paused, "space while buffering → paused");
    assert!(
        matches!(rx.try_recv(), Ok(SessionCommand::Pause)),
        "sends Pause (cancels the pending play), not a reload"
    );
    assert!(!a.sp_buffering(), "no longer shown as buffering");
}

#[test]
fn librespot_events_dont_override_the_user_pause_intent() {
    // regression: rapid space presses desynced the play/pause icon + visualizer
    // from the real state because laggy, out-of-order Playing/Paused events
    // wrote `spotify_paused`. They must only update position/started now; the
    // pause flag is owned by toggle_play.
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::session::SessionEvent;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:x".into(),
        name: "Song".into(),
        kind: Kind::Track,
        duration_ms: 200_000,
        ..Default::default()
    });
    a.spov.spotify_paused = false; // user's latest intent: playing
    a.spov.sp_started = true;
    let (tx, rx) = crossbeam_channel::unbounded::<SessionEvent>();
    a.spov.session_rx = Some(rx);
    // a stale Paused event (from an earlier toggle) arrives late
    tx.send(SessionEvent::Paused { position_ms: 5000 }).unwrap();
    a.pump_spotify_session();
    assert!(
        !a.spov.spotify_paused,
        "a late Paused event must not flip the UI to paused"
    );
    assert_eq!(a.spov.sp_pos, 5.0, "it still updates the position");
}

#[test]
fn playing_an_episode_resolves_its_source_first() {
    // an episode isn't Load-ed via librespot directly — it's resolved first, so
    // externally-hosted ones can stream outside librespot (which can't play them)
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::session::SessionCommand;
    let mut a = demo();
    a.layout = Layout::Spotify;
    let (tx, rx) = crossbeam_channel::unbounded::<SessionCommand>();
    a.spov.session_cmd = Some(tx); // makes ensure_session a no-op success
    a.spotify.tokens = Some(crate::spotify::Tokens {
        access_token: "a".into(),
        refresh_token: "r".into(),
        expires_at: crate::datetime::now_unix() + 3600, // valid → session is reused
        scopes: "streaming".into(),
    });
    let ep = Item {
        uri: "spotify:episode:1".into(),
        name: "Ep".into(),
        kind: Kind::Track,
        duration_ms: 3_600_000,
        ..Default::default()
    };
    a.spotify_play(vec![ep], 0);
    assert!(
        matches!(rx.try_recv(), Ok(SessionCommand::ResolveEpisode { uri, .. }) if uri == "spotify:episode:1"),
        "episode playback resolves the source first (not a direct librespot Load)"
    );
}

#[test]
fn external_episode_streams_via_the_engine() {
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::session::SessionEvent;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spov.now_spotify = Some(Item {
        uri: "spotify:episode:1".into(),
        kind: Kind::Track,
        duration_ms: 3_600_000,
        ..Default::default()
    });
    let (tx, rx) = crossbeam_channel::unbounded::<SessionEvent>();
    a.spov.session_rx = Some(rx);
    tx.send(SessionEvent::EpisodeResolved {
        uri: "spotify:episode:1".into(),
        url: Some("https://cdn.example/ep.mp3".into()),
        position_ms: 0,
    })
    .unwrap();
    a.pump_spotify_session();
    assert!(
        a.spov.sp_stream,
        "an external-URL episode plays through lyrfin's engine stream"
    );
}

#[test]
fn spotify_hosted_episode_is_unplayable() {
    // DRM'd episode with no external MP3 AND no show name to look up → no RSS
    // fallback possible → fails immediately (never issues a hanging librespot
    // Load). librespot is denied the decryption key for episodes.
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::session::{SessionCommand, SessionEvent};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spov.now_spotify = Some(Item {
        uri: "spotify:episode:1".into(),
        kind: Kind::Track,
        ..Default::default() // no `album` → no show name → no RSS attempt
    });
    let (etx, erx) = crossbeam_channel::unbounded::<SessionEvent>();
    a.spov.session_rx = Some(erx);
    let (ctx, crx) = crossbeam_channel::unbounded::<SessionCommand>();
    a.spov.session_cmd = Some(ctx);
    etx.send(SessionEvent::EpisodeResolved {
        uri: "spotify:episode:1".into(),
        url: None,
        position_ms: 0,
    })
    .unwrap();
    a.pump_spotify_session();
    assert!(!a.spov.sp_stream, "not streamed");
    assert!(a.spov.spotify_paused, "stopped immediately (no 25s hang)");
    assert!(
        crx.try_recv().is_err(),
        "no librespot Load is issued for a key-denied episode"
    );
}

#[test]
fn drm_episode_falls_back_to_rss_resolution() {
    // a Spotify-hosted episode (no external MP3) with a known show name asks the
    // podcast resolver to find its public RSS MP3 instead of failing.
    use crate::podcastfetch::PodcastRequest;
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::session::SessionEvent;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spov.now_spotify = Some(Item {
        uri: "spotify:episode:1".into(),
        name: "Ep Title".into(),
        album: "My Show".into(), // episode_item stores the show name here
        kind: Kind::Track,
        ..Default::default()
    });
    let (ptx, prx) = crossbeam_channel::unbounded::<PodcastRequest>();
    a.set_podcast_sender(ptx);
    let (etx, erx) = crossbeam_channel::unbounded::<SessionEvent>();
    a.spov.session_rx = Some(erx);
    etx.send(SessionEvent::EpisodeResolved {
        uri: "spotify:episode:1".into(),
        url: None,
        position_ms: 0,
    })
    .unwrap();
    a.pump_spotify_session();
    assert!(
        matches!(prx.try_recv(), Ok(PodcastRequest { show, title, .. }) if show == "My Show" && title == "Ep Title"),
        "a DRM'd episode triggers an RSS lookup by show + title"
    );
}

#[test]
fn streamed_episode_seek_is_debounced_not_burst() {
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spov.now_spotify = Some(Item {
        uri: "spotify:episode:1".into(),
        kind: Kind::Track,
        ..Default::default()
    });
    a.spov.sp_started = true;
    a.spov.sp_stream = true; // an externally-streamed episode (engine, ranged re-open)
    a.spov.sp_dur = 3600.0;
    a.spov.sp_pos = 100.0;

    let start = a.spov.sp_pos;
    a.spotify_seek(1); // sign gives direction; lyrfin computes its own (accelerating) step
    let first = a.spov.sp_pos;
    assert!(first > start, "the bar scrubs forward immediately");
    assert!(
        a.spov.sp_seek_at.is_some() && a.spov.sp_seek_target.is_some(),
        "the engine re-open is deferred and the bar is locked to the target"
    );

    // a held seek accelerates: the second step is larger than the first, and it still
    // coalesces into the single pending re-open (not a burst)
    a.spotify_seek(1);
    let second = a.spov.sp_pos;
    assert!(
        second - first > first - start,
        "holding h/l ramps the step up (accelerates)"
    );
    assert!(
        a.spov.sp_seek_at.is_some(),
        "still one pending seek, not a burst of re-buffers"
    );
}

#[test]
fn resolved_podcast_mp3_streams_via_engine() {
    use crate::podcastfetch::PodcastResult;
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spov.now_spotify = Some(Item {
        uri: "spotify:episode:1".into(),
        kind: Kind::Track,
        ..Default::default()
    });
    a.on_podcast_result(PodcastResult {
        key: "spotify:episode:1".into(),
        url: Some("https://cdn.example/ep.mp3".into()),
    });
    assert!(
        a.spov.sp_stream,
        "the resolved public MP3 streams via the engine"
    );
    // a stale result (different episode) is ignored
    a.on_podcast_result(PodcastResult {
        key: "spotify:episode:OTHER".into(),
        url: None,
    });
    assert!(a.spov.sp_stream, "a stale resolver result is ignored");
}

#[test]
fn unresolvable_podcast_reports_unplayable() {
    use crate::podcastfetch::PodcastResult;
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spov.now_spotify = Some(Item {
        uri: "spotify:episode:1".into(),
        kind: Kind::Track,
        ..Default::default()
    });
    a.on_podcast_result(PodcastResult {
        key: "spotify:episode:1".into(),
        url: None,
    });
    assert!(
        a.spov.spotify_paused && !a.spov.sp_stream,
        "no public match → stopped, not streaming"
    );
}

#[test]
fn streamed_episode_seek_goes_to_the_engine_not_librespot() {
    // a streamed episode scrubs via lyrfin's engine (ranged HTTP), so no librespot
    // Seek is issued; the position still advances.
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::session::SessionCommand;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spov.now_spotify = Some(Item {
        uri: "spotify:episode:1".into(),
        kind: Kind::Track,
        ..Default::default()
    });
    a.spov.sp_dur = 100.0;
    a.spov.sp_pos = 10.0;
    a.spov.sp_started = true;
    a.spov.sp_stream = true;
    let (tx, rx) = crossbeam_channel::unbounded::<SessionCommand>();
    a.spov.session_cmd = Some(tx);
    a.spotify_seek(30);
    assert!(a.spov.sp_pos > 10.0, "seek advances the clock");
    assert!(
        rx.try_recv().is_err(),
        "no librespot Seek — a streamed episode seeks through the engine"
    );
}

#[test]
fn librespot_track_seek_uses_the_session() {
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::session::SessionCommand;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:1".into(),
        kind: Kind::Track,
        ..Default::default()
    });
    a.spov.sp_dur = 100.0;
    a.spov.sp_pos = 10.0;
    a.spov.sp_started = true;
    a.spov.sp_stream = false; // a librespot track
    let (tx, rx) = crossbeam_channel::unbounded::<SessionCommand>();
    a.spov.session_cmd = Some(tx);
    a.spotify_seek(5);
    assert!(
        matches!(rx.try_recv(), Ok(SessionCommand::Seek(ms)) if ms == 15_000),
        "a librespot track seeks via the session"
    );
}

#[test]
fn audio_key_throttle_retries_in_place_instead_of_backing_off() {
    // The dominant audio-key "denial" is a transient throttle from skipping fast:
    // lyrfin schedules a quick same-session retry rather than tearing the track down
    // and arming a 20s back-off (a fresh session would hit the same throttle).
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::session::SessionCommand;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:1".into(),
        kind: Kind::Track,
        ..Default::default()
    });
    a.spov.sp_started = false; // still buffering — the key was refused before play
    a.spov.sp_stream = false; // a librespot track (audio keys apply)
    let (tx, rx) = crossbeam_channel::unbounded::<SessionCommand>();
    a.spov.session_cmd = Some(tx);

    a.spotify_playback_blocked();
    assert!(
        a.spov.sp_keyretry_at.is_some(),
        "a quick retry is scheduled"
    );
    assert_eq!(a.spov.sp_keyretry_n, 1, "one retry spent");
    assert_eq!(a.spov.sp_cooldown_until, 0, "no back-off armed");
    assert!(a.spov.now_spotify.is_some(), "the track stays on screen");
    assert!(!a.spov.spotify_paused, "not marked failed");
    assert!(
        rx.try_recv().is_err(),
        "the reload is deferred to the tick, not sent immediately"
    );

    // A burst of key errors sets the global probe flag repeatedly → the extra echoes
    // must fold into the one pending retry, not spend the whole budget at once.
    a.spotify_playback_blocked();
    assert_eq!(
        a.spov.sp_keyretry_n, 1,
        "burst echoes fold into the pending retry"
    );
}

#[test]
fn audio_key_denial_escalates_once_the_retry_budget_is_spent() {
    // A genuine DRM/unavailable block fails every retry: once the quick-retry budget
    // is exhausted lyrfin falls through to the back-off (here, with an empty queue, the
    // reconnect path is skipped and the cooldown trips).
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::session::SessionCommand;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:1".into(),
        kind: Kind::Track,
        ..Default::default()
    });
    a.spov.sp_started = false;
    a.spov.sp_stream = false;
    a.spov.sp_queue.clear(); // no queue → reconnect skipped, so the cooldown escalates
    a.spov.sp_keyretry_n = 5; // budget already spent
    let (tx, _rx) = crossbeam_channel::unbounded::<SessionCommand>();
    a.spov.session_cmd = Some(tx);

    a.spotify_playback_blocked();
    assert!(
        a.spov.sp_cooldown_until > 0,
        "backs off after the retries are spent"
    );
    assert!(a.spov.spotify_paused, "the failed track is paused/stopped");
}

#[test]
fn rapid_skips_debounce_into_a_single_load() {
    // Hammering "next" must not fire a librespot Load per intermediate track (the
    // audio-key burst Spotify throttles): the target accumulates and previews, and
    // only the track finally landed on loads once skipping stops.
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::session::SessionCommand;
    let mk = |name: &str| Item {
        uri: format!("spotify:track:{name}"),
        name: name.into(),
        kind: Kind::Track,
        duration_ms: 200_000,
        ..Default::default()
    };
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.tokens = Some(crate::spotify::Tokens {
        access_token: "a".into(),
        refresh_token: "r".into(),
        expires_at: crate::datetime::now_unix() + 3600, // valid → session is reused
        scopes: "streaming".into(),
    });
    a.spov.sp_queue = vec![mk("A"), mk("B"), mk("C"), mk("D")];
    a.spov.sp_idx = 0;
    a.spov.now_spotify = Some(mk("A"));
    a.spov.sp_started = true; // A is playing
    let (tx, rx) = crossbeam_channel::unbounded::<SessionCommand>();
    a.spov.session_cmd = Some(tx);

    // three fast presses → the target accumulates to D (idx 3) and previews it, but
    // nothing loads yet and the playing track is untouched
    a.spotify_track(1);
    a.spotify_track(1);
    a.spotify_track(1);
    assert_eq!(
        a.spov.sp_skip_target,
        Some(3),
        "target accumulates across presses"
    );
    assert_eq!(
        a.spotify.queue_sel, 3,
        "queue cursor previews the landing row"
    );
    assert_eq!(
        a.spov.sp_idx, 0,
        "the playing index is untouched until the load fires"
    );
    assert!(
        rx.try_recv().is_err(),
        "no librespot Load while the skip is debounced"
    );

    // the debounce elapses → exactly one Load fires, for the final track D (driven
    // through the real pump, the way the run loop calls it every frame)
    a.spov.sp_skip_at = Some(std::time::Instant::now()); // due now
    a.pump_spotify();
    assert_eq!(
        a.spov.sp_idx, 3,
        "the landed-on track becomes the playing index"
    );
    assert_eq!(a.spov.sp_skip_target, None, "the pending skip is consumed");
    let loads = std::iter::from_fn(|| rx.try_recv().ok())
        .filter(|c| matches!(c, SessionCommand::Load { .. }))
        .count();
    assert_eq!(loads, 1, "one Load for the whole burst, not one per skip");
}

#[test]
fn spotify_browse_view_survives_a_session_round_trip() {
    use crate::spotify::api::{Item, Kind, Section, SpResult};
    let mut a = demo();
    a.spotify.section = Section::Albums;
    a.spotify.sel = 3;
    a.spotify.query = "radiohead".into();
    a.spotify.in_search = true;

    let sess = a.session();
    let mut b = demo();
    b.apply_session(sess);
    assert_eq!(b.spotify.section, Section::Albums, "section restored");
    assert_eq!(b.spotify.query, "radiohead", "search query restored");
    assert!(b.spotify.in_search, "search mode restored");
    assert_eq!(
        b.spotify.restore_sel,
        Some(3),
        "cursor pending until list loads"
    );

    // when the list arrives (empty key matches a fresh state), the cursor
    // lands on the saved row and the one-shot restore clears.
    let mk = |n: &str| Item {
        uri: format!("spotify:album:{n}"),
        name: n.into(),
        subtitle: String::new(),
        album: String::new(),
        image: None,
        kind: Kind::Album,
        duration_ms: 0,
        artist_uri: None,
        ..Default::default()
    };
    b.on_spotify_result(SpResult::Library {
        key: String::new(),
        items: vec![mk("a"), mk("b"), mk("c"), mk("d"), mk("e")],
    });
    assert_eq!(
        b.spotify.sel, 3,
        "restored cursor applied to the loaded list"
    );
    assert_eq!(
        b.spotify.restore_sel, None,
        "one-shot cleared after applying"
    );
}

#[test]
fn spotify_drill_in_restores_on_reconnect() {
    use crate::spotify::api::{Item, Kind, Section, SpResult};
    use crate::spotify::{ConnState, Tokens};
    let mk = |n: &str, k: Kind| Item {
        uri: format!("spotify:{n}"),
        name: n.into(),
        subtitle: String::new(),
        album: String::new(),
        image: None,
        kind: k,
        duration_ms: 0,
        artist_uri: None,
        ..Default::default()
    };
    // drilled into an album, cursor on its 3rd track
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.section = Section::Albums;
    a.spotify.open_item = Some(mk("album:1", Kind::Album));
    a.spotify.crumb = Some("◉ Some Album".into());
    a.spotify.sel = 2;

    let sess = a.session();
    assert!(sess.spotify_open.is_some(), "drill-in container persisted");

    let mut b = demo();
    b.layout = Layout::Spotify;
    b.spotify.tokens = Some(Tokens {
        access_token: "t".into(),
        refresh_token: "r".into(),
        expires_at: 0,
        scopes: String::new(),
    });
    b.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    b.apply_session(sess);
    // a cross-launch drill-in is pending as a reconstructed Item (uri/kind/name)
    assert_eq!(
        b.spotify.restore_open.as_ref().map(|i| i.uri.as_str()),
        Some("spotify:album:1"),
        "drill-in pending restore"
    );
    assert_eq!(
        b.spotify.restore_sel,
        Some(2),
        "in-container cursor pending"
    );

    // the section list lands (empty key matches a fresh state) → re-opens the
    // drilled-in container, leaving the cursor pending for ITS tracks
    b.on_spotify_result(SpResult::Library {
        key: String::new(),
        items: vec![mk("album:1", Kind::Album), mk("album:2", Kind::Album)],
    });
    assert!(
        b.spotify.crumb.is_some(),
        "re-entered the drilled-in container"
    );
    assert_eq!(
        b.spotify.open_item.as_ref().map(|i| i.uri.as_str()),
        Some("spotify:album:1"),
        "the open container is restored"
    );
    assert!(b.spotify.restore_open.is_none(), "restore_open consumed");
    assert_eq!(
        b.spotify.restore_sel,
        Some(2),
        "cursor still pending — applied when the container's tracks land"
    );
}

#[test]
fn spotify_drill_restores_even_when_container_is_off_list() {
    // The container you were drilled into (e.g. an artist opened from the now-playing
    // track) need NOT be in the reloaded section list on reconnect. The restore
    // reconstructs it from the persisted URI+kind+name and re-opens it anyway —
    // otherwise a new binary drops you back on the bare section (the reported bug).
    use crate::spotify::api::{Item, Kind, Section, SpResult};
    use crate::spotify::{ConnState, Tokens};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.section = Section::Artists;
    a.spotify.open_item = Some(Item {
        uri: "spotify:artist:rahma".into(),
        name: "Rahma Riad".into(),
        kind: Kind::Artist,
        ..Default::default()
    });
    a.spotify.crumb = Some("☻ Rahma Riad".into());
    let sess = a.session();
    // the drill persists as stable primitives, not the whole Item
    let drill = sess.spotify_open.as_ref().expect("drill persisted");
    assert_eq!(drill.uri, "spotify:artist:rahma");
    assert_eq!(drill.kind, Kind::Artist);
    assert_eq!(drill.name, "Rahma Riad");

    let mut b = demo();
    b.layout = Layout::Spotify;
    b.spotify.tokens = Some(Tokens {
        access_token: "t".into(),
        refresh_token: "r".into(),
        expires_at: 0,
        scopes: String::new(),
    });
    b.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    b.apply_session(sess);
    // the Artists list lands WITHOUT Rahma Riad (not a followed artist) — the drill
    // must still re-open from the reconstructed descriptor
    b.on_spotify_result(SpResult::Library {
        key: String::new(),
        items: vec![Item {
            uri: "spotify:artist:someone-else".into(),
            name: "Someone Else".into(),
            kind: Kind::Artist,
            ..Default::default()
        }],
    });
    assert_eq!(
        b.spotify.open_item.as_ref().map(|i| i.uri.as_str()),
        Some("spotify:artist:rahma"),
        "the off-list artist drill re-opened from the persisted descriptor"
    );
    assert_eq!(
        b.spotify.crumb.as_deref(),
        Some("☻ Rahma Riad"),
        "breadcrumb restored from the persisted name"
    );
}

#[test]
fn spotify_browse_section_restores_a_drilled_container_on_reopen() {
    // The Home/Browse feed lands via the pathfinder `SessionEvent::Browse`, NOT the
    // Web-API `Library` result — so that path must run the same restore hook, else a
    // container drilled from Browse (e.g. a chart) is dropped on every reopen, even
    // the same binary. (Kind::Album here keeps the open on the Web-API path so the
    // test doesn't spawn a librespot session; the kind is incidental to the hook.)
    use crate::spotify::api::{Item, Kind, Section};
    use crate::spotify::session::SessionEvent;
    use crate::spotify::{ConnState, Tokens};

    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.section = Section::Browse;
    a.spotify.open_item = Some(Item {
        uri: "spotify:album:chart".into(),
        name: "Top 50".into(),
        kind: Kind::Album,
        ..Default::default()
    });
    a.spotify.crumb = Some("◉ Top 50".into());
    let sess = a.session();

    let mut b = demo();
    b.layout = Layout::Spotify;
    b.spotify.tokens = Some(Tokens {
        access_token: "t".into(),
        refresh_token: "r".into(),
        expires_at: 0,
        scopes: String::new(),
    });
    b.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    b.apply_session(sess);
    assert_eq!(
        b.spotify.section,
        Section::Browse,
        "section restored to Browse"
    );

    // the Browse feed lands via a session event with category tiles — NOT the chart.
    // (the empty key matches the fresh `spotify.key` default, as the Library test does)
    let (tx, rx) = crossbeam_channel::unbounded();
    b.spov.session_rx = Some(rx);
    tx.send(SessionEvent::Browse {
        key: String::new(),
        items: vec![Item {
            uri: "spotify:page:pop".into(),
            name: "Pop".into(),
            kind: Kind::Category,
            ..Default::default()
        }],
        error: None,
    })
    .unwrap();
    b.pump_spotify_session();

    assert_eq!(
        b.spotify.open_item.as_ref().map(|i| i.uri.as_str()),
        Some("spotify:album:chart"),
        "the container drilled from Browse re-opened on reconnect"
    );
    assert_eq!(b.spotify.crumb.as_deref(), Some("◉ Top 50"));
}

#[test]
fn spotify_view_cache_restores_the_last_view_instantly() {
    use crate::spotify::Tokens;
    use crate::spotify::api::{Item, Kind, Section};
    use crate::spotify::view_cache::SpotifyViewCache;

    let mut a = demo();
    a.config.dir = std::env::temp_dir().join("lyrfin-view-cache-restore");
    let _ = std::fs::remove_dir_all(&a.config.dir);
    a.spotify.tokens = Some(Tokens {
        access_token: "t".into(),
        refresh_token: "r".into(),
        expires_at: 0,
        scopes: String::new(),
    });
    // we were connected + drilled into a Browse chart, then quit → snapshot to disk
    a.spotify.account_id = Some("acc1".into());
    a.spotify.section = Section::Browse;
    a.spotify.crumb = Some("≡ Top 50".into());
    a.spotify.open_item = Some(Item {
        uri: "spotify:playlist:chart".into(),
        name: "Top 50".into(),
        kind: Kind::Playlist,
        ..Default::default()
    });
    a.spotify.items = vec![Item {
        uri: "spotify:track:1".into(),
        name: "Song".into(),
        kind: Kind::Track,
        ..Default::default()
    }];
    a.spotify.sel = 0;
    a.spotify_save_view_cache();

    // a fresh launch clears the in-memory view; applying the cache shows it instantly
    a.spotify.section = Section::LikedSongs;
    a.spotify.crumb = None;
    a.spotify.open_item = None;
    a.spotify.items.clear();
    a.spotify.restored_account = None;
    a.spotify_apply_view_cache();

    assert_eq!(
        a.spotify.section,
        Section::Browse,
        "section restored from cache"
    );
    assert_eq!(
        a.spotify.open_item.as_ref().map(|i| i.uri.as_str()),
        Some("spotify:playlist:chart"),
        "the drilled container shows instantly"
    );
    assert_eq!(a.spotify.crumb.as_deref(), Some("≡ Top 50"));
    assert_eq!(
        a.spotify.items.len(),
        1,
        "the list shows instantly (before the network)"
    );
    assert_eq!(
        a.spotify.restored_account.as_deref(),
        Some("acc1"),
        "account recorded for the reconnect match check"
    );

    SpotifyViewCache::delete(&a.config.dir);
    let _ = std::fs::remove_dir_all(&a.config.dir);
}

#[test]
fn spotify_view_cache_is_gated_on_a_token() {
    use crate::spotify::api::{Item, Kind, Section};
    use crate::spotify::view_cache::SpotifyViewCache;

    let mut a = demo();
    a.config.dir = std::env::temp_dir().join("lyrfin-view-cache-nogate");
    let _ = std::fs::remove_dir_all(&a.config.dir);
    // a cache exists on disk...
    SpotifyViewCache {
        account_id: "acc1".into(),
        section: Section::Browse,
        items: vec![Item {
            uri: "spotify:track:1".into(),
            kind: Kind::Track,
            ..Default::default()
        }],
        ..Default::default()
    }
    .save(&a.config.dir);
    // ...but there's no token, so we won't reconnect to refresh → don't show stale data
    a.spotify.tokens = None;
    a.spotify.section = Section::LikedSongs;
    a.spotify_apply_view_cache();
    assert_eq!(
        a.spotify.section,
        Section::LikedSongs,
        "no token → the stale cache is not applied"
    );
    assert!(a.spotify.items.is_empty());

    SpotifyViewCache::delete(&a.config.dir);
    let _ = std::fs::remove_dir_all(&a.config.dir);
}

#[test]
fn spotify_load_initial_refreshes_the_open_container_in_place() {
    use crate::spotify::Tokens;
    use crate::spotify::api::{Item, Kind};

    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.tokens = Some(Tokens {
        access_token: "t".into(),
        refresh_token: "r".into(),
        expires_at: 0,
        scopes: String::new(),
    });
    // the cache put an album on screen; a session drill is also pending (redundant)
    a.spotify.open_item = Some(Item {
        uri: "spotify:album:1".into(),
        name: "A".into(),
        kind: Kind::Album, // Web-API path → no librespot session spawned in the test
        ..Default::default()
    });
    a.spotify.crumb = Some("◉ A".into());
    a.spotify.items = vec![Item {
        uri: "spotify:track:1".into(),
        kind: Kind::Track,
        ..Default::default()
    }];
    a.spotify.restore_open = Some(Item {
        uri: "spotify:album:1".into(),
        kind: Kind::Album,
        ..Default::default()
    });

    a.spotify_load_initial();

    // refresh-in-place: the shown container stays put (no section flash / blank), the
    // redundant pending drill is dropped, and a fresh fetch is in flight.
    assert!(
        a.spotify.restore_open.is_none(),
        "redundant pending drill cleared"
    );
    assert_eq!(
        a.spotify.open_item.as_ref().map(|i| i.uri.as_str()),
        Some("spotify:album:1"),
        "still showing the container"
    );
    assert_eq!(
        a.spotify.crumb.as_deref(),
        Some("◉ A"),
        "breadcrumb kept (no flash)"
    );
    assert!(
        !a.spotify.items.is_empty(),
        "the visible list is not blanked"
    );
    assert!(a.spotify.loading, "a background refresh is in flight");
}

#[test]
fn spotify_restored_cover_is_requested_on_launch() {
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:x".into(),
        name: "X".into(),
        subtitle: "Y".into(),
        album: "Z".into(),
        image: Some("http://img/1".into()),
        kind: Kind::Track,
        duration_ms: 1000,
        artist_uri: None,
        ..Default::default()
    });
    // wiring the art worker (as at launch, after the session is restored)
    // should fetch the restored track's cover — not wait for the user to play.
    let (tx, rx) = crossbeam_channel::unbounded();
    a.set_spotify_art_sender(tx);
    assert!(rx.try_recv().is_ok(), "cover art requested on launch");
    assert_eq!(a.spov.sp_cover_url.as_deref(), Some("http://img/1"));
}

/// The last track of a (long) Spotify queue ending under repeat-all wraps back to
/// the first and keeps playing — and under repeat-off it stops at the end instead.
/// A live (reusable) session is faked so the auto-advance's `spotify_play` runs
/// without spawning a real librespot thread.
#[test]
fn spotify_repeat_all_wraps_last_track_to_top() {
    use crate::core::player::Repeat;
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::session::{SessionCommand, SessionEvent};
    let build = |repeat: Repeat| {
        let mut a = demo();
        a.layout = Layout::Spotify;
        let (tx, _rx) = crossbeam_channel::unbounded::<SessionCommand>();
        a.spov.session_cmd = Some(tx); // ensure_session reuses this (no spawn)
        a.spotify.tokens = Some(crate::spotify::Tokens {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: crate::datetime::now_unix() + 3600,
            scopes: "streaming".into(),
        });
        let mk = |n: usize| Item {
            uri: format!("spotify:track:{n}"),
            name: format!("T{n}"),
            kind: Kind::Track,
            duration_ms: 1000,
            ..Default::default()
        };
        a.spov.sp_queue = (0..100).map(mk).collect();
        a.spov.sp_idx = 99; // last track
        a.spov.now_spotify = Some(a.spov.sp_queue[99].clone());
        a.spov.sp_repeat = repeat;
        a.spov.sp_started = true;
        a.spotify.queue_sel = 99; // Up Next cursor parked on the last track
        // the last track ends
        let (etx, erx) = crossbeam_channel::unbounded::<SessionEvent>();
        etx.send(SessionEvent::EndOfTrack).unwrap();
        a.spov.session_rx = Some(erx);
        a.pump_spotify_session();
        a
    };
    let all = build(Repeat::All);
    assert_eq!(all.spov.sp_idx, 0, "repeat-all wraps last→first");
    assert!(!all.spov.spotify_paused, "and keeps playing after the wrap");
    assert_eq!(
        all.spotify.queue_sel, 0,
        "the Up Next cursor follows the wrap so the now-playing row stays visible"
    );
    let off = build(Repeat::Off);
    assert_eq!(off.spov.sp_idx, 99, "repeat-off stays on the last track");
    assert!(off.spov.spotify_paused, "and stops at the end of the queue");
}

/// Build a Spotify overlay mid-play on a 3-track queue with a faked (reusable)
/// session, so an `EndOfTrack` runs the advance/re-buffer decision without spawning
/// a real librespot thread. Returns the app and the session-command receiver.
#[cfg(test)]
fn stall_fixture() -> (
    AppState,
    crossbeam_channel::Receiver<crate::spotify::session::SessionCommand>,
) {
    use crate::spotify::api::{Item, Kind};
    let mk = |n: usize| Item {
        uri: format!("spotify:track:{n}"),
        name: format!("T{n}"),
        kind: Kind::Track,
        duration_ms: 200_000,
        ..Default::default()
    };
    let mut a = demo();
    a.layout = Layout::Spotify;
    let (tx, rx) = crossbeam_channel::unbounded::<crate::spotify::session::SessionCommand>();
    a.spov.session_cmd = Some(tx); // ensure_session reuses this (no spawn)
    a.spotify.tokens = Some(crate::spotify::Tokens {
        access_token: "a".into(),
        refresh_token: "r".into(),
        expires_at: crate::datetime::now_unix() + 3600,
        scopes: "streaming".into(),
    });
    a.spov.sp_queue = vec![mk(0), mk(1), mk(2)];
    a.spov.sp_idx = 0;
    a.spov.now_spotify = Some(a.spov.sp_queue[0].clone());
    a.spov.sp_dur = 200.0;
    a.spov.sp_started = true;
    a.spov.spotify_paused = false;
    (a, rx)
}

/// Drive one librespot `EndOfTrack` through the session pump.
#[cfg(test)]
fn send_end_of_track(a: &mut AppState) {
    use crate::spotify::session::SessionEvent;
    let (etx, erx) = crossbeam_channel::unbounded::<SessionEvent>();
    etx.send(SessionEvent::EndOfTrack).unwrap();
    a.spov.session_rx = Some(erx);
    a.pump_spotify_session();
}

/// librespot's `EndOfTrack` is overloaded: it also fires when a track aborts
/// mid-play because librespot couldn't fetch the next packet in time (a network
/// stall). Such a premature end must NOT skip the track — it re-buffers the same
/// track where it stalled, so a transient hiccup doesn't drop the track or (under
/// congestion) race the whole queue.
#[test]
fn spotify_midtrack_stall_rebuffers_in_place() {
    use crate::spotify::session::SessionCommand;
    let (mut a, rx) = stall_fixture();
    a.spov.sp_pos = 50.0; // stalled far from the end

    send_end_of_track(&mut a);

    assert_eq!(
        a.spov.sp_idx, 0,
        "a mid-track stall does not advance the queue"
    );
    assert!(
        a.spov.sp_stall_at.is_some(),
        "an in-place re-buffer is scheduled"
    );
    assert_eq!(a.spov.sp_stall_n, 1, "one re-buffer counted");
    assert!(!a.spov.sp_started, "shown as buffering while it re-fetches");
    assert!(
        rx.try_recv().is_err(),
        "the re-buffer waits out its short delay before loading"
    );

    // once the delay elapses, the tick re-loads the SAME track at the stall position
    a.spov.sp_stall_at = Some(std::time::Instant::now() - std::time::Duration::from_millis(1));
    a.pump_spotify();
    match rx.try_recv().expect("a Load is issued after the delay") {
        SessionCommand::Load { uri, position_ms } => {
            assert_eq!(uri, "spotify:track:0", "re-buffers the SAME track");
            assert_eq!(position_ms, 50_000, "resumes where it stalled");
        }
        other => panic!("expected a Load, got {other:?}"),
    }
}

/// An `EndOfTrack` at (or near) the track's full duration is a genuine end — it
/// advances the queue rather than re-buffering.
#[test]
fn spotify_genuine_end_advances_not_rebuffers() {
    let (mut a, _rx) = stall_fixture();
    a.spov.sp_pos = 199.0; // essentially at the end (within SP_END_SLACK of 200s)

    send_end_of_track(&mut a);

    assert_eq!(a.spov.sp_idx, 1, "a genuine end advances to the next track");
    assert!(
        a.spov.sp_stall_at.is_none(),
        "no re-buffer scheduled at a real end"
    );
}

/// A segment that keeps stalling at the same spot (a corrupt/undecodable chunk) is
/// re-buffered a bounded number of times, then skipped — never rebuffered forever.
#[test]
fn spotify_repeated_stall_at_one_spot_eventually_skips() {
    let (mut a, _rx) = stall_fixture();
    // three stalls clustered at ~0:50 each re-buffer (the budget only depletes when
    // stalls cluster without forward progress).
    for expect_n in 1..=3 {
        a.spov.sp_pos = 50.0; // same spot: no real progress between stalls
        send_end_of_track(&mut a);
        assert_eq!(a.spov.sp_idx, 0, "still re-buffering the same track");
        assert_eq!(a.spov.sp_stall_n, expect_n, "the stall budget depletes");
        // simulate the re-buffer briefly resuming then re-stalling at the same spot
        a.spov.sp_stall_at = None; // the tick consumed the scheduled reload
        a.spov.sp_started = true; // librespot resumed …
    }
    // budget spent → the next stall skips past the bad segment
    a.spov.sp_pos = 50.0;
    send_end_of_track(&mut a);
    assert_eq!(
        a.spov.sp_idx, 1,
        "a segment that keeps stalling is finally skipped"
    );
}

/// A long track riding out several *separate* transient hiccups keeps retrying: real
/// forward progress between stalls refills the budget, so it's never wrongly skipped.
#[test]
fn spotify_separated_stalls_keep_retrying() {
    let (mut a, _rx) = stall_fixture();

    a.spov.sp_pos = 50.0; // first hiccup at 0:50
    send_end_of_track(&mut a);
    assert_eq!(a.spov.sp_stall_n, 1);
    assert_eq!(a.spov.sp_idx, 0);

    // recovered, played well past it, then a distinct hiccup at 2:30
    a.spov.sp_stall_at = None;
    a.spov.sp_started = true;
    a.spov.sp_pos = 150.0;
    send_end_of_track(&mut a);
    assert_eq!(
        a.spov.sp_stall_n, 1,
        "forward progress since the last stall refills the budget"
    );
    assert_eq!(a.spov.sp_idx, 0, "a track making progress is never skipped");
}

/// The status-bar "▶ Next:" hint follows the active source (like the QUEUE pane),
/// mirroring `spotify_advance`'s repeat rules — not the local queue alone.
#[test]
fn status_next_hint_follows_the_active_source() {
    use crate::core::player::Repeat;
    use crate::spotify::api::{Item, Kind};
    let mk = |n: &str| Item {
        uri: format!("spotify:track:{n}"),
        name: n.into(),
        kind: Kind::Track,
        duration_ms: 1000,
        ..Default::default()
    };
    let mut a = demo();
    a.spov.sp_queue = vec![mk("A"), mk("B"), mk("C")];
    a.spov.now_spotify = Some(a.spov.sp_queue[0].clone());
    a.spov.spotify_paused = false; // Spotify is the active source

    // Spotify active → the hint reads the Spotify up-next, not the local queue.
    a.spov.sp_idx = 0;
    a.spov.sp_repeat = Repeat::Off;
    assert_eq!(
        a.status_next_title().as_deref(),
        Some("B"),
        "shows the next Spotify track while Spotify plays"
    );

    // On the last track the hint mirrors auto-advance: Off stops, All wraps, One
    // replays the current track.
    a.spov.sp_idx = 2;
    a.spov.sp_repeat = Repeat::Off;
    assert_eq!(
        a.status_next_title(),
        None,
        "Off: nothing after the last track"
    );
    a.spov.sp_repeat = Repeat::All;
    assert_eq!(
        a.status_next_title().as_deref(),
        Some("A"),
        "All wraps last→first"
    );
    a.spov.sp_repeat = Repeat::One;
    assert_eq!(
        a.status_next_title().as_deref(),
        Some("C"),
        "One replays the current track"
    );

    // Paused Spotify (now_spotify lingers Some) is no longer the active source, so
    // the hint falls back to the local queue — never a stale Spotify title.
    a.player.queue.items.clear();
    a.spov.spotify_paused = true;
    assert_eq!(
        a.status_next_title(),
        None,
        "paused Spotify yields to the (empty) local queue, not a stale up-next"
    );
}

/// Prefetching: `spotify_preload_next` sends a Preload for the upcoming track so the
/// transition is gapless, and stays quiet when there's nothing new to prefetch.
#[test]
fn spotify_preloads_the_next_track() {
    use crate::core::player::Repeat;
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::session::SessionCommand;
    let mk = |n: &str| Item {
        uri: format!("spotify:track:{n}"),
        name: n.into(),
        kind: Kind::Track,
        duration_ms: 1000,
        ..Default::default()
    };
    let (tx, rx) = crossbeam_channel::unbounded::<SessionCommand>();
    let mut a = demo();
    a.spov.session_cmd = Some(tx);
    a.spov.sp_queue = vec![mk("a"), mk("b"), mk("c")];
    a.spov.sp_idx = 0;
    a.spov.sp_repeat = Repeat::Off;

    // mid-queue → prefetch the following track
    a.spotify_preload_next();
    assert!(
        matches!(rx.try_recv(), Ok(SessionCommand::Preload { uri }) if uri == "spotify:track:b"),
        "prefetches the next track"
    );

    // repeat-one: the "next" is the current track (already loaded) → nothing sent
    a.spov.sp_repeat = Repeat::One;
    a.spotify_preload_next();
    assert!(
        rx.try_recv().is_err(),
        "repeat-one has nothing new to preload"
    );

    // last track, no repeat → no next → nothing sent
    a.spov.sp_repeat = Repeat::Off;
    a.spov.sp_idx = 2;
    a.spotify_preload_next();
    assert!(
        rx.try_recv().is_err(),
        "the end of a non-repeating queue preloads nothing"
    );

    // repeat-all wraps last→first → prefetch the first track
    a.spov.sp_repeat = Repeat::All;
    a.spotify_preload_next();
    assert!(
        matches!(rx.try_recv(), Ok(SessionCommand::Preload { uri }) if uri == "spotify:track:a"),
        "repeat-all preloads the wrapped-to first track"
    );

    // a streamed episode owns the engine → librespot preload doesn't apply
    a.spov.sp_idx = 0;
    a.spov.sp_repeat = Repeat::Off;
    a.spov.sp_stream = true;
    a.spotify_preload_next();
    assert!(
        rx.try_recv().is_err(),
        "streamed episodes bypass librespot preload"
    );
    a.spov.sp_stream = false;

    // the next track is itself an episode → skip (episodes stream outside librespot)
    a.spov.sp_queue = vec![
        mk("x"),
        Item {
            uri: "spotify:episode:z".into(),
            name: "ep".into(),
            kind: Kind::Track,
            duration_ms: 1000,
            ..Default::default()
        },
    ];
    a.spov.sp_idx = 0;
    a.spotify_preload_next();
    assert!(
        rx.try_recv().is_err(),
        "an episode next is not preloaded via librespot"
    );
}

/// The Artist pane's "open artist page" action is source-aware: the Spotify view
/// opens the Spotify artist (drills the Spotify nav), other views open the local
/// artist and leave the Spotify nav untouched. (Regression: it used to always call
/// the local path, so a double-click in the Spotify view did nothing.)
#[test]
fn artist_pane_opens_the_view_correct_page() {
    use crate::spotify::Tokens;
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.spotify.tokens = Some(Tokens {
        access_token: "t".into(),
        refresh_token: "r".into(),
        expires_at: 0,
        scopes: String::new(),
    });
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:x".into(),
        name: "Song".into(),
        subtitle: "The Artist".into(),
        artist_uri: Some("spotify:artist:abc".into()),
        kind: Kind::Track,
        ..Default::default()
    });

    // Spotify view → opens the Spotify artist page (a Spotify nav frame is pushed)
    a.layout = Layout::Spotify;
    a.open_artist_page();
    assert_eq!(
        a.spotify.nav_stack_len(),
        1,
        "the Spotify view's Artist pane opens the Spotify artist page"
    );

    // a local view → opens the local artist instead, leaving the Spotify nav alone
    let before = a.spotify.nav_stack_len();
    a.layout = Layout::Dashboard;
    a.open_artist_page();
    assert_eq!(
        a.spotify.nav_stack_len(),
        before,
        "a local view does not drill the Spotify nav"
    );
}

#[test]
fn follow_target_resolves_show_artist_track_and_none() {
    use crate::spotify::api::{Item, Kind};
    let mk = |uri: &str, name: &str, kind: Kind| Item {
        uri: uri.into(),
        name: name.into(),
        kind,
        ..Default::default()
    };
    let mut a = demo();
    a.layout = Layout::Spotify;

    // a Show row → follow the show by its own uri
    a.spotify.items = vec![mk("spotify:show:S", "The Show", Kind::Show)];
    a.spotify.sel = 0;
    assert_eq!(
        a.spotify_follow_target(),
        Some(("spotify:show:S".into(), Kind::Show, "The Show".into()))
    );

    // an Artist row → follow the artist by its own uri
    a.spotify.items = vec![mk("spotify:artist:A", "The Artist", Kind::Artist)];
    a.spotify.sel = 0;
    assert_eq!(
        a.spotify_follow_target(),
        Some(("spotify:artist:A".into(), Kind::Artist, "The Artist".into()))
    );

    // a Track row with an artist uri → follow that artist (subtitle names them)
    let mut track = mk("spotify:track:T", "A Song", Kind::Track);
    track.artist_uri = Some("spotify:artist:B".into());
    track.subtitle = "Band Name, Someone Else".into();
    a.spotify.items = vec![track];
    a.spotify.sel = 0;
    assert_eq!(
        a.spotify_follow_target(),
        Some(("spotify:artist:B".into(), Kind::Artist, "Band Name".into())),
        "a track follows its primary artist"
    );

    // a Track row without an artist uri (e.g. a podcast episode) → nothing to follow
    a.spotify.items = vec![mk("spotify:episode:E", "Ep 1", Kind::Track)];
    a.spotify.sel = 0;
    assert_eq!(a.spotify_follow_target(), None);
}

#[test]
fn transient_key_blip_auto_resumes_but_a_confirmed_block_stops() {
    // Regression for the "intermittent stop → cooldown → nothing happens → restart"
    // bug. A single mid-playback audio-key error (a bad CDN node) used to set a
    // sticky process-global flag that permanently disabled the reconnect recovery AND
    // showed "blocked", so the queue stayed dead until lyrfin was restarted. Once a
    // track has played this session the account is provably NOT key-blocked, so a
    // later denial is transient: it must NOT confirm a block, and it arms an
    // auto-resume so playback recovers on its own. The genuine account-level block
    // (nothing ever plays, several tracks denied) is still detected and does not loop.
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::session::SessionCommand;
    use crate::spotify::{ConnState, Tokens};
    use std::sync::atomic::Ordering;

    // premium + streaming scope: the block verdict only applies to an account that
    // otherwise could stream (a free account fails for a different, handled reason).
    let setup = |a: &mut crate::app::AppState| {
        a.layout = Layout::Spotify;
        a.spotify.conn = ConnState::Connected {
            name: "me".into(),
            premium: true,
        };
        a.spotify.tokens = Some(Tokens {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: 0,
            scopes: "streaming".into(),
        });
        a.spov.now_spotify = Some(Item {
            uri: "spotify:track:1".into(),
            kind: Kind::Track,
            ..Default::default()
        });
        a.spov.sp_started = false;
        a.spov.sp_stream = false;
        a.spov.sp_queue.clear(); // empty → the reconnect path short-circuits, so the
        a.spov.sp_keyretry_n = 5; // decision reduces to the block verdict (no live respawn)
    };
    // the sticky probe flag as a real librespot audio-key error would raise it
    crate::spotify::logprobe::AUDIO_KEY_BLOCKED.store(true, Ordering::Relaxed);

    // --- transient: a track already played, so a later denial is a blip to recover.
    let mut a = demo();
    setup(&mut a);
    a.spov.sp_played_ok = true;
    a.spov.sp_key_denials = 9; // even many prior denials can't confirm once one played
    let (tx, _rx) = crossbeam_channel::unbounded::<SessionCommand>();
    a.spov.session_cmd = Some(tx);
    assert!(
        !a.spotify_key_block_confirmed(),
        "a session that has played a track is never a confirmed block"
    );
    a.spotify_playback_blocked();
    assert!(
        a.spov.sp_resume_at.is_some(),
        "a transient failure arms an auto-resume so it recovers without a restart"
    );

    // --- genuine block: nothing ever played and several tracks are denied.
    let mut b = demo();
    setup(&mut b);
    b.spov.sp_played_ok = false;
    b.spov.sp_key_denials = 5; // well past the confirm threshold
    let (tx, _rx) = crossbeam_channel::unbounded::<SessionCommand>();
    b.spov.session_cmd = Some(tx);
    assert!(
        b.spotify_key_block_confirmed(),
        "premium+streaming, key refused, nothing ever played → confirmed account block"
    );
    b.spotify_playback_blocked();
    assert!(
        b.spov.sp_resume_at.is_none(),
        "a confirmed account-level block must NOT loop on auto-resume"
    );
    assert!(b.spov.sp_cooldown_until > 0, "it still backs off");

    // leave the process-global flag clean for other tests
    crate::spotify::logprobe::AUDIO_KEY_BLOCKED.store(false, Ordering::Relaxed);
}
