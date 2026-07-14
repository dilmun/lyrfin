//! Spotify auth snapshot/behaviour tests, split out of spotify_tests.rs.
//! A child of `tests`, so `use super::*` pulls in the shared demo()/render_layout.

use super::*;

#[test]
fn spotify_audio_key_denial_stops_and_reports() {
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::session::SessionEvent;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:x".into(),
        name: "Song".into(),
        kind: Kind::Track,
        ..Default::default()
    });
    a.spov.sp_started = false; // still buffering
    // librespot's audio-key denial arrives as a session event (raised by the log
    // probe). It must stop the buffering and tell the user, not spin forever — but
    // it must NOT blank the overlay: the track + artist info stay on screen.
    let (tx, rx) = crossbeam_channel::unbounded();
    tx.send(SessionEvent::AudioKeyDenied).unwrap();
    a.spov.session_rx = Some(rx);
    a.pump_spotify_session();
    assert!(
        a.spov.now_spotify.is_some(),
        "an audio-key denial keeps the now-playing overlay on screen (not blanked)"
    );
    assert!(
        a.spov.spotify_paused && !a.spov.sp_started,
        "the failed track is paused/stopped (no longer buffering), not playing"
    );
    let msg = a
        .notification
        .as_ref()
        .map(|n| n.text.as_str())
        .unwrap_or("");
    assert!(
        msg.to_lowercase().contains("blocked") || msg.to_lowercase().contains("decryption"),
        "the user is told why playback stopped: {msg:?}"
    );
}

#[test]
fn spotify_backs_off_after_a_denial() {
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::session::SessionEvent;
    use crate::spotify::{ConnState, Tokens};
    let mut a = demo();
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
        uri: "spotify:track:x".into(),
        name: "Song".into(),
        kind: Kind::Track,
        ..Default::default()
    });
    // a denial arrives → buffering stops AND a back-off is armed (the failed track
    // stays on screen, paused — it is no longer blanked)
    let (tx, rx) = crossbeam_channel::unbounded();
    tx.send(SessionEvent::AudioKeyDenied).unwrap();
    a.spov.session_rx = Some(rx);
    a.pump_spotify_session();
    assert!(
        a.spotify_cooldown_remaining() > 0,
        "a denial arms a playback back-off"
    );
    // a new play attempt during the back-off is refused — no re-hammering. The
    // refused track never loads (the failed one stays, so check by uri, not is_none)
    a.spotify_play(
        vec![Item {
            uri: "spotify:track:y".into(),
            name: "Other".into(),
            kind: Kind::Track,
            ..Default::default()
        }],
        0,
    );
    assert_ne!(
        a.spov.now_spotify.as_ref().map(|t| t.uri.as_str()),
        Some("spotify:track:y"),
        "playback is refused while cooling down (the new track never loads)"
    );
}

#[test]
fn spotify_not_registered_error_is_actionable() {
    use crate::app::MouseTarget;
    use crate::spotify::ConnState;
    let mut a = demo();
    a.config.mouse = true; // so the clickable link registers a hit target
    a.spotify.conn = ConnState::Error {
        msg: crate::spotify::api::NOT_REGISTERED_MSG.into(),
    };
    let s = render_layout(&mut a, Layout::Spotify, 140, 30);
    assert!(
        s.contains("developer.spotify.com"),
        "the panel shows the dashboard link"
    );
    assert!(
        s.to_lowercase().contains("client id"),
        "it points at the client-id fix"
    );
    // the dashboard link is a clickable mouse target
    assert!(
        a.hit
            .borrow()
            .iter()
            .any(|(_, t)| matches!(t, MouseTarget::OpenSpotifyDashboard)),
        "the dashboard URL registers a click target"
    );
}

#[test]
fn spotify_browse_not_registered_shows_centered_card() {
    use crate::app::MouseTarget;
    use crate::spotify::ConnState;
    let mut a = demo();
    a.config.mouse = true;
    a.layout = Layout::Spotify;
    // connected, but a browse/search came back 403 "not registered"
    a.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    a.spotify.items.clear();
    a.spotify.note = crate::spotify::api::NOT_REGISTERED_MSG.into();
    let s = render_layout(&mut a, Layout::Spotify, 140, 30);
    assert!(
        s.contains("developer.spotify.com"),
        "the browse note renders the dashboard-link card, not a raw line"
    );
    assert!(
        s.to_lowercase().contains("client id"),
        "shows the client-id fix"
    );
    assert!(
        a.hit
            .borrow()
            .iter()
            .any(|(_, t)| matches!(t, MouseTarget::OpenSpotifyDashboard)),
        "the dashboard URL is clickable in the browse view too"
    );
}

#[test]
fn spotify_logout_clears_all_account_state() {
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::{ConnState, Tokens};
    let mut a = demo();
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
    let mk = |u: &str| Item {
        uri: u.into(),
        name: u.into(),
        kind: Kind::Track,
        ..Default::default()
    };
    a.spov.now_spotify = Some(mk("spotify:track:a"));
    a.spov.sp_queue = vec![mk("spotify:track:a"), mk("spotify:track:b")];
    a.spov.sp_idx = 1;
    a.spotify.items = vec![mk("spotify:track:a")];
    a.spotify.query = "rihanna".into();
    a.spotify.in_search = true;
    a.spov.sp_cooldown_until = crate::datetime::now_unix() + 999;
    a.spotify_logout();
    // a logout is a clean slate — nothing from this account survives
    assert!(matches!(a.spotify.conn, ConnState::Disconnected));
    assert!(a.spotify.tokens.is_none(), "token forgotten");
    assert!(a.spov.now_spotify.is_none(), "now-playing cleared");
    assert!(a.spov.sp_queue.is_empty(), "queue cleared");
    assert!(a.spotify.items.is_empty(), "browse list cleared");
    assert!(
        a.spotify.query.is_empty() && !a.spotify.in_search,
        "search cleared"
    );
    assert_eq!(a.spotify_cooldown_remaining(), 0, "back-off cleared");
}

#[test]
fn spotify_drops_restored_state_from_a_different_account() {
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::{AuthEvent, Tokens};
    let mut a = demo();
    a.layout = Layout::Spotify;
    let mk = |u: &str| Item {
        uri: u.into(),
        name: u.into(),
        kind: Kind::Track,
        ..Default::default()
    };
    // launch restored state belonging to account "alice"
    a.spotify.restored_account = Some("alice".into());
    a.spov.now_spotify = Some(mk("spotify:track:x"));
    a.spov.sp_queue = vec![mk("spotify:track:y")];
    // now connect as a DIFFERENT account ("bob")
    let (tx, rx) = crossbeam_channel::unbounded();
    tx.send(AuthEvent::Connected {
        tokens: Tokens {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: u64::MAX,
            scopes: String::new(),
        },
        account_id: "bob".into(),
        name: "Bob".into(),
        premium: true,
    })
    .unwrap();
    a.spotify.auth_rx = Some(rx);
    a.pump_spotify();
    assert!(
        a.spov.now_spotify.is_none(),
        "a different account's restored now-playing is dropped"
    );
    assert!(
        a.spov.sp_queue.is_empty(),
        "a different account's restored queue is dropped"
    );
    assert_eq!(a.spotify.account_id.as_deref(), Some("bob"));
}

#[test]
fn spotify_settings_group_has_auth_rows() {
    use crate::action::Action;
    use crate::app::{NameTarget, Setting};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.update(Action::OpenViewSettings); // open the `;` popup
    // switch to the Spotify tab (auth rows live there)
    let sp = a
        .popup_tab_names()
        .iter()
        .position(|t| *t == "Spotify")
        .unwrap();
    a.set_overlay_tab(sp);
    let items = a.settings_group_items();
    assert!(
        items.contains(&Setting::SpotifyClientId),
        "set/clear client id row present"
    );
    assert!(
        items.contains(&Setting::SpotifyReauth),
        "re-authenticate row present"
    );
    // both sit under the SPOTIFY header
    assert_eq!(Setting::SpotifyClientId.group(), "Spotify");
    assert_eq!(Setting::SpotifyReauth.group(), "Spotify");
    // activating the client-id row opens the paste prompt
    a.config.spotify_client_id = "abc123".into();
    let idx = items
        .iter()
        .position(|s| *s == Setting::SpotifyClientId)
        .unwrap();
    a.settings.sel = idx;
    a.settings_activate();
    assert!(
        matches!(a.input.naming, Some(NameTarget::SpotifyClientId)),
        "client-id row opens the paste prompt"
    );
    assert_eq!(a.input.buffer, "abc123", "prefilled with the current id");
}

#[test]
fn spotify_bitrate_row_cycles_and_persists() {
    use crate::action::Action;
    use crate::app::Setting;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.update(Action::OpenViewSettings); // open the `;` popup
    assert!(
        a.popup_all_settings().contains(&Setting::SpotifyBitrate),
        "streaming-quality row present in the Spotify group"
    );
    assert_eq!(Setting::SpotifyBitrate.group(), "Spotify");

    // default is 160; cycling up walks 160 → 320 → 96 → 160 and persists
    assert_eq!(a.config.spotify_bitrate, 160);
    a.cycle_spotify_bitrate(1);
    assert_eq!(a.config.spotify_bitrate, 320);
    a.cycle_spotify_bitrate(1);
    assert_eq!(a.config.spotify_bitrate, 96);
    a.cycle_spotify_bitrate(1);
    assert_eq!(a.config.spotify_bitrate, 160);
    // and back down
    a.cycle_spotify_bitrate(-1);
    assert_eq!(a.config.spotify_bitrate, 96);

    // the `set` command accepts the three valid steps and rejects others
    assert!(a.cmd_set("spotify_quality 320").contains("320"));
    assert_eq!(a.config.spotify_bitrate, 320);
    let bad = a.cmd_set("spotify_quality 256");
    assert!(
        bad.contains("96 kbps") && bad.contains("320 kbps"),
        "invalid bitrate rejected with the valid options: {bad}"
    );
    assert_eq!(
        a.config.spotify_bitrate, 320,
        "rejected value left unchanged"
    );
}

#[test]
fn spotify_free_account_is_refused_up_front_with_a_logged_error() {
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::{ConnState, Tokens};
    let mut a = demo();
    a.layout = Layout::Spotify;
    // connected, but NOT premium — librespot can't stream, so every track would
    // fail. The fix refuses up front instead of racing the queue into a cooldown.
    a.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: false,
    };
    a.spotify.tokens = Some(Tokens {
        access_token: "a".into(),
        refresh_token: "r".into(),
        expires_at: 0,
        scopes: "streaming".into(),
    });
    a.spotify_play(
        vec![Item {
            uri: "spotify:track:x".into(),
            name: "Song".into(),
            kind: Kind::Track,
            ..Default::default()
        }],
        0,
    );
    assert!(
        a.spov.now_spotify.is_none(),
        "a free account never starts playback"
    );
    assert_eq!(
        a.spotify_cooldown_remaining(),
        0,
        "the premium refusal does not arm a cooldown — it's not a transient failure"
    );
    assert!(
        a.error_log
            .iter()
            .any(|e| e.msg.to_lowercase().contains("premium")),
        "the reason is recorded in the error log (the error pane is no longer empty)"
    );
}

#[test]
fn spotify_connect_error_lands_in_the_error_log() {
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::session::SessionEvent;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:x".into(),
        name: "Song".into(),
        kind: Kind::Track,
        ..Default::default()
    });
    // a session connect error (token refresh / login / timeout / rate-limit) is the
    // actual reason playback failed — it must be logged, not just flashed as a toast,
    // since that's exactly what the user inspects when nothing plays.
    let (tx, rx) = crossbeam_channel::unbounded();
    tx.send(SessionEvent::ConnectError("login timed out".into()))
        .unwrap();
    a.spov.session_rx = Some(rx);
    a.pump_spotify_session();
    assert!(
        a.error_log
            .iter()
            .any(|e| e.msg.contains("login timed out")),
        "the connect error is recorded in the error log"
    );
    assert!(
        a.spotify_cooldown_remaining() > 0,
        "a connect error still arms a (now shorter) back-off"
    );
    assert!(
        a.spov.now_spotify.is_some() && a.spov.spotify_paused,
        "the now-playing overlay is retained (paused), not blanked, on a connect error"
    );
}

#[test]
fn spotify_expired_token_forces_a_session_respawn() {
    use crate::spotify::Tokens;
    let mut a = demo();
    // a live librespot session handle exists from earlier...
    let (tx, _rx) = crossbeam_channel::unbounded::<crate::spotify::session::SessionCommand>();
    a.spov.session_cmd = Some(tx);
    // ...but the token it was built on has expired (app idle for hours/days). The
    // session is now stale — its connection can't resolve audio — so it must NOT
    // be reused; the next play respawns with a refreshed token.
    a.spotify.tokens = Some(Tokens {
        access_token: "a".into(),
        refresh_token: "r".into(),
        expires_at: crate::datetime::now_unix(), // inside the 30s expiry leeway
        scopes: "streaming".into(),
    });
    assert!(
        !a.spotify_session_reusable(),
        "a session built on an expired token is not reused (it would silently fail)"
    );
    // a still-valid token → reuse the live session as-is (no needless respawn)
    a.spotify.tokens = Some(Tokens {
        access_token: "a".into(),
        refresh_token: "r".into(),
        expires_at: crate::datetime::now_unix() + 3600,
        scopes: "streaming".into(),
    });
    assert!(
        a.spotify_session_reusable(),
        "a live session with a valid token is reused"
    );
}

#[test]
fn spotify_reauth_keeps_the_overlay_and_snapshots_the_view() {
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.account_id = Some("acct-1".into());
    // a drilled-in artist page (Odya) with the cursor parked, a now-playing track,
    // and a loaded artist pane
    a.spotify.open_item = Some(Item {
        uri: "spotify:artist:odya".into(),
        name: "Odya".into(),
        kind: Kind::Artist,
        ..Default::default()
    });
    a.spotify.sel = 3;
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:x".into(),
        name: "Song".into(),
        kind: Kind::Track,
        ..Default::default()
    });
    a.spov.sp_artist_uri = Some("spotify:artist:odya".into());
    a.spov.sp_artist_top = vec![Item {
        uri: "spotify:track:top".into(),
        name: "Top".into(),
        kind: Kind::Track,
        ..Default::default()
    }];
    let (tx, _rx) = crossbeam_channel::unbounded::<crate::spotify::session::SessionCommand>();
    a.spov.session_cmd = Some(tx);

    a.spotify_prepare_reauth();

    // the now-playing overlay + artist pane stay on screen (paused), not blanked
    assert!(
        a.spov.now_spotify.is_some(),
        "re-auth keeps the now-playing track"
    );
    assert!(
        a.spov.sp_artist_uri.is_some() && !a.spov.sp_artist_top.is_empty(),
        "re-auth keeps the artist pane (uri + top tracks)"
    );
    assert!(a.spov.spotify_paused, "playback is paused for the re-auth");
    // the drill-in + cursor are snapshotted so reconnect restores the artist page
    assert_eq!(
        a.spotify.restore_open.as_ref().map(|i| i.uri.as_str()),
        Some("spotify:artist:odya"),
        "the drilled-in container is saved for restore on reconnect"
    );
    assert_eq!(a.spotify.restore_sel, Some(3), "the cursor is saved");
    // the current account is marked so logging in as a DIFFERENT one clears state
    assert_eq!(
        a.spotify.restored_account.as_deref(),
        Some("acct-1"),
        "the current account is marked for the switch-account guard"
    );
    // the old session is dropped so the fresh token spawns a new one
    assert!(
        a.spov.session_cmd.is_none(),
        "the stale session is dropped for the fresh token"
    );
}

#[test]
fn spotify_ensure_session_defers_while_auth_is_in_flight() {
    use crate::spotify::Tokens;
    let mut a = demo();
    a.spotify.tokens = Some(Tokens {
        access_token: "a".into(),
        refresh_token: "r".into(),
        expires_at: crate::datetime::now_unix() + 3600,
        scopes: "streaming".into(),
    });
    // a login/resume is mid-flight — spawning a session now would fire a SECOND,
    // racing refresh of the same single-use token (corruption risk)
    let (_tx, rx) = crossbeam_channel::unbounded::<crate::spotify::AuthEvent>();
    a.spotify.auth_rx = Some(rx);
    assert!(
        !a.spotify_ensure_session(),
        "no session is spawned while a refresh/login is in flight"
    );
    assert!(
        a.spov.session_cmd.is_none(),
        "and no session handle is created (the respawn comes after Connected)"
    );
}

#[test]
fn spotify_reconnect_drops_a_stale_pre_refresh_session() {
    use crate::spotify::{AuthEvent, Tokens};
    let mut a = demo();
    a.layout = Layout::Spotify;
    // a librespot session left over from before the app went idle — its connection
    // is dead, but token-freshness alone would wrongly mark it reusable
    let (scmd, _srx) = crossbeam_channel::unbounded::<crate::spotify::session::SessionCommand>();
    let (_stx, ssrx) = crossbeam_channel::unbounded::<crate::spotify::session::SessionEvent>();
    a.spov.session_cmd = Some(scmd);
    a.spov.session_rx = Some(ssrx);
    a.spov.sp_started = false; // not actively streaming (idle)
    // a silent resume (Web-API 401 path) completes with a fresh token
    let (tx, rx) = crossbeam_channel::unbounded();
    tx.send(AuthEvent::Connected {
        tokens: Tokens {
            access_token: "fresh".into(),
            refresh_token: "r".into(),
            expires_at: crate::datetime::now_unix() + 3600,
            scopes: String::new(),
        },
        account_id: "acct".into(),
        name: "Me".into(),
        premium: true,
    })
    .unwrap();
    a.spotify.auth_rx = Some(rx);
    a.pump_spotify();
    assert!(
        a.spov.session_cmd.is_none(),
        "an out-of-band token refresh drops the stale session so the next play respawns a live one"
    );
}

#[test]
fn spotify_reconnect_keeps_an_actively_streaming_session() {
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::{AuthEvent, Tokens};
    let mut a = demo();
    a.layout = Layout::Spotify;
    let (scmd, _srx) = crossbeam_channel::unbounded::<crate::spotify::session::SessionCommand>();
    a.spov.session_cmd = Some(scmd);
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:x".into(),
        name: "Song".into(),
        kind: Kind::Track,
        ..Default::default()
    });
    a.spov.sp_started = true; // actively streaming
    a.spov.spotify_paused = false;
    // a Web-API 401 refreshed the token mid-playback
    let (tx, rx) = crossbeam_channel::unbounded();
    tx.send(AuthEvent::Connected {
        tokens: Tokens {
            access_token: "fresh".into(),
            refresh_token: "r".into(),
            expires_at: crate::datetime::now_unix() + 3600,
            scopes: String::new(),
        },
        account_id: "acct".into(),
        name: "Me".into(),
        premium: true,
    })
    .unwrap();
    a.spotify.auth_rx = Some(rx);
    a.pump_spotify();
    assert!(
        a.spov.session_cmd.is_some(),
        "an actively-streaming session is NOT dropped mid-playback by a routine refresh"
    );
}

/// Build an app mid-playback whose librespot session has silently died (Spotify
/// closed the AP connection). `auth_rx` is set so the reconnect's `ensure_session`
/// *defers* instead of opening a real socket — isolating the recovery *decision*
/// (the unit under test) from the network respawn.
fn playing_with_dead_session() -> AppState {
    use crate::spotify::Tokens;
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.tokens = Some(Tokens {
        access_token: "a".into(),
        refresh_token: "r".into(),
        expires_at: crate::datetime::now_unix() + 3600,
        scopes: "streaming".into(),
    });
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:x".into(),
        name: "Song".into(),
        kind: Kind::Track,
        ..Default::default()
    });
    a.spov.sp_queue = vec![Item {
        uri: "spotify:track:x".into(),
        name: "Song".into(),
        kind: Kind::Track,
        ..Default::default()
    }];
    a.spov.sp_idx = 0;
    a.spov.spotify_paused = false;
    a.spov.sp_recovery = crate::app::spotify::SpRecovery::Normal;
    // a still-connected command channel (lyrfin can't see librespot's dead socket)
    let (scmd, _srx) = crossbeam_channel::unbounded::<crate::spotify::session::SessionCommand>();
    a.spov.session_cmd = Some(scmd);
    a
}

#[test]
fn spotify_dropped_connection_reconnects_instead_of_backing_off() {
    use crate::app::spotify::SpRecovery;
    use crate::spotify::session::SessionEvent;
    let mut a = playing_with_dead_session();
    // block the real respawn so no socket is opened; the recovery decision still runs
    let (_atx, arx) = crossbeam_channel::unbounded::<crate::spotify::AuthEvent>();
    a.spotify.auth_rx = Some(arx);
    // the next track's load fails (dead-session audio-key timeout → Unavailable)
    let (tx, rx) = crossbeam_channel::unbounded();
    tx.send(SessionEvent::Unavailable).unwrap();
    a.spov.session_rx = Some(rx);
    a.pump_spotify_session();
    // it reconnects and replays the track — NOT a back-off + re-authenticate dead end
    assert_eq!(
        a.spov.sp_recovery,
        SpRecovery::Reconnecting,
        "a dropped connection kicks off a reconnect-and-retry"
    );
    assert!(
        a.spov.session_cmd.is_none(),
        "the dead session handle is dropped so the next play respawns a live one"
    );
    assert_eq!(
        a.spov.sp_cooldown_until, 0,
        "no punitive back-off is armed — this is a recoverable drop, not repeated failure"
    );
    assert!(
        !a.spov.spotify_paused,
        "the track isn't given up on (no terminal failure); it's being retried"
    );
}

#[test]
fn spotify_reconnect_ignores_echo_failures_until_the_fresh_session_is_up() {
    use crate::app::spotify::SpRecovery;
    use crate::spotify::session::SessionEvent;
    let mut a = playing_with_dead_session();
    a.spov.sp_recovery = SpRecovery::Reconnecting; // a reconnect is already in flight

    // The dead session emits a burst of echoes for the same load (one timed-out key
    // surfaces as both AudioKeyDenied AND Unavailable). They must be ignored — not
    // trip the cooldown, not skip the track — while the fresh session connects.
    let (tx, rx) = crossbeam_channel::unbounded();
    tx.send(SessionEvent::AudioKeyDenied).unwrap();
    tx.send(SessionEvent::Unavailable).unwrap();
    a.spov.session_rx = Some(rx);
    a.pump_spotify_session();
    assert_eq!(
        a.spov.sp_recovery,
        SpRecovery::Reconnecting,
        "echo failures during reconnect are ignored (stay Reconnecting)"
    );
    assert_eq!(a.spov.sp_idx, 0, "the track is not skipped by an echo");
    assert_eq!(
        a.spov.sp_cooldown_until, 0,
        "an echo does not arm a back-off"
    );

    // the fresh session comes up → a failure from here is real, not an echo
    let (tx, rx) = crossbeam_channel::unbounded();
    tx.send(SessionEvent::Connected).unwrap();
    a.spov.session_rx = Some(rx);
    a.pump_spotify_session();
    assert_eq!(
        a.spov.sp_recovery,
        SpRecovery::Reconnected,
        "once the fresh session connects, later failures are treated as genuine"
    );
}

#[test]
fn spotify_transient_connection_loss_keeps_token_and_schedules_a_retry() {
    use crate::spotify::{AuthEvent, ConnState, Tokens};
    let mut a = demo();
    a.spotify.tokens = Some(Tokens {
        access_token: "a".into(),
        refresh_token: "r".into(),
        expires_at: crate::datetime::now_unix() + 3600,
        scopes: "streaming".into(),
    });
    // a resume failed because Spotify was unreachable (network blip on wake) — NOT
    // an expired session
    let (tx, rx) = crossbeam_channel::unbounded();
    tx.send(AuthEvent::ConnLost {
        msg: "can't reach Spotify (Connection Failed) — check your connection/VPN".into(),
    })
    .unwrap();
    a.spotify.auth_rx = Some(rx);
    a.pump_spotify();
    assert!(
        a.spotify.tokens.is_some(),
        "the cached token is kept — a network blip is not an expiry"
    );
    assert!(
        matches!(a.spotify.conn, ConnState::Reconnecting { .. }),
        "shown as a soft reconnecting state, not a hard 'log in again' error"
    );
    assert!(
        a.spotify.reconnect_at.is_some() && a.spotify.reconnect_attempts == 1,
        "an auto-retry is scheduled with the first back-off step"
    );
    // the scheduled retry is in the FUTURE (back-off), so it doesn't fire this tick
    assert!(
        !a.spotify_reconnect_due(),
        "the first retry waits for its back-off before firing"
    );
}

#[test]
fn spotify_terminal_auth_error_is_not_auto_retried() {
    use crate::spotify::{AuthEvent, ConnState, Tokens};
    let mut a = demo();
    a.spotify.tokens = Some(Tokens {
        access_token: "a".into(),
        refresh_token: "r".into(),
        expires_at: crate::datetime::now_unix() + 3600,
        scopes: "streaming".into(),
    });
    // a real rejection (already wrapped as "Session expired … log in again")
    let (tx, rx) = crossbeam_channel::unbounded();
    tx.send(AuthEvent::Error {
        msg: "Session expired (Spotify rejected the login). Press ⏎ to log in again.".into(),
    })
    .unwrap();
    a.spotify.auth_rx = Some(rx);
    a.pump_spotify();
    assert!(
        matches!(a.spotify.conn, ConnState::Error { .. }),
        "a genuine auth failure stays a hard error"
    );
    assert!(
        a.spotify.reconnect_at.is_none() && !a.spotify_reconnect_due(),
        "and is NOT put on the auto-retry schedule"
    );
}

#[test]
fn spotify_reconnect_fires_only_when_due_idle_and_holding_a_token() {
    use crate::spotify::Tokens;
    let mut a = demo();
    let past = crate::datetime::now_unix().saturating_sub(1);
    // armed + due + token + nothing in flight → fire
    a.spotify.tokens = Some(Tokens {
        access_token: "a".into(),
        refresh_token: "r".into(),
        expires_at: crate::datetime::now_unix() + 3600,
        scopes: "streaming".into(),
    });
    a.spotify.reconnect_at = Some(past);
    assert!(a.spotify_reconnect_due(), "a due, armed reconnect fires");
    // a resume already in flight → don't double-spawn
    let (_tx, rx) = crossbeam_channel::unbounded::<crate::spotify::AuthEvent>();
    a.spotify.auth_rx = Some(rx);
    assert!(
        !a.spotify_reconnect_due(),
        "not while a resume is in flight"
    );
    a.spotify.auth_rx = None;
    // deadline still in the future → wait
    a.spotify.reconnect_at = Some(crate::datetime::now_unix() + 30);
    assert!(
        !a.spotify_reconnect_due(),
        "not before the back-off elapses"
    );
    // no token to resume with → nothing to do
    a.spotify.reconnect_at = Some(past);
    a.spotify.tokens = None;
    assert!(!a.spotify_reconnect_due(), "not without a cached token");
}

#[test]
fn spotify_client_id_persists_in_its_own_file() {
    use crate::spotify::auth::{load_persisted_client_id, persist_client_id};
    let dir = std::env::temp_dir().join("lyrfin_client_id_persist_test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    assert_eq!(load_persisted_client_id(&dir), None, "absent → None");

    persist_client_id(&dir, "my-client-id");
    assert_eq!(
        load_persisted_client_id(&dir).as_deref(),
        Some("my-client-id"),
        "the client id round-trips from its own file"
    );
    // it lives in its own file — independent of config.toml, so a config rewrite or
    // a defaults fall-back can never wipe it (the recurring "Client ID wiped" bug)
    assert!(dir.join("spotify_client_id").exists());
    assert!(
        !dir.join("config.toml").exists(),
        "persistence does not depend on config.toml"
    );

    // clearing removes the file → revert to the shared keymaster id
    persist_client_id(&dir, "");
    assert_eq!(load_persisted_client_id(&dir), None, "empty clears it");

    let _ = std::fs::remove_dir_all(&dir);
}
