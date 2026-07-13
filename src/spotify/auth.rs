//! Spotify OAuth (Authorization Code + PKCE) — a one-time browser login that
//! yields a single token usable for BOTH librespot streaming and the Web API.
//!
//! Uses Spotify's public "desktop" client id + a `127.0.0.1` loopback redirect
//! (the exact pair librespot itself uses), so the user never has to register
//! their own Spotify app. The blocking pieces (loopback wait, token exchange)
//! run on a worker thread; the UI stays responsive (see `spotify::spawn_login`).

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
        let _ = std::fs::create_dir_all(dir);
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, id).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
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

/// Persisted token set (`spotify_token.json`), refreshed when near expiry.
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
    fn path(dir: &Path) -> PathBuf {
        dir.join("spotify_token.json")
    }
    pub fn load(dir: &Path) -> Option<Tokens> {
        std::fs::read_to_string(Self::path(dir))
            .ok()
            .and_then(|t| serde_json::from_str::<Tokens>(&t).ok())
            .filter(|t| !t.access_token.is_empty() && !t.refresh_token.is_empty())
    }
    pub fn save(&self, dir: &Path) {
        if let Ok(j) = serde_json::to_string_pretty(self) {
            let _ = std::fs::create_dir_all(dir);
            // atomic (sibling temp + rename): two refresh paths can save near-
            // simultaneously, and a torn write would leave invalid JSON that
            // `load` rejects → the token silently lost on the next start.
            let path = Self::path(dir);
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, j).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            } else {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }
    pub fn clear(dir: &Path) {
        let _ = std::fs::remove_file(Self::path(dir));
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

/// Build the authorize URL; returns `(url, pkce_verifier, csrf_state)`.
pub fn authorize_url() -> (String, String, String) {
    let verifier = random_token(48);
    let challenge = b64url(&Sha256::digest(verifier.as_bytes())[..]);
    let state = random_token(16);
    let url = format!(
        "{AUTH_URL}?response_type=code&client_id={cid}&redirect_uri={redir}\
&code_challenge_method=S256&code_challenge={chal}&state={state}&scope={scope}",
        cid = client_id(),
        redir = enc(&redirect_uri()),
        chal = challenge,
        state = enc(&state),
        scope = enc(SCOPES),
    );
    (url, verifier, state)
}

/// Bind the loopback redirect listener (so the caller can report "ready" before
/// opening the browser). A bind error usually means a stale login is running.
pub fn bind_listener() -> std::io::Result<TcpListener> {
    TcpListener::bind(("127.0.0.1", REDIRECT_PORT))
}

/// Block until Spotify redirects back, validate `state`, and return the code.
/// Always writes a friendly page to the browser tab.
pub fn wait_for_code(listener: &TcpListener, expect_state: &str) -> Result<String, String> {
    let mut stream = listener
        .incoming()
        .flatten()
        .next()
        .ok_or("login listener closed")?;
    let mut line = String::new();
    BufReader::new(&stream)
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
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
    code.filter(|c| !c.is_empty())
        .ok_or_else(|| "no authorization code was returned".into())
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

/// Exchange an authorization `code` (+ PKCE verifier) for tokens.
pub fn exchange_code(code: &str, verifier: &str) -> Result<Tokens, String> {
    let redir = redirect_uri();
    let cid = client_id();
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
pub fn refresh(refresh_token: &str) -> Result<Tokens, String> {
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
    let fresh = refresh_uncached(refresh_token)?;
    *guard = Some((refresh_token.to_string(), fresh.clone()));
    Ok(fresh)
}

/// The actual token endpoint round-trip (no serialization). Call via [`refresh`].
fn refresh_uncached(refresh_token: &str) -> Result<Tokens, String> {
    let cid = client_id();
    log::info!(target: "lyrfin::spotify", "token refresh: client_id custom={}", has_custom_client_id());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authorize_url_has_pkce_and_scopes() {
        let (url, verifier, state) = authorize_url();
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
        t.save(&dir);
        let back = Tokens::load(&dir).expect("load");
        assert_eq!(back.access_token, "AT");
        assert!(!back.is_expired());
        let stale = Tokens {
            expires_at: now_unix(),
            ..t.clone()
        };
        assert!(stale.is_expired());
        Tokens::clear(&dir);
        assert!(Tokens::load(&dir).is_none());
    }
}
