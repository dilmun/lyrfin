//! Radio station playlists: CRUD plus the name-entry / add-picker / delete-confirm
//! modal flow for the Radio view's Playlists section. Kept out of `app/radio.rs` so
//! that file stays about search / browse / playback. Stations are stored inline in
//! each [`crate::radio::Playlist`] (self-contained value objects), so — unlike the
//! local music `PlaylistStore` — there is no id/path resolution on load.

use super::*;
use crate::radio::{Playlist, Station};

impl AppState {
    /// Playlists sorted by lowercased name — the stable display order the flat-list
    /// cursor (`pl.sel`) and the add-picker index into.
    pub(crate) fn radio_playlists_sorted(&self) -> Vec<&Playlist> {
        let mut v: Vec<&Playlist> = self.radio.playlists.iter().collect();
        v.sort_by_key(|p| p.name.to_lowercase());
        v
    }

    /// The playlist id under the flat-list cursor (`None` when there are none).
    pub(crate) fn radio_selected_playlist(&self) -> Option<u32> {
        let sorted = self.radio_playlists_sorted();
        sorted
            .get(self.radio.pl.sel.min(sorted.len().saturating_sub(1)))
            .map(|p| p.id)
    }

    fn save_radio_playlists(&self) {
        crate::library::store::RadioPlaylists::save(&self.radio.playlists, &self.config.dir);
    }

    /// Next free playlist id (max existing + 1).
    fn next_radio_playlist_id(&self) -> u32 {
        self.radio.playlists.iter().map(|p| p.id).max().unwrap_or(0) + 1
    }

    // ---- drill in / out --------------------------------------------------
    /// Enter the highlighted playlist so its stations show in the main pane; no-op
    /// when the list is empty.
    pub(crate) fn radio_playlist_open(&mut self) {
        if let Some(id) = self.radio_selected_playlist() {
            self.radio.pl.open = Some(id);
            self.radio.sel = 0;
            self.set_focus(Focus::Main);
        }
    }

    /// Leave the open playlist, back to the flat list of playlists.
    pub(crate) fn radio_playlist_back(&mut self) {
        self.radio.pl.open = None;
        self.radio.sel = 0;
    }

    // ---- create / rename / delete ---------------------------------------
    pub(crate) fn radio_begin_new_playlist(&mut self) {
        self.radio.pl.naming = Some(RadioNameTarget::New);
        self.radio.pl.buffer.clear();
    }

    pub(crate) fn radio_begin_rename_playlist(&mut self) {
        if let Some(id) = self.radio_selected_playlist() {
            let name = self
                .radio
                .playlists
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            self.radio.pl.naming = Some(RadioNameTarget::Rename(id));
            self.radio.pl.buffer = name;
        }
    }

    pub(crate) fn radio_name_input(&mut self, s: String) {
        self.radio.pl.buffer = s;
    }

    /// Commit the open name-entry (create or rename), then persist. A pending
    /// "add station" (from the add-picker's New-playlist row) seeds the new list.
    pub(crate) fn radio_confirm_name(&mut self) {
        let name = self.radio.pl.buffer.trim().to_string();
        let target = self.radio.pl.naming.take();
        self.radio.pl.buffer.clear();
        if name.is_empty() {
            self.radio.pl.adding = None; // drop the bridged station too
            return;
        }
        match target {
            Some(RadioNameTarget::New) => {
                let id = self.next_radio_playlist_id();
                let stations = self.radio.pl.adding.take().into_iter().collect();
                self.radio.playlists.push(Playlist { id, name, stations });
                self.save_radio_playlists();
                self.notify("Playlist created".into());
            }
            Some(RadioNameTarget::Rename(id)) => {
                if let Some(p) = self.radio.playlists.iter_mut().find(|p| p.id == id) {
                    p.name = name;
                }
                self.save_radio_playlists();
                self.notify("Playlist renamed".into());
            }
            None => {}
        }
    }

    pub(crate) fn radio_delete_playlist_prompt(&mut self) {
        if let Some(id) = self.radio_selected_playlist() {
            self.radio.pl.confirm_delete = Some(id);
        }
    }

    pub(crate) fn radio_confirm_delete(&mut self) {
        let Some(id) = self.radio.pl.confirm_delete.take() else {
            return;
        };
        self.radio.playlists.retain(|p| p.id != id);
        if self.radio.pl.open == Some(id) {
            self.radio.pl.open = None;
        }
        let n = self.radio.playlists.len();
        if self.radio.pl.sel >= n {
            self.radio.pl.sel = n.saturating_sub(1);
        }
        self.save_radio_playlists();
        self.notify("Playlist deleted".into());
    }

    // ---- add / remove stations ------------------------------------------
    /// `a` on a station: open the "add to playlist" picker for the highlighted
    /// station (a no-op when the current list has no station under the cursor).
    pub(crate) fn radio_add_current_to_playlist(&mut self) {
        if let Some(st) = self.radio_view_list().get(self.radio.sel).cloned() {
            self.radio.pl.adding = Some(st);
            self.radio.pl.add_sel = 0;
        }
    }

    /// Commit the add-picker: add the pending station to the chosen playlist, or —
    /// on the trailing "New playlist" row — open name entry to create one first
    /// (keeping the pending station so `radio_confirm_name` seeds the new list).
    pub(crate) fn radio_confirm_add(&mut self) {
        let ids: Vec<u32> = self.radio_playlists_sorted().iter().map(|p| p.id).collect();
        if self.radio.pl.add_sel >= ids.len() {
            self.radio.pl.naming = Some(RadioNameTarget::New);
            self.radio.pl.buffer.clear();
            return;
        }
        let id = ids[self.radio.pl.add_sel];
        if let Some(st) = self.radio.pl.adding.take() {
            self.radio_playlist_add(id, st);
        }
    }

    /// Add `st` to playlist `id`, deduped by [`station_key`]; persists on change.
    fn radio_playlist_add(&mut self, id: u32, st: Station) {
        let key = station_key(&st).to_string();
        enum Outcome {
            Added(String),
            Dup(String),
            Gone,
        }
        let outcome = match self.radio.playlists.iter_mut().find(|p| p.id == id) {
            None => Outcome::Gone,
            Some(p) if p.stations.iter().any(|s| station_key(s) == key) => {
                Outcome::Dup(p.name.clone())
            }
            Some(p) => {
                p.stations.push(st);
                Outcome::Added(p.name.clone())
            }
        };
        match outcome {
            Outcome::Added(name) => {
                self.save_radio_playlists();
                self.notify(format!("Added to {name}"));
            }
            Outcome::Dup(name) => self.notify(format!("Already in {name}")),
            Outcome::Gone => {}
        }
    }

    /// `d`/`x` inside an open playlist: remove the highlighted station from it.
    pub(crate) fn radio_remove_from_playlist(&mut self) {
        let Some(id) = self.radio.pl.open else {
            return;
        };
        let Some(st) = self.radio_view_list().get(self.radio.sel).cloned() else {
            return;
        };
        let key = station_key(&st).to_string();
        if let Some(p) = self.radio.playlists.iter_mut().find(|p| p.id == id) {
            p.stations.retain(|s| station_key(s) != key);
        }
        let n = self.radio_view_list().len();
        if self.radio.sel >= n {
            self.radio.sel = n.saturating_sub(1);
        }
        self.save_radio_playlists();
        self.notify(format!("Removed: {}", st.name));
    }

    // ---- modal dispatch --------------------------------------------------
    /// Enter inside a radio-playlist modal: commit whichever one is open.
    pub(crate) fn radio_modal_confirm(&mut self) {
        if self.radio.pl.naming.is_some() {
            self.radio_confirm_name();
        } else if self.radio.pl.confirm_delete.is_some() {
            self.radio_confirm_delete();
        } else if self.radio.pl.adding.is_some() {
            self.radio_confirm_add();
        }
    }

    /// Esc inside a radio-playlist modal: close whichever one is open.
    pub(crate) fn radio_modal_cancel(&mut self) {
        let ui = &mut self.radio.pl;
        if ui.naming.is_some() {
            ui.naming = None;
            ui.buffer.clear();
            // a name entry opened from the add-picker keeps the picker open behind it
        } else if ui.adding.is_some() {
            ui.adding = None;
        } else if ui.confirm_delete.is_some() {
            ui.confirm_delete = None;
        }
    }
}
