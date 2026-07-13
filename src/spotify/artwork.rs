//! Tiny worker that downloads + decodes the now-playing Spotify track's cover URL
//! for the playback bar. Latest-wins coalesced — only the current track's art
//! matters, so a fast skip through tracks never queues stale downloads.

use std::io::Read;

use crossbeam_channel::{Receiver, Sender, TryRecvError, unbounded};

const UA: &str = "lyrfin/0.1 (+https://github.com/)";

/// Fetch the cover at `url`. `circle` masks the decoded image into a centred disc
/// with transparent corners, so the artist photo reads as a round avatar that
/// blends into whatever panel it sits on.
#[derive(Debug, Clone)]
pub struct ArtRequest {
    pub url: String,
    pub circle: bool,
}

/// A decoded cover, tagged with its source `url` so the app can ignore it if the
/// track has changed since the request.
pub struct ArtResult {
    pub url: String,
    pub img: image::DynamicImage,
}

pub fn spawn() -> (Sender<ArtRequest>, Receiver<ArtResult>) {
    let (req_tx, req_rx) = unbounded::<ArtRequest>();
    let (res_tx, res_rx) = unbounded::<ArtResult>();
    std::thread::Builder::new()
        .name("lyrfin-spotify-art".into())
        .spawn(move || {
            let agent: ureq::Agent = ureq::Agent::config_builder()
                .timeout_connect(Some(std::time::Duration::from_secs(8)))
                .timeout_recv_body(Some(std::time::Duration::from_secs(15)))
                .build()
                .into();
            while let Ok(first) = req_rx.recv() {
                // coalesce: drop all but the newest pending request
                let mut latest = first;
                loop {
                    match req_rx.try_recv() {
                        Ok(r) => latest = r,
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
                if let Some(mut img) = fetch(&agent, &latest.url) {
                    if latest.circle {
                        img = circle_crop(img);
                    }
                    let _ = res_tx.send(ArtResult {
                        url: latest.url,
                        img,
                    });
                }
            }
        })
        .expect("spawn spotify-art thread");
    (req_tx, res_rx)
}

/// GET + decode a cover (body capped at 12 MB). `None` on any failure.
fn fetch(agent: &ureq::Agent, url: &str) -> Option<image::DynamicImage> {
    let resp = agent.get(url).header("User-Agent", UA).call().ok()?;
    let mut buf = Vec::new();
    resp.into_body()
        .into_reader()
        .take(12 * 1024 * 1024)
        .read_to_end(&mut buf)
        .ok()?;
    image::load_from_memory(&buf).ok()
}

/// Centre-crop `img` to a square and mask it into a disc: pixels outside the
/// inscribed circle become fully transparent, with a 1px anti-aliased alpha edge.
/// Transparent (rather than bg-filled) corners let the panel behind show through,
/// so the avatar blends into whatever it sits on — and a theme change needs no
/// re-crop: the new panel simply shows through the same unchanged image.
pub(crate) fn circle_crop(img: image::DynamicImage) -> image::DynamicImage {
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let side = w.min(h);
    let (x0, y0) = ((w - side) / 2, (h - side) / 2);
    let mut sq = image::RgbaImage::from_fn(side, side, |x, y| *rgba.get_pixel(x0 + x, y0 + y));
    let r = side as f32 / 2.0;
    for y in 0..side {
        for x in 0..side {
            let dx = x as f32 + 0.5 - r;
            let dy = y as f32 + 0.5 - r;
            let dist = (dx * dx + dy * dy).sqrt();
            // coverage: 1 inside, 0 outside, a 1px ramp across the edge
            let cov = (r - dist).clamp(0.0, 1.0);
            if cov < 1.0 {
                // scale alpha by coverage — opaque inside, transparent outside, a
                // smooth edge; the cell background (panel) shows through the rest.
                let p = sq.get_pixel_mut(x, y);
                p.0[3] = (p.0[3] as f32 * cov).round() as u8;
            }
        }
    }
    image::DynamicImage::ImageRgba8(sq)
}
