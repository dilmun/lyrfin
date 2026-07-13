//! Library discovery / serendipity: surface tracks the user might have
//! forgotten or want to rediscover — a random album, "on this day" memories,
//! and long-neglected favorites — plus the tiny PRNG that drives the random
//! pick. These are presentation-agnostic library queries (the smart lists in
//! `find.rs` and the `R` / palette random-album action call into them).

use super::AppState;
use crate::core::model::{AlbumId, Track, TrackId};

impl AppState {
    /// xorshift64 PRNG → an index in `0..n` (no `rand` dependency). Seeded lazily
    /// from the wall clock on first use. `n == 0` yields 0.
    pub(super) fn next_rand_below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        if self.rng == 0 {
            self.rng = crate::datetime::now_unix()
                .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                .max(1)
                | 1;
        }
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        (x % n as u64) as usize
    }

    /// Pick a random album, browse it into the tracklist, and start playing it.
    pub(super) fn random_album(&mut self) {
        let mut ids: Vec<AlbumId> = self.library.albums.keys().copied().collect();
        if ids.is_empty() {
            self.notify("No albums to pick from".into());
            return;
        }
        ids.sort_by_key(|id| id.get()); // stable order so the PRNG, not HashMap, decides
        let pick = ids[self.next_rand_below(ids.len())];
        let title = self
            .library
            .albums
            .get(&pick)
            .map(|a| a.title.clone())
            .unwrap_or_default();
        let track_ids: Vec<TrackId> = self.library.tracks_of(pick).iter().map(|t| t.id).collect();
        if track_ids.is_empty() {
            self.notify("Random album was empty".into());
            return;
        }
        self.browse(title.clone(), track_ids);
        self.selection = 0;
        // play in the displayed (sorted) order so the queue matches the view
        self.player.queue.items = self.browser.list.clone();
        self.player.queue.position = 0;
        self.player.current = self.player.queue.items.first().copied();
        self.play_current();
        self.notify(format!("Random album: {title}"));
    }

    /// Tracks added on today's calendar day (month + day) in an earlier year —
    /// a "memory" list. Most recent prior year first.
    pub(super) fn on_this_day_ids(&self) -> Vec<TrackId> {
        let (cy, cm, cd) = crate::datetime::ymd_from_unix(crate::datetime::now_unix());
        let mut v: Vec<&Track> = self
            .library
            .tracks
            .values()
            .filter(|t| t.added_at > 0)
            .filter(|t| {
                let (y, m, d) = crate::datetime::ymd_from_unix(t.added_at as u64);
                m == cm && d == cd && y < cy
            })
            .collect();
        v.sort_by_key(|b| std::cmp::Reverse(b.added_at));
        v.into_iter().map(|t| t.id).collect()
    }

    /// Tracks played before but not in the last 90 days (loved-then-neglected).
    /// Falls back to `added_at` for plays recorded before `last_played` existed.
    /// Most-forgotten (oldest activity) first.
    pub(super) fn forgotten_ids(&self) -> Vec<TrackId> {
        const FORGOTTEN_DAYS: u64 = 90;
        let now = crate::datetime::now_unix();
        let cutoff = now.saturating_sub(FORGOTTEN_DAYS * crate::datetime::DAY);
        let last = |t: &Track| -> u64 {
            if t.last_played > 0 {
                t.last_played as u64
            } else {
                t.added_at as u64
            }
        };
        let mut v: Vec<&Track> = self
            .library
            .tracks
            .values()
            .filter(|t| t.play_count > 0)
            .filter(|t| {
                let l = last(t);
                l > 0 && l < cutoff
            })
            .collect();
        v.sort_by_key(|a| last(a));
        v.into_iter().map(|t| t.id).collect()
    }
}
