//! Machine translation of lyric lines via Google's public (unofficial) translate
//! endpoint — free, no API key, and it auto-detects the source language (so it
//! works for lyrics in any language). Runs on a worker thread so the UI never
//! blocks; results are cached on disk by the app (one lookup per track + target).
//!
//! It fills the same per-line `Lyrics::trans` slot the bilingual-`.lrc` feature
//! uses, so the existing "dual" render path displays it. A human translation in a
//! `.lrc` always wins — machine translation only runs when there is none.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};
use serde_json::Value;

/// Target languages offered in Settings, `(code, display name)`. `""` = off. The
/// codes are Google language codes; the source is always auto-detected.
pub const LANGS: &[(&str, &str)] = &[
    ("", "off"),
    ("en", "English"),
    ("ar", "Arabic"),
    ("es", "Spanish"),
    ("fr", "French"),
    ("de", "German"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("ru", "Russian"),
    ("tr", "Turkish"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("zh-CN", "Chinese"),
    ("hi", "Hindi"),
];

/// Display name for a language code (falls back to the code itself if unknown, so
/// a hand-edited `config.toml` value still renders).
pub fn lang_label(code: &str) -> &str {
    LANGS
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, n)| *n)
        .unwrap_or(code)
}

/// Per-request soft cap on the characters sent in one HTTP call. Lines are packed
/// up to this, never split, so line↔translation alignment is preserved; a typical
/// song is one or two requests.
const CHUNK_CHARS: usize = 1400;

#[derive(Debug, Clone)]
pub struct TranslateRequest {
    /// Lyrics cache key (echoed back so the app can match the current track).
    pub key: String,
    /// Target language code (e.g. `"en"`).
    pub target: String,
    /// The lyric lines to translate, in order.
    pub lines: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TranslateResult {
    pub key: String,
    pub target: String,
    /// Per-line translations aligned 1:1 with the request's `lines`, or `None`
    /// when the lookup failed (the app then just shows the originals).
    pub lines: Option<Vec<String>>,
}

/// Spawn the translation worker; returns (request sender, result receiver).
pub fn spawn() -> (Sender<TranslateRequest>, Receiver<TranslateResult>) {
    let (req_tx, req_rx) = unbounded::<TranslateRequest>();
    let (res_tx, res_rx) = unbounded::<TranslateResult>();
    std::thread::Builder::new()
        .name("lyrfin-translate".into())
        .spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(6)))
                .timeout_recv_body(Some(Duration::from_secs(10)))
                .build()
                .into();
            while let Ok(req) = req_rx.recv() {
                // coalesce: if newer requests are queued, only serve the latest
                let mut req = req;
                while let Ok(newer) = req_rx.try_recv() {
                    req = newer;
                }
                let lines = translate_lines(&agent, &req.lines, &req.target);
                let _ = res_tx.send(TranslateResult {
                    key: req.key,
                    target: req.target,
                    lines,
                });
            }
        })
        .expect("spawn translate thread");
    (req_tx, res_rx)
}

/// Translate `lines` into `target`, packing them into as few requests as the
/// [`CHUNK_CHARS`] cap allows. Returns a vector aligned 1:1 with `lines` (missing
/// entries padded empty), or `None` if any request fails.
fn translate_lines(agent: &ureq::Agent, lines: &[String], target: &str) -> Option<Vec<String>> {
    if lines.is_empty() {
        return Some(Vec::new());
    }
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut chunk: Vec<&str> = Vec::new();
    let mut chunk_len = 0usize;
    for line in lines {
        if chunk_len + line.len() > CHUNK_CHARS && !chunk.is_empty() {
            push_aligned(&mut out, &chunk, translate_chunk(agent, &chunk, target)?);
            chunk.clear();
            chunk_len = 0;
        }
        chunk_len += line.len() + 1;
        chunk.push(line);
    }
    if !chunk.is_empty() {
        push_aligned(&mut out, &chunk, translate_chunk(agent, &chunk, target)?);
    }
    Some(out)
}

/// Append `translated` to `out`, forcing it to exactly `chunk.len()` entries so the
/// result stays index-aligned with the input even if the service merged/split a
/// line (extra entries dropped, shortfall padded empty → that line shows untranslated).
fn push_aligned(out: &mut Vec<String>, chunk: &[&str], mut translated: Vec<String>) {
    translated.truncate(chunk.len());
    while translated.len() < chunk.len() {
        translated.push(String::new());
    }
    out.extend(translated);
}

/// One HTTP round-trip: translate a chunk of lines (newline-joined) and split the
/// response back into lines. Google returns the text as sentence segments; joining
/// them preserves the newlines, which we split on to realign.
fn translate_chunk(agent: &ureq::Agent, chunk: &[&str], target: &str) -> Option<Vec<String>> {
    let joined = chunk.join("\n");
    let mut resp = agent
        .post("https://translate.googleapis.com/translate_a/single")
        .query("client", "gtx")
        .query("sl", "auto")
        .query("tl", target)
        .query("dt", "t")
        .send_form([("q", joined.as_str())])
        .ok()?;
    let v: Value = resp.body_mut().read_json().ok()?;
    let segs = v.get(0)?.as_array()?;
    let mut joined_out = String::new();
    for s in segs {
        if let Some(t) = s.get(0).and_then(Value::as_str) {
            joined_out.push_str(t);
        }
    }
    Some(joined_out.split('\n').map(str::to_string).collect())
}

// ---- on-disk cache (one file per track + target language) -----------------
fn cache_path(dir: &Path, key: &str, lang: &str) -> PathBuf {
    dir.join("translations").join(format!("{key}.{lang}.txt"))
}

/// Cached translation lines for `key`+`lang`, if present.
pub fn load_cached(dir: &Path, key: &str, lang: &str) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(cache_path(dir, key, lang)).ok()?;
    Some(text.split('\n').map(str::to_string).collect())
}

/// Persist translation lines for `key`+`lang`.
pub fn save_cached(dir: &Path, key: &str, lang: &str, lines: &[String]) {
    // a default-constructed Config has an empty dir — never write in that state
    // (mirrors `Config::save`), so a test can't land on the real config dir / CWD.
    if dir.as_os_str().is_empty() {
        return;
    }
    let path = cache_path(dir, key, lang);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, lines.join("\n"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lang_label_maps_codes_and_falls_back() {
        assert_eq!(lang_label("en"), "English");
        assert_eq!(lang_label(""), "off");
        assert_eq!(lang_label("xx"), "xx"); // unknown → the code itself
    }

    #[test]
    fn push_aligned_forces_line_count() {
        // service merged two lines into one → padded back to 2 (2nd shows untranslated)
        let mut out = Vec::new();
        push_aligned(&mut out, &["a", "b"], vec!["merged".into()]);
        assert_eq!(out, vec!["merged".to_string(), String::new()]);
        // service split one line into two → extra dropped
        let mut out = Vec::new();
        push_aligned(&mut out, &["a"], vec!["one".into(), "two".into()]);
        assert_eq!(out, vec!["one".to_string()]);
    }

    #[test]
    fn cache_round_trips() {
        let dir = std::env::temp_dir().join("lyrfin_translate_cache_test");
        let _ = std::fs::remove_dir_all(&dir);
        let lines = vec!["hola".to_string(), "mundo".to_string()];
        assert!(load_cached(&dir, "k", "en").is_none(), "empty before save");
        save_cached(&dir, "k", "en", &lines);
        assert_eq!(load_cached(&dir, "k", "en"), Some(lines));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
