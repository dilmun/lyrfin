//! The full-directory cache: download the entire Radio Browser station list
//! once (~66 MB, gzipped on the wire), persist it as `radio_directory.json`,
//! and serve future requests from that local snapshot — so browsing the whole
//! directory is instant and offline-capable. A stale cache is preferred over
//! nothing when the network fails. The live per-query search lives in the
//! parent module; `spawn` calls [`load_directory`] for `LoadDirectory` requests.

use std::io::Read;
use std::path::{Path, PathBuf};

use crossbeam_channel::Sender;
use serde::{Deserialize, Serialize};

use super::*;

/// On-disk snapshot of the station directory (`radio_directory.json`).
#[derive(Serialize, Deserialize)]
struct Directory {
    /// Unix seconds when this snapshot was downloaded.
    #[serde(default)]
    fetched: u64,
    #[serde(default)]
    stations: Vec<Station>,
}

fn directory_path(dir: &Path) -> PathBuf {
    dir.join("radio_directory.json")
}

fn load_cache(dir: &Path) -> Option<Directory> {
    let bytes = std::fs::read(directory_path(dir)).ok()?;
    serde_json::from_slice::<Directory>(&bytes)
        .ok()
        .filter(|d| !d.stations.is_empty())
}

fn save_cache(dir: &Path, stations: &[Station], fetched: u64) {
    let snap = Directory {
        fetched,
        stations: stations.to_vec(),
    };
    if let Ok(json) = serde_json::to_vec(&snap) {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(directory_path(dir), json);
    }
}

/// Download the full directory (gzip-compressed on the wire — ureq inflates it),
/// reporting bytes read via `progress`. `hidebroken=true` drops dead stations
/// server-side; `clean` then drops HLS / empty-URL ones — only playable, working
/// stations are kept.
fn download_directory(
    agent: &ureq::Agent,
    mut progress: impl FnMut(u64),
) -> Result<Vec<Station>, String> {
    let url = format!(
        "{BASE}/json/stations/search?hidebroken=true&order=clickcount&reverse=true&limit=1000000"
    );
    let resp = agent.get(&url).call().map_err(err_msg)?;
    let mut reader = resp.into_body().into_reader();
    let mut buf: Vec<u8> = Vec::with_capacity(DIRECTORY_EST_BYTES as usize);
    let mut chunk = vec![0u8; 256 * 1024];
    let mut last = 0u64;
    loop {
        let n = reader.read(&mut chunk).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        let len = buf.len() as u64;
        if len - last >= 1024 * 1024 {
            last = len;
            progress(len);
        }
    }
    let rows: Vec<RawStation> = serde_json::from_slice(&buf)
        .map_err(|_| "couldn't parse the radio directory".to_string())?;
    Ok(clean(rows))
}

/// Resolve the directory: serve a fresh-enough cache, else (re)download and
/// re-cache. A network failure falls back to a stale cache rather than nothing.
/// Progress + the final result are sent on `res_tx`.
pub(super) fn load_directory(
    agent: &ureq::Agent,
    dir: &Path,
    force: bool,
    max_age_secs: u64,
    res_tx: &Sender<RadioResult>,
) {
    let now = crate::datetime::now_unix();
    if !force
        && let Some(c) = load_cache(dir)
        && (max_age_secs == 0 || now.saturating_sub(c.fetched) < max_age_secs)
    {
        let _ = res_tx.send(RadioResult::Directory {
            stations: c.stations,
            from_cache: true,
        });
        return;
    }
    let tx = res_tx.clone();
    match download_directory(agent, |read| {
        let _ = tx.send(RadioResult::DirectoryProgress { read });
    }) {
        Ok(stations) => {
            save_cache(dir, &stations, now);
            let _ = res_tx.send(RadioResult::Directory {
                stations,
                from_cache: false,
            });
        }
        Err(msg) => {
            if let Some(c) = load_cache(dir) {
                let _ = res_tx.send(RadioResult::Directory {
                    stations: c.stations,
                    from_cache: true,
                });
            } else {
                let _ = res_tx.send(RadioResult::Error {
                    key: String::new(),
                    msg,
                });
            }
        }
    }
}
