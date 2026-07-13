//! Album-art extraction: pull the embedded cover picture out of an audio file
//! and decode it to an `image::DynamicImage` for the inline-image renderer
//! (`ratatui-image`). Returns `None` when there's no embedded art.

use std::path::Path;

use lofty::prelude::*;

/// Largest side we keep a decoded cover at. Embedded art is often 1500–3000px+;
/// `ratatui-image` re-resizes from the source to the target cell rect on the
/// render thread (a blocking op), so an oversized source makes every track /
/// view change janky. 1280px is ample for any terminal-cell display (even
/// fullscreen Concert) while keeping that resize cheap. Bounded once, here.
const MAX_COVER_PX: u32 = 1280;

/// Run `f` over the chosen cover picture (the front cover, else the
/// largest embedded picture) from `path`'s primary tag. Shared by [`load_cover`]
/// and [`cover_bytes`] so the selection lives in one place; `f` runs inside the
/// tag's borrow, so neither caller has to clone the picture.
fn with_cover_picture<T>(
    path: &Path,
    f: impl FnOnce(&lofty::picture::Picture) -> Option<T>,
) -> Option<T> {
    let tagged = lofty::read_from_path(path).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let pics = tag.pictures();
    let pic = pics
        .iter()
        .find(|p| p.pic_type() == lofty::picture::PictureType::CoverFront)
        .or_else(|| pics.iter().max_by_key(|p| p.data().len()))?;
    f(pic)
}

/// Load the (largest) embedded cover image from `path`, decoded and downscaled
/// to at most [`MAX_COVER_PX`] on its longest side.
pub fn load_cover(path: &Path) -> Option<image::DynamicImage> {
    with_cover_picture(path, |pic| image::load_from_memory(pic.data()).ok()).map(downscale)
}

/// The raw (undecoded) embedded cover bytes + a file extension for them, for
/// writing straight to disk. Unlike [`load_cover`] it does no decode/re-encode —
/// the OS "Now Playing" integration (`crate::media`) needs a plain image *file*
/// it can read itself. `None` when there's no embedded art.
pub fn cover_bytes(path: &Path) -> Option<(Vec<u8>, &'static str)> {
    use lofty::picture::MimeType;
    with_cover_picture(path, |pic| {
        // NSImage / desktop art loaders sniff by content, so the extension is
        // cosmetic; still, name PNGs correctly and default everything else to jpg.
        let ext = match pic.mime_type() {
            Some(MimeType::Png) => "png",
            _ => "jpg",
        };
        Some((pic.data().to_vec(), ext))
    })
}

/// Shrink an oversized cover to [`MAX_COVER_PX`] (aspect-preserving); a no-op
/// when it already fits. Done once at load so the hot render path never resizes
/// from a multi-megapixel source.
fn downscale(img: image::DynamicImage) -> image::DynamicImage {
    if img.width() <= MAX_COVER_PX && img.height() <= MAX_COVER_PX {
        img
    } else {
        img.resize(
            MAX_COVER_PX,
            MAX_COVER_PX,
            image::imageops::FilterType::Triangle,
        )
    }
}

/// A representative *vibrant* colour from the cover, for the dynamic accent.
/// Downsamples, then averages saturated mid-bright pixels (skipping near-black /
/// near-white / grey), falling back to the plain average when nothing is vivid.
pub fn dominant_color(img: &image::DynamicImage) -> crate::ui::theme::Rgb {
    use image::imageops::FilterType;
    let small = img.resize(48, 48, FilterType::Triangle).to_rgb8();
    let (mut wr, mut wg, mut wb, mut wsum) = (0f32, 0f32, 0f32, 0f32);
    let (mut ar, mut ag, mut ab, mut an) = (0f32, 0f32, 0f32, 0f32);
    for p in small.pixels() {
        let [r, g, b] = p.0;
        let (rf, gf, bf) = (r as f32, g as f32, b as f32);
        ar += rf;
        ag += gf;
        ab += bf;
        an += 1.0;
        let max = rf.max(gf).max(bf);
        let min = rf.min(gf).min(bf);
        let val = max / 255.0;
        let sat = if max <= 0.0 { 0.0 } else { (max - min) / max };
        if val > 0.15 && val < 0.98 && sat > 0.25 {
            // prefer saturated, mid-bright pixels
            let w = (sat * (1.0 - (val - 0.6).abs())).max(0.0);
            wr += rf * w;
            wg += gf * w;
            wb += bf * w;
            wsum += w;
        }
    }
    let (r, g, b) = if wsum > 0.0 {
        (wr / wsum, wg / wsum, wb / wsum)
    } else if an > 0.0 {
        (ar / an, ag / an, ab / an)
    } else {
        (120.0, 120.0, 120.0)
    };
    crate::ui::theme::Rgb(r as u8, g as u8, b as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downscale_bounds_oversized_covers_and_keeps_aspect() {
        // a huge, non-square cover → longest side capped, aspect preserved
        let big = image::DynamicImage::ImageRgb8(image::RgbImage::new(3000, 2000));
        let out = downscale(big);
        assert!(out.width() <= MAX_COVER_PX && out.height() <= MAX_COVER_PX);
        assert_eq!(out.width(), MAX_COVER_PX, "longest side hits the cap");
        // 3000:2000 == 3:2 preserved (±1px rounding)
        assert!((out.height() as i32 - (MAX_COVER_PX as i32 * 2 / 3)).abs() <= 1);
    }

    #[test]
    fn downscale_leaves_small_covers_untouched() {
        let small = image::DynamicImage::ImageRgb8(image::RgbImage::new(600, 600));
        let out = downscale(small);
        assert_eq!(
            (out.width(), out.height()),
            (600, 600),
            "no upscale / re-encode"
        );
    }

    #[test]
    fn dominant_color_picks_the_vibrant_region() {
        // half a strong red, half near-black → the vibrant pick should be reddish
        let mut img = image::RgbImage::new(16, 16);
        for (x, _y, p) in img.enumerate_pixels_mut() {
            *p = if x < 8 {
                image::Rgb([220, 30, 30])
            } else {
                image::Rgb([4, 4, 4])
            };
        }
        let c = dominant_color(&image::DynamicImage::ImageRgb8(img));
        assert!(c.0 > c.1 + 40 && c.0 > c.2 + 40, "red dominates: {c:?}");
    }
}
