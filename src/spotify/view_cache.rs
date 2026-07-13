//! Optimistic on-disk snapshot of the Spotify browse view, so the next launch shows
//! the list you left on **instantly** while the network reconnects and refreshes it
//! behind the scenes — instead of a blank pane until librespot is up and the section
//! + drill-in have been re-fetched.
//!
//! Reliability contract:
//! - **Fail-safe read.** Any error (missing / unreadable / schema drift across builds)
//!   yields `None` — the launch simply falls back to the normal network load. A stale
//!   cache never crashes or corrupts the view.
//! - **Atomic write.** temp-file + rename, so a crash mid-write can't truncate it.
//! - **Account-scoped.** Tied to `account_id`; the app drops it on a mismatch
//!   (`spotify_reset_browse_and_queue`) so one account's view is never shown to another.
//! - **Bounded.** Item lists are capped ([`MAX_ITEMS`]) so a huge playlist can't bloat
//!   the file or slow the parse — the cache is a preview; the refresh loads the rest.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::spotify::api::{Item, Section};

/// Cap the cached item list — enough to fill any screen; the background refresh
/// replaces it with the full list. Keeps the cache small + fast to parse.
pub const MAX_ITEMS: usize = 500;

const FILE: &str = "spotify_view.json";

/// A persisted snapshot of the Spotify browse view (the section/search context, the
/// currently-shown list + cursor, and the drilled-in container if any). Every field
/// defaults so the struct stays readable as it evolves; a value that no longer fits
/// (e.g. an `Item` field changed) fails the whole load, which the caller treats as
/// "no cache" — a safe fall-back to the network.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SpotifyViewCache {
    /// Whose view this is — validated against the connected account on reconnect.
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
    pub section: Section,
    #[serde(default)]
    pub in_search: bool,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub sel: usize,
    /// Breadcrumb when a container is open (e.g. "≡ Top 50"); `None` at the top level.
    #[serde(default)]
    pub crumb: Option<String>,
    /// The drilled-in container whose tracks `items` holds — re-fetched in place on
    /// reconnect. `None` at the section/search level.
    #[serde(default)]
    pub open_item: Option<Item>,
    /// The currently-visible list (a container's tracks, or the section/search list).
    #[serde(default)]
    pub items: Vec<Item>,
}

impl SpotifyViewCache {
    /// Read the cache, or `None` on any error (missing / unreadable / schema drift) —
    /// a bad cache degrades to a normal network load, never a failure.
    pub fn load(dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(dir.join(FILE)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Write atomically (temp + rename) so a crash mid-write can't leave a truncated
    /// file the next launch fails to parse. Silently no-ops on I/O errors.
    pub fn save(&self, dir: &Path) {
        let Ok(json) = serde_json::to_string(self) else {
            return;
        };
        let _ = std::fs::create_dir_all(dir);
        let tmp = dir.join(format!("{FILE}.tmp"));
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(&tmp, dir.join(FILE));
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }

    /// Remove the cache (on logout / account switch, or when there's nothing to cache).
    pub fn delete(dir: &Path) {
        let _ = std::fs::remove_file(dir.join(FILE));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spotify::api::Kind;

    #[test]
    fn round_trips_and_is_fail_safe() {
        let dir = std::env::temp_dir().join("lyrfin_spotify_view_cache");
        let _ = std::fs::remove_dir_all(&dir);

        // absent → None (first run)
        assert!(SpotifyViewCache::load(&dir).is_none());

        let c = SpotifyViewCache {
            account_id: "acc1".into(),
            section: Section::Browse,
            crumb: Some("≡ Top 50".into()),
            open_item: Some(Item {
                uri: "spotify:playlist:chart".into(),
                name: "Top 50".into(),
                kind: Kind::Playlist,
                ..Default::default()
            }),
            items: vec![Item {
                uri: "spotify:track:1".into(),
                name: "Song".into(),
                kind: Kind::Track,
                ..Default::default()
            }],
            sel: 0,
            ..Default::default()
        };
        c.save(&dir);
        assert!(
            !dir.join(format!("{FILE}.tmp")).exists(),
            "temp file renamed away"
        );
        let back = SpotifyViewCache::load(&dir).expect("round-trips");
        assert_eq!(back.account_id, "acc1");
        assert_eq!(back.section, Section::Browse);
        assert_eq!(
            back.open_item.as_ref().map(|i| i.uri.as_str()),
            Some("spotify:playlist:chart")
        );
        assert_eq!(back.items.len(), 1);

        // a corrupt file → None (fall back to the network), never a panic
        std::fs::write(dir.join(FILE), "{ not valid json").unwrap();
        assert!(SpotifyViewCache::load(&dir).is_none());

        SpotifyViewCache::delete(&dir);
        assert!(SpotifyViewCache::load(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
