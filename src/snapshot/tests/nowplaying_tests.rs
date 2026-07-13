//! OS "Now Playing" snapshot + remote-command routing tests (`app/nowplaying.rs`).
//! Pure `AppState` logic over the NullEngine — no terminal, network, or real OS.

use super::*;
use crate::core::player::Status;
use crate::media::MediaCommand;
use crate::radio::Station;
use crate::spotify::api::{Item, Kind};
use std::time::Duration;

/// A minimal streaming Spotify track for the overlay.
fn sp_track(name: &str) -> Item {
    Item {
        uri: format!("spotify:track:{name}"),
        name: name.into(),
        subtitle: "The Band".into(),
        album: "The Album".into(),
        image: Some("https://img/cover.jpg".into()),
        kind: Kind::Track,
        duration_ms: 200_000,
        ..Default::default()
    }
}

#[test]
fn snapshot_reflects_the_playing_local_track() {
    let mut app = demo();
    app.player.status = Status::Playing;
    let title = app.current_track().unwrap().title.clone();
    let snap = app.now_playing_snapshot().expect("a local snapshot");
    assert_eq!(snap.title, title);
    assert!(snap.playing, "status Playing → snapshot playing");
}

#[test]
fn snapshot_is_none_when_nothing_is_loaded() {
    let mut app = demo();
    app.player.current = None;
    app.player.status = Status::Stopped;
    assert!(
        app.now_playing_snapshot().is_none(),
        "no local track and no overlay → the OS info is cleared"
    );
}

#[test]
fn playing_spotify_wins_over_a_paused_local_track() {
    let mut app = demo();
    // local is "playing" too, but the actively-playing Spotify overlay owns the OS
    app.player.status = Status::Playing;
    app.spov.now_spotify = Some(sp_track("Ecstasy"));
    app.spov.spotify_paused = false;
    app.spov.sp_pos = 12.0;
    app.spov.sp_dur = 200.0;
    let snap = app.now_playing_snapshot().expect("a spotify snapshot");
    assert_eq!(snap.title, "Ecstasy");
    assert_eq!(snap.artist, "The Band");
    assert_eq!(snap.cover.as_deref(), Some("https://img/cover.jpg"));
    assert_eq!(snap.elapsed, Duration::from_secs(12));
}

#[test]
fn os_toggle_pauses_the_playing_local_track() {
    let mut app = demo();
    app.player.status = Status::Playing;
    app.on_media_command(MediaCommand::Toggle);
    assert_eq!(app.player.status, Status::Paused);
}

#[test]
fn os_play_and_pause_are_directional() {
    let mut app = demo();
    app.player.status = Status::Playing;
    // Play while already playing is a no-op (not a toggle to pause)
    app.on_media_command(MediaCommand::Play);
    assert_eq!(app.player.status, Status::Playing);
    // Pause halts it
    app.on_media_command(MediaCommand::Pause);
    assert_eq!(app.player.status, Status::Paused);
    // Pause again is a no-op
    app.on_media_command(MediaCommand::Pause);
    assert_eq!(app.player.status, Status::Paused);
}

#[test]
fn os_next_advances_the_local_queue() {
    let mut app = demo();
    app.player.status = Status::Playing;
    app.player.queue.position = 0;
    app.player.current = app.player.queue.items.first().copied();
    app.on_media_command(MediaCommand::Next);
    assert_eq!(app.player.queue.position, 1, "Next stepped the local queue");
}

#[test]
fn os_seek_to_maps_to_the_local_position() {
    let mut app = demo();
    app.player.status = Status::Playing;
    app.player.duration = Duration::from_secs(100);
    app.on_media_command(MediaCommand::SeekTo(50.0));
    assert_eq!(
        app.player.elapsed.as_secs(),
        50,
        "an absolute OS seek maps onto the track"
    );
}

#[test]
fn os_toggle_pauses_the_playing_spotify_overlay() {
    let mut app = demo();
    app.spov.now_spotify = Some(sp_track("Perfect"));
    app.spov.spotify_paused = false;
    app.spov.sp_started = true; // streaming, so toggle is an in-place pause
    app.on_media_command(MediaCommand::Toggle);
    assert!(
        app.spov.spotify_paused,
        "Toggle routed to the active Spotify source and paused it"
    );
}

#[test]
fn os_toggle_pauses_the_playing_radio_station() {
    let mut app = demo();
    app.rnow.now_station = Some(Station {
        name: "SomaFM".into(),
        ..Default::default()
    });
    app.rnow.radio_paused = false;
    app.on_media_command(MediaCommand::Toggle);
    assert!(
        app.rnow.radio_paused,
        "Toggle routed to the active radio source and paused it"
    );
}

#[test]
fn radio_snapshot_prefers_the_icy_title() {
    let mut app = demo();
    app.rnow.now_station = Some(Station {
        name: "SomaFM".into(),
        ..Default::default()
    });
    app.rnow.radio_paused = false;
    app.rnow.now_station_title = Some("Artist — Song".into());
    let snap = app.now_playing_snapshot().expect("a radio snapshot");
    assert_eq!(snap.title, "Artist — Song");
    assert_eq!(snap.artist, "SomaFM");
    assert!(snap.duration.is_zero(), "a live stream has no fixed length");
}
