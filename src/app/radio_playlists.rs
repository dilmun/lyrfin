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
            self.radio.pl.adding.clear(); // drop the bridged station(s) too
            return;
        }
        match target {
            Some(RadioNameTarget::New) => {
                let id = self.next_radio_playlist_id();
                let stations = std::mem::take(&mut self.radio.pl.adding);
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
    /// `a` on a station: open the "add to playlist" picker for the selection
    /// (marked set / visual range, else the cursor station). A no-op when the
    /// current list has no station under the cursor.
    pub(crate) fn radio_add_current_to_playlist(&mut self) {
        let sel = self.selected_stations();
        if sel.is_empty() {
            return;
        }
        self.radio.pl.adding = sel;
        self.radio.pl.add_sel = 0;
        self.clear_marks(); // captured into `adding`; drop the on-list highlight
    }

    /// Commit the add-picker: add the pending station(s) to the chosen playlist,
    /// or — on the trailing "New playlist" row — open name entry to create one
    /// first (keeping the pending stations so `radio_confirm_name` seeds the list).
    pub(crate) fn radio_confirm_add(&mut self) {
        let ids: Vec<u32> = self.radio_playlists_sorted().iter().map(|p| p.id).collect();
        if self.radio.pl.add_sel >= ids.len() {
            self.radio.pl.naming = Some(RadioNameTarget::New);
            self.radio.pl.buffer.clear();
            return;
        }
        let id = ids[self.radio.pl.add_sel];
        let stations = std::mem::take(&mut self.radio.pl.adding);
        self.radio_playlist_add_many(id, stations);
    }

    /// Add `stations` to playlist `id`, deduped by [`station_key`]; persists once
    /// and reports how many were newly added (vs. already present).
    fn radio_playlist_add_many(&mut self, id: u32, stations: Vec<Station>) {
        let Some(p) = self.radio.playlists.iter_mut().find(|p| p.id == id) else {
            return;
        };
        let name = p.name.clone();
        let mut added = 0usize;
        for st in stations {
            let key = station_key(&st).to_string();
            if !p.stations.iter().any(|s| station_key(s) == key) {
                p.stations.push(st);
                added += 1;
            }
        }
        if added > 0 {
            self.save_radio_playlists();
            let noun = if added == 1 { "station" } else { "stations" };
            self.notify(format!("Added {added} {noun} to {name}"));
        } else {
            self.notify(format!("Already in {name}"));
        }
    }

    /// `d`/`x` inside an open playlist: remove the highlighted station from it.
    pub(crate) fn radio_remove_from_playlist(&mut self) {
        let Some(id) = self.radio.pl.open else {
            return;
        };
        // the selection (marked set / visual range, else the cursor station)
        let keys: std::collections::HashSet<String> = self
            .selected_stations()
            .iter()
            .map(|s| station_key(s).to_string())
            .collect();
        if keys.is_empty() {
            return;
        }
        let mut removed = 0usize;
        if let Some(p) = self.radio.playlists.iter_mut().find(|p| p.id == id) {
            let before = p.stations.len();
            p.stations.retain(|s| !keys.contains(station_key(s)));
            removed = before - p.stations.len();
        }
        let n = self.radio_view_list().len();
        if self.radio.sel >= n {
            self.radio.sel = n.saturating_sub(1);
        }
        self.save_radio_playlists();
        self.clear_marks();
        let noun = if removed == 1 { "station" } else { "stations" };
        self.notify(format!("Removed {removed} {noun}"));
    }

    // ---- modal dispatch --------------------------------------------------
    /// Enter inside a radio-playlist modal: commit whichever one is open.
    pub(crate) fn radio_modal_confirm(&mut self) {
        if self.radio.pl.naming.is_some() {
            self.radio_confirm_name();
        } else if self.radio.pl.confirm_delete.is_some() {
            self.radio_confirm_delete();
        } else if !self.radio.pl.adding.is_empty() {
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
        } else if !ui.adding.is_empty() {
            ui.adding.clear();
        } else if ui.confirm_delete.is_some() {
            ui.confirm_delete = None;
        }
    }
}
