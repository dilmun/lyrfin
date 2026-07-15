//! Track marking + Vim-style visual selection on `AppState` (extracted from
//! app/mod.rs): toggling marks (per-track, per-album/artist on the tree, or
//! committing a visual range), the visual-mode anchor, and the derived
//! interaction mode. The marked set + live visual range feed the bulk operators
//! (tag edit / add-to-playlist) via `selected_track_ids`.

use super::*;

/// Bulk multi-select state, grouped out of `AppState` (the state behind this
/// module): `ids` is the marked-track set (`x` toggles, drives the ✓ marks +
/// bulk operators), `anchor` the Vim visual-mode anchor — `Some` while visual
/// mode is active, with the live range `anchor..=selection`.
#[derive(Default)]
pub struct MultiSelect {
    pub ids: std::collections::HashSet<crate::core::model::TrackId>,
    pub anchor: Option<usize>,
}

/// Which flat track list is focused right now — the backing model differs
/// (search results vs the local browse list) but marking / visual selection /
/// the bulk operators treat them identically once resolved to `(ids, cursor)`.
enum ActiveList {
    /// The library search results (`self.selection` over `search_results`).
    Search,
    /// The Dashboard's local browse list, when it's a flat run of tracks — a
    /// drilled album/playlist/genre or a leaf section (`self.local.sel` over the
    /// track ids of `self.local.items`).
    Local,
}

impl AppState {
    /// The focused flat track list, or `None` when the current view has no
    /// selectable tracklist (a container / grid / artist page, the section
    /// sidebar, or the Radio/Spotify views). This is the ONE place that decides
    /// "what is the tracklist here", so marking, visual selection, and the bulk
    /// operators always agree with what the tracklist renderer draws.
    fn active_list(&self) -> Option<ActiveList> {
        if matches!(self.layout, Layout::Radio | Layout::Spotify) {
            return None;
        }
        if self.is_searching() {
            return Some(ActiveList::Search);
        }
        if self.layout == Layout::Dashboard
            && self.focus == Focus::Main
            && !self.local.items.is_empty()
            && self.local.items.iter().all(|i| i.is_track())
        {
            return Some(ActiveList::Local);
        }
        None
    }

    /// The cursor row of the focused tracklist (clamped in-bounds). Cheap — no
    /// allocation — so the renderer / status bar can call it per frame.
    pub(crate) fn active_track_cursor(&self) -> Option<usize> {
        Some(match self.active_list()? {
            ActiveList::Search => self.selection.min(self.display_len().saturating_sub(1)),
            ActiveList::Local => self.local.sel.min(self.local.items.len().saturating_sub(1)),
        })
    }

    /// The track ids of the focused tracklist, in display order (empty when there
    /// is none). Allocates — call it from key handlers, not the render loop.
    pub(crate) fn active_track_ids(&self) -> Vec<crate::core::model::TrackId> {
        match self.active_list() {
            Some(ActiveList::Search) => self.display_ids(),
            Some(ActiveList::Local) => self
                .local
                .items
                .iter()
                .filter_map(|i| match i {
                    LocalItem::Track(id) => Some(*id),
                    _ => None,
                })
                .collect(),
            None => Vec::new(),
        }
    }

    /// `x`: in visual mode, commit the live range to the marked set and leave
    /// visual mode; otherwise toggle the current track and advance. A no-op when
    /// no tracklist is focused (the section sidebar, a container/grid view, …).
    pub(crate) fn toggle_mark(&mut self) {
        let Some(cursor) = self.active_track_cursor() else {
            return;
        };
        let ids = self.active_track_ids();
        if let Some(a) = self.marks.anchor.take() {
            let (lo, hi) = (a.min(cursor), a.max(cursor));
            for i in lo..=hi {
                if let Some(&id) = ids.get(i) {
                    self.marks.ids.insert(id);
                }
            }
        } else if let Some(&id) = ids.get(cursor) {
            if !self.marks.ids.remove(&id) {
                self.marks.ids.insert(id);
            }
            self.move_selection(Motion::Down);
        }
    }

    /// `f`: toggle favourite on the current selection — the marked set / visual
    /// range, else the cursor track (falling back to the now-playing track), the
    /// same target every bulk operator (tag edit / add-to-playlist) resolves. If
    /// the whole selection is already favourited it un-favourites; otherwise it
    /// favourites all of them (so a mixed selection becomes all-favourite).
    pub(crate) fn toggle_favorite_selection(&mut self) {
        // Favouriting is a local-library concept — the Radio/Spotify views have
        // their own star keys (RadioStar / SpotifyLike) and their "selection" is
        // not an editable local file.
        if matches!(self.layout, Layout::Radio | Layout::Spotify) {
            return;
        }
        let ids = self.selected_track_ids();
        if ids.is_empty() {
            return;
        }
        // favourite unless every selected track is already favourited (then clear)
        let make_fav = !ids
            .iter()
            .all(|id| self.library.track(*id).is_some_and(|t| t.favorite));
        let mut changed = 0usize;
        for id in &ids {
            if let Some(t) = self.library.tracks.get_mut(id)
                && t.favorite != make_fav
            {
                t.favorite = make_fav;
                changed += 1;
            }
        }
        self.library.favorites = self
            .library
            .tracks
            .values()
            .filter(|t| t.favorite)
            .map(|t| t.id)
            .collect();
        self.search.lib_gen += 1; // fav:/rating: searches may change
        let noun = if changed == 1 { "track" } else { "tracks" };
        self.notify(if make_fav {
            format!("♥ Favorited {changed} {noun}")
        } else {
            format!("Removed {changed} {noun} from favorites")
        });
    }

    /// The global interaction mode ([`Mode`]). Not yet shown in the status bar.
    #[allow(dead_code)]
    pub fn mode(&self) -> Mode {
        if self.tags.edit.is_some() {
            Mode::Edit
        } else if self.marks.anchor.is_some() {
            Mode::Visual
        } else {
            Mode::View
        }
    }

    /// `V`: toggle Vim-style visual selection, anchored at the focused tracklist's
    /// cursor. A no-op when no tracklist is focused.
    pub(crate) fn toggle_visual(&mut self) {
        if self.marks.anchor.is_some() {
            self.marks.anchor = None;
        } else if let Some(cursor) = self.active_track_cursor() {
            self.marks.anchor = Some(cursor);
        }
    }

    /// The live visual range `(lo, hi)` over the focused tracklist, or `None` when
    /// not in visual mode. The renderer draws it and [`selected_track_ids`] resolves
    /// it — both against the same `(anchor, cursor)`, so highlight and effect agree.
    pub(crate) fn visual_range(&self) -> Option<(usize, usize)> {
        let (anchor, cursor) = (self.marks.anchor?, self.active_track_cursor()?);
        Some((anchor.min(cursor), anchor.max(cursor)))
    }
}
