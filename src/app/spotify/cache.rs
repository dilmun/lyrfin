//! Optimistic Spotify view cache — capture the browse view on exit and restore it
//! instantly on the next launch, so the pane shows the list you left on while the
//! network reconnects and refreshes it behind the scenes (see
//! [`crate::spotify::view_cache`]). The refresh-in-place itself lives in `browse`
//! (`spotify_load_initial` → `spotify_refresh_open`); this module is only the
//! disk snapshot + its account-safety gate.

use super::AppState;
use crate::spotify::view_cache::{MAX_ITEMS, SpotifyViewCache};

impl AppState {
    /// Snapshot the current Spotify browse view to disk (called on exit). No-op unless
    /// we know the connected account — the cache is account-scoped, so an unknown
    /// account can't be tied safely and any stale cache is dropped instead. An empty
    /// view is also dropped (nothing worth restoring).
    pub(crate) fn spotify_save_view_cache(&self) {
        let dir = &self.config.dir;
        let Some(account_id) = self.spotify.account_id.clone() else {
            SpotifyViewCache::delete(dir);
            return;
        };
        if self.spotify.items.is_empty() && self.spotify.open_item.is_none() {
            SpotifyViewCache::delete(dir);
            return;
        }
        let items = self.spotify.items.iter().take(MAX_ITEMS).cloned().collect();
        SpotifyViewCache {
            account_id,
            section: self.spotify.section,
            in_search: self.spotify.in_search,
            query: self.spotify.query.clone(),
            sel: self.spotify.sel,
            crumb: self.spotify.crumb.clone(),
            open_item: self.spotify.open_item.clone(),
            items,
        }
        .save(dir);
    }

    /// Restore the persisted browse view at launch so it shows immediately, before the
    /// network reconnects. Gated on having a token (else we won't reconnect to refresh,
    /// so we'd be stranding stale data the user can't act on). The account is validated
    /// on `AuthEvent::Connected`; a mismatch clears everything
    /// (`spotify_reset_browse_and_queue`). No-op when there's no cache.
    pub(crate) fn spotify_apply_view_cache(&mut self) {
        if self.spotify.tokens.is_none() {
            return; // won't reconnect → don't show un-refreshable stale data
        }
        let Some(cache) = SpotifyViewCache::load(&self.config.dir) else {
            return;
        };
        // Tie the shown view to its account so the existing reconnect check can drop it
        // on a mismatch. `apply_session` normally sets this from the session; back it up
        // from the cache in case the session lacked it.
        if self.spotify.restored_account.is_none() {
            self.spotify.restored_account = Some(cache.account_id);
        }
        self.spotify.section = cache.section;
        self.spotify.in_search = cache.in_search;
        self.spotify.query = cache.query;
        self.spotify.items = cache.items;
        self.spotify.sel = cache.sel.min(self.spotify.items.len().saturating_sub(1));
        self.spotify.crumb = cache.crumb;
        self.spotify.open_item = cache.open_item;
        // The cache's cursor is authoritative for the shown list, so drop the session's
        // pending cursor/drill — `spotify_load_initial` refreshes the open container
        // directly on reconnect rather than re-deriving it from the session.
        self.spotify.restore_sel = None;
        if self.spotify.open_item.is_some() {
            self.spotify.restore_open = None;
        }
    }
}
