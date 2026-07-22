//! Off-thread artwork loader for the library grid + the Artist pane. Decodes
//! embedded cover art and fetches online ARTIST photos (Deezer — keyless, the
//! same source `coversearch` already uses for album art) without ever blocking
//! the UI: the worker returns a decoded `DynamicImage`, and the main thread
//! builds the inline-image protocol via the picker (mirrors `spotify::artwork`).
//! Online artist photos are disk-cached, so it's a one-time fetch per artist.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender, unbounded};
use serde_json::Value;

use crate::core::model::{AlbumId, ArtistId};

const UA: &str = "lyrfin/0.1 ( https://github.com/lyrfin-player )";
/// Thumbnails only ever fill a card / pane, never the screen — keep them small.
const THUMB: u32 = 320;

/// What a cached thumbnail belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArtKey {
    Album(AlbumId),
    Artist(ArtistId),
    /// The now-playing artist's pane PHOTO — distinct from `Artist` (the grid card)
    /// so the large pane render and the small grid card each own a `StatefulProtocol`
    /// and never thrash one shared resize cache.
    ArtistPhoto(ArtistId),
    /// The Concert view's fullscreen artist photo. Its own bucket because it is
    /// always a **square** (masks are baked at decode, and `request_art` dedups by
    /// key alone), while `ArtistPhoto` follows the `grid_circle` setting — sharing a
    /// key would let the pane's circle satisfy Concert's square request and leave it
    /// round.
    ConcertArtist(ArtistId),
    /// A remote image addressed by a stable hash of its URL (Spotify covers /
    /// artist photos). Hashing keeps `ArtKey` `Copy` — see [`ArtKey::remote`].
    Remote(u64),
    /// An artist photo keyed by a stable hash of the artist *name* — for the
    /// Spotify artist pane, which has no local `ArtistId` to key on. Built by
    /// [`ArtKey::artist_name`]; distinct bucket from `Remote` (URL) and
    /// `ArtistPhoto` (local id).
    ArtistName(u64),
    /// A top-slice "peek" of another cover (a partially-visible carousel row),
    /// keyed by a hash of the base key + slice height so it caches separately from
    /// the full cover. Built by [`ArtKey::peek`].
    Peek(u64),
    /// A generated solid-colour placeholder disc/tile (for a cover-less card), keyed
    /// by a hash of the colour + shape so all cards of one colour share one image.
    /// Built by [`ArtKey::solid`].
    Solid(u64),
}

impl ArtKey {
    /// Key a remote image by a stable hash of its URL.
    pub fn remote(url: &str) -> ArtKey {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        url.hash(&mut h);
        ArtKey::Remote(h.finish())
    }

    /// Key an artist photo by a stable hash of the artist's NAME — for the Spotify
    /// artist pane, which has no local `ArtistId`. The photo itself is fetched via
    /// [`ArtSource::Artist`] (Deezer by name, disk-cached by the same name slug).
    pub fn artist_name(name: &str) -> ArtKey {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        name.hash(&mut h);
        "artist".hash(&mut h);
        ArtKey::ArtistName(h.finish())
    }

    /// Key a **square** copy of a remote image by URL — a distinct bucket from
    /// [`ArtKey::remote`] (which the pane may cache circle-masked) so the Concert
    /// view's square artist photo never collides with a round one of the same URL.
    pub fn square(url: &str) -> ArtKey {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        url.hash(&mut h);
        "square".hash(&mut h);
        ArtKey::Remote(h.finish())
    }

    /// Key the top-slice peek of `base` at `rows` cells tall — distinct from the
    /// full cover so both coexist in the cache.
    pub fn peek(base: ArtKey, rows: u16) -> ArtKey {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        base.hash(&mut h);
        rows.hash(&mut h);
        "peek".hash(&mut h);
        ArtKey::Peek(h.finish())
    }

    /// Key a generated solid-colour placeholder by its packed `0xRRGGBB` colour +
    /// shape, so every cover-less card of one colour shares a single circle image.
    pub fn solid(color: u32, circle: bool) -> ArtKey {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        color.hash(&mut h);
        circle.hash(&mut h);
        "solid".hash(&mut h);
        ArtKey::Solid(h.finish())
    }
}

/// Where the worker sources the image.
#[derive(Debug, Clone)]
pub enum ArtSource {
    /// Decode the embedded cover from this audio file (albums).
    Embedded(PathBuf),
    /// An artist photo: try Deezer (disk-cached), else the embedded `fallback`
    /// (the newest album/single's cover).
    Artist {
        name: String,
        fallback: Option<PathBuf>,
    },
    /// Download a remote image by URL (Spotify covers / artist photos).
    Url(String),
}

pub struct ArtRequest {
    pub key: ArtKey,
    pub source: ArtSource,
    /// Mask the result into a round avatar (transparent corners).
    pub circle: bool,
    /// Disk-cache root for online artist photos (`{config.dir}/cache/artists`).
    pub cache_dir: PathBuf,
    /// For a carousel "peek": scale the cover to `(width_px, _)` and return its top
    /// `height_px` slice, so a partially-visible row shows the top of each cover
    /// (not a squashed or corner-cropped one). `None` → a normal full thumbnail.
    pub crop: Option<(u32, u32)>,
}

pub struct ArtResult {
    pub key: ArtKey,
    /// `None` when nothing could be loaded (the app negative-caches it).
    pub img: Option<image::DynamicImage>,
}

/// Spawn the artwork worker; returns (request sender, result receiver).
pub fn spawn() -> (Sender<ArtRequest>, Receiver<ArtResult>) {
    let (req_tx, req_rx) = unbounded::<ArtRequest>();
    let (res_tx, res_rx) = unbounded::<ArtResult>();
    // A small pool, not one thread. Covers were fetched strictly serially, so a
    // screenful on a cold start or a fast scroll waited out one network round trip
    // after another and filled in visibly one card at a time. The channel is MPMC,
    // so the workers just share the queue.
    //
    // Deliberately small and fixed: these are CDN image fetches, and the point is to
    // overlap the latency, not to open as many sockets as there are cards. Four
    // keeps a screenful moving while staying politely below anything that would look
    // like a burst. A warm disk cache (see `cached_download`) means most requests
    // never leave the machine at all.
    const ART_WORKERS: usize = 4;
    for n in 0..ART_WORKERS {
        let req_rx = req_rx.clone();
        let res_tx = res_tx.clone();
        std::thread::Builder::new()
            .name(format!("lyrfin-artwork-{n}"))
            .spawn(move || {
                let agent: ureq::Agent = ureq::Agent::config_builder()
                    .timeout_connect(Some(Duration::from_secs(6)))
                    .timeout_recv_body(Some(Duration::from_secs(12)))
                    .build()
                    .into();
                while let Ok(req) = req_rx.recv() {
                    let img = resolve(&agent, &req.source, &req.cache_dir).map(|im| {
                        match req.crop {
                            // peek: scale to the card width, (optionally) round it, then
                            // keep the top `h` px so a partial row shows the cover's top.
                            Some((w, h)) => {
                                let t = im.thumbnail(w, w);
                                let shaped = if req.circle {
                                    crate::spotify::artwork::circle_crop(t)
                                } else {
                                    t
                                };
                                let (cw, ch) = (shaped.width(), shaped.height());
                                shaped.crop_imm(0, 0, w.min(cw), h.min(ch))
                            }
                            None => {
                                let t = im.thumbnail(THUMB, THUMB);
                                if req.circle {
                                    crate::spotify::artwork::circle_crop(t)
                                } else {
                                    t
                                }
                            }
                        }
                    });
                    if res_tx.send(ArtResult { key: req.key, img }).is_err() {
                        return; // app gone
                    }
                }
            })
            .expect("spawn artwork thread");
    }
    (req_tx, res_rx)
}

fn resolve(
    agent: &ureq::Agent,
    source: &ArtSource,
    cache_dir: &Path,
) -> Option<image::DynamicImage> {
    match source {
        ArtSource::Embedded(path) => crate::cover::load_cover(path),
        ArtSource::Artist { name, fallback } => {
            artist_image(agent, name, fallback.as_deref(), cache_dir)
        }
        ArtSource::Url(url) => cached_download(agent, url, cache_dir),
    }
}

/// A THUMB×THUMB opaque image of one `0xRRGGBB` colour — the worker rounds it (like
/// any cover) into a clean placeholder disc/tile for a cover-less card.
fn solid_image(rgb: u32) -> image::DynamicImage {
    let px = image::Rgba([(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255]);
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(THUMB, THUMB, px))
}

/// A **shaped** solid-colour placeholder: a THUMB×THUMB colour, circle-cropped when
/// `circle`. Built *synchronously* on the main thread for a cover-less card — it's a
/// trivial fill + mask, far cheaper than queueing it behind network cover downloads
/// on the single worker thread (which is why cover-less circle cards used to get
/// stuck on the blocky cell disc).
pub(crate) fn solid_art(rgb: u32, circle: bool) -> image::DynamicImage {
    let t = solid_image(rgb).thumbnail(THUMB, THUMB);
    if circle {
        crate::spotify::artwork::circle_crop(t)
    } else {
        t
    }
}

/// Online-first artist photo: a disk-cached Deezer picture, else the embedded
/// cover of the newest album/single.
fn artist_image(
    agent: &ureq::Agent,
    name: &str,
    fallback: Option<&Path>,
    cache_dir: &Path,
) -> Option<image::DynamicImage> {
    let cached = cache_dir.join(format!("{}.jpg", slug(name)));
    // 1) disk cache — a one-time fetch per artist
    if let Ok(bytes) = std::fs::read(&cached)
        && let Ok(img) = image::load_from_memory(&bytes)
    {
        return Some(img);
    }
    // 2) Deezer (keyless): best name match, most fans → its picture
    if let Some(url) = deezer_artist(agent, name)
        && let Some(bytes) = download(agent, &url)
        && let Ok(img) = image::load_from_memory(&bytes)
    {
        let _ = std::fs::create_dir_all(cache_dir);
        let _ = std::fs::write(&cached, &bytes);
        return Some(img);
    }
    // 3) embedded cover of the newest album/single
    fallback.and_then(crate::cover::load_cover)
}

/// Deezer artist search → the best match's `picture_xl`. Requires a name match
/// (case/accent-folded), breaking ties by fan count, so we never grab the wrong
/// act's face.
fn deezer_artist(agent: &ureq::Agent, name: &str) -> Option<String> {
    let v: Value = agent
        .get("https://api.deezer.com/search/artist")
        .header("User-Agent", UA)
        .query("q", name)
        .query("limit", "10")
        .call()
        .ok()?
        .body_mut()
        .read_json()
        .ok()?;
    let want = fold(name);
    v.get("data")?
        .as_array()?
        .iter()
        .filter(|a| fold(a.get("name").and_then(Value::as_str).unwrap_or("")) == want)
        .max_by_key(|a| a.get("nb_fan").and_then(Value::as_i64).unwrap_or(0))
        .and_then(|a| {
            a.get("picture_xl")
                .or_else(|| a.get("picture_big"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
}

/// GET `url` and read the body (capped at 12 MB).
/// A remote cover, disk-cached by a hash of its URL — the same one-time-fetch
/// treatment artist photos already get.
///
/// Without this, every artwork invalidation (a card size or shape change, an
/// overlay closing) re-downloads every visible cover at once through this single
/// worker thread. That burst is what left cards blank: whichever requests timed
/// out were negative-cached, and the covers didn't come back until a restart.
/// Re-fetching from disk makes invalidation cheap enough to be routine.
fn cached_download(
    agent: &ureq::Agent,
    url: &str,
    cache_dir: &Path,
) -> Option<image::DynamicImage> {
    use std::hash::{Hash, Hasher};
    // `cache_dir` is the artists directory; covers get their own sibling.
    let dir = cache_dir.parent().unwrap_or(cache_dir).join("covers");
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    let path = dir.join(format!("{:016x}", h.finish()));

    if let Ok(bytes) = std::fs::read(&path)
        && let Ok(img) = image::load_from_memory(&bytes)
    {
        return Some(img);
    }
    let bytes = download(agent, url)?;
    let img = image::load_from_memory(&bytes).ok()?;
    // Best-effort: a cache write failing must never fail the render.
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(&path, &bytes);
    Some(img)
}

fn download(agent: &ureq::Agent, url: &str) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    agent
        .get(url)
        .header("User-Agent", UA)
        .call()
        .ok()?
        .into_body()
        .into_reader()
        .take(12 * 1024 * 1024)
        .read_to_end(&mut buf)
        .ok()?;
    Some(buf)
}

/// Case/accent-fold for matching: lowercase, keep only alphanumerics.
fn fold(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// A filesystem-safe disk-cache key for an artist name.
fn slug(s: &str) -> String {
    let f = fold(s);
    if f.is_empty() {
        "_".into()
    } else {
        f.chars().take(64).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{ArtKey, fold, slug};

    #[test]
    fn remote_art_key_is_stable_per_url() {
        // the same URL always keys the same cache entry; different URLs don't collide
        let a = ArtKey::remote("https://i.scdn.co/image/abc");
        let b = ArtKey::remote("https://i.scdn.co/image/abc");
        let c = ArtKey::remote("https://i.scdn.co/image/xyz");
        assert_eq!(a, b, "same URL → same key");
        assert_ne!(a, c, "different URL → different key");
        assert!(matches!(a, ArtKey::Remote(_)));
    }

    #[test]
    fn name_folding_matches_across_punctuation_and_case() {
        // the Deezer match-guard folds both sides: punctuation/case ignored
        assert_eq!(fold("The Beatles!"), fold("the beatles"));
        assert_eq!(fold("AC/DC"), "acdc");
        assert_ne!(
            fold("Wham"),
            fold("Whamageddon"),
            "no loose substring match"
        );
        // the disk-cache key is filesystem-safe + never empty
        assert_eq!(slug("AC/DC"), "acdc");
        assert_eq!(slug("***"), "_");
    }
}
