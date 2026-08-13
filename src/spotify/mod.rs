//! Spotify integration: a feature-rich, "feel at home" client for a personal
//! Premium account, using librespot for audio. This module owns auth + (later)
//! the Web API worker and the librespot session. Built in phases; Phase 2 is
//! the smooth in-TUI login.

pub mod api;
pub mod artwork;
pub mod auth;
pub mod logprobe;
/// Spotify's internal pathfinder GraphQL client (home/browse over librespot).
pub mod pathfinder;
pub mod session;
pub mod view_cache;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};

pub use auth::Tokens;

/// A running login/resume worker: its event stream plus the cancel flag the
/// worker polls while waiting on the browser.
///
/// Dropping it cancels the login — which is exactly what the app already does
/// when it clears `auth_rx`, so abandoning a sign-in now releases the loopback
/// port and the worker thread instead of leaking both. Combined with
/// [`auth::LOGIN_TIMEOUT`], a browser tab the user never authorizes can no longer
/// leave the app stuck reporting "a login is already running" forever.
pub struct AuthSession {
    rx: Receiver<AuthEvent>,
    cancel: Arc<AtomicBool>,
}

impl AuthSession {
    /// Next event from the worker, if any (the app polls this each loop).
    pub fn try_recv(&self) -> Result<AuthEvent, TryRecvError> {
        self.rx.try_recv()
    }
}

impl Drop for AuthSession {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
impl AuthSession {
    /// Wrap a bare receiver for tests that drive auth events by hand — there is
    /// no worker thread behind it, so the cancel flag has nothing to stop.
    pub(crate) fn for_test(rx: Receiver<AuthEvent>) -> Self {
        Self {
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }
}

/// Progress/result of a background login (or token resume), drained by the app.
#[derive(Debug, Clone)]
pub enum AuthEvent {
    /// Browser opened; we're waiting for the user to authorize. `url` is shown in
    /// the TUI too, in case the browser didn't open. `playback` marks the second
    /// leg (the keymaster sign-in that backs playback + browse), so the panel can
    /// say why a private client id needs two authorizations.
    Waiting { url: String, playback: bool },
    /// Logged in: a usable token + the account's id, display name, and premium
    /// flag. `account_id` ties cached state to this account (empty if a transient
    /// profile-fetch failure meant we couldn't read it). `audio_tokens` is the
    /// keymaster set the librespot session needs; `None` means playback + browse
    /// are not authorized yet (see [`auth::TokenKind::Audio`]).
    Connected {
        tokens: Tokens,
        audio_tokens: Option<Tokens>,
        account_id: String,
        name: String,
        premium: bool,
    },
    /// The playback/browse (keymaster) leg finished on its own — the self-heal
    /// path, which re-mints only that token without touching the Web login.
    AudioReady { tokens: Tokens },
    /// Something went wrong; `msg` is user-facing and actionable.
    Error { msg: String },
    /// A *transient* failure to reach Spotify (network down / rate-limited) while
    /// resuming — NOT an auth problem. The cached token is still good, so the app
    /// keeps it and retries automatically; `msg` explains why. Distinct from
    /// [`AuthEvent::Error`], which means "log in again".
    ConnLost { msg: String },
}

/// Connection state shown in the Spotify view.
#[derive(Debug, Clone, Default)]
pub enum ConnState {
    /// No token — show the "Log in with Spotify" panel.
    #[default]
    Disconnected,
    /// A login/resume is running; `url` is set once the browser step is reached.
    Connecting { url: Option<String> },
    /// Ready. `premium` gates playback (browsing works either way).
    Connected { name: String, premium: bool },
    /// Failed; the message guides the user to a fix (needs the user to act, e.g.
    /// re-login).
    Error { msg: String },
    /// Can't currently reach Spotify (a transient network/rate-limit blip while
    /// resuming). The token is kept and the app retries on its own; `msg` is the
    /// reason. Softer than [`ConnState::Error`] — no user action required.
    Reconnecting { msg: String },
}

/// One browser round-trip: bind the loopback listener, open the authorize URL for
/// `kind`, and exchange the returned code for tokens. Both login legs share it —
/// they differ only in the client id and scopes carried by `kind`.
fn browser_leg(
    kind: auth::TokenKind,
    tx: &Sender<AuthEvent>,
    cancel: &AtomicBool,
) -> Result<Tokens, String> {
    let listener = auth::bind_listener().map_err(|e| {
        format!(
            "Couldn't start the local login server on 127.0.0.1:{} ({e}). \
Another login may be in progress — wait a moment and retry.",
            auth::REDIRECT_PORT
        )
    })?;
    let (url, verifier, state) = auth::authorize_url(kind);
    // best-effort browser open; the URL is also shown in the TUI
    let _ = webbrowser::open(&url);
    let _ = tx.send(AuthEvent::Waiting {
        url,
        playback: kind == auth::TokenKind::Audio,
    });
    let deadline = std::time::Instant::now() + auth::LOGIN_TIMEOUT;
    let code = auth::wait_for_code(&listener, &state, deadline, cancel)?;
    auth::exchange_code(kind, &code, &verifier)
}

/// Spawn a login/resume worker: builds the event channel + cancel flag, hands
/// both to `job`, and returns the handle the app polls (and drops to cancel).
fn spawn_auth<F>(name: &str, job: F) -> AuthSession
where
    F: FnOnce(Sender<AuthEvent>, Arc<AtomicBool>) + Send + 'static,
{
    let (tx, rx) = unbounded();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let _ = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || job(tx, worker_cancel));
    AuthSession { rx, cancel }
}

/// Run the full interactive login on a worker thread; the app drains the events.
///
/// With a private client id this is TWO authorizations: the Web one (library,
/// search, playlists) and the keymaster one that librespot and browse require.
/// Without a private id the single Web token is already keymaster-minted and
/// serves both, so the second leg is skipped.
pub fn spawn_login(dir: PathBuf) -> AuthSession {
    spawn_auth("lyrfin-spotify-login", move |tx, cancel| {
        let tokens = match browser_leg(auth::TokenKind::Web, &tx, &cancel) {
            Ok(t) => t,
            Err(msg) => {
                let _ = tx.send(AuthEvent::Error { msg });
                return;
            }
        };
        // The audio leg is best-effort: a failure here still leaves a working
        // Web login (search + library), so report it as a playback-only
        // problem rather than failing the whole sign-in.
        let audio = if auth::has_custom_client_id() {
            match browser_leg(auth::TokenKind::Audio, &tx, &cancel) {
                Ok(t) => {
                    t.save(&dir, auth::TokenKind::Audio);
                    Some(t)
                }
                Err(msg) => {
                    log::warn!(target: "lyrfin::spotify", "playback sign-in failed: {msg}");
                    None
                }
            }
        } else {
            Some(tokens.clone())
        };
        finish(tokens, audio, &dir, &tx);
    })
}

/// Mint ONLY the playback/browse (keymaster) token, leaving the Web login alone.
/// This is the self-heal path: the librespot session reports that Spotify refused
/// its stored credentials, and the app re-authorizes just that leg.
pub fn spawn_audio_login(dir: PathBuf) -> AuthSession {
    spawn_auth(
        "lyrfin-spotify-audio-login",
        move |tx, cancel| match browser_leg(auth::TokenKind::Audio, &tx, &cancel) {
            Ok(tokens) => {
                tokens.save(&dir, auth::TokenKind::Audio);
                let _ = tx.send(AuthEvent::AudioReady { tokens });
            }
            Err(msg) => {
                let _ = tx.send(AuthEvent::Error { msg });
            }
        },
    )
}

/// The "session expired" recovery message. When the refresh was rejected as
/// `invalid_client` AND no private client id is set, the shared keymaster app was
/// refused for the Web API token endpoint — re-login would just hit the same wall,
/// so point at configuring a Client ID. Otherwise the generic re-login hint.
fn session_expired_msg(err: &str, has_custom_client: bool) -> String {
    if err.contains("invalid_client") && !has_custom_client {
        format!(
            "Session expired ({err}). The shared Spotify app was rejected — set your own \
             Client ID (press ; → Spotify), then log in."
        )
    } else {
        format!("Session expired ({err}). Press ⏎ to log in again.")
    }
}

/// Resume from a cached token on a worker thread (refresh if near expiry, then
/// confirm via the profile). No browser needed.
pub fn spawn_resume(dir: PathBuf, tokens: Tokens) -> AuthSession {
    // A resume never waits on the browser, so it has nothing to cancel — it only
    // makes bounded (20s) token/profile calls and then reports.
    spawn_auth("lyrfin-spotify-resume", move |tx, _cancel| {
        {
            let toks = if tokens.is_expired() {
                match auth::refresh(auth::TokenKind::Web, &tokens.refresh_token) {
                    Ok(t) => t,
                    // A transient network/rate-limit blip (e.g. resuming before the
                    // connection is back after sleep) is NOT an expired session: the
                    // cached token is still good. Report it as a recoverable
                    // ConnLost so the app keeps the token and retries — don't tell
                    // the user to log in again.
                    Err(msg) if auth::is_transient(&msg) => {
                        let _ = tx.send(AuthEvent::ConnLost { msg });
                        return;
                    }
                    Err(msg) => {
                        // a real rejection (401 / invalid_client) → must log in again
                        let _ = tx.send(AuthEvent::Error {
                            msg: session_expired_msg(&msg, auth::has_custom_client_id()),
                        });
                        return;
                    }
                }
            } else {
                tokens
            };
            // Resume the playback/browse token alongside it: it has its own
            // lifetime and its own client id, so it refreshes independently. A
            // failure here only costs playback + browse, never the Web session.
            let audio = auth::session_tokens(&dir, &toks).and_then(|a| {
                if !a.is_expired() {
                    return Some(a);
                }
                match auth::refresh(auth::TokenKind::Audio, &a.refresh_token) {
                    Ok(fresh) => {
                        fresh.save(&dir, auth::TokenKind::Audio);
                        Some(fresh)
                    }
                    Err(msg) => {
                        log::warn!(target: "lyrfin::spotify", "playback token refresh failed: {msg}");
                        None
                    }
                }
            });
            finish(toks, audio, &dir, &tx);
        }
    })
}

fn finish(
    tokens: Tokens,
    audio_tokens: Option<Tokens>,
    dir: &std::path::Path,
    tx: &Sender<AuthEvent>,
) {
    // The token is valid here (code exchange / refresh already succeeded), so
    // save it first — it's what everything else uses.
    tokens.save(dir, auth::TokenKind::Web);
    // The profile is just a greeting + premium hint. A real 401 means the token
    // is bad (re-login); anything else (a transient 429 / network blip) must NOT
    // block the connection — proceed with a fallback name.
    match auth::fetch_profile(&tokens.access_token) {
        Ok((account_id, name, premium)) => {
            let _ = tx.send(AuthEvent::Connected {
                tokens,
                audio_tokens,
                account_id,
                name,
                premium,
            });
        }
        // A 401 (token rejected) or a 403 "not registered for this app" means the
        // Web API is unusable for this account — surface it clearly rather than
        // proceeding to a "Connected" state that 403s on every browse. (Audio via
        // librespot is independent and still works.)
        Err(msg)
            if msg.contains("rejected the login")
                || msg == crate::spotify::api::NOT_REGISTERED_MSG =>
        {
            let _ = tx.send(AuthEvent::Error { msg });
        }
        Err(_) => {
            // a transient 429 / network blip must NOT block the connection; we just
            // don't know the account id, so leave it empty (skips the identity check)
            let _ = tx.send(AuthEvent::Connected {
                tokens,
                audio_tokens,
                account_id: String::new(),
                name: "Spotify".into(),
                premium: true,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::session_expired_msg;

    #[test]
    fn session_expired_msg_points_to_client_id_when_the_shared_app_is_rejected() {
        // invalid_client + no private client id → tell them to set a Client ID
        let m = session_expired_msg("Spotify error 400: invalid_client", false);
        assert!(m.contains("Client ID") && m.contains("; → Spotify"));
        assert!(
            !m.contains("Press ⏎"),
            "re-login alone won't help on the shared app"
        );
        // a user WITH a private client id gets the plain re-login hint
        let m = session_expired_msg("Spotify error 400: invalid_client", true);
        assert!(m.contains("Press ⏎ to log in again") && !m.contains("Client ID"));
        // any other failure → the plain re-login hint regardless of client id
        let m = session_expired_msg("network blip", false);
        assert!(m.contains("Press ⏎ to log in again"));
    }
}
