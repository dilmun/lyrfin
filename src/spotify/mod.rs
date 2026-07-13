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

use crossbeam_channel::{Receiver, Sender, unbounded};

pub use auth::Tokens;

/// Progress/result of a background login (or token resume), drained by the app.
#[derive(Debug, Clone)]
pub enum AuthEvent {
    /// Browser opened; we're waiting for the user to authorize. `url` is shown in
    /// the TUI too, in case the browser didn't open.
    Waiting { url: String },
    /// Logged in: a usable token + the account's id, display name, and premium
    /// flag. `account_id` ties cached state to this account (empty if a transient
    /// profile-fetch failure meant we couldn't read it).
    Connected {
        tokens: Tokens,
        account_id: String,
        name: String,
        premium: bool,
    },
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

/// Run the full interactive login on a worker thread; the app drains the events.
pub fn spawn_login(dir: PathBuf) -> Receiver<AuthEvent> {
    let (tx, rx) = unbounded();
    let _ = std::thread::Builder::new()
        .name("lyrfin-spotify-login".into())
        .spawn(move || {
            let listener = match auth::bind_listener() {
                Ok(l) => l,
                Err(e) => {
                    let _ = tx.send(AuthEvent::Error {
                        msg: format!(
                            "Couldn't start the local login server on 127.0.0.1:{} ({e}). \
Another login may be in progress — wait a moment and retry.",
                            auth::REDIRECT_PORT
                        ),
                    });
                    return;
                }
            };
            let (url, verifier, state) = auth::authorize_url();
            // best-effort browser open; the URL is also shown in the TUI
            let _ = webbrowser::open(&url);
            let _ = tx.send(AuthEvent::Waiting { url });
            let code = match auth::wait_for_code(&listener, &state) {
                Ok(c) => c,
                Err(msg) => {
                    let _ = tx.send(AuthEvent::Error { msg });
                    return;
                }
            };
            match auth::exchange_code(&code, &verifier) {
                Ok(tokens) => finish(tokens, &dir, &tx),
                Err(msg) => {
                    let _ = tx.send(AuthEvent::Error { msg });
                }
            }
        });
    rx
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
pub fn spawn_resume(dir: PathBuf, tokens: Tokens) -> Receiver<AuthEvent> {
    let (tx, rx) = unbounded();
    let _ = std::thread::Builder::new()
        .name("lyrfin-spotify-resume".into())
        .spawn(move || {
            let toks = if tokens.is_expired() {
                match auth::refresh(&tokens.refresh_token) {
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
            finish(toks, &dir, &tx);
        });
    rx
}

fn finish(tokens: Tokens, dir: &std::path::Path, tx: &Sender<AuthEvent>) {
    // The token is valid here (code exchange / refresh already succeeded), so
    // save it first — it's what everything else uses.
    tokens.save(dir);
    // The profile is just a greeting + premium hint. A real 401 means the token
    // is bad (re-login); anything else (a transient 429 / network blip) must NOT
    // block the connection — proceed with a fallback name.
    match auth::fetch_profile(&tokens.access_token) {
        Ok((account_id, name, premium)) => {
            let _ = tx.send(AuthEvent::Connected {
                tokens,
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
