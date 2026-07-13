//! Library-grid + Artist-pane artwork cache + request methods, driven by the
//! `artwork` worker. The worker decodes/fetches off-thread; this builds the
//! inline-image protocol on the main thread (where the picker lives) and caches
//! it per album/artist. Render requests thumbnails for on-screen items only.

use crate::artwork::{ArtKey, ArtRequest, ArtResult, ArtSource};

use super::*;

/// A per-item thumbnail in the cache.
pub enum ArtThumb {
    /// Requested; the worker hasn't answered yet.
    Pending,
    /// Decoded + protocol built — render it. Boxed: a v11 `StatefulProtocol` is
    /// ~264 bytes, so keeping it inline would bloat every `Pending`/`Missing`
    /// cache entry to that size (clippy `large_enum_variant`).
    Ready(Box<std::cell::RefCell<ratatui_image::protocol::StatefulProtocol>>),
    /// Nothing could be loaded — show the placeholder, don't re-request.
    Missing,
}

/// The cover-art cache: `ArtKey → (last-used clock tick, thumbnail)`. The tick is
/// bumped whenever an on-screen card requests its art, so least-recently-used
/// (off-screen) entries can be evicted to keep the cache bounded.
pub(crate) type ArtCache = std::collections::HashMap<ArtKey, (u64, ArtThumb)>;

/// Max cached thumbnails. Comfortably larger than any on-screen card count (a few
/// dozen) plus the artist-pane photo, so scrolling back and forth doesn't thrash;
/// bounds memory for huge libraries scrolled extensively. Each Ready entry holds a
/// decoded ~320px image protocol.
const GRID_ART_CAP: usize = 256;

impl AppState {
    pub fn set_art_sender(&mut self, tx: crossbeam_channel::Sender<ArtRequest>) {
        self.workers.art = Some(tx);
    }

    /// Request a thumbnail for `key` unless it's already cached or in flight. Safe
    /// to call every frame from render (`&self`, interior mutability). Bumps the
    /// key's recency each call (so on-screen cards stay; off-screen ones age out).
    pub(crate) fn request_art(&self, key: ArtKey, source: ArtSource, circle: bool) {
        let tick = self.grid_art_clock.get().wrapping_add(1);
        self.grid_art_clock.set(tick);
        {
            let mut cache = self.grid_art.borrow_mut();
            if let Some(entry) = cache.get_mut(&key) {
                entry.0 = tick; // already cached / in flight → just mark it fresh
                return;
            }
            cache.insert(key, (tick, ArtThumb::Pending));
            evict_lru(&mut cache);
        }
        if let Some(tx) = &self.workers.art {
            let _ = tx.send(ArtRequest {
                key,
                source,
                circle,
                cache_dir: self.config.dir.join("cache").join("artists"),
                crop: None,
            });
        }
    }

    /// Ensure the solid-colour placeholder image for `color`+`circle` is built and
    /// cached, generating it **synchronously** on this (main) thread — it's a
    /// trivial fill + circle-mask, so building it here is instant and, unlike a
    /// worker request, never waits behind network cover downloads (the cause of
    /// cover-less circle cards sticking on the blocky cell disc). Returns its key
    /// when inline images are available (a picker exists), else `None` — the caller
    /// then draws the cell disc. Bumps recency so an on-screen solid isn't evicted.
    pub(crate) fn ensure_solid_art(&self, color: u32, circle: bool) -> Option<ArtKey> {
        let key = ArtKey::solid(color, circle);
        let tick = self.grid_art_clock.get().wrapping_add(1);
        self.grid_art_clock.set(tick);
        {
            let mut cache = self.grid_art.borrow_mut();
            if let Some(entry) = cache.get_mut(&key) {
                entry.0 = tick; // keep it fresh while on screen
                if matches!(entry.1, ArtThumb::Ready(_)) {
                    return Some(key);
                }
            }
        }
        // not built yet → build it now (needs the picker to make the protocol)
        let picker = self.art.picker.as_ref()?;
        let proto = picker.new_resize_protocol(crate::artwork::solid_art(color, circle));
        let mut cache = self.grid_art.borrow_mut();
        cache.insert(
            key,
            (
                tick,
                ArtThumb::Ready(Box::new(std::cell::RefCell::new(proto))),
            ),
        );
        evict_lru(&mut cache);
        Some(key)
    }

    /// Request (and cache) a top-slice "peek" of a cover for a partially-visible
    /// carousel row: the worker scales the cover to `w_px` wide and returns its top
    /// `h_px`, so the peek shows the cover's top instead of a squashed/corner-cropped
    /// image. Keyed separately from the full cover ([`ArtKey::peek`]); returns that
    /// key so the caller can render it once ready. `&self` — safe from render.
    pub(crate) fn request_peek(
        &self,
        base: ArtKey,
        source: ArtSource,
        w_px: u32,
        h_px: u32,
        rows: u16,
    ) -> ArtKey {
        let key = ArtKey::peek(base, rows);
        let tick = self.grid_art_clock.get().wrapping_add(1);
        self.grid_art_clock.set(tick);
        {
            let mut cache = self.grid_art.borrow_mut();
            if let Some(entry) = cache.get_mut(&key) {
                entry.0 = tick;
                return key;
            }
            cache.insert(key, (tick, ArtThumb::Pending));
            evict_lru(&mut cache);
        }
        if let Some(tx) = &self.workers.art {
            let _ = tx.send(ArtRequest {
                key,
                source,
                circle: self.config.grid_circle,
                cache_dir: self.config.dir.join("cache").join("artists"),
                crop: Some((w_px, h_px)),
            });
        }
        key
    }

    /// Whether `key`'s thumbnail is decoded and ready to draw (vs pending, missing,
    /// or never requested). Lets a pane reserve its photo box only once a real image
    /// exists — so a source with no online photo stays text-only instead of showing a
    /// permanent placeholder box.
    pub(crate) fn art_ready(&self, key: ArtKey) -> bool {
        matches!(
            self.grid_art.borrow().get(&key),
            Some((_, ArtThumb::Ready(_)))
        )
    }

    /// Receive a decoded thumbnail: build its protocol (the picker lives on this
    /// thread) and cache it, or mark it Missing. Redraws so the cover appears.
    pub fn on_art_result(&mut self, res: ArtResult) {
        let thumb = match (res.img, self.art.picker.as_ref()) {
            (Some(img), Some(p)) => ArtThumb::Ready(Box::new(std::cell::RefCell::new(
                p.new_resize_protocol(img),
            ))),
            _ => ArtThumb::Missing,
        };
        let tick = self.grid_art_clock.get();
        self.grid_art.borrow_mut().insert(res.key, (tick, thumb));
        self.dirty = true;
    }

    /// The art key + source + shape for a browse item: an Album shows its own
    /// embedded cover; an Artist shows an online photo with the newest album's
    /// cover as the embedded fallback. The shape (circle vs rounded square) is the
    /// global `grid_circle` setting — uniform across every card.
    pub(crate) fn item_art(&self, item: &LocalItem) -> Option<(ArtKey, ArtSource, bool)> {
        let circle = self.config.grid_circle;
        match item {
            LocalItem::Album(id) => {
                let path = self
                    .library
                    .tracks_of(*id)
                    .first()
                    .map(|t| t.path.clone())?;
                Some((ArtKey::Album(*id), ArtSource::Embedded(path), circle))
            }
            LocalItem::Artist(id) => {
                let source = self.artist_art_source(*id)?;
                Some((ArtKey::Artist(*id), source, circle))
            }
            _ => None,
        }
    }

    /// Build the online-first artist photo source (Deezer name → embedded newest-
    /// album fallback). Shared by the grid card (`item_art`) and the pane photo.
    fn artist_art_source(&self, id: ArtistId) -> Option<ArtSource> {
        let name = self.library.artists.get(&id).map(|a| a.name.clone())?;
        let fallback = self
            .library
            .albums_of_by_year(id)
            .first()
            .map(|a| a.id)
            .and_then(|aid| self.library.tracks_of(aid).first().map(|t| t.path.clone()));
        Some(ArtSource::Artist { name, fallback })
    }

    /// Art request for the Artist pane's PHOTO: the same image as the grid card but
    /// under a distinct `ArtKey::ArtistPhoto` key, so the large pane render owns its
    /// own `StatefulProtocol` and doesn't thrash the small grid card's resize cache.
    pub(crate) fn artist_pane_art(&self, id: ArtistId) -> Option<(ArtKey, ArtSource, bool)> {
        let source = self.artist_art_source(id)?;
        Some((ArtKey::ArtistPhoto(id), source, self.config.grid_circle))
    }

    /// Toggle the grid card shape (circle ↔ rounded square) and clear the artwork
    /// cache so existing thumbnails re-decode in the new shape. Persists the choice.
    pub(crate) fn toggle_grid_shape(&mut self) {
        self.config.grid_circle = !self.config.grid_circle;
        self.grid_art.borrow_mut().clear();
        self.config.save();
    }

    /// Step the grid card size (small ↔ medium ↔ large) and persist it. No cache
    /// clear needed — the cached image protocols re-scale to the new card rect.
    pub(crate) fn cycle_grid_size(&mut self, dir: i32) {
        self.config.grid_card_size = self.config.grid_card_size.step(dir);
        self.config.save();
    }
}

/// Evict least-recently-used entries until the cache is back within `GRID_ART_CAP`.
/// The just-inserted key has the highest tick, so it's never the one dropped; the
/// oldest off-screen thumbnail goes first. O(n) over a small (≤cap) map, and only
/// runs when a NEW key is inserted (i.e. when new cards scroll into view).
fn evict_lru(cache: &mut ArtCache) {
    while cache.len() > GRID_ART_CAP {
        let Some(oldest) = cache
            .iter()
            .min_by_key(|(_, (tick, _))| *tick)
            .map(|(k, _)| *k)
        else {
            break;
        };
        cache.remove(&oldest);
    }
}
