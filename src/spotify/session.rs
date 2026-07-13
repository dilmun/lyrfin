//! librespot playback session on a dedicated tokio thread. Authenticates with
//! the OAuth access token, hosts a `Player` whose decoded audio is captured by a
//! custom [`Sink`] into a [`Bridge`], and exposes that bridge as an
//! [`ExternalAudioSource`] so lyrfin's engine plays it (with visualizer + volume).
//! Control is direct (load/play/pause/seek); the app manages the queue.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};
use librespot::core::authentication::Credentials;
use librespot::core::config::SessionConfig;
use librespot::core::session::Session;
use librespot::core::spotify_uri::SpotifyUri;
use librespot::metadata::image::Images;
use librespot::metadata::{Album, Artist, Episode, Metadata, Playlist, Track};
use librespot::playback::audio_backend::{Sink, SinkResult};
use librespot::playback::config::{Bitrate, PlayerConfig};
use librespot::playback::convert::Converter;
use librespot::playback::decoder::AudioPacket;
use librespot::playback::mixer::NoOpVolume;
use librespot::playback::player::{Player, PlayerEvent};

use crate::audio::ExternalAudioSource;
use crate::spotify::auth::{self, Tokens};

/// librespot decodes Vorbis/AAC at 44.1 kHz, interleaved stereo. lyrfin's engine
/// resamples this to the device rate.
const SRC_RATE: u32 = 44_100;
/// Cap the bridge buffer (~1s stereo) so the sink applies backpressure rather
/// than ballooning memory if the consumer stalls.
const BRIDGE_CAP: usize = (SRC_RATE as usize) * 2;

/// Commands the app sends to the session thread.
#[derive(Debug, Clone)]
pub enum SessionCommand {
    Load {
        uri: String,
        position_ms: u32,
    },
    Play,
    Pause,
    Seek(u32),
    /// Prefetch (fetch + decode-ahead) the next track so the upcoming transition is
    /// gapless: librespot reuses the preloaded track on the following `Load` instead
    /// of re-fetching it from the CDN. Re-preloading the same track is a no-op inside
    /// librespot, so the app can send this freely. See [`Player::preload`].
    Preload {
        uri: String,
    },
    /// Fetch a container's tracks over librespot's metadata protocol (the Web API
    /// blocks playlist tracks + artist top-tracks for dev-mode apps). `artist`
    /// picks top-tracks vs. a playlist's contents.
    FetchTracks {
        uri: String,
        artist: bool,
        key: String,
    },
    /// Build a grouped artist page (popular tracks + albums + singles +
    /// compilations) over librespot metadata for the artist `uri`.
    FetchArtistPage {
        uri: String,
        key: String,
    },
    /// Fetch a playlist's RAW track uris (up to 101) for a SAFE remove-a-track:
    /// personal apps can't `DELETE` playlist items, so lyrfin removes by replacing
    /// the playlist with this list minus the track. Capped at 101 so the app can
    /// detect (and refuse) playlists over 100 tracks instead of risking a replace
    /// with a truncated list. Unlike `FetchTracks` these uris are NOT
    /// metadata-resolved, so unavailable tracks are kept (never silently dropped)
    /// — essential, else rebuilding the playlist would delete them.
    FetchPlaylistUris {
        uri: String,
        key: String,
    },
    /// Resolve a podcast episode's playable source. Most podcasts are externally
    /// hosted (a plain MP3 `external_url`) which librespot can't decode
    /// (librespot#818), so lyrfin streams those itself; Spotify-hosted episodes fall
    /// back to librespot. Replies with [`SessionEvent::EpisodeResolved`].
    ResolveEpisode {
        uri: String,
        position_ms: u32,
    },
    /// Fetch Spotify's editorial **home** feed over the pathfinder GraphQL gateway
    /// (the Web API no longer serves browse to dev-mode apps). Replies with
    /// [`SessionEvent::Browse`].
    FetchHome {
        key: String,
    },
    /// Fetch a browse page over pathfinder: the "Browse all" categories root, or one
    /// category's page (`uri` = `spotify:page:…`). `limit` bounds items-per-shelf —
    /// grown on scroll to page a flat grid (Podcast Charts / Categories) in. Replies
    /// with [`SessionEvent::Browse`].
    FetchBrowsePage {
        uri: String,
        key: String,
        limit: usize,
    },
}

/// Events the session thread sends back to the app.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    Connected,
    ConnectError(String),
    /// The cached access token was expired; the session refreshed it. The app
    /// adopts the fresh set so the Web API benefits too.
    TokenRefreshed(Tokens),
    Playing {
        position_ms: u32,
    },
    Paused {
        position_ms: u32,
    },
    EndOfTrack,
    Unavailable,
    /// Spotify refused the per-track audio decryption key (DRM) to this librespot
    /// client. Detected via the log probe ([`crate::spotify::logprobe`]) because
    /// librespot only logs it and then stalls — there is no player event. The app
    /// stops the buffering and tells the user playback is blocked.
    AudioKeyDenied,
    /// Resolved tracks of a container (playlist / artist top-tracks), keyed so
    /// the app can ignore a stale request.
    Tracks {
        key: String,
        items: Vec<crate::spotify::api::Item>,
    },
    /// A playlist's RAW track uris (answering `FetchPlaylistUris`), for the safe
    /// remove-a-track replace path. `ok` is false when the fetch failed (so the
    /// app never mistakes a failure for an empty playlist and clears it); more
    /// than 100 entries means the playlist is too large to rebuild safely → the
    /// app refuses and changes nothing.
    PlaylistUris {
        key: String,
        uris: Vec<String>,
        ok: bool,
    },
    /// Artist popularity (0–100) from librespot, sent alongside an artist
    /// `FetchTracks`. The Web API strips follower counts for dev-mode apps, so this
    /// is the headline stat the artist pane can always show on the shared client id.
    ArtistMeta {
        key: String,
        popularity: u32,
        /// Spotify's own artist biography (empty if none) — preferred over the
        /// Wikipedia bio in the pane, since it's the official, right-language text.
        bio: String,
    },
    /// A grouped artist page (items tagged with their [`crate::spotify::api::Group`]:
    /// popular tracks, albums, singles, compilations), keyed like `Tracks`.
    ArtistPage {
        key: String,
        items: Vec<crate::spotify::api::Item>,
    },
    /// A resolved podcast episode: `url` is a directly-streamable external MP3
    /// (lyrfin plays it through its own engine, since librespot can't), or `None` to
    /// fall back to librespot playback. `uri` keys it to the request.
    EpisodeResolved {
        uri: String,
        url: Option<String>,
        position_ms: u32,
    },
    /// Browse results (currently the home feed) from the pathfinder GraphQL
    /// gateway, keyed like `Tracks`. `error` is set (and `items` empty) when the
    /// query failed — e.g. a rotated persisted-query hash — so the pane can say so.
    Browse {
        key: String,
        items: Vec<crate::spotify::api::Item>,
        error: Option<String>,
    },
}

/// Lock-protected FIFO of f32 samples bridging librespot's sink (producer) to
/// lyrfin's engine (consumer). Interleaved stereo at [`SRC_RATE`].
#[derive(Debug, Default)]
pub struct Bridge {
    buf: Mutex<VecDeque<f32>>,
    active: AtomicBool,
}

impl Bridge {
    fn push(&self, s: &[f32]) {
        self.buf.lock().unwrap().extend(s.iter().copied());
    }
    fn len(&self) -> usize {
        self.buf.lock().unwrap().len()
    }
    /// Drop buffered audio (used on stop/track-change to avoid stale playback).
    pub fn clear(&self) {
        self.buf.lock().unwrap().clear();
    }
}

impl ExternalAudioSource for Bridge {
    fn pull(&self, out: &mut [f32]) -> usize {
        let mut q = self.buf.lock().unwrap();
        let n = out.len().min(q.len());
        for slot in out.iter_mut().take(n) {
            *slot = q.pop_front().unwrap();
        }
        n
    }
    fn sample_rate(&self) -> u32 {
        SRC_RATE
    }
    fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }
}

/// The librespot audio backend: convert f64 samples → f32 and push to the bridge.
struct BridgeSink {
    bridge: Arc<Bridge>,
}

impl Sink for BridgeSink {
    fn start(&mut self) -> SinkResult<()> {
        self.bridge.active.store(true, Ordering::Relaxed);
        Ok(())
    }
    fn stop(&mut self) -> SinkResult<()> {
        self.bridge.active.store(false, Ordering::Relaxed);
        Ok(())
    }
    fn write(&mut self, packet: AudioPacket, _converter: &mut Converter) -> SinkResult<()> {
        if let AudioPacket::Samples(samples) = packet {
            // backpressure: throttle librespot if lyrfin hasn't drained the bridge
            while self.bridge.len() > BRIDGE_CAP {
                std::thread::sleep(Duration::from_millis(5));
            }
            let f32s: Vec<f32> = samples.iter().map(|&x| x as f32).collect();
            self.bridge.push(&f32s);
        }
        Ok(())
    }
}

fn forward_event(ev: PlayerEvent, tx: &Sender<SessionEvent>) {
    let mapped = match ev {
        PlayerEvent::Playing { position_ms, .. } => Some(SessionEvent::Playing { position_ms }),
        PlayerEvent::Paused { position_ms, .. } => Some(SessionEvent::Paused { position_ms }),
        PlayerEvent::EndOfTrack { .. } => Some(SessionEvent::EndOfTrack),
        PlayerEvent::Unavailable { .. } => Some(SessionEvent::Unavailable),
        _ => None,
    };
    if let Some(e) = mapped {
        let _ = tx.send(e);
    }
}

/// Pick the cover ~300px wide (crisp without the full 640px) and build its CDN
/// URL — Spotify serves images at `i.scdn.co/image/{hex file id}`.
fn cover_url(images: &Images) -> Option<String> {
    let best = images.0.iter().min_by_key(|im| (im.width - 300).abs())?;
    Some(format!("https://i.scdn.co/image/{}", best.id.to_base16()))
}

/// One metadata `Track` → a UI [`Item`] (name, joined artists, cover, duration).
fn track_to_item(t: &Track, uri: &SpotifyUri) -> crate::spotify::api::Item {
    let subtitle = t
        .artists
        .0
        .iter()
        .map(|a| a.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let cover = if !t.album.covers.is_empty() {
        cover_url(&t.album.covers)
    } else {
        cover_url(&t.album.cover_group)
    };
    let artist_uri = t.artists.0.first().map(|a| a.id.to_uri());
    crate::spotify::api::Item {
        uri: uri.to_uri(),
        name: t.name.trim().to_string(),
        subtitle,
        album: t.album.name.trim().to_string(),
        image: cover,
        kind: crate::spotify::api::Kind::Track,
        duration_ms: t.duration.max(0) as u32,
        artist_uri,
        ..Default::default()
    }
}

/// A podcast `Episode` → a UI item (playlists can mix tracks + episodes).
fn episode_to_item(e: &Episode, uri: &SpotifyUri) -> crate::spotify::api::Item {
    crate::spotify::api::Item {
        uri: uri.to_uri(),
        name: e.name.trim().to_string(),
        subtitle: e.show_name.trim().to_string(),
        album: e.show_name.trim().to_string(),
        image: cover_url(&e.covers),
        kind: crate::spotify::api::Kind::Track,
        duration_ms: e.duration.max(0) as u32,
        artist_uri: None,
        ..Default::default()
    }
}

/// One metadata `Album` → a UI [`Item`] (name, joined artists, cover, year),
/// tagged with the artist-page section it belongs to.
fn album_to_item(
    a: &Album,
    uri: &SpotifyUri,
    group: crate::spotify::api::Group,
) -> crate::spotify::api::Item {
    let subtitle = a
        .artists
        .0
        .iter()
        .map(|x| x.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let cover = if !a.covers.is_empty() {
        cover_url(&a.covers)
    } else {
        cover_url(&a.cover_group)
    };
    let year = u16::try_from(a.date.as_utc().year())
        .ok()
        .filter(|y| *y > 0);
    crate::spotify::api::Item {
        uri: uri.to_uri(),
        name: a.name.clone(),
        subtitle,
        image: cover,
        kind: crate::spotify::api::Kind::Album,
        year,
        artist_uri: a.artists.0.first().map(|x| x.id.to_uri()),
        group,
        ..Default::default()
    }
}

/// Resolve one playlist entry (track or podcast episode) to a UI item.
async fn resolve_item(session: Session, uri: SpotifyUri) -> Option<crate::spotify::api::Item> {
    match uri {
        // some episodes (region-restricted / odd metadata) fail to parse; skip them
        SpotifyUri::Episode { .. } => Episode::get(&session, &uri)
            .await
            .ok()
            .map(|e| episode_to_item(&e, &uri)),
        _ => Track::get(&session, &uri)
            .await
            .ok()
            .map(|t| track_to_item(&t, &uri)),
    }
}

/// Resolve a playlist's contents (or an artist's top-tracks) to UI items via the
/// metadata protocol. Items are fetched in small concurrent batches (order
/// preserved) and capped so a huge playlist can't stall the session thread.
const MAX_TRACKS: usize = 100;

/// A playlist's track uris over librespot (the Web API blocks them for dev-mode
/// apps). Empty on any failure.
async fn fetch_playlist_track_uris(session: &Session, uri: &str) -> Vec<SpotifyUri> {
    let Ok(parsed) = SpotifyUri::from_uri(uri) else {
        return Vec::new();
    };
    match Playlist::get(session, &parsed).await {
        Ok(p) => p.tracks().take(MAX_TRACKS).cloned().collect(),
        Err(_) => Vec::new(),
    }
}

/// A playlist's RAW track uris (up to `MAX_TRACKS + 1`), for the safe
/// remove-a-track path. Two deliberate differences from
/// [`fetch_playlist_track_uris`]: it takes 101 (so the caller can tell a >100
/// playlist apart and refuse), and it does NOT resolve metadata — the uris are
/// the real playlist refs, so an unavailable track is kept, never dropped.
/// `None` distinguishes a fetch failure from a genuinely empty playlist (so the
/// remove path never rebuilds an empty playlist over a real one).
async fn fetch_playlist_uris_capped(session: &Session, uri: &str) -> Option<Vec<String>> {
    let parsed = SpotifyUri::from_uri(uri).ok()?;
    let p = Playlist::get(session, &parsed).await.ok()?;
    Some(
        p.tracks()
            .take(MAX_TRACKS + 1)
            .map(|u| u.to_uri())
            .collect(),
    )
}

/// Artist metadata over librespot: (0–100 popularity, top-track uris). The Web
/// API strips follower counts for dev-mode apps, so popularity is the stat the
/// pane can always show on the shared client id. `None` on any failure.
async fn fetch_artist_meta(session: &Session, uri: &str) -> Option<(u32, Vec<SpotifyUri>, String)> {
    let parsed = SpotifyUri::from_uri(uri).ok()?;
    let a = Artist::get(session, &parsed).await.ok()?;
    let tops = a
        .top_tracks
        .for_country(&session.country())
        .0
        .into_iter()
        .take(MAX_TRACKS)
        .collect();
    // Spotify's own artist bio (first/primary), if any
    let bio = a
        .biographies
        .first()
        .map(|b| b.text.clone())
        .unwrap_or_default();
    Some((a.popularity.max(0) as u32, tops, bio))
}

/// A podcast episode's external MP3 URL when Spotify only *indexes* it (the audio
/// lives on the publisher's CDN). librespot can't decode those — see librespot#818
/// — so lyrfin streams the URL itself. Returns `None` for Spotify-hosted episodes
/// (non-empty `audio`), which librespot plays, and on any metadata failure.
async fn resolve_episode_url(session: &Session, uri: &str) -> Option<String> {
    let parsed = SpotifyUri::from_uri(uri).ok()?;
    let ep = Episode::get(session, &parsed).await.ok()?;
    // An external MP3 is the only thing we can play: Spotify-hosted episode audio
    // is DRM'd and the audio-key server denies the decryption key to third-party
    // clients ("error audio key"), so librespot can't decode it. Prefer the
    // external URL whenever present.
    let ext = ep.external_url.trim();
    (!ext.is_empty()).then(|| ext.to_string())
}

/// Resolve track uris to display [`Item`]s, in bounded concurrent chunks.
async fn resolve_items(session: &Session, uris: Vec<SpotifyUri>) -> Vec<crate::spotify::api::Item> {
    let mut items = Vec::with_capacity(uris.len());
    for chunk in uris.chunks(8) {
        let mut handles = Vec::with_capacity(chunk.len());
        for tu in chunk {
            handles.push(tokio::spawn(resolve_item(session.clone(), tu.clone())));
        }
        for h in handles {
            if let Ok(Some(it)) = h.await {
                items.push(it);
            }
        }
    }
    items
}

/// Resolve album uris to display [`Item`]s (tagged `group`), in bounded chunks.
async fn resolve_albums(
    session: &Session,
    uris: Vec<SpotifyUri>,
    group: crate::spotify::api::Group,
) -> Vec<crate::spotify::api::Item> {
    let mut items = Vec::with_capacity(uris.len());
    for chunk in uris.chunks(8) {
        let mut handles = Vec::with_capacity(chunk.len());
        for u in chunk {
            let s = session.clone();
            let u = u.clone();
            handles.push(tokio::spawn(async move {
                Album::get(&s, &u)
                    .await
                    .ok()
                    .map(|a| album_to_item(&a, &u, group))
            }));
        }
        for h in handles {
            if let Ok(Some(it)) = h.await {
                items.push(it);
            }
        }
    }
    items
}

/// Build a grouped artist page over librespot metadata (one `Artist::get` + an
/// album lookup per release): the artist's popular tracks, then their albums,
/// singles/EPs and compilations — each item tagged with its [`Group`] so the UI
/// renders sections. Empty on failure. Caps each section so a prolific artist
/// doesn't fan out into hundreds of metadata fetches.
async fn fetch_artist_page(session: &Session, uri: &str) -> Vec<crate::spotify::api::Item> {
    use crate::spotify::api::Group;
    const POPULAR_MAX: usize = 10;
    const RELEASES_MAX: usize = 20;
    let Ok(parsed) = SpotifyUri::from_uri(uri) else {
        return Vec::new();
    };
    let Ok(a) = Artist::get(session, &parsed).await else {
        return Vec::new();
    };
    // representative uri of each release group (first edition)
    let group_uris = |groups: &librespot::metadata::artist::AlbumGroups| -> Vec<SpotifyUri> {
        groups
            .iter()
            .filter_map(|g| g.first().cloned())
            .take(RELEASES_MAX)
            .collect()
    };
    let top_uris: Vec<SpotifyUri> = a
        .top_tracks
        .for_country(&session.country())
        .0
        .into_iter()
        .take(POPULAR_MAX)
        .collect();
    let album_uris = group_uris(&a.albums);
    let single_uris = group_uris(&a.singles);
    let comp_uris = group_uris(&a.compilations);

    let mut items = Vec::new();
    let mut popular = resolve_items(session, top_uris).await;
    for it in &mut popular {
        it.group = Group::Popular;
    }
    items.extend(popular);
    items.extend(resolve_albums(session, album_uris, Group::Albums).await);
    items.extend(resolve_albums(session, single_uris, Group::Singles).await);
    items.extend(resolve_albums(session, comp_uris, Group::Compilations).await);
    items
}

fn handle_command(cmd: SessionCommand, player: &Player, bridge: &Bridge) {
    match cmd {
        SessionCommand::Load { uri, position_ms } => {
            if let Ok(parsed) = SpotifyUri::from_uri(&uri) {
                bridge.clear(); // a hard cut — drop any buffered audio
                player.load(parsed, true, position_ms);
            }
        }
        SessionCommand::Play => player.play(),
        SessionCommand::Pause => player.pause(),
        SessionCommand::Seek(ms) => player.seek(ms),
        SessionCommand::Preload { uri } => {
            if let Ok(parsed) = SpotifyUri::from_uri(&uri) {
                player.preload(parsed);
            }
        }
        // handled in the command loop (needs the session + async); never here
        SessionCommand::FetchTracks { .. }
        | SessionCommand::FetchArtistPage { .. }
        | SessionCommand::FetchPlaylistUris { .. }
        | SessionCommand::ResolveEpisode { .. }
        | SessionCommand::FetchHome { .. }
        | SessionCommand::FetchBrowsePage { .. } => {}
    }
}

/// Spawn the session thread. Returns command sender, event receiver, and the
/// shared bridge to hand to the audio engine via `AudioCommand::SetExternalSource`.
///
/// `tokens` may be expired (access tokens last ~1h, so a long browse session
/// outlives them); the session refreshes on its own thread before connecting and
/// reports the fresh set via [`SessionEvent::TokenRefreshed`] — the UI thread is
/// never blocked on the network round-trip.
pub fn spawn(
    tokens: Tokens,
    dir: PathBuf,
    bitrate_kbps: u16,
) -> (Sender<SessionCommand>, Receiver<SessionEvent>, Arc<Bridge>) {
    let bridge = Arc::new(Bridge::default());
    let (cmd_tx, cmd_rx) = unbounded::<SessionCommand>();
    let (evt_tx, evt_rx) = unbounded::<SessionEvent>();
    let bridge_thread = bridge.clone();
    let _ = std::thread::Builder::new()
        .name("lyrfin-spotify-session".into())
        .spawn(move || {
            // Start clean: drop any audio-key-denied flags left by a prior session so
            // this one isn't tripped by a stale signal.
            crate::spotify::logprobe::AUDIO_KEY_DENIED.store(false, Ordering::Relaxed);
            crate::spotify::logprobe::AUDIO_KEY_BLOCKED.store(false, Ordering::Relaxed);
            // Refresh a stale token here, on this plain thread (not the UI thread,
            // not yet inside the tokio runtime) — a blocking ureq call is fine.
            let token = if tokens.is_expired() {
                match auth::refresh(&tokens.refresh_token) {
                    Ok(fresh) => {
                        fresh.save(&dir);
                        let _ = evt_tx.send(SessionEvent::TokenRefreshed(fresh.clone()));
                        fresh.access_token
                    }
                    Err(e) => {
                        // the refresh token is dead (revoked / too old) — only a
                        // fresh browser login recovers, so say so plainly
                        let _ = evt_tx.send(SessionEvent::ConnectError(format!(
                            "session expired — re-authenticate (; → Re-authenticate). [{e}]"
                        )));
                        return;
                    }
                }
            } else {
                tokens.access_token.clone()
            };
            let rt = match tokio::runtime::Runtime::new() {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = evt_tx.send(SessionEvent::ConnectError(format!(
                        "couldn't start audio runtime: {e}"
                    )));
                    return;
                }
            };
            rt.block_on(async move {
                // Audio playback requires the keymaster client id: a custom
                // developer id can't obtain the "client token" the spclient needs
                // to resolve audio (it 400s), so every track comes back
                // Unavailable. The user token (minted with any client id) still
                // authenticates the session; only the client_id used for audio
                // resolution must be the keymaster one. The Web API keeps the
                // custom id (its own rate-limit quota) — that's unaffected here.
                let session_config = SessionConfig {
                    client_id: crate::spotify::auth::KEYMASTER_CLIENT_ID.to_string(),
                    ..SessionConfig::default()
                };
                let session = Session::new(session_config, None);
                // a clone for metadata fetches (the original is moved into Player)
                let meta_session = session.clone();
                log::info!(
                    target: "lyrfin::spotify",
                    "librespot session: connecting (audio client_id=keymaster, web custom={}, bitrate={}kbps)",
                    crate::spotify::auth::has_custom_client_id(),
                    bitrate_kbps
                );
                let connect = session.connect(Credentials::with_access_token(token), false);
                match tokio::time::timeout(Duration::from_secs(30), connect).await {
                    Ok(Ok(())) => {
                        log::info!(target: "lyrfin::spotify", "librespot session: connected");
                    }
                    Ok(Err(e)) => {
                        log::warn!(target: "lyrfin::spotify", "librespot session: connect failed: {e}");
                        let _ = evt_tx.send(SessionEvent::ConnectError(format!(
                            "Spotify playback login failed: {e}"
                        )));
                        return;
                    }
                    Err(_) => {
                        log::warn!(target: "lyrfin::spotify", "librespot session: connect timed out (30s)");
                        let _ = evt_tx.send(SessionEvent::ConnectError(
                            "Spotify playback login timed out (30s) — rate-limited or offline"
                                .into(),
                        ));
                        return;
                    }
                }
                let _ = evt_tx.send(SessionEvent::Connected);

                let sink_bridge = bridge_thread.clone();
                // map the user's quality choice to librespot's three valid steps
                let bitrate = match bitrate_kbps {
                    96 => Bitrate::Bitrate96,
                    320 => Bitrate::Bitrate320,
                    _ => Bitrate::Bitrate160,
                };
                let player = Player::new(
                    PlayerConfig {
                        bitrate,
                        ..PlayerConfig::default()
                    },
                    session,
                    Box::new(NoOpVolume),
                    move || {
                        Box::new(BridgeSink {
                            bridge: sink_bridge,
                        })
                    },
                );

                // forward librespot player events to the app
                let mut events = player.get_player_event_channel();
                let etx = evt_tx.clone();
                tokio::spawn(async move {
                    while let Some(ev) = events.recv().await {
                        forward_event(ev, &etx);
                    }
                });

                // poll the (sync) command channel; player methods are non-blocking,
                // metadata fetches run as detached tasks (slow, must not stall here)
                loop {
                    match cmd_rx.try_recv() {
                        Ok(SessionCommand::FetchTracks { uri, artist, key }) => {
                            let s = meta_session.clone();
                            let tx = evt_tx.clone();
                            tokio::spawn(async move {
                                let track_uris = if artist {
                                    // one Artist::get yields popularity + top tracks;
                                    // forward the stat, then resolve the tracks
                                    match fetch_artist_meta(&s, &uri).await {
                                        Some((popularity, tops, bio)) => {
                                            let _ = tx.send(SessionEvent::ArtistMeta {
                                                key: key.clone(),
                                                popularity,
                                                bio,
                                            });
                                            tops
                                        }
                                        None => Vec::new(),
                                    }
                                } else {
                                    fetch_playlist_track_uris(&s, &uri).await
                                };
                                let items = resolve_items(&s, track_uris).await;
                                let _ = tx.send(SessionEvent::Tracks { key, items });
                            });
                        }
                        Ok(SessionCommand::FetchArtistPage { uri, key }) => {
                            let s = meta_session.clone();
                            let tx = evt_tx.clone();
                            tokio::spawn(async move {
                                let items = fetch_artist_page(&s, &uri).await;
                                let _ = tx.send(SessionEvent::ArtistPage { key, items });
                            });
                        }
                        Ok(SessionCommand::FetchPlaylistUris { uri, key }) => {
                            let s = meta_session.clone();
                            let tx = evt_tx.clone();
                            tokio::spawn(async move {
                                let fetched = fetch_playlist_uris_capped(&s, &uri).await;
                                let ok = fetched.is_some();
                                let _ = tx.send(SessionEvent::PlaylistUris {
                                    key,
                                    uris: fetched.unwrap_or_default(),
                                    ok,
                                });
                            });
                        }
                        Ok(SessionCommand::ResolveEpisode { uri, position_ms }) => {
                            let s = meta_session.clone();
                            let tx = evt_tx.clone();
                            tokio::spawn(async move {
                                let url = resolve_episode_url(&s, &uri).await;
                                let _ = tx.send(SessionEvent::EpisodeResolved {
                                    uri,
                                    url,
                                    position_ms,
                                });
                            });
                        }
                        Ok(SessionCommand::FetchHome { key }) => {
                            let s = meta_session.clone();
                            let tx = evt_tx.clone();
                            tokio::spawn(async move {
                                let (items, error) =
                                    match crate::spotify::pathfinder::fetch_home(&s).await {
                                        Ok(items) => (items, None),
                                        Err(e) => (Vec::new(), Some(e)),
                                    };
                                let _ = tx.send(SessionEvent::Browse { key, items, error });
                            });
                        }
                        Ok(SessionCommand::FetchBrowsePage { uri, key, limit }) => {
                            let s = meta_session.clone();
                            let tx = evt_tx.clone();
                            tokio::spawn(async move {
                                let (items, error) = match crate::spotify::pathfinder::fetch_browse_page(&s, &uri, limit)
                                    .await
                                {
                                    Ok(items) => (items, None),
                                    Err(e) => (Vec::new(), Some(e)),
                                };
                                let _ = tx.send(SessionEvent::Browse { key, items, error });
                            });
                        }
                        Ok(cmd) => handle_command(cmd, &player, &bridge_thread),
                        Err(TryRecvError::Empty) => {
                            // librespot logs an audio-key denial then stalls silently
                            // (no player event), so surface it from the log probe.
                            if crate::spotify::logprobe::AUDIO_KEY_DENIED
                                .swap(false, Ordering::Relaxed)
                            {
                                let _ = evt_tx.send(SessionEvent::AudioKeyDenied);
                            }
                            tokio::time::sleep(Duration::from_millis(40)).await;
                        }
                        Err(TryRecvError::Disconnected) => break,
                    }
                }
            });
        });
    (cmd_tx, evt_rx, bridge)
}
