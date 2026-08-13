//! Spotify OAuth (Authorization Code + PKCE) — a browser login yielding the
//! token(s) the Web API and librespot run on.
//!
//! Uses Spotify's public "desktop" (keymaster) client id + a `127.0.0.1` loopback
//! redirect (the exact pair librespot itself uses), so the user never has to
//! register their own Spotify app. Registering one is still worthwhile for the
//! Web API's quota — and doing so splits the login in two, because librespot can
//! only authenticate on a keymaster-minted token: see [`TokenKind`], which owns
//! that policy. The blocking pieces (loopback wait, token exchange) run on a
//! worker thread; the UI stays responsive (see `spotify::spawn_login`).

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Spotify's public desktop client id (same one librespot uses). Shared across
/// many apps, so its Web API quota is easily exhausted (429). Used unless the
/// user configures their own private client id.
pub const KEYMASTER_CLIENT_ID: &str = "65b708073fc0480ea92a077233ca87bd";

static CLIENT_ID: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// Set the active client id (from config at startup, or live when the user
/// enters their own). Empty → keymaster default. Takes effect immediately.
pub fn set_client_id(id: String) {
    log::info!(target: "lyrfin::spotify", "client_id set: custom={}", !id.is_empty());
    *CLIENT_ID.lock().unwrap() = id;
}

/// Whether a private (user) Web API client id is configured — vs the shared
/// keymaster fallback, which Spotify can reject for the Web API token endpoint.
pub fn has_custom_client_id() -> bool {
    !CLIENT_ID.lock().unwrap().is_empty()
}

/// The active client id: the configured private one, else the shared keymaster.
pub fn client_id() -> String {
    let id = CLIENT_ID.lock().unwrap();
    if id.is_empty() {
        KEYMASTER_CLIENT_ID.to_string()
    } else {
        id.clone()
    }
}

/// The custom client id lives in its OWN file (auth setup, like the token) instead
/// of `config.toml`, so a config rewrite / parse-error / missing-file fall-back-to-
/// defaults can never wipe it — the recurring "Client ID keeps getting wiped" bug.
/// This file is the source of truth; `config.toml` keeps only a display mirror.
fn client_id_path(dir: &Path) -> PathBuf {
    dir.join("spotify_client_id")
}

/// Read the persisted custom client id (`None` when unset / blank).
pub fn load_persisted_client_id(dir: &Path) -> Option<String> {
    std::fs::read_to_string(client_id_path(dir))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Persist the custom client id to its own file (atomic temp+rename) AND apply it
/// live. An empty id removes the file → revert to the shared keymaster id.
pub fn persist_client_id(dir: &Path, id: &str) {
    let id = id.trim();
    let path = client_id_path(dir);
    if id.is_empty() {
        let _ = std::fs::remove_file(&path);
    } else {
        // owner-only, like the tokens: it identifies the user's own Spotify app
        let _ = crate::atomicfile::write_private(&path, id.as_bytes());
    }
    set_client_id(id.to_string());
}
/// Loopback redirect port (matches librespot's `http://127.0.0.1:<port>/login`).
pub const REDIRECT_PORT: u16 = 8898;
const AUTH_URL: &str = "https://accounts.spotify.com/authorize";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";

/// Everything we need: `streaming` (librespot) + library/playback/playlists.
/// `playlist-modify-private`/`-public` enable the create/add/rename/remove/unfollow
/// writes (the Spotify view's playlist management); `user-follow-modify` enables
/// following/unfollowing artists (`user-library-modify` already covers saving shows).
/// Adding any scope means a returning user must re-login once so the new consent is
/// granted.
const SCOPES: &str = "streaming user-read-email user-read-private \
user-library-read user-library-modify playlist-read-private \
playlist-read-collaborative playlist-modify-private playlist-modify-public \
user-follow-read user-follow-modify user-read-playback-state \
user-modify-playback-state user-read-currently-playing \
user-read-recently-played user-top-read";

pub fn redirect_uri() -> String {
    format!("http://127.0.0.1:{REDIRECT_PORT}/login")
}

/// Which login a token set belongs to. lyrfin holds two, because one token
/// cannot serve both jobs:
///
/// - [`TokenKind::Web`] talks to the Web API and is minted with the user's own
///   client id when configured — the shared keymaster id has a global quota that
///   is routinely exhausted (429).
/// - [`TokenKind::Audio`] backs the librespot session (playback) and the
///   pathfinder browse feed, and is ALWAYS minted with keymaster: librespot's
///   login5 exchange presents the session's client id, and Spotify rejects
///   stored credentials minted by a different one (`INVALID_CREDENTIALS`).
///   Keymaster is also the only id the client-token endpoint accepts at all.
///
/// With no custom client id the Web token is already keymaster-minted, so it
/// doubles as the audio token and only one login is needed (see
/// [`session_tokens`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Web,
    Audio,
}

impl TokenKind {
    /// The file this token set persists to, inside the config dir.
    fn file(self) -> &'static str {
        match self {
            Self::Web => "spotify_token.json",
            Self::Audio => "spotify_audio_token.json",
        }
    }

    /// The client id a token of this kind must be minted (and refreshed) with.
    /// Presenting a refresh token to a different client id is rejected, so this
    /// is the single source of truth for both legs of the OAuth flow.
    pub fn client_id(self) -> String {
        match self {
            Self::Web => client_id(),
            Self::Audio => KEYMASTER_CLIENT_ID.to_string(),
        }
    }

    /// Scopes requested for this login. The audio token only ever authenticates
    /// librespot, so it asks for `streaming` alone — a narrower consent screen,
    /// and everything else already comes from the Web token.
    fn scopes(self) -> &'static str {
        match self {
            Self::Web => SCOPES,
            Self::Audio => "streaming",
        }
    }
}

/// The token set the librespot session (playback + browse) must use, given the
/// cached `web` token. Without a custom client id the Web token is itself
/// keymaster-minted and serves both roles; with one, the separately-minted audio
/// token is required and its absence means playback/browse can't work yet.
pub fn session_tokens(dir: &Path, web: &Tokens) -> Option<Tokens> {
    if has_custom_client_id() {
        Tokens::load(dir, TokenKind::Audio)
    } else {
        Some(web.clone())
    }
}

/// Persist a refreshed *session* token back to the file it came from: the audio
/// file when a private client id splits the two logins, else the Web file that
/// doubles as the session token. The mirror of [`session_tokens`] — writing to
/// the wrong one would strand the other holding a refresh token Spotify has
/// already rotated away, surfacing as "session expired" on the next launch.
pub fn save_session_tokens(dir: &Path, tokens: &Tokens) {
    tokens.save(
        dir,
        if has_custom_client_id() {
            TokenKind::Audio
        } else {
            TokenKind::Web
        },
    );
}

/// Persisted token set (`spotify_token.json` / `spotify_audio_token.json`),
/// refreshed when near expiry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Unix seconds at which the access token expires.
    pub expires_at: u64,
    #[serde(default)]
    pub scopes: String,
}

impl Tokens {
    /// Expired (or within a 30s skew window)?
    pub fn is_expired(&self) -> bool {
        now_unix() + 30 >= self.expires_at
    }
    fn path(dir: &Path, kind: TokenKind) -> PathBuf {
        dir.join(kind.file())
    }
    pub fn load(dir: &Path, kind: TokenKind) -> Option<Tokens> {
        std::fs::read_to_string(Self::path(dir, kind))
            .ok()
            .and_then(|t| serde_json::from_str::<Tokens>(&t).ok())
            .filter(|t| !t.access_token.is_empty() && !t.refresh_token.is_empty())
    }
    /// Atomic (so a torn write can't leave invalid JSON that `load` rejects — the
    /// token silently lost on the next start) and owner-only: this file holds a
    /// long-lived refresh token, and the default mode would leave it readable by
    /// every user and process on the machine.
    pub fn save(&self, dir: &Path, kind: TokenKind) {
        if let Ok(j) = serde_json::to_string_pretty(self) {
            let _ = crate::atomicfile::write_private(&Self::path(dir, kind), j.as_bytes());
        }
    }
    /// Forget every cached token. Logging out has to drop BOTH sets — leaving the
    /// audio token behind would keep librespot able to stream on the old account.
    pub fn clear(dir: &Path) {
        for kind in [TokenKind::Web, TokenKind::Audio] {
            let _ = std::fs::remove_file(Self::path(dir, kind));
        }
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A url-safe random string from `n` bytes of OS entropy (PKCE verifier / state).
fn random_token(n: usize) -> String {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).expect("OS RNG unavailable");
    b64url(&buf)
}

/// Minimal percent-encoding for a query value (keeps unreserved chars).
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Decode a percent-encoded query value (`+` is a space).
fn dec(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let h = |c: u8| (c as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (h(b[i + 1]), h(b[i + 2])) {
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                    continue;
                }
                out.push(b[i]);
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Build the authorize URL for one login leg; returns
/// `(url, pkce_verifier, csrf_state)`. The client id and scopes come from `kind`,
/// so the audio leg always authorizes against keymaster.
pub fn authorize_url(kind: TokenKind) -> (String, String, String) {
    let verifier = random_token(48);
    let challenge = b64url(&Sha256::digest(verifier.as_bytes())[..]);
    let state = random_token(16);
    let url = format!(
        "{AUTH_URL}?response_type=code&client_id={cid}&redirect_uri={redir}\
&code_challenge_method=S256&code_challenge={chal}&state={state}&scope={scope}",
        cid = kind.client_id(),
        redir = enc(&redirect_uri()),
        chal = challenge,
        state = enc(&state),
        scope = enc(kind.scopes()),
    );
    (url, verifier, state)
}

/// Bind the loopback redirect listener (so the caller can report "ready" before
/// opening the browser). A bind error usually means a stale login is running.
pub fn bind_listener() -> std::io::Result<TcpListener> {
    TcpListener::bind(("127.0.0.1", REDIRECT_PORT))
}

/// How long a browser sign-in may stay open before it gives up. Generous enough
/// for a password + 2FA on a slow phone, bounded so an abandoned tab can't pin
/// the loopback port — and the app's "a login is running" gate — for the rest of
/// the run (which left playback stuck on "Connecting…" with no way to retry).
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(180);

/// The user-facing timeout message, matched by the app to keep the cached token
/// (a timeout says nothing about whether the token is still good).
pub const LOGIN_TIMEOUT_MSG: &str =
    "Spotify sign-in timed out — the browser tab was never authorized. Press ⏎ to try again.";

/// How long an accepted connection has to send its request line. The browser
/// also *preconnects* to this port (a socket opened with no request), and a
/// blocking read on one of those would hang the whole login — the redirect that
/// follows on another socket would never be read.
const CALLBACK_READ_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the accept loop re-checks the deadline and the cancel flag.
const ACCEPT_POLL: Duration = Duration::from_millis(100);

/// Block until Spotify redirects back, validate `state`, and return the code.
/// Always writes a friendly page to the browser tab.
///
/// Requests that carry neither `code` nor `error` are answered and ignored rather
/// than treated as the callback: the browser also asks this port for things we
/// never sent it — most reliably `/favicon.ico` for the success page — and a login
/// that consumed the first connection whatever it was would fail whenever one of
/// those arrived first. That is easy to hit with two logins back to back, where
/// the previous page's favicon request can land on this listener.
///
/// Bounded three ways, because every one of them is a real browser behaviour:
/// `deadline` caps the whole wait (an abandoned tab), `cancel` lets the app drop
/// the login (see [`crate::spotify::AuthSession`]), and each accepted socket gets
/// a read timeout (a preconnect that never sends a request).
pub fn wait_for_code(
    listener: &TcpListener,
    expect_state: &str,
    deadline: std::time::Instant,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<String, String> {
    use std::sync::atomic::Ordering;
    // Non-blocking accept so the deadline/cancel checks below actually run; the
    // accepted socket is put back into blocking mode (with a read timeout) since
    // the request line is read synchronously.
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("couldn't watch the login port ({e})"))?;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("login cancelled".into());
        }
        if std::time::Instant::now() >= deadline {
            return Err(LOGIN_TIMEOUT_MSG.into());
        }
        let mut stream = match listener.accept() {
            Ok((s, _)) => s,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(ACCEPT_POLL);
                continue;
            }
            // a failed accept says nothing about the login; keep waiting for the
            // real redirect rather than failing the whole sign-in
            Err(_) => continue,
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(CALLBACK_READ_TIMEOUT));
        let mut line = String::new();
        if BufReader::new(&stream).read_line(&mut line).is_err() {
            continue; // half-open, or a preconnect that never spoke; wait for the real one
        }
        // request line: `GET /login?code=...&state=... HTTP/1.1`
        let path = line.split_whitespace().nth(1).unwrap_or("");
        let query = path.split_once('?').map(|(_, q)| q).unwrap_or("");
        let (mut code, mut state, mut err) = (None, None, None);
        for pair in query.split('&') {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            match k {
                "code" => code = Some(dec(v)),
                "state" => state = Some(dec(v)),
                "error" => err = Some(dec(v)),
                _ => {}
            }
        }
        if code.is_none() && err.is_none() {
            // not the callback (favicon, prefetch, a stray tab) — dismiss it and
            // keep listening, or the real redirect is never read
            let _ = stream.write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n");
            continue;
        }
        let body = "<!doctype html><html><body style=\"font-family:system-ui,sans-serif;\
background:#16181C;color:#F2F3F6;text-align:center;padding-top:80px\">\
<h2>✓ lyrfin is connected to Spotify</h2><p>You can close this tab and return to your terminal.</p>\
</body></html>";
        let _ = stream.write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .as_bytes(),
        );
        if let Some(e) = err {
            return Err(format!("Spotify authorization was denied ({e})"));
        }
        if state.as_deref() != Some(expect_state) {
            return Err("login state mismatch (possible CSRF) — please try again".into());
        }
        return code
            .filter(|c| !c.is_empty())
            .ok_or_else(|| "no authorization code was returned".into());
    }
}

#[derive(Deserialize)]
struct TokenResp {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_in: u64,
    #[serde(default)]
    scope: String,
}

fn to_tokens(r: TokenResp, fallback_refresh: &str) -> Tokens {
    Tokens {
        access_token: r.access_token,
        refresh_token: if r.refresh_token.is_empty() {
            fallback_refresh.to_string()
        } else {
            r.refresh_token
        },
        expires_at: now_unix() + if r.expires_in > 0 { r.expires_in } else { 3600 },
        scopes: r.scope,
    }
}

/// Exchange an authorization `code` (+ PKCE verifier) for tokens. `kind` must
/// match the leg that produced the code — Spotify ties the code to the client id
/// that requested it.
pub fn exchange_code(kind: TokenKind, code: &str, verifier: &str) -> Result<Tokens, String> {
    let redir = redirect_uri();
    let cid = kind.client_id();
    let mut resp = token_agent()
        .post(TOKEN_URL)
        .send_form([
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redir.as_str()),
            ("client_id", cid.as_str()),
            ("code_verifier", verifier),
        ])
        .map_err(token_transport)?;
    if !resp.status().is_success() {
        return Err(token_err(&mut resp));
    }
    let tr: TokenResp = resp
        .body_mut()
        .read_json()
        .map_err(|e| format!("unexpected token response: {e}"))?;
    Ok(to_tokens(tr, ""))
}

/// Serializes refreshes and caches the most recent (consumed refresh token →
/// result), so a single-use PKCE refresh token is never presented to Spotify
/// twice. Both refresh paths (the Web-API resume and the librespot session) can
/// fire concurrently once the access token expires; Spotify rotates/revokes the
/// refresh token on first use, and a second presentation of the same token can
/// revoke the whole token family (permanent `invalid_grant`).
static REFRESH_GUARD: std::sync::Mutex<Option<(String, Tokens)>> = std::sync::Mutex::new(None);

/// Refresh an access token. Spotify may or may not return a new refresh token;
/// the old one is kept if not. Concurrent callers presenting the SAME refresh
/// token are collapsed onto one network call (see [`REFRESH_GUARD`]).
pub fn refresh(kind: TokenKind, refresh_token: &str) -> Result<Tokens, String> {
    // Hold the lock across the network call: refreshes are rare and run only on
    // worker threads, so serializing them is cheap and is what prevents the race.
    let mut guard = REFRESH_GUARD.lock().unwrap_or_else(|e| e.into_inner());
    // a concurrent / just-prior refresh already consumed this exact token and its
    // result is still valid → reuse it instead of presenting the consumed token again
    if let Some((consumed, fresh)) = guard.as_ref()
        && consumed == refresh_token
        && !fresh.is_expired()
    {
        return Ok(fresh.clone());
    }
    let fresh = refresh_uncached(kind, refresh_token)?;
    *guard = Some((refresh_token.to_string(), fresh.clone()));
    Ok(fresh)
}

/// The actual token endpoint round-trip (no serialization). Call via [`refresh`].
fn refresh_uncached(kind: TokenKind, refresh_token: &str) -> Result<Tokens, String> {
    let cid = kind.client_id();
    log::info!(
        target: "lyrfin::spotify",
        "token refresh: kind={kind:?} client_id custom={}",
        kind == TokenKind::Web && has_custom_client_id()
    );
    // bounded (token_agent's 20s global timeout): refresh runs under REFRESH_GUARD,
    // so a hung request must not pin the lock and stall every other refresh.
    let mut resp = token_agent()
        .post(TOKEN_URL)
        .send_form([
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", cid.as_str()),
        ])
        .map_err(token_transport)
        .inspect_err(|e| log::warn!(target: "lyrfin::spotify", "token refresh FAILED: {e}"))?;
    if !resp.status().is_success() {
        let msg = token_err(&mut resp);
        log::warn!(target: "lyrfin::spotify", "token refresh FAILED: {msg}");
        return Err(msg);
    }
    let tr: TokenResp = resp
        .body_mut()
        .read_json()
        .map_err(|e| format!("unexpected refresh response: {e}"))?;
    let toks = to_tokens(tr, refresh_token);
    log::info!(
        target: "lyrfin::spotify",
        "token refresh: ok (rotated={}, expires_at={})",
        toks.refresh_token != refresh_token,
        toks.expires_at
    );
    Ok(toks)
}

/// `GET /me` → (display name, is_premium). Cosmetic (greeting + premium note);
/// retries briefly on a 429 rate-limit (respecting Retry-After, capped) so a
/// transient limit right after login doesn't matter.
/// Returns `(account_id, display_name, premium)`. The stable `id` ties cached
/// playback/browse state to the account it belongs to, so it's never applied to a
/// different account on the next launch.
pub fn fetch_profile(access_token: &str) -> Result<(String, String, bool), String> {
    let mut attempt = 0;
    let agent = token_agent();
    let mut resp = loop {
        let mut r = agent
            .get("https://api.spotify.com/v1/me")
            .header("Authorization", &format!("Bearer {access_token}"))
            .call()
            .map_err(token_transport)?;
        match r.status().as_u16() {
            200..=299 => break r,
            429 if attempt < 2 => {
                let wait = r
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(1)
                    .clamp(1, 5);
                std::thread::sleep(std::time::Duration::from_secs(wait));
                attempt += 1;
            }
            _ => return Err(token_err(&mut r)),
        }
    };
    let v: serde_json::Value = resp.body_mut().read_json().map_err(|e| e.to_string())?;
    let id = v
        .get("id")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();
    let name = v
        .get("display_name")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| v.get("id").and_then(|x| x.as_str()))
        .unwrap_or("Spotify user")
        .to_string();
    let product = v.get("product").and_then(|x| x.as_str());
    let premium = product == Some("premium");
    log::info!(target: "lyrfin::spotify", "profile: product={product:?} premium={premium}");
    Ok((id, name, premium))
}

/// Token/profile agent: `http_status_as_error(false)` keeps 4xx/5xx as Ok(response)
/// so [`token_err`] can read Spotify's error body (e.g. the dev-mode "not
/// registered" 403). A 20s global cap bounds a hung refresh (see `refresh_uncached`).
fn token_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .http_status_as_error(false)
        .build()
        .into()
}

/// A helpful message from a non-2xx token/profile response (reads the error body).
fn token_err(resp: &mut ureq::http::Response<ureq::Body>) -> String {
    match resp.status().as_u16() {
        401 => "Spotify rejected the login (token invalid or expired)".into(),
        429 => {
            "Spotify is rate-limiting right now (too many recent requests) — wait a moment".into()
        }
        c => {
            let body = resp.body_mut().read_to_string().unwrap_or_default();
            // 403 "not registered for this application" → this account isn't on the
            // dev app's allowlist (same actionable guidance the Web API path gives).
            if c == 403 && body.to_lowercase().contains("not registered") {
                return crate::spotify::api::NOT_REGISTERED_MSG.into();
            }
            let snippet: String = body.chars().take(160).collect();
            format!("Spotify error {c}: {snippet}")
        }
    }
}

/// Transport (network) failure — the request never reached a response.
fn token_transport(e: ureq::Error) -> String {
    format!("can't reach Spotify ({e}) — check your connection/VPN")
}

/// Whether a [`token_err`] string is a *transient* reach-Spotify failure (network
/// down, VPN, a 429 rate-limit) rather than a real auth rejection. Transient ones
/// are worth retrying automatically with the same cached token; auth rejections
/// (401 / invalid_client / not-registered) are not — they need the user to act.
/// Kept next to `token_err` so the two never drift apart.
pub fn is_transient(msg: &str) -> bool {
    msg.contains("can't reach Spotify") || msg.contains("rate-limiting")
}

/// Serializes tests that touch the process-global client id ([`CLIENT_ID`]).
/// cargo runs tests as parallel threads in ONE process, so a test that sets the
/// id would otherwise race any test reading it. Every test that reads or writes
/// it — here and in the snapshot suite — takes this first.
#[cfg(test)]
pub(crate) static CLIENT_ID_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    /// No cancellation requested — the common case for the wait tests.
    static NOT_CANCELLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// A deadline far enough out that the test's own progress decides the outcome.
    fn never() -> std::time::Instant {
        std::time::Instant::now() + Duration::from_secs(30)
    }

    /// Take [`CLIENT_ID_TEST_LOCK`], ignoring poisoning: a panic in one client-id
    /// test must not cascade into "everything else fails too".
    fn client_id_guard() -> std::sync::MutexGuard<'static, ()> {
        CLIENT_ID_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn authorize_url_has_pkce_and_scopes() {
        let _guard = client_id_guard();
        let (url, verifier, state) = authorize_url(TokenKind::Web);
        assert!(url.starts_with(AUTH_URL));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&format!("client_id={}", client_id())));
        assert!(url.contains("scope=streaming")); // first scope, url-encoded spaces after
        assert!(url.contains(&format!("state={state}")));
        // the challenge is the b64url sha256 of the verifier
        let expect = b64url(&Sha256::digest(verifier.as_bytes())[..]);
        assert!(url.contains(&format!("code_challenge={expect}")));
        assert!(verifier.len() >= 43, "PKCE verifier must be >= 43 chars");
    }

    /// The audio leg must authorize against keymaster whatever the user's Web
    /// client id is: librespot's login5 exchange only accepts stored credentials
    /// minted by the client id the session itself presents, and keymaster is the
    /// only id the client-token endpoint accepts. Getting this wrong is silent —
    /// the login succeeds and only playback + browse die.
    #[test]
    fn audio_leg_always_authorizes_against_keymaster() {
        let _guard = client_id_guard();
        set_client_id("a-private-app-id".into());
        let (url, _, _) = authorize_url(TokenKind::Audio);
        assert!(
            url.contains(&format!("client_id={KEYMASTER_CLIENT_ID}")),
            "audio leg must use keymaster, got: {url}"
        );
        assert_eq!(TokenKind::Audio.client_id(), KEYMASTER_CLIENT_ID);
        // …while the Web leg keeps the user's own id (its own API quota)
        let (web, _, _) = authorize_url(TokenKind::Web);
        assert!(web.contains("client_id=a-private-app-id"));
        set_client_id(String::new()); // don't leak into other tests
    }

    /// With no private client id there is only ONE login: the Web token is already
    /// keymaster-minted, so it doubles as the session token and the user is never
    /// asked to authorize twice.
    #[test]
    fn session_tokens_reuse_web_token_without_a_private_client_id() {
        let dir = std::env::temp_dir().join("lyrfin-sp-session-tokens");
        let _ = std::fs::remove_dir_all(&dir);
        let _guard = client_id_guard();
        set_client_id(String::new());
        let web = Tokens {
            access_token: "WEB".into(),
            refresh_token: "R".into(),
            expires_at: now_unix() + 3600,
            scopes: "streaming".into(),
        };
        assert_eq!(
            session_tokens(&dir, &web).map(|t| t.access_token),
            Some("WEB".into()),
            "shared-id installs reuse the one token"
        );
        // with a private id the Web token is NOT usable for the session, and none
        // is cached yet → playback/browse are unauthorized rather than silently
        // riding credentials Spotify will refuse
        set_client_id("a-private-app-id".into());
        assert!(session_tokens(&dir, &web).is_none());
        let audio = Tokens {
            access_token: "AUDIO".into(),
            ..web.clone()
        };
        audio.save(&dir, TokenKind::Audio);
        assert_eq!(
            session_tokens(&dir, &web).map(|t| t.access_token),
            Some("AUDIO".into()),
            "the separately-minted audio token is what the session gets"
        );
        set_client_id(String::new());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The browser hits this port with requests we never sent it — a favicon
    /// fetch for the success page above all — and with two logins back to back
    /// one of those can arrive before the second redirect. Consuming it as the
    /// callback would fail the login with "no authorization code was returned".
    #[test]
    fn wait_for_code_ignores_requests_that_are_not_the_callback() {
        use std::io::Write as _;
        use std::net::TcpStream;
        // ephemeral port: the test must not depend on (or squat) the real one
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let client = std::thread::spawn(move || {
            // a preconnect (opened, never written) precedes the favicon + the real
            // redirect: the login must survive all three, in order
            let _silent = TcpStream::connect(("127.0.0.1", port)).expect("preconnect");
            for req in [
                "GET /favicon.ico HTTP/1.1\r\nHost: localhost\r\n\r\n",
                "GET /login?code=THE_CODE&state=st8 HTTP/1.1\r\nHost: localhost\r\n\r\n",
            ] {
                let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect");
                s.write_all(req.as_bytes()).expect("write");
                // read the reply so the server's write can't race the close
                let mut sink = Vec::new();
                let _ = std::io::Read::read_to_end(&mut s, &mut sink);
            }
        });
        assert_eq!(
            wait_for_code(&listener, "st8", never(), &NOT_CANCELLED).as_deref(),
            Ok("THE_CODE"),
            "a silent preconnect and the favicon request are skipped; the real redirect is read"
        );
        client.join().expect("client thread");
    }

    /// A deadline that has already passed: the browser never came back (the tab
    /// was closed / the user walked away). Without this the login thread — and
    /// the loopback port — would be pinned for the rest of the run, and the app
    /// would refuse every retry with "a login is already running".
    #[test]
    fn wait_for_code_gives_up_at_the_deadline() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let err = wait_for_code(&listener, "st8", std::time::Instant::now(), &NOT_CANCELLED)
            .expect_err("an elapsed deadline ends the wait");
        assert_eq!(err, LOGIN_TIMEOUT_MSG);
    }

    /// Dropping the [`crate::spotify::AuthSession`] raises this flag; the wait
    /// must end promptly rather than holding the port until the full timeout.
    #[test]
    fn wait_for_code_honours_cancellation() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let cancel = std::sync::atomic::AtomicBool::new(true);
        let err = wait_for_code(&listener, "st8", never(), &cancel)
            .expect_err("a cancelled login ends the wait");
        assert!(err.contains("cancelled"), "got: {err}");
    }

    #[test]
    fn is_transient_flags_network_and_ratelimit_only() {
        // exactly the strings token_err emits for a network / rate-limit failure
        assert!(is_transient(
            "can't reach Spotify (Connection Failed) — check your connection/VPN"
        ));
        assert!(is_transient(
            "Spotify is rate-limiting right now (too many recent requests) — wait a moment"
        ));
        // real auth rejections are NOT transient — they need the user to act
        assert!(!is_transient(
            "Spotify rejected the login (token invalid or expired)"
        ));
        assert!(!is_transient("Spotify error 400: invalid_client"));
    }

    #[test]
    fn percent_roundtrip() {
        assert_eq!(enc("a b/c"), "a%20b%2Fc");
        assert_eq!(dec("a%20b%2Fc"), "a b/c");
        assert_eq!(dec("x+y"), "x y");
    }

    #[test]
    fn token_cache_roundtrip_and_expiry() {
        let dir = std::env::temp_dir().join("lyrfin-sp-test");
        let _ = std::fs::remove_dir_all(&dir);
        let t = Tokens {
            access_token: "AT".into(),
            refresh_token: "RT".into(),
            expires_at: now_unix() + 3600,
            scopes: "streaming".into(),
        };
        t.save(&dir, TokenKind::Web);
        let back = Tokens::load(&dir, TokenKind::Web).expect("load");
        assert_eq!(back.access_token, "AT");
        assert!(!back.is_expired());
        let stale = Tokens {
            expires_at: now_unix(),
            ..t.clone()
        };
        assert!(stale.is_expired());
        // the two kinds are separate files — saving one must not touch the other
        let audio = Tokens {
            access_token: "AUDIO".into(),
            ..t.clone()
        };
        audio.save(&dir, TokenKind::Audio);
        assert_eq!(
            Tokens::load(&dir, TokenKind::Web).map(|t| t.access_token),
            Some("AT".into())
        );
        assert_eq!(
            Tokens::load(&dir, TokenKind::Audio).map(|t| t.access_token),
            Some("AUDIO".into())
        );
        // …and logging out drops BOTH, so librespot can't keep streaming after it
        Tokens::clear(&dir);
        assert!(Tokens::load(&dir, TokenKind::Web).is_none());
        assert!(
            Tokens::load(&dir, TokenKind::Audio).is_none(),
            "logout must clear the playback token too"
        );
    }
}
