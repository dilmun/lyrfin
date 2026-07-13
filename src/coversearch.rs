//! Online album-art search via **iTunes Search** + **Deezer** (both keyless,
//! JSON, 1000px artwork) with good Arabic *and* English coverage. Runs on a
//! worker thread: a `Search` request returns decoded thumbnail candidates for
//! the picker; an `Embed` request downloads the chosen full-res image and writes
//! it into every album track's tags. The UI never blocks.

use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};
use serde_json::Value;

const UA: &str = "lyrfin/0.1 ( https://github.com/lyrfin-player )";
const MAX_RESULTS: usize = 8;

/// A found cover, with its full-res URL and a small decoded thumbnail for preview.
#[derive(Clone)]
pub struct Candidate {
    pub source: &'static str,
    pub width: u32,
    pub height: u32,
    pub full_url: String,
    pub thumb: image::DynamicImage,
}

/// Pre-thumbnail hit (just URLs) before the thumbnail is fetched/decoded.
struct Raw {
    source: &'static str,
    width: u32,
    height: u32,
    full_url: String,
    thumb_url: String,
}

pub enum CoverRequest {
    Search {
        query: String,
        key: String,
    },
    Embed {
        url: String,
        paths: Vec<PathBuf>,
        key: String,
    },
}

pub enum CoverResult {
    Found {
        key: String,
        candidates: Vec<Candidate>,
    },
    Error {
        key: String,
        msg: String,
    },
    Embedded {
        key: String,
        count: usize,
        msg: String,
    },
}

/// Spawn the cover worker; returns (request sender, result receiver).
pub fn spawn() -> (Sender<CoverRequest>, Receiver<CoverResult>) {
    let (req_tx, req_rx) = unbounded::<CoverRequest>();
    let (res_tx, res_rx) = unbounded::<CoverResult>();
    std::thread::Builder::new()
        .name("lyrfin-cover".into())
        .spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(6)))
                .timeout_recv_body(Some(Duration::from_secs(10)))
                .build()
                .into();
            let process = |req: CoverRequest| match req {
                CoverRequest::Search { query, key } => {
                    let mut raw = Vec::new();
                    itunes(&agent, &query, &mut raw);
                    deezer(&agent, &query, &mut raw);
                    dedupe(&mut raw);
                    raw.truncate(MAX_RESULTS);
                    let candidates: Vec<Candidate> = raw
                        .into_iter()
                        .filter_map(|r| fetch_thumb(&agent, r))
                        .collect();
                    let _ = res_tx.send(CoverResult::Found { key, candidates });
                }
                CoverRequest::Embed { url, paths, key } => {
                    let res = match download(&agent, &url) {
                        Ok(data) => {
                            let mut count = 0usize;
                            let mut last_err = None;
                            for p in &paths {
                                match crate::tags::embed_cover(p, &data) {
                                    Ok(()) => count += 1,
                                    Err(e) => last_err = Some(e),
                                }
                            }
                            let msg = match last_err {
                                Some(e) if count == 0 => format!("Embed failed: {e}"),
                                _ => format!("Embedded cover in {count} track(s)"),
                            };
                            CoverResult::Embedded { key, count, msg }
                        }
                        Err(e) => CoverResult::Error {
                            key,
                            msg: format!("Download failed: {e}"),
                        },
                    };
                    let _ = res_tx.send(res);
                }
            };
            // Coalesce: process every Embed, collapse queued searches to the latest.
            while let Ok(req) = req_rx.recv() {
                let mut pending = vec![req];
                while let Ok(more) = req_rx.try_recv() {
                    pending.push(more);
                }
                let is_search = |r: &CoverRequest| matches!(r, CoverRequest::Search { .. });
                let last_search = pending.iter().rposition(is_search);
                for (i, r) in pending.into_iter().enumerate() {
                    if is_search(&r) && Some(i) != last_search {
                        continue; // stale search
                    }
                    process(r);
                }
            }
        })
        .expect("spawn cover thread");
    (req_tx, res_rx)
}

/// iTunes Search API — `artworkUrl100` upscaled to 1000px (250px for the thumb).
fn itunes(agent: &ureq::Agent, query: &str, out: &mut Vec<Raw>) {
    let _ = (|| -> Option<()> {
        let v: Value = agent
            .get("https://itunes.apple.com/search")
            .header("User-Agent", UA)
            .query("term", query)
            .query("entity", "album")
            .query("limit", "10")
            .call()
            .ok()?
            .body_mut()
            .read_json()
            .ok()?;
        for r in v.get("results")?.as_array()? {
            if let Some(url) = r.get("artworkUrl100").and_then(Value::as_str) {
                out.push(Raw {
                    source: "iTunes",
                    width: 1000,
                    height: 1000,
                    full_url: url.replace("100x100bb", "1000x1000bb"),
                    thumb_url: url.replace("100x100bb", "500x500bb"),
                });
            }
        }
        Some(())
    })();
}

/// Deezer album search — `cover_xl` (1000px) full, `cover_medium` (250px) thumb.
fn deezer(agent: &ureq::Agent, query: &str, out: &mut Vec<Raw>) {
    let _ = (|| -> Option<()> {
        let v: Value = agent
            .get("https://api.deezer.com/search/album")
            .header("User-Agent", UA)
            .query("q", query)
            .query("limit", "10")
            .call()
            .ok()?
            .body_mut()
            .read_json()
            .ok()?;
        for r in v.get("data")?.as_array()? {
            let xl = r.get("cover_xl").and_then(Value::as_str).unwrap_or("");
            let big = r.get("cover_big").and_then(Value::as_str).unwrap_or("");
            if !xl.is_empty() && !big.is_empty() {
                out.push(Raw {
                    source: "Deezer",
                    width: 1000,
                    height: 1000,
                    full_url: xl.to_string(),
                    thumb_url: big.to_string(),
                });
            }
        }
        Some(())
    })();
}

/// Drop duplicate full-res URLs, keeping first-seen order.
fn dedupe(v: &mut Vec<Raw>) {
    let mut seen = std::collections::HashSet::new();
    v.retain(|r| seen.insert(r.full_url.clone()));
}

/// Fetch + decode a candidate's thumbnail; `None` if it can't be loaded.
fn fetch_thumb(agent: &ureq::Agent, r: Raw) -> Option<Candidate> {
    let data = download(agent, &r.thumb_url).ok()?;
    let thumb = image::load_from_memory(&data).ok()?;
    Some(Candidate {
        source: r.source,
        width: r.width,
        height: r.height,
        full_url: r.full_url,
        thumb,
    })
}

/// GET `url` and read the body (capped at 12 MB).
fn download(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>, String> {
    let resp = agent
        .get(url)
        .header("User-Agent", UA)
        .call()
        .map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    resp.into_body()
        .into_reader()
        .take(12 * 1024 * 1024)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    if buf.is_empty() {
        return Err("empty response".into());
    }
    Ok(buf)
}
