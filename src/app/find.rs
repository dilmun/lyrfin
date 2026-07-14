//! Search + smart-list queries methods on `AppState` (extracted from app/mod.rs).

use super::*;

/// Local-library search state, grouped out of `AppState` — the engine behind
/// `search_results`/`ensure_search`. `active` + `query` are the search box;
/// `cache` holds the last computed result, `index` the prefix-token inverted
/// index, and `lib_gen` is bumped on any library/user-data change to invalidate
/// both caches.
#[derive(Default)]
pub struct SearchState {
    pub active: bool,
    pub query: String,
    pub lib_gen: u64,
    pub cache: std::cell::RefCell<SearchCache>,
    pub index: std::cell::RefCell<crate::library::index::SearchIndex>,
}

impl AppState {
    /// Insert a char at the query caret (char-indexed, so Arabic edits cleanly).
    pub fn query_insert(&mut self, c: char) {
        if let Some((q, caret)) = self.active_query() {
            let mut chars: Vec<char> = q.chars().collect();
            let i = (*caret).min(chars.len());
            chars.insert(i, c);
            *caret = i + 1;
            *q = chars.into_iter().collect();
        }
    }

    /// Delete the char before the caret (Backspace) or at it (`forward` = Delete).
    pub fn query_del(&mut self, forward: bool) {
        if let Some((q, caret)) = self.active_query() {
            let mut chars: Vec<char> = q.chars().collect();
            let i = (*caret).min(chars.len());
            if forward {
                if i < chars.len() {
                    chars.remove(i);
                }
            } else if i > 0 {
                chars.remove(i - 1);
                *caret = i - 1;
            }
            *q = chars.into_iter().collect();
        }
    }

    /// Move the query caret.
    pub fn query_caret(&mut self, c: crate::action::Caret) {
        use crate::action::Caret;
        if let Some((q, caret)) = self.active_query() {
            let len = q.chars().count();
            *caret = match c {
                Caret::Left => caret.saturating_sub(1),
                Caret::Right => (*caret + 1).min(len),
                Caret::Home => 0,
                Caret::End => len,
            };
        }
    }

    /// Track ids for a smart/discovery list (the resolver behind the local
    /// library's derived section lists).
    pub fn smart_ids(&self, list: SmartList) -> Vec<crate::core::model::TrackId> {
        match list {
            SmartList::AllTracks => self.library.all_tracks_sorted(),
            SmartList::RecentlyAdded => self.library.recently_added.clone(),
            SmartList::RecentlyPlayed => self.library.recently_played.clone(),
            SmartList::MostPlayed => self.library.most_played.clone(),
            SmartList::Favorites => self.library.favorites.clone(),
            SmartList::OnThisDay => self.on_this_day_ids(),
            SmartList::Forgotten => self.forgotten_ids(),
            SmartList::Duplicates => self.library.duplicates.clone(),
            SmartList::Untagged => self.library.untagged.clone(),
        }
    }

    /// Count for a smart list. O(1) for the precomputed lists; the date-relative
    /// ones (On This Day / Forgotten) are computed. Pairs with `smart_ids`; kept
    /// complete for the discovery views the sections model doesn't surface yet.
    #[allow(dead_code)] // counterpart of smart_ids; re-wired when discovery sections land
    pub fn smart_count(&self, list: SmartList) -> usize {
        match list {
            SmartList::AllTracks => self.library.track_count(),
            SmartList::RecentlyAdded => self.library.recently_added.len(),
            SmartList::RecentlyPlayed => self.library.recently_played.len(),
            SmartList::MostPlayed => self.library.most_played.len(),
            SmartList::Favorites => self.library.favorites.len(),
            SmartList::OnThisDay => self.on_this_day_ids().len(),
            SmartList::Forgotten => self.forgotten_ids().len(),
            SmartList::Duplicates => self.library.duplicates.len(),
            SmartList::Untagged => self.library.untagged.len(),
        }
    }

    /// Filter + sort the in-memory directory by the current query / country /
    /// genre / sort and publish the top results. Pure local work — instant.
    pub(crate) fn run_local_search(&mut self) {
        let q = self.radio.query.trim().to_lowercase();
        let code = self.radio.country.as_ref().map(|(_, c)| c.clone());
        let tag = self.radio.tag.as_ref().map(|t| t.to_lowercase());
        let mut matched: Vec<usize> = (0..self.radio.all_stations.len())
            .filter(|&i| {
                (q.is_empty() || self.radio.name_lc[i].contains(&q))
                    && code.as_deref().is_none_or(|c| {
                        self.radio.all_stations[i]
                            .countrycode
                            .eq_ignore_ascii_case(c)
                    })
                    && tag
                        .as_deref()
                        .is_none_or(|t| self.radio.tags_lc[i].contains(t))
            })
            .collect();
        // all_stations is pre-sorted by clickcount, so Popular needs no re-sort
        let st = &self.radio.all_stations;
        match self.radio.sort {
            crate::radio::Sort::Popular => {}
            crate::radio::Sort::Votes => matched.sort_by(|&a, &b| st[b].votes.cmp(&st[a].votes)),
            crate::radio::Sort::Bitrate => {
                matched.sort_by(|&a, &b| st[b].bitrate.cmp(&st[a].bitrate))
            }
            crate::radio::Sort::Name => {
                let nl = &self.radio.name_lc;
                matched.sort_by(|&a, &b| nl[a].cmp(&nl[b]));
            }
        }
        let total = matched.len();
        self.radio.stations = matched
            .iter()
            .take(Self::LOCAL_RESULT_CAP)
            .map(|&i| self.radio.all_stations[i].clone())
            .collect();
        self.radio.loading = false;
        self.radio.note = if total == 0 {
            String::new() // the empty-state panel explains it
        } else if total > self.radio.stations.len() {
            format!("top {} of {total}", self.radio.stations.len())
        } else {
            String::new()
        };
        // keep the cursor on the now-playing station if it's still in view
        self.radio.sel = self
            .rnow
            .now_station
            .as_ref()
            .and_then(|cur| {
                let id = station_key(cur);
                self.radio
                    .stations
                    .iter()
                    .position(|s| station_key(s) == id)
            })
            .unwrap_or(0);
    }

    /// Set the query and run a local search (used after the directory loads).
    pub(crate) fn run_local_search_with(&mut self, query: String) {
        self.radio.query = query;
        self.run_local_search();
    }

    /// Borrow the current search results (cached).
    pub fn search_results(&self) -> std::cell::Ref<'_, Vec<crate::core::model::TrackId>> {
        self.ensure_search();
        std::cell::Ref::map(self.search.cache.borrow(), |c| &c.ids)
    }

    /// `true` when the tracklist shows search results (vs a browse list / queue).
    pub(crate) fn is_searching(&self) -> bool {
        self.search.active || !self.search.query.is_empty()
    }

    /// Rebuild the prefix-token search index iff it's stale (the library
    /// generation moved). Query changes never touch `lib_gen`, so the index is
    /// built once per generation and reused across every keystroke in it.
    fn ensure_index(&self) {
        if self.search.index.borrow().is_fresh(self.search.lib_gen) {
            return;
        }
        self.search
            .index
            .borrow_mut()
            .rebuild(self.library.tracks.values(), self.search.lib_gen);
    }

    /// Recompute the search result into the cache only when the query / sort /
    /// library generation has changed (not per frame).
    fn ensure_search(&self) {
        {
            let c = self.search.cache.borrow();
            if c.lib_gen == self.search.lib_gen
                && c.sort == self.sort
                && c.query == self.search.query
            {
                return;
            }
        }
        // a field/OR/negation query filters precisely (and is reused by smart
        // playlists); plain words keep using fuzzy ranking.
        let q = crate::query::Query::parse(&self.search.query);
        let ids = if q.is_structured() {
            let ids: Vec<_> = self
                .library
                .all_tracks_sorted()
                .into_iter()
                .filter(|id| self.library.track(*id).is_some_and(|t| q.matches(t)))
                .collect();
            self.sort_ids(ids)
        } else {
            // Plain-word fuzzy search, narrowed by the prefix-token index: take
            // its candidate set and let nucleo rank just those. The index returns
            // an empty/None set for typos and mid-word queries it can't express —
            // there we fall back to a full fuzzy scan so nothing fuzzy would have
            // matched is dropped.
            self.ensure_index();
            let idx = self.search.index.borrow();
            let ranked = match idx.candidates(&self.search.query) {
                Some(cand) if !cand.is_empty() => {
                    let tracks = cand
                        .iter()
                        .filter_map(|b| self.library.track(crate::core::model::TrackId::new(b)));
                    search::search(tracks, &self.search.query)
                }
                _ => search::search(self.library.tracks.values(), &self.search.query),
            };
            ranked.into_iter().map(|m| m.id).collect()
        };
        let mut c = self.search.cache.borrow_mut();
        c.query = self.search.query.clone();
        c.sort = self.sort.clone();
        c.lib_gen = self.search.lib_gen;
        c.ids = ids;
    }

    /// Track ids the tracklist should show: search results, else a browsed
    /// album/playlist, else the current queue. Prefer [`display_len`] /
    /// [`search_results`] / the borrowed `browse_list`/`queue.items` in hot paths
    /// — this clones and is for one-shot logic callers.
    pub fn display_ids(&self) -> Vec<crate::core::model::TrackId> {
        if self.is_searching() {
            self.search_results().clone()
        } else if !self.browser.list.is_empty() {
            self.browser.list.clone()
        } else {
            self.player.queue.items.clone()
        }
    }

    /// Number of rows the tracklist shows — without cloning the list.
    pub fn display_len(&self) -> usize {
        if self.is_searching() {
            self.search_results().len()
        } else if !self.browser.list.is_empty() {
            self.browser.list.len()
        } else {
            self.player.queue.items.len()
        }
    }
}
