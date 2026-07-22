//! Multi-select on `AppState`: Vim-style visual selection (`V`) + per-item marks
//! (`x`) over whatever list is focused — the local library tracklist, the radio
//! station list, or a Spotify track list. The selection is a set of tagged keys
//! ([`MarkKey`]) plus a shared visual-mode anchor (an index into the focused
//! list); each view resolves the set to its own items for the bulk operators
//! (tag edit / favourite / add-to-playlist / star / like).

use super::*;

/// A selected item, tagged by which list it belongs to so one marks set serves
/// every view — the identity type differs (a local [`TrackId`], a radio station
/// key (uuid or stream url), or a Spotify item `uri`).
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum MarkKey {
    Track(crate::core::model::TrackId),
    Station(String),
    Spotify(String),
}

/// Bulk multi-select state: `ids` is the marked set (`x` toggles, drives the ✓
/// marks + bulk operators), `anchor` the Vim visual-mode anchor — `Some` while
/// visual mode is active, giving the live range `anchor..=cursor`.
#[derive(Default)]
pub struct MultiSelect {
    pub ids: std::collections::HashSet<MarkKey>,
    pub anchor: Option<usize>,
}

/// Which selectable list is focused right now. The backing cursor + item identity
/// differ per view, but marking / visual selection treat them uniformly once
/// resolved to `(cursor, keys)`.
enum ActiveList {
    /// Local library search results (`self.selection` over `search_results`).
    Search,
    /// The Dashboard's local browse list, when it's a flat run of tracks.
    Local,
    /// The Radio station list under `radio.sel` (`radio_view_list`).
    Radio,
    /// A Spotify flat track list (Liked, or a drilled playlist / album).
    Spotify,
}

impl AppState {
    /// The focused selectable list, or `None` when the current view has none (a
    /// container/grid, the section sidebar, a non-track pane). The ONE place that
    /// decides "what list is here", so marking, visual selection, the bulk
    /// operators, and the renderers all agree.
    fn active_list(&self) -> Option<ActiveList> {
        match self.layout {
            Layout::Radio => self.radio_list_focused().then_some(ActiveList::Radio),
            Layout::Spotify => self
                .spotify_tracklist_focused()
                .then_some(ActiveList::Spotify),
            _ if self.is_searching() => Some(ActiveList::Search),
            Layout::Dashboard
                if self.focus == Focus::Main
                    && !self.local.items.is_empty()
                    && self.local.items.iter().all(|i| i.is_track()) =>
            {
                Some(ActiveList::Local)
            }
            _ => None,
        }
    }

    /// The Radio station table (not the flat playlist list) is focused, with no
    /// sub-mode (filter picker / search box / playlist modal) capturing input.
    fn radio_list_focused(&self) -> bool {
        self.focus == Focus::Main
            && self.radio.picker.is_none()
            && !self.radio.editing
            && !self.radio.pl.modal_open()
            && !(self.radio.section == RadioSection::Playlists && self.radio.pl.open.is_none())
    }

    /// The Spotify main list is a focused flat run of tracks (Liked, a drilled
    /// playlist / album) — not a grid, a container list, or the search box.
    fn spotify_tracklist_focused(&self) -> bool {
        self.focus == Focus::Main
            && !self.spotify.searching
            && !self.spotify.items.is_empty()
            && self
                .spotify
                .items
                .iter()
                .all(|it| it.kind == crate::spotify::api::Kind::Track)
    }

    /// The cursor row of the focused list (clamped in-bounds), or `None` when
    /// there is no selectable list. Cheap — no allocation.
    pub(crate) fn sel_cursor(&self) -> Option<usize> {
        let clamp = |i: usize, len: usize| i.min(len.saturating_sub(1));
        Some(match self.active_list()? {
            ActiveList::Search => clamp(self.selection, self.display_len()),
            ActiveList::Local => clamp(self.local.sel, self.local.items.len()),
            ActiveList::Radio => clamp(self.radio.sel, self.radio_view_list().len()),
            ActiveList::Spotify => clamp(self.spotify.sel, self.spotify.items.len()),
        })
    }

    /// The focused list's items as tagged keys, in display order (empty when there
    /// is none). Allocates — call from key handlers, not the render loop.
    pub(crate) fn sel_keys(&self) -> Vec<MarkKey> {
        match self.active_list() {
            Some(ActiveList::Search) => {
                self.display_ids().into_iter().map(MarkKey::Track).collect()
            }
            Some(ActiveList::Local) => self
                .local
                .items
                .iter()
                .filter_map(|i| match i {
                    LocalItem::Track(id) => Some(MarkKey::Track(*id)),
                    _ => None,
                })
                .collect(),
            Some(ActiveList::Radio) => self
                .radio_view_list()
                .iter()
                .map(|st| MarkKey::Station(station_key(st).to_string()))
                .collect(),
            Some(ActiveList::Spotify) => self
                .spotify
                .items
                .iter()
                .map(|it| MarkKey::Spotify(it.uri.clone()))
                .collect(),
            None => Vec::new(),
        }
    }

    /// The keys currently selected in the focused list — the marked set plus the
    /// live visual range, in display order. With nothing marked/anchored, the
    /// single item under the cursor (empty when there is no list). Each view's
    /// resolver filters this to its own key variant.
    pub(crate) fn selected_keys(&self) -> Vec<MarkKey> {
        let keys = self.sel_keys();
        if self.marks.ids.is_empty() && self.marks.anchor.is_none() {
            return self
                .sel_cursor()
                .and_then(|c| keys.into_iter().nth(c))
                .into_iter()
                .collect();
        }
        let vis = self.visual_range();
        keys.into_iter()
            .enumerate()
            .filter(|(i, k)| {
                self.marks.ids.contains(k) || vis.is_some_and(|(lo, hi)| *i >= lo && *i <= hi)
            })
            .map(|(_, k)| k)
            .collect()
    }

    /// Clear the marked set + visual anchor. Called when the selection can no
    /// longer refer to the visible list — a view switch, or after a bulk operator
    /// consumes it.
    pub(crate) fn clear_marks(&mut self) {
        self.marks.ids.clear();
        self.marks.anchor = None;
    }

    /// Whether a multi-selection (a marked set or a live visual range) is active.
    pub(crate) fn has_selection(&self) -> bool {
        !self.marks.ids.is_empty() || self.marks.anchor.is_some()
    }

    /// `x`: in visual mode, commit the live range to the marked set and leave it;
    /// otherwise toggle the item under the cursor and advance. A no-op when no
    /// selectable list is focused.
    pub(crate) fn toggle_mark(&mut self) {
        let Some(cursor) = self.sel_cursor() else {
            return;
        };
        let keys = self.sel_keys();
        if let Some(a) = self.marks.anchor.take() {
            let (lo, hi) = (a.min(cursor), a.max(cursor));
            for k in keys.iter().take(hi + 1).skip(lo) {
                self.marks.ids.insert(k.clone());
            }
        } else if let Some(k) = keys.get(cursor) {
            if !self.marks.ids.remove(k) {
                self.marks.ids.insert(k.clone());
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
        // A player view has no selection of its own: `f` means "favourite what is
        // playing", which for Spotify or radio is that source's own star. Without
        // this it favourited whatever local track happened to be loaded behind a
        // Spotify one.
        match self
            .layout
            .is_player_view()
            .then(|| self.now_playing_source())
            .flatten()
        {
            Some(crate::app::NpSource::Spotify) => {
                self.spotify_like_selection();
                return;
            }
            Some(crate::app::NpSource::Radio) => {
                self.radio_star();
                return;
            }
            _ => {}
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

    /// `V`: toggle Vim-style visual selection, anchored at the focused list's
    /// cursor. A no-op when no selectable list is focused.
    pub(crate) fn toggle_visual(&mut self) {
        if self.marks.anchor.is_some() {
            self.marks.anchor = None;
        } else if let Some(cursor) = self.sel_cursor() {
            self.marks.anchor = Some(cursor);
        }
    }

    /// The live visual range `(lo, hi)` over the focused list, or `None` when not
    /// in visual mode. Renderer + resolvers both read it, so highlight == effect.
    pub(crate) fn visual_range(&self) -> Option<(usize, usize)> {
        let (anchor, cursor) = (self.marks.anchor?, self.sel_cursor()?);
        Some((anchor.min(cursor), anchor.max(cursor)))
    }

    /// The local library tracks currently selected (marked set / visual range,
    /// else the cursor track, falling back to the now-playing track). The target
    /// every local bulk operator — tag edit, favourite, add-to-playlist — resolves.
    pub(crate) fn selected_track_ids(&self) -> Vec<crate::core::model::TrackId> {
        let ids: Vec<_> = self
            .selected_keys()
            .into_iter()
            .filter_map(|k| match k {
                MarkKey::Track(id) => Some(id),
                _ => None,
            })
            .collect();
        if ids.is_empty() {
            self.player.current.into_iter().collect()
        } else {
            ids
        }
    }

    /// The radio stations currently selected (marked set / visual range, else the
    /// cursor station), resolved back to `Station`s in display order.
    pub(crate) fn selected_stations(&self) -> Vec<crate::radio::Station> {
        let keys: std::collections::HashSet<String> = self
            .selected_keys()
            .into_iter()
            .filter_map(|k| match k {
                MarkKey::Station(s) => Some(s),
                _ => None,
            })
            .collect();
        self.radio_view_list()
            .iter()
            .filter(|st| keys.contains(station_key(st)))
            .cloned()
            .collect()
    }

    /// The Spotify track URIs currently selected (marked set / visual range, else
    /// the cursor track), in display order.
    pub(crate) fn selected_spotify_uris(&self) -> Vec<String> {
        self.selected_keys()
            .into_iter()
            .filter_map(|k| match k {
                MarkKey::Spotify(uri) => Some(uri),
                _ => None,
            })
            .collect()
    }
}
