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

impl AppState {
    /// `x`: in visual mode, commit the live range to the marked set and leave
    /// visual mode; otherwise toggle the current track and advance. (Marking is a
    /// tracklist concept — a no-op while the section sidebar is focused.)
    pub(crate) fn toggle_mark(&mut self) {
        if self.focus == Focus::Sidebar {
            return;
        }
        if let Some(a) = self.marks.anchor.take() {
            let (lo, hi) = (a.min(self.selection), a.max(self.selection));
            let ids = self.display_ids();
            for i in lo..=hi {
                if let Some(&id) = ids.get(i) {
                    self.marks.ids.insert(id);
                }
            }
        } else if let Some(&id) = self.display_ids().get(self.selection) {
            if !self.marks.ids.remove(&id) {
                self.marks.ids.insert(id);
            }
            self.move_selection(Motion::Down);
        }
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

    /// `V`: toggle Vim-style visual selection (anchor at the current track).
    pub(crate) fn toggle_visual(&mut self) {
        self.marks.anchor = if self.marks.anchor.is_some() {
            None
        } else {
            Some(self.selection)
        };
    }

    /// Is the tracklist row `idx` inside the live visual range?
    pub fn in_visual_range(&self, idx: usize) -> bool {
        self.marks.anchor.is_some_and(|a| {
            let (lo, hi) = (a.min(self.selection), a.max(self.selection));
            idx >= lo && idx <= hi
        })
    }
}
