//! Spotify view + playback-overlay state + the session/connection + browse-result
//! methods on `AppState`. Playback/transport lives in `playback`, browse/nav in
//! `browse`.

use super::{AppState, Layout, Panel};
use crate::audio::AudioCommand;
use crate::core::player::{Repeat, Status};

mod browse;
mod cache;
mod events;
mod playback;
pub(crate) mod playlist;

/// Sentinel request key that routes a result to the artist pane (vs the browse
/// list), reusing the existing Web-API / librespot fetch paths.
const ARTIST_PANE_KEY: &str = "@artist-pane";

/// Recovery state for a dropped librespot connection. Spotify can close a
/// session's access-point connection at any time; librespot's bare `Session`
/// can't be reused afterward and emits **no event** for it — only a log line — so
/// the first sign lyrfin gets is a track failing to load. Instead of dead-ending in
/// a back-off + "re-authenticate" prompt (which only a full app restart cleared),
/// lyrfin drops the dead session and replays the current track on a fresh one — the
/// in-process equivalent of that restart.
///
/// The state exists to fire that retry exactly once: a single timed-out audio key
/// surfaces as **both** [`SessionEvent::AudioKeyDenied`] and
/// [`SessionEvent::Unavailable`], and the dead session keeps emitting such echoes
/// until it's torn down, so a naive "retry on failure" would double-fire.
///
/// [`SessionEvent::AudioKeyDenied`]: crate::spotify::session::SessionEvent::AudioKeyDenied
/// [`SessionEvent::Unavailable`]: crate::spotify::session::SessionEvent::Unavailable
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SpRecovery {
    /// Playing normally. A load failure means "reconnect + retry this track once".
    #[default]
    Normal,
    /// Dropped the dead session and re-issued the track on a fresh session that is
    /// still connecting. Failure events here are echoes from the dead session —
    /// ignore them until the fresh session reports `Connected`.
    Reconnecting,
    /// The fresh session connected. A failure now is real (the track is genuinely
    /// unavailable), so fall through to the normal skip / back-off.
    Reconnected,
}

/// Spotify playback-overlay state, grouped out of [`AppState`] (the local player
/// is preserved/frozen while this drives the engine). Accessed as `app.spov.*`.
pub struct SpOverlay {
    pub now_spotify: Option<crate::spotify::api::Item>,
    pub spotify_paused: bool,
    pub sp_pos: f64,
    pub sp_dur: f64,
    pub sp_queue: Vec<crate::spotify::api::Item>,
    pub sp_idx: usize,
    pub sp_started: bool,
    pub sp_fail_streak: u32,
    /// Dropped-connection recovery state (see [`SpRecovery`]). Drives the
    /// reconnect-and-retry-once path in [`AppState::spotify_try_reconnect_retry`].
    pub sp_recovery: SpRecovery,
    /// Unix-seconds deadline before which Spotify playback won't be re-attempted,
    /// after repeated failures (audio-key denial / connect errors). Exponential
    /// back-off (20s → 5 min cap) so a failing account can't hammer Spotify into a
    /// rate-limit. 0 = none. See [`AppState::spotify_trip_cooldown`].
    pub sp_cooldown_until: u64,
    /// A track has actually played (reached `Playing`) at least once this session.
    /// Proof the account is **not** audio-key-blocked at the account level — so a
    /// later key denial is a transient CDN/throttle blip to recover from, never the
    /// permanent block. Reset only on teardown (logout / account switch). See
    /// [`AppState::spotify_key_block_confirmed`].
    pub sp_played_ok: bool,
    /// Tracks whose audio key stayed denied since the last successful play (reset to 0
    /// on any `Playing`). Only once this crosses [`SP_KEY_BLOCK_CONFIRM`] *with nothing
    /// ever having played* does lyrfin treat it as the account-level block; a lone blip
    /// mid-playback never does.
    pub sp_key_denials: u16,
    /// Unix-seconds deadline to **auto-resume** playback after a transient-failure
    /// back-off (`None` = no resume pending). Set by [`AppState::spotify_playback_failed`]
    /// so a bad-CDN / throttle / brief-drop stall recovers on its own (a fresh session)
    /// instead of leaving the queue stopped until the user restarts lyrfin; cleared the
    /// moment the user pauses/plays, a track plays, or the overlay tears down. Skipped
    /// for a confirmed account-level block (which won't recover). Drained in
    /// [`AppState::spotify_tick_cooldown_resume`].
    pub sp_resume_at: Option<u64>,
    /// A quick same-session retry of the current track is scheduled for this instant
    /// (`None` = none). Armed when Spotify **throttles** the per-track audio key —
    /// the classic "skip fast → `error audio key`" burst, which is transient: the
    /// same key succeeds a moment later. Retrying in place beats the heavyweight
    /// reconnect + 20s back-off (a fresh session hits the same account-level
    /// throttle). Drained in [`AppState::spotify_tick_keyretry`]; only after the
    /// bounded retries are spent does lyrfin fall through to reconnect/back-off.
    pub sp_keyretry_at: Option<std::time::Instant>,
    /// Quick key-retries already spent on the current track (reset when a new track
    /// is loaded or one actually plays). Bounds the transient-throttle retries so a
    /// *genuine* DRM denial still escalates promptly.
    pub sp_keyretry_n: u8,
    /// An in-place re-buffer of the current track is scheduled for this instant
    /// (`None` = none). Armed when librespot reports `EndOfTrack` **mid-track** — its
    /// event is overloaded (a genuine end AND a "couldn't fetch/decode the next
    /// packet" abort under network congestion), so a premature one re-buffers the
    /// same track where it stalled rather than skipping. Without this a transient
    /// stall silently drops the track — and under sustained congestion races the
    /// whole queue ("buffering and flipping tracks until it settles"). Drained in
    /// [`AppState::spotify_tick_stall`].
    pub sp_stall_at: Option<std::time::Instant>,
    /// Consecutive in-place re-buffers spent at (roughly) the same spot. Reset on a
    /// fresh load and whenever playback makes real forward progress past the last
    /// stall (see [`AppState::spotify_arm_stall_retry`]) — so a track riding out
    /// several *separate* transient hiccups keeps retrying, while a *repeated* stall
    /// at one point (a corrupt segment) depletes the budget and skips instead of
    /// looping forever.
    pub sp_stall_n: u8,
    /// Track position (seconds) of the last stall, so the budget refills only when
    /// playback advances meaningfully past it. See [`sp_stall_n`].
    pub sp_stall_pos: f64,
    /// Queue index a manual skip (n/p / transport click) landed on while its load is
    /// **debounced** (`None` = no skip pending). Hammering next only loads the track
    /// finally landed on, so a burst of skips fires one audio-key request instead of
    /// one per intermediate track — the burst is what trips Spotify's key throttle
    /// (see [`sp_keyretry_at`]). Drained in [`AppState::spotify_tick_skip`].
    pub sp_skip_target: Option<usize>,
    /// Debounce deadline for a pending manual skip: each press pushes it out, so the
    /// load fires only once the user stops skipping.
    pub sp_skip_at: Option<std::time::Instant>,
    /// Debounce deadline for a pending seek on a STREAMED episode: holding `,`/`.` scrubs
    /// the bar (`sp_pos`) every key-repeat, but each engine seek is a ranged HTTP
    /// re-open, so the actual re-open fires only once scrubbing settles — otherwise a
    /// burst of seeks queues dozens of re-buffers and the stream stalls.
    pub sp_seek_at: Option<std::time::Instant>,
    /// The position a streamed scrub is heading to. While `Some`, the bar is LOCKED to
    /// it: the engine's live progress (which lags behind the scrub) is ignored so the
    /// bar can't jump back, until the engine re-opens and reaches the target.
    pub sp_seek_target: Option<f64>,
    /// Consecutive-seek count + timestamp, so a held `,`/`.` ACCELERATES the step (with a
    /// cap that scales to the episode length) instead of crawling 5s at a time.
    pub sp_seek_streak: u32,
    pub sp_seek_streak_at: Option<std::time::Instant>,
    pub sp_cover: crate::ui::components::CoverState,
    pub sp_cover_url: Option<String>,
    pub sp_saved: bool,
    /// A follow/unfollow the worker is resolving: `(uri, display name)`, set when the
    /// request is sent so the result's toast can name the show/artist. The URI matches
    /// the echoed `SpResult::Follow`; cleared once handled.
    pub sp_follow_pending: Option<(String, String)>,
    pub sp_shuffle: bool,
    pub sp_repeat: Repeat,
    // ---- rich artist pane (the now-playing track's artist) ----
    /// Photo/genres/followers of the now-playing artist (`None` until fetched).
    pub sp_artist: Option<SpArtist>,
    /// The artist uri currently loaded into `sp_artist` (dedups refetches).
    pub sp_artist_uri: Option<String>,
    pub sp_artist_cover: crate::ui::components::CoverState,
    pub sp_artist_cover_url: Option<String>,
    /// The artist's top tracks (via librespot), shown in the pane.
    pub sp_artist_top: Vec<crate::spotify::api::Item>,
    /// Podcast analogue of `sp_artist`: the now-playing EPISODE's show metadata
    /// (publisher + "about" description), shown in the pane in place of an artist
    /// (episodes have none). `None` for music tracks.
    pub sp_show_meta: Option<ShowMeta>,
    pub session_cmd: Option<crossbeam_channel::Sender<crate::spotify::session::SessionCommand>>,
    pub session_rx: Option<crossbeam_channel::Receiver<crate::spotify::session::SessionEvent>>,
    /// The now-playing item is an externally-hosted podcast episode that librespot
    /// can't decode, so lyrfin streams its MP3 through its own engine (not the
    /// librespot bridge). Play/pause + the position clock route to the engine.
    pub sp_stream: bool,
    /// Kept so the engine can be re-pointed at librespot (`SetExternalSource`)
    /// after a streamed episode released it (`ClearExternalSource`).
    pub sp_bridge: Option<std::sync::Arc<crate::spotify::session::Bridge>>,
}

/// Rich details for the now-playing track's artist, shown in the artist pane.
/// `followers` comes from the Web API (0 when the dev-mode app strips it);
/// `genres`/`popularity` come from librespot, which always provides them.
#[derive(Clone, Default)]
pub struct SpArtist {
    pub name: String,
    pub genres: String,
    pub followers: u64,
    pub popularity: u32,
    /// Spotify's own artist biography (from librespot); preferred over the
    /// Wikipedia bio in the pane when present.
    pub bio: String,
}

/// The now-playing podcast episode's SHOW metadata (publisher + description),
/// shown in the artist pane's "About" in place of `SpArtist` — episodes have no
/// artist. Keyed by `uri` so a stale fetch never overwrites the current show.
#[derive(Clone, Default)]
pub struct ShowMeta {
    pub uri: String,
    pub publisher: String,
    pub description: String,
}

impl AppState {
    // ---- Spotify ---------------------------------------------------------
    /// Open the Spotify view. If a cached token exists and we're not already
    /// connected, resume the session in the background (refresh + greet).
    pub(crate) fn open_spotify(&mut self) {
        self.set_layout(Layout::Spotify);
        let connected = matches!(
            self.spotify.conn,
            crate::spotify::ConnState::Connected { .. }
        );
        let busy = self.spotify.auth_rx.is_some();
        if !connected && !busy {
            if let Some(tokens) = self.spotify.tokens.clone() {
                self.spotify.conn = crate::spotify::ConnState::Connecting { url: None };
                self.spotify.auth_rx = Some(crate::spotify::spawn_resume(
                    self.config.dir.clone(),
                    tokens,
                ));
            } else {
                self.spotify.conn = crate::spotify::ConnState::Disconnected;
            }
        } else if connected && self.spotify.items.is_empty() && !self.spotify.loading {
            self.spotify_load_section();
        }
    }

    /// ⏎ on the auth panel: if a token is already cached (e.g. after a transient
    /// error), just resume it — no browser. Otherwise start the browser login.
    pub(crate) fn spotify_login(&mut self) {
        if self.spotify.auth_rx.is_some() {
            return; // a login/resume is already running
        }
        // re-authing is a clean slate: stop any stale playback overlay (so a
        // track can't keep streaming, and space can't control it, behind the
        // login panel) and drop the old librespot session so the next play
        // spawns a fresh one bound to the new token.
        self.stop_spotify_overlay();
        self.spov.session_cmd = None;
        self.spov.session_rx = None;
        self.spotify.conn = crate::spotify::ConnState::Connecting { url: None };
        if let Some(tokens) = self.spotify.tokens.clone() {
            self.spotify.auth_rx = Some(crate::spotify::spawn_resume(
                self.config.dir.clone(),
                tokens,
            ));
        } else {
            self.spotify.auth_rx = Some(crate::spotify::spawn_login(self.config.dir.clone()));
            self.notify("Opening your browser to log in to Spotify…".into());
        }
    }

    /// Snapshot what should survive a re-auth, then pause (don't wipe) the overlay.
    /// Split from [`Self::spotify_reauthenticate`] so the state handling is
    /// unit-testable without spawning the browser-login thread.
    ///
    /// Preserves: the now-playing track + artist pane + cover (paused, via
    /// [`Self::pause_spotify_overlay`]); the drilled-in container + cursor (saved to
    /// `restore_open`/`restore_sel`, re-applied by [`Self::spotify_apply_initial_restore`]
    /// once the section reloads). Marks the current account so logging back in as a
    /// *different* one still clears its state (the existing `restored_account` guard
    /// in `pump_spotify`). Drops the librespot session so the fresh token spawns a new one.
    pub(crate) fn spotify_prepare_reauth(&mut self) {
        self.spotify.restore_open = self.spotify.open_item.clone();
        self.spotify.restore_sel = Some(self.spotify.sel);
        self.spotify.restored_account = self.spotify.account_id.clone();
        self.pause_spotify_overlay(); // keep the overlay on screen, just paused
        self.spov.session_cmd = None;
        self.spov.session_rx = None;
    }

    /// Explicit "Re-authenticate / switch account" (Spotify settings `;`). Unlike
    /// the login panel's ⏎ (which silently *resumes* a cached token — what made the
    /// button look like it "did nothing"), this ALWAYS opens the browser for a fresh
    /// login, and keeps the current view + now-playing overlay on screen rather than
    /// blanking it. The new token overwrites the cached one on success.
    pub(crate) fn spotify_reauthenticate(&mut self) {
        if self.spotify.auth_rx.is_some() {
            return; // a login/resume is already running
        }
        self.spotify_prepare_reauth();
        self.spotify.conn = crate::spotify::ConnState::Connecting { url: None };
        self.spotify.auth_rx = Some(crate::spotify::spawn_login(self.config.dir.clone()));
        self.notify("Opening your browser to re-authenticate…".into());
    }

    /// Disconnect and forget the cached token. Tears down playback too — the
    /// librespot session keeps streaming on its own creds otherwise.
    pub(crate) fn spotify_logout(&mut self) {
        let was_connected = matches!(
            self.spotify.conn,
            crate::spotify::ConnState::Connected { .. }
        );
        self.stop_spotify_overlay(); // pause + clear now-playing + release engine
        self.spov.session_cmd = None; // drop the session → its thread exits, audio stops
        self.spov.session_rx = None;
        self.spotify_reset_browse_and_queue(); // don't leak this account's data into the next
        crate::spotify::Tokens::clear(&self.config.dir);
        self.spotify.tokens = None;
        self.spotify.account_id = None;
        self.spotify.restored_account = None;
        self.spotify.auth_rx = None;
        self.spotify.conn = crate::spotify::ConnState::Disconnected;
        self.notify(
            if was_connected {
                "Logged out of Spotify"
            } else {
                "Cleared cached Spotify login"
            }
            .into(),
        );
    }

    /// Wipe all per-account Spotify state — queue, browse list, drill-in, search,
    /// cached art, and the failure back-off — so logging out (or switching accounts)
    /// never leaks one account's data into the next session. The token, conn state,
    /// and the now-playing teardown are handled by the caller.
    fn spotify_reset_browse_and_queue(&mut self) {
        // playback overlay (now_spotify is cleared by stop_spotify_overlay)
        self.spov.sp_queue.clear();
        self.spov.sp_idx = 0;
        self.spov.sp_pos = 0.0;
        self.spov.sp_dur = 0.0;
        self.spov.sp_started = false;
        self.spov.sp_bridge = None;
        self.spov.sp_cover = None;
        self.spov.sp_cover_url = None;
        self.spov.sp_saved = false;
        self.spotify_clear_cooldown();
        // browse view
        self.spotify.items.clear();
        self.spotify.sel = 0;
        self.spotify.crumb = None;
        self.spotify.open_item = None;
        self.spotify.nav.clear();
        self.spotify.query.clear();
        self.spotify.searching = false;
        self.spotify.in_search = false;
        self.spotify.note.clear();
        self.spotify.loading = false;
        self.spotify.restore_sel = None;
        self.spotify.restore_open = None;
        // any open playlist-management modal belongs to the old account
        self.spotify.pl_modal = None;
        self.spotify.pl_confirm_delete = None;
        self.spotify.pl_pending_remove = None;
        // and the on-disk optimistic view cache — never resurface it on the next launch
        crate::spotify::view_cache::SpotifyViewCache::delete(&self.config.dir);
    }

    /// Drain Spotify auth/resume events (called each loop iteration, like
    /// `pump_audio`). Cheap no-op when nothing is in flight.
    pub fn pump_spotify(&mut self) {
        self.pump_spotify_session(); // librespot playback events
        self.spotify_tick_skip(); // load the track a burst of skips finally landed on
        self.spotify_tick_seek(); // re-open a streamed episode once scrubbing settles
        self.spotify_tick_keyretry(); // re-issue a track whose audio key was throttled
        self.spotify_tick_stall(); // re-buffer a track that stalled mid-play
        self.spotify_tick_stream_watchdog(); // re-open a streamed episode that stalled
        self.spotify_tick_cooldown_resume(); // auto-resume once a failure back-off elapses
        self.spotify_tick_reconnect(); // auto-retry a transient connection loss
        let Some(rx) = &self.spotify.auth_rx else {
            return;
        };
        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        if events.is_empty() {
            return;
        }
        self.dirty = true;
        for ev in events {
            match ev {
                crate::spotify::AuthEvent::Waiting { url } => {
                    self.spotify.conn = crate::spotify::ConnState::Connecting { url: Some(url) };
                }
                crate::spotify::AuthEvent::Connected {
                    tokens,
                    account_id,
                    name,
                    premium,
                } => {
                    self.spotify.tokens = Some(tokens);
                    self.spotify.auth_rx = None;
                    self.spotify.reconnect_at = None; // reached Spotify → stop retrying
                    self.spotify.reconnect_attempts = 0;
                    self.spotify_clear_cooldown(); // fresh login → retry playback
                    // If the restored playback/browse state belongs to a DIFFERENT
                    // account than the one we just connected as, drop it — never apply
                    // one account's now-playing/queue/drill-in to another. (Skipped
                    // when the id is unknown, e.g. a transient profile-fetch failure.)
                    if !account_id.is_empty() {
                        if let Some(prev) = self.spotify.restored_account.take()
                            && prev != account_id
                        {
                            self.stop_spotify_overlay();
                            self.spotify_reset_browse_and_queue();
                        }
                        self.spotify.account_id = Some(account_id);
                    }
                    // This fresh token came from a resume/login OUTSIDE the librespot
                    // session, so any existing session predates it — and after idle its
                    // connection is likely dead (token-freshness is NOT connection
                    // liveness, the #1 "silent refresh doesn't fix it" bug). Drop it so
                    // the next play respawns a live one. KEEP it only while actively
                    // streaming, where it's confirmed alive and dropping would cut audio.
                    let streaming = self.spov.sp_started && !self.spov.spotify_paused;
                    if !streaming {
                        self.spov.session_cmd = None;
                        self.spov.session_rx = None;
                    }
                    if !premium {
                        self.notify("Connected — note: full playback needs Spotify Premium".into());
                    } else {
                        self.notify(format!("Spotify connected as {name}"));
                    }
                    self.spotify.conn = crate::spotify::ConnState::Connected { name, premium };
                    // now that we're connected, load the current view (a restored
                    // search re-runs; otherwise the current/​restored section)
                    self.spotify_load_initial();
                }
                crate::spotify::AuthEvent::Error { msg } => {
                    self.spotify.auth_rx = None;
                    self.spotify.reconnect_at = None; // a real auth failure isn't retried
                    self.spotify.reconnect_attempts = 0;
                    self.log_error(msg.clone()); // copyable (y) + in the error log
                    self.spotify.conn = crate::spotify::ConnState::Error { msg };
                }
                // A transient reach-Spotify failure (network/rate-limit): keep the
                // token and retry with back-off instead of dead-ending on "log in
                // again". Common on wake-from-sleep, when a resume races the network
                // coming back.
                crate::spotify::AuthEvent::ConnLost { msg } => {
                    self.spotify.auth_rx = None;
                    if self.spotify.reconnect_attempts == 0 {
                        self.log_error(msg.clone()); // record the reason once per outage
                    }
                    self.spotify.reconnect_attempts =
                        self.spotify.reconnect_attempts.saturating_add(1);
                    // 5s, 10s, 20s, 40s, 60s cap — quick enough to recover promptly on
                    // wake, slow enough not to hammer a still-down connection.
                    let backoff = (5u64 << (self.spotify.reconnect_attempts - 1).min(4)).min(60);
                    self.spotify.reconnect_at = Some(crate::datetime::now_unix() + backoff);
                    self.spotify.conn = crate::spotify::ConnState::Reconnecting { msg };
                }
            }
        }
    }

    /// Whether a scheduled transient reconnect (see [`AuthEvent::ConnLost`]) is due:
    /// one is armed, its deadline has passed, nothing else is in flight, and a cached
    /// token exists to resume with. Split out so the timing gate is unit-testable
    /// without spawning the (networked) resume thread.
    ///
    /// [`AuthEvent::ConnLost`]: crate::spotify::AuthEvent::ConnLost
    pub(crate) fn spotify_reconnect_due(&self) -> bool {
        self.spotify.auth_rx.is_none()
            && self.spotify.tokens.is_some()
            && self
                .spotify
                .reconnect_at
                .is_some_and(|at| crate::datetime::now_unix() >= at)
    }

    /// Respawn a resume once a scheduled transient reconnect comes due. Driven by
    /// [`Self::pump_spotify`], which the run loop calls every tick — so it fires even
    /// while the app sits idle.
    fn spotify_tick_reconnect(&mut self) {
        if !self.spotify_reconnect_due() {
            return;
        }
        self.spotify.reconnect_at = None; // consumed; re-armed if this attempt fails too
        let Some(tokens) = self.spotify.tokens.clone() else {
            return;
        };
        self.spotify.auth_rx = Some(crate::spotify::spawn_resume(
            self.config.dir.clone(),
            tokens,
        ));
        self.dirty = true;
    }

    /// Whether a Spotify login/resume is in flight (keeps the loop ticking so the
    /// spinner animates + events land promptly).
    pub fn spotify_busy(&self) -> bool {
        self.spotify.auth_rx.is_some() || self.spotify.loading
    }

    pub fn set_spotify_sender(
        &mut self,
        tx: crossbeam_channel::Sender<crate::spotify::api::SpRequest>,
    ) {
        self.workers.spotify = Some(tx);
        // auto-resume a cached session at startup so opening the view lands on
        // "Connected" without a manual ⏎ (the token + library load in the bg)
        if let Some(tokens) = self.spotify.tokens.clone() {
            self.spotify.conn = crate::spotify::ConnState::Connecting { url: None };
            self.spotify.auth_rx = Some(crate::spotify::spawn_resume(
                self.config.dir.clone(),
                tokens,
            ));
        }
    }

    pub fn set_spotify_art_sender(
        &mut self,
        tx: crossbeam_channel::Sender<crate::spotify::artwork::ArtRequest>,
    ) {
        self.workers.spotify_art = Some(tx);
        // A session-restored now-playing track has no cover yet (art is normally
        // fetched on play). Now that the art worker exists, request it so the bar
        // shows the cover without waiting for the user to press play.
        if let Some(tr) = self.spov.now_spotify.clone() {
            self.spotify_load_art(&tr);
            self.spotify_load_artist(); // restored track → fill the rich artist pane
        }
    }

    /// Request the now-playing track's cover for the playback bar (skips a
    /// re-download when the URL is unchanged). Clears the old art immediately so
    /// the bar never shows a stale cover during the fetch.
    pub(crate) fn spotify_load_art(&mut self, track: &crate::spotify::api::Item) {
        if self.spov.sp_cover_url.as_deref() == track.image.as_deref()
            && self.spov.sp_cover.is_some()
        {
            return; // already showing this cover
        }
        self.spov.sp_cover = None;
        self.spov.sp_cover_url = track.image.clone();
        if let (Some(url), Some(tx)) = (track.image.as_ref(), self.workers.spotify_art.as_ref()) {
            let _ = tx.send(crate::spotify::artwork::ArtRequest {
                url: url.clone(),
                circle: false, // the now-playing cover stays square
            });
        }
    }

    /// A downloaded Spotify cover arrived — build its image protocol for whichever
    /// slot asked for this URL (the now-bar track cover or the artist-pane photo),
    /// unless that slot has since moved on.
    pub fn on_spotify_art(&mut self, res: crate::spotify::artwork::ArtResult) {
        let Some(p) = self.art.picker.as_ref() else {
            return;
        };
        if self.spov.sp_cover_url.as_deref() == Some(res.url.as_str()) {
            self.spov.sp_cover = Some(crate::ui::components::Cover::new(p, res.img));
            self.dirty = true;
        } else if self.spov.sp_artist_cover_url.as_deref() == Some(res.url.as_str()) {
            self.spov.sp_artist_cover = Some(crate::ui::components::Cover::new(p, res.img));
            self.dirty = true;
        }
    }

    /// Load the now-playing track's artist into the artist pane (photo + genres +
    /// follower count via the Web API, top tracks via librespot). De-duped by the
    /// artist uri; a no-op for tracks without a resolved artist (e.g. podcasts).
    pub(crate) fn spotify_load_artist(&mut self) {
        // podcasts have no artist — load the SHOW's metadata (publisher + "about")
        // for the pane instead. Handled first because an episode's `artist_uri` is
        // None, which would otherwise just clear the pane.
        if let Some(tr) = self.spov.now_spotify.clone()
            && Self::is_episode_uri(&tr.uri)
        {
            self.clear_spotify_artist();
            self.spotify_load_show_meta(tr.show_uri.clone());
            return;
        }
        self.spov.sp_show_meta = None; // leaving a podcast → drop its show "about"
        let uri = self
            .spov
            .now_spotify
            .as_ref()
            .and_then(|t| t.artist_uri.clone());
        let Some(uri) = uri else {
            self.clear_spotify_artist();
            return;
        };
        if self.spov.sp_artist_uri.as_deref() == Some(uri.as_str()) {
            return; // already loaded for this artist
        }
        self.clear_spotify_artist();
        self.spov.sp_artist_uri = Some(uri.clone());
        // details (photo / genres / followers) over the Web API
        if let (Some(tokens), Some(tx)) =
            (self.spotify.tokens.as_ref(), self.workers.spotify.as_ref())
        {
            let _ = tx.send(crate::spotify::api::SpRequest::Artist {
                uri: uri.clone(),
                token: tokens.access_token.clone(),
                key: ARTIST_PANE_KEY.into(),
            });
        }
        // top tracks over librespot — only if a session is already up (don't spawn
        // one just for the pane; it'll fill in once playback starts a session)
        if let Some(cmd) = &self.spov.session_cmd {
            let _ = cmd.send(crate::spotify::session::SessionCommand::FetchTracks {
                uri,
                artist: true,
                key: ARTIST_PANE_KEY.into(),
            });
        }
        // bio / formed / country come from the MusicBrainz+Wikipedia info worker
        // (Spotify has no bio API) — by artist name, like the local pane
        self.request_artist_info();
    }

    fn clear_spotify_artist(&mut self) {
        self.spov.sp_artist = None;
        self.spov.sp_artist_uri = None;
        self.spov.sp_artist_cover = None;
        self.spov.sp_artist_cover_url = None;
        self.spov.sp_artist_top.clear();
    }

    /// Fetch the now-playing episode's SHOW metadata (publisher + "about"
    /// description) over the Web API, for the podcast artist pane. De-duped by show
    /// uri; a no-op without a show uri or a connected worker.
    fn spotify_load_show_meta(&mut self, show_uri: Option<String>) {
        let Some(uri) = show_uri else {
            self.spov.sp_show_meta = None;
            return;
        };
        if self.spov.sp_show_meta.as_ref().map(|m| m.uri.as_str()) == Some(uri.as_str()) {
            return; // already loaded for this show
        }
        self.spov.sp_show_meta = None;
        if let (Some(tokens), Some(tx)) =
            (self.spotify.tokens.as_ref(), self.workers.spotify.as_ref())
        {
            let _ = tx.send(crate::spotify::api::SpRequest::ShowMeta {
                uri,
                token: tokens.access_token.clone(),
            });
        }
    }

    /// Whether the running librespot session can be reused as-is. It can only
    /// while the token it was built on is still valid: after the app sits idle
    /// (overnight / a few days) the cached token expires and that session goes
    /// stale — its connection can no longer resolve audio, so playback silently
    /// fails. Once the token is expired the session must be respawned with a fresh
    /// one. `pub(crate)` for the expiry-recovery test.
    pub(crate) fn spotify_session_reusable(&self) -> bool {
        self.spov.session_cmd.is_some()
            && self
                .spotify
                .tokens
                .as_ref()
                .is_some_and(|t| !t.is_expired())
    }

    /// Ensure a *usable* librespot session thread is running (spawned lazily on
    /// first play, using the current token) and the engine is pulling from its
    /// bridge. A stale session built on an expired token is dropped and respawned
    /// — `session::spawn` refreshes the token on its own thread and reports it back
    /// (`TokenRefreshed`), so the whole app adopts the fresh one.
    pub(crate) fn spotify_ensure_session(&mut self) -> bool {
        if self.spotify_session_reusable() {
            return true;
        }
        // A login/resume is refreshing the token — don't spawn a session now: it
        // would fire a second, racing refresh of the same token, and would be
        // dropped anyway when that resume completes (the Connected handler above).
        // The fresh token + a respawn arrive right after Connected.
        if self.spotify.auth_rx.is_some() {
            return false;
        }
        // No session, or a stale one — drop the old handle so its thread exits
        // (its command channel disconnects), then respawn fresh below.
        self.spov.session_cmd = None;
        self.spov.session_rx = None;
        let Some(tokens) = self.spotify.tokens.clone() else {
            return false;
        };
        let (cmd, rx, bridge) = crate::spotify::session::spawn(
            tokens,
            self.config.dir.clone(),
            self.config.spotify_bitrate,
        );
        // Keep the bridge handle but DON'T attach it to the engine here. The session
        // is also used for metadata-only fetches (artist page / playlist tracks), and
        // attaching the bridge then would hijack the shared audio output from local /
        // radio playback. The bridge is attached only when playback starts
        // (`spotify_attach_bridge`, from `spotify_begin`).
        self.spov.sp_bridge = Some(bridge);
        self.spov.session_cmd = Some(cmd);
        self.spov.session_rx = Some(rx);
        true
    }
}

/// Spotify view + connection state. Auth runs on a worker thread; `auth_rx`
/// carries its progress, drained by [`AppState::pump_spotify`]. Library/search
/// fields arrive in later phases.
#[derive(Default)]
pub struct Spotify {
    /// Connection/login state shown in the view.
    pub conn: crate::spotify::ConnState,
    /// The current token set once connected (used by the Web API + librespot).
    pub tokens: Option<crate::spotify::Tokens>,
    /// In-flight auth/resume event stream, if a login or resume is running.
    pub auth_rx: Option<crossbeam_channel::Receiver<crate::spotify::AuthEvent>>,
    /// When set (unix seconds), a transient reconnect is scheduled: the last resume
    /// failed to reach Spotify (network/rate-limit), so [`AppState::pump_spotify`]
    /// respawns it once this deadline passes. Cleared on connect or a fresh login.
    pub reconnect_at: Option<u64>,
    /// Consecutive transient reconnect failures, driving the back-off (5s → 60s cap)
    /// so a long outage doesn't retry in a tight loop. Reset on a successful connect.
    pub reconnect_attempts: u32,
    /// Spotify account id of the currently-connected account (from `/me`). Ties
    /// cached state to its owner so it's never applied to a different account.
    pub account_id: Option<String>,
    /// The account id whose state was restored from `session.json` at launch
    /// (one-shot: compared against the connected account, then cleared).
    pub restored_account: Option<String>,
    // ---- browse state (phase 3) ----
    /// Selected library section (left sidebar).
    pub section: crate::spotify::api::Section,
    /// Cursor in the Spotify QUEUE pane while it's focused (j/k moves it; ⏎ plays it).
    pub queue_sel: usize,
    /// The current list: the section's items, or search results.
    pub items: Vec<crate::spotify::api::Item>,
    /// Cursor within `items`.
    pub sel: usize,
    /// Cover-art grid (vs list) for the current Albums/Artists section — the `#`
    /// toggle. Set per-section on load (default on for Albums/Artists); persisted
    /// in `ViewState.spotify_grid`. `spotify_grid_active` gates where it applies.
    pub grid: bool,
    /// Last-rendered grid column count (the row stride for 2-D nav); set in render,
    /// read in `spotify_grid_move`. Interior-mutable so render can write it.
    pub cols: std::cell::Cell<usize>,
    /// Persisted top-row scroll offset of the cover-art grid — sticky viewport (see
    /// the local library's `LocalBrowse::row_off`). Set during render.
    pub row_off: std::cell::Cell<usize>,
    /// Persisted scroll offset of the Spotify item list (track table/rows + the
    /// mixed-kind browse list) — sticky viewport so clicking a visible row selects
    /// it in place instead of recentring under the cursor. Set during render.
    pub list_off: std::cell::Cell<usize>,
    /// Persisted scroll offset of the Spotify QUEUE pane — sticky, same reason. Set
    /// during render.
    pub queue_off: std::cell::Cell<usize>,
    /// Persisted horizontal scroll offset of the SELECTED release carousel (artist
    /// page + Home/Browse feed), plus its carousel identity (`car_key`) so the offset
    /// resets when the selection moves to another carousel — sticky horizontal scroll,
    /// mirroring `LocalBrowse::car_off`. Set during render.
    pub car_off: std::cell::Cell<usize>,
    pub car_key: std::cell::Cell<usize>,
    /// A cursor restored from a session, applied (clamped) to the first list that
    /// arrives after reconnecting, then cleared. `None` in normal operation.
    pub restore_sel: Option<usize>,
    /// A Web API request is in flight.
    pub loading: bool,
    /// Status line under the search box ("", "Loading…", "Nothing", error).
    pub note: String,
    /// Search box focused (typing edits `query`).
    pub searching: bool,
    /// Showing search results instead of the section list.
    pub in_search: bool,
    pub query: String,
    /// Latest in-flight request key (stale results are ignored).
    key: String,
    /// Drill-in breadcrumb (e.g. "◉ Discovery") when viewing a container's
    /// tracks; `None` at the top level (section/search).
    pub crumb: Option<String>,
    /// The container whose tracks are currently shown (the deepest drill-in), so
    /// a session can re-open it on reconnect. `None` at the section/search level.
    pub open_item: Option<crate::spotify::api::Item>,
    /// Load-more paging for a drilled-in flat browse grid (Podcast Charts /
    /// Categories): the browse page uri being paged (`None` = the current view isn't
    /// a pageable grid), the current items-per-shelf `browse_limit` (grown by
    /// [`BROWSE_PAGE_STEP`] on scroll), whether a grow is in flight, and whether the
    /// last grow returned nothing new (fully loaded → stop asking).
    pub browse_page: Option<String>,
    pub browse_limit: usize,
    pub browse_loading_more: bool,
    pub browse_exhausted: bool,
    /// URIs of the shows the user follows (from `/me/shows`), so browse rows/cards can
    /// mark an already-followed show with a ♥. Seeded from the Podcasts (Your Shows)
    /// load and kept live by follow/unfollow toggles.
    pub followed_shows: std::collections::HashSet<String>,
    /// A drill-in pending re-open once the initial section/search list lands after
    /// reconnecting, then cleared. Set from a same-session re-auth (the full in-memory
    /// `Item`) or a cross-launch restore (a minimal `Item` reconstructed from the
    /// persisted [`SpotifyDrill`](crate::session::SpotifyDrill) — URI + kind + name),
    /// so it re-opens even when the container isn't in the reloaded list. Re-applied
    /// by `spotify_apply_initial_restore`.
    pub restore_open: Option<crate::spotify::api::Item>,
    /// Back-stack of parent lists, restored on Esc (back out of a drill-in). The
    /// generic drill-in engine shared with the local library.
    nav: super::nav::NavStack<crate::spotify::api::Item, SpCtx>,
    /// The "add to / create / rename Spotify playlist" modal, when open (`None` =
    /// closed). Holds the pending track URIs + the account's writable playlists.
    pub pl_modal: Option<playlist::SpPlaylistModal>,
    /// A Spotify playlist awaiting unfollow ("delete") confirmation: `(uri, name)`.
    /// Inert (`None`) until the user asks; the confirm dialog clears it either way.
    pub pl_confirm_delete: Option<(String, String)>,
    /// A remove in flight: `(playlist_uri, track_uris, playlist_name)` — one or
    /// more tracks (the selection). Set when the user hits `d`; consumed when the
    /// fresh raw track list arrives (`spotify_apply_remove`), which does the
    /// completeness check + replace. See the module docs for why removal goes
    /// through a fresh-fetch-then-replace.
    pub pl_pending_remove: Option<(String, Vec<String>, String)>,
}

/// Spotify's per-frame restore context for the shared drill-in stack: the search
/// state + open container, saved on drill-in and reapplied on Back.
#[derive(Default)]
pub(crate) struct SpCtx {
    pub open_item: Option<crate::spotify::api::Item>,
    pub in_search: bool,
    pub query: String,
    pub note: String,
}

impl Spotify {
    /// Current drill-in depth (0 at the section/search level). Read by the mini
    /// layout to decide whether Back goes anywhere, and by tests asserting
    /// push/pop without reaching into the frames.
    pub(crate) fn nav_depth(&self) -> usize {
        self.nav.depth()
    }

    /// Whether a Forward step is available — a Back was taken and no fresh
    /// drill-in has truncated the branch.
    pub(crate) fn can_forward(&self) -> bool {
        self.nav.can_forward()
    }
}
