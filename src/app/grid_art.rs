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
    /// Nothing could be loaded. Negative-cached so a cover-less card doesn't
    /// re-fetch every frame — but *not forever*: `at`/`tries` let a transient
    /// failure be retried, because a network blip must not blank a card for the
    /// life of the process (see [`AppState::request_art`]).
    Missing { at: std::time::Instant, tries: u8 },
}

/// How many times a failed thumbnail is retried before it's accepted as genuinely
/// absent. Small: covers that truly have no art must not re-hit the network
/// forever, but a burst of timeouts should heal itself.
const ART_MAX_RETRIES: u8 = 3;

/// Backoff before retrying a failed thumbnail — 2s, 4s, 8s. Long enough that a
/// rate-limited or saturated fetch has room to recover, short enough that the card
/// fills in while the user is still looking at it.
fn art_retry_after(tries: u8) -> std::time::Duration {
    std::time::Duration::from_secs(2u64 << tries.min(3))
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

/// Why rendered artwork has to be rebuilt. **The single vocabulary for artwork
/// invalidation** — every UI change that can stale a cover names its reason here
/// and calls [`AppState::invalidate_art`], instead of each call site remembering
/// which caches to poke.
///
/// Adding a setting that affects artwork means adding a variant and answering, in
/// one place, what it invalidates. That is deliberate: the bugs this replaces were
/// all "this path cleared the cache, that path forgot to".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtChange {
    /// Card geometry changed (grid card size). Cached protocols were encoded for
    /// the old rect; a re-encode can fail silently and strand the card blank.
    Geometry,
    /// Card shape changed (circle ⇄ rounded square). The mask is baked into the
    /// decoded image by the worker, so the pixels themselves are wrong.
    Shape,
    /// The theme changed. Only matters where the panel colour is baked into the
    /// image — i.e. every protocol except Kitty, which composites transparency
    /// against the cell and so recolours for free.
    Theme,
    /// An overlay covered the artwork and has now closed. Inline images are a
    /// layer the *terminal* owns: drawing a modal over one destroys it, and the
    /// protocol won't re-transmit because, as far as it knows, nothing about its
    /// area changed — so the art stays blank until something forces a rebuild.
    ///
    /// This used to be handled only for the four persistent covers, by calling
    /// `rebuild_persistent_covers` straight from the event loop; grid thumbnails
    /// were left blank, which is why closing the settings overlay stranded every
    /// card it had covered.
    Occluded,
}

impl AppState {
    /// Rebuild whatever `change` staled. The one place that decides which caches a
    /// UI change invalidates.
    ///
    /// Grid thumbnails are dropped and refetched lazily as cards come back on
    /// screen; the persistent covers (now-playing, transport bar, Spotify panes)
    /// retain their source image and rebuild in place, so they never blank.
    pub(crate) fn invalidate_art(&mut self, change: ArtChange) {
        // Theme is the one change that some terminals don't care about: with the
        // Kitty protocol the panel shows *through* the art's transparent corners,
        // so a repaint is enough and re-encoding would be pure cost.
        if change == ArtChange::Theme {
            if !self.art_needs_opaque_bg() {
                return;
            }
            // Protocols bake the underlay in when built, so point it at the new
            // panel colour before anything is rebuilt below.
            self.sync_art_background();
        }
        self.grid_art.borrow_mut().clear();
        self.rebuild_persistent_covers();
        self.dirty = true;
    }

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
                entry.0 = tick; // already cached / in flight → mark it fresh
                // A previous failure is retried once its backoff has passed. This is
                // what stops one bad fetch from blanking a card permanently: clearing
                // the cache (a size or shape change) re-downloads every visible
                // cover at once through a single worker, and whichever requests time
                // out under that burst used to stay blank until a restart.
                let retry = match entry.1 {
                    ArtThumb::Missing { at, tries } => {
                        tries < ART_MAX_RETRIES && at.elapsed() >= art_retry_after(tries)
                    }
                    _ => false,
                };
                if !retry {
                    return;
                }
                entry.1 = ArtThumb::Pending;
            } else {
                cache.insert(key, (tick, ArtThumb::Pending));
                evict_lru(&mut cache);
            }
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
        // Carry the attempt count across a failure so the backoff actually widens
        // and a hopeless cover stops re-requesting after `ART_MAX_RETRIES`.
        let tries = match self.grid_art.borrow().get(&res.key) {
            Some((_, ArtThumb::Missing { tries, .. })) => *tries,
            _ => 0,
        };
        let thumb = match (res.img, self.art.picker.as_ref()) {
            (Some(img), Some(p)) => ArtThumb::Ready(Box::new(std::cell::RefCell::new(
                p.new_resize_protocol(img),
            ))),
            _ => ArtThumb::Missing {
                at: std::time::Instant::now(),
                tries: tries.saturating_add(1),
            },
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
        self.invalidate_art(ArtChange::Shape);
        self.config.save();
    }

    /// Step the grid card size (small ↔ medium ↔ large).
    pub(crate) fn cycle_grid_size(&mut self, dir: i32) {
        self.set_grid_card_size(self.config.grid_card_size.step(dir));
    }

    /// Set the grid card size, dropping the cached thumbnails so every card is
    /// re-encoded for the new geometry. **The single place the size changes** — the
    /// settings value-picker routes here too, so the cache can't be left stale by
    /// one path and cleared by another.
    ///
    /// The cache used to be kept here, on the theory that a cached protocol simply
    /// re-scales itself to the new card rect. It does try — but a re-encode that
    /// fails leaves that entry blank *permanently*: `resize_encode` stores the error
    /// and, on failure, doesn't advance the source hash, so nothing rebuilds it and
    /// nothing reports it. Cycling small → medium → large stranded a spread of
    /// covers that only a restart brought back. Re-encoding from scratch costs one
    /// decode per visible card, and only when the size actually changes.
    pub(crate) fn set_grid_card_size(&mut self, size: crate::config::GridCardSize) {
        if self.config.grid_card_size == size {
            return;
        }
        self.config.grid_card_size = size;
        self.invalidate_art(ArtChange::Geometry);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, GridCardSize};

    fn app() -> AppState {
        AppState::new(Config {
            dir: std::env::temp_dir().join("lyrfin-test-gridsize"),
            ..Config::default()
        })
    }

    /// The regression: cycling the card size used to keep the cached protocols,
    /// and a re-encode that failed left those covers blank until a restart.
    #[test]
    fn changing_card_size_drops_cached_thumbnails() {
        let mut a = app();
        a.grid_art.borrow_mut().insert(
            ArtKey::solid(0x00ff00, true),
            (
                1,
                ArtThumb::Missing {
                    at: std::time::Instant::now(),
                    tries: 0,
                },
            ),
        );
        assert_eq!(a.grid_art.borrow().len(), 1);

        a.set_grid_card_size(GridCardSize::Large);
        assert_eq!(a.config.grid_card_size, GridCardSize::Large);
        assert!(
            a.grid_art.borrow().is_empty(),
            "cache must be dropped so covers re-encode for the new geometry"
        );
    }

    /// Re-selecting the size already in effect must not throw away good art.
    #[test]
    fn setting_the_same_card_size_keeps_the_cache() {
        let mut a = app();
        let size = a.config.grid_card_size;
        a.grid_art.borrow_mut().insert(
            ArtKey::solid(0x00ff00, true),
            (
                1,
                ArtThumb::Missing {
                    at: std::time::Instant::now(),
                    tries: 0,
                },
            ),
        );

        a.set_grid_card_size(size);
        assert_eq!(a.grid_art.borrow().len(), 1, "no change, no invalidation");
    }

    /// Every artwork-staling change routes through one seam, so a new setting
    /// can't quietly forget to invalidate.
    #[test]
    fn every_art_change_drops_the_cache() {
        for change in [ArtChange::Geometry, ArtChange::Shape] {
            let mut a = app();
            a.grid_art.borrow_mut().insert(
                ArtKey::solid(0x00ff00, true),
                (
                    1,
                    ArtThumb::Missing {
                        at: std::time::Instant::now(),
                        tries: 0,
                    },
                ),
            );
            a.invalidate_art(change);
            assert!(a.grid_art.borrow().is_empty(), "{change:?} must invalidate");
        }
    }

    /// A failed fetch is retried after its backoff — a network blip during the
    /// re-download burst that follows a size/shape change must not blank a card
    /// for the life of the process.
    #[test]
    fn a_failed_thumbnail_is_retried_once_its_backoff_passes() {
        let a = app();
        let key = ArtKey::solid(0x00ff00, true);
        // failed long ago -> eligible
        a.grid_art.borrow_mut().insert(
            key,
            (
                1,
                ArtThumb::Missing {
                    at: std::time::Instant::now() - std::time::Duration::from_secs(60),
                    tries: 0,
                },
            ),
        );
        a.request_art(key, ArtSource::Embedded("/nonexistent".into()), true);
        assert!(
            matches!(a.grid_art.borrow().get(&key), Some((_, ArtThumb::Pending))),
            "an expired failure should be retried"
        );
    }

    /// ...but not immediately, and not forever.
    #[test]
    fn a_fresh_or_exhausted_failure_is_not_retried() {
        let a = app();
        let fresh = ArtKey::solid(0x111111, true);
        let spent = ArtKey::solid(0x222222, true);
        let long_ago = std::time::Instant::now() - std::time::Duration::from_secs(600);
        a.grid_art.borrow_mut().insert(
            fresh,
            (
                1,
                ArtThumb::Missing {
                    at: std::time::Instant::now(),
                    tries: 0,
                },
            ),
        );
        a.grid_art.borrow_mut().insert(
            spent,
            (
                1,
                ArtThumb::Missing {
                    at: long_ago,
                    tries: ART_MAX_RETRIES,
                },
            ),
        );
        for k in [fresh, spent] {
            a.request_art(k, ArtSource::Embedded("/nonexistent".into()), true);
            assert!(
                matches!(
                    a.grid_art.borrow().get(&k),
                    Some((_, ArtThumb::Missing { .. }))
                ),
                "must not re-request"
            );
        }
    }

    /// Both entry points must invalidate — the keyboard cycle and the settings
    /// value-picker used to be separate code paths, and only one cleared.
    #[test]
    fn cycling_the_size_also_drops_the_cache() {
        let mut a = app();
        a.grid_art.borrow_mut().insert(
            ArtKey::solid(0x00ff00, true),
            (
                1,
                ArtThumb::Missing {
                    at: std::time::Instant::now(),
                    tries: 0,
                },
            ),
        );

        a.cycle_grid_size(1);
        assert!(a.grid_art.borrow().is_empty());
    }
}
