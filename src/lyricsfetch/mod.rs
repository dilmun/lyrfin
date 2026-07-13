//! Online lyrics lookup on a worker thread so the UI never blocks.
//!
//! Providers are tried in order until one returns lyrics:
//!   1. [`lrclib`] — LRCLIB (lrclib.net): free, no key; strong for Western /
//!      popular tracks, returns synced (LRC) + plain, with growing Arabic
//!      coverage.
//!   2. [`netease`] — NetEase Cloud Music (music.163.com): the largest
//!      Chinese/Japanese/Korean catalogue (incl. K-pop), synced LRC; its
//!      (always-Chinese) translation track is merged bilingually only when the
//!      user's translation target is Chinese, else lyrfin's own translator applies.
//!   3. [`jiosaavn`] — JioSaavn (jiosaavn.com): India's largest catalogue, for
//!      Hindi/Bollywood and other Indian-language tracks. **Plain** lyrics only
//!      (no timestamps) — last, since we prefer synced sources first.
//!
//! Each provider is a [`Provider`] fn returning raw LRC/plain text (or `None`);
//! adding a source is a new file plus a line in [`PROVIDERS`]. Results are
//! parsed + cached on disk by the app (see [`crate::lyrics`]).

mod jiosaavn;
mod lrclib;
mod netease;

use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};

/// A lyrics source: raw LRC/plain text for `req`, or `None` if it has nothing
/// (or on any network/parse error — providers fail soft so the next one in
/// [`PROVIDERS`] gets a turn).
type Provider = fn(&ureq::Agent, &LyricsRequest) -> Option<String>;

/// Sources tried in order; the first non-`None` wins. Synced sources first —
/// LRCLIB (Western), NetEase (CJK / global) — then JioSaavn (plain, Indian).
const PROVIDERS: &[Provider] = &[lrclib::fetch, netease::fetch, jiosaavn::fetch];

#[derive(Debug, Clone)]
pub struct LyricsRequest {
    pub artist: String,
    pub title: String,
    pub album: String,
    pub duration_secs: u64,
    /// The user's lyric-translation target (`config.lyrics_translate_to`, e.g.
    /// `"en"`; `""` = off). A provider that ships its own translation (NetEase's
    /// is always Chinese) only merges it when it matches this — otherwise lyrfin's
    /// own translator supplies the configured language instead.
    pub translate_to: String,
    /// Cache key the app assigned; echoed back so it can match the current track.
    pub key: String,
}

#[derive(Debug, Clone)]
pub struct LyricsResult {
    pub key: String,
    /// Raw LRC (synced) or plain text, or `None` if nothing was found.
    pub text: Option<String>,
}

/// Spawn the lyrics worker; returns (request sender, result receiver).
pub fn spawn() -> (Sender<LyricsRequest>, Receiver<LyricsResult>) {
    let (req_tx, req_rx) = unbounded::<LyricsRequest>();
    let (res_tx, res_rx) = unbounded::<LyricsResult>();
    std::thread::Builder::new()
        .name("lyrfin-lyrics".into())
        .spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(6)))
                .timeout_recv_body(Some(Duration::from_secs(8)))
                .build()
                .into();
            while let Ok(req) = req_rx.recv() {
                // coalesce: if newer requests are queued, only serve the latest
                let mut req = req;
                while let Ok(newer) = req_rx.try_recv() {
                    req = newer;
                }
                let text = PROVIDERS.iter().find_map(|fetch| fetch(&agent, &req));
                let _ = res_tx.send(LyricsResult { key: req.key, text });
            }
        })
        .expect("spawn lyrics thread");
    (req_tx, res_rx)
}
