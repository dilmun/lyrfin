//! Playlist & bookmark management on `AppState` (extracted from app/mod.rs):
//! confirming the modal text-entry (`naming`) — create/rename/folder a playlist,
//! a smart playlist, a bookmark, a music dir, or the Spotify client id — and the
//! "add to playlist" picker, plus the small persist helpers and the sidebar
//! selected-playlist accessor.

use super::*;

/// Transient modal-input state, grouped out of `AppState` (driven from this
/// module's confirm_name / confirm_add_to_playlist): the new/rename/bookmark
/// text-entry — `naming` is which prompt is active, `buffer` the typed text — and
/// the "add to playlist" picker — `add_targets` the tracks being added, `add_sel`
/// the picker cursor. All inert (naming None / add_targets empty) when closed.
#[derive(Default)]
pub struct ModalInput {
    pub naming: Option<NameTarget>,
    pub buffer: String,
    pub add_targets: Vec<TrackId>,
    pub add_sel: usize,
    /// A playlist awaiting delete confirmation (the `d` two-step). Inert (`None`)
    /// until the user asks to delete; the confirm dialog clears it on either path.
    pub confirm_delete: Option<PlaylistId>,
}

impl AppState {
    /// `a` — add to a playlist, routed to the destination that matches what the
    /// track actually *is*.
    ///
    /// A source view already routes this itself (the Spotify and Radio keymaps
    /// claim `a` before the global binding). The player views (Now Playing /
    /// Lyrics / Concert) browse nothing, so `a` used to fall through to the local
    /// path unconditionally and add `player.current` — a stale local track — while
    /// Spotify or radio was playing. Here it follows the audio instead.
    pub(crate) fn add_to_playlist_prompt(&mut self) {
        if self.layout.is_player_view() {
            match self.now_playing_source() {
                Some(crate::app::NpSource::Spotify) => {
                    self.spotify_add_to_playlist_prompt();
                    return;
                }
                Some(crate::app::NpSource::Radio) => {
                    self.radio_add_current_to_playlist();
                    return;
                }
                // local, or nothing loaded → the local picker below
                _ => {}
            }
        }
        let ids = self.selected_track_ids();
        if !ids.is_empty() {
            self.input.add_targets = ids;
            self.input.add_sel = 0;
            self.marks.anchor = None;
        }
    }

    /// The playlist id selected in the Playlists section (if the cursor is on a
    /// `LocalItem::Playlist` row). Resolves the target for rename / delete /
    /// add-current while browsing the flat Playlists section.
    pub(crate) fn selected_local_playlist(&self) -> Option<PlaylistId> {
        match self.local.items.get(self.local.sel) {
            Some(LocalItem::Playlist(id)) => Some(*id),
            _ => None,
        }
    }

    /// The normal (non-smart) playlist we're currently drilled *into* — i.e. the
    /// last container opened on the nav back-stack is a playlist. This is the
    /// target for removing a track (vs `selected_local_playlist`, which is the
    /// playlist under the cursor in the Playlists list). `None` for smart
    /// playlists (their membership is rule-based and can't be hand-edited).
    pub(crate) fn current_local_playlist(&self) -> Option<PlaylistId> {
        match self.local.nav.opened().last() {
            Some(LocalItem::Playlist(id))
                if self
                    .library
                    .playlists
                    .get(id)
                    .is_some_and(|p| p.query.is_none()) =>
            {
                Some(*id)
            }
            _ => None,
        }
    }

    /// Remove the selected track from the playlist we're drilled into, then
    /// refresh the open track list in place (keeping the cursor in range).
    pub(crate) fn remove_selected_from_playlist(&mut self) {
        let Some(pid) = self.current_local_playlist() else {
            return;
        };
        // the selection (marked set / visual range, else the cursor track) — all of
        // which are tracks of the drilled-in playlist.
        let ids: Vec<_> = self
            .selected_keys()
            .into_iter()
            .filter_map(|k| match k {
                MarkKey::Track(id) => Some(id),
                _ => None,
            })
            .collect();
        if ids.is_empty() {
            return;
        }
        for tid in &ids {
            self.library.remove_from_playlist(pid, *tid);
        }
        self.save_playlists();
        self.clear_marks();
        let n = ids.len();
        self.notify(format!(
            "Removed {n} {} from playlist",
            if n == 1 { "track" } else { "tracks" }
        ));
        self.local.items = self
            .library
            .playlist_tracks(pid)
            .into_iter()
            .map(LocalItem::Track)
            .collect();
        self.local.sel = self.local.sel.min(self.local.items.len().saturating_sub(1));
    }

    /// Refresh the sidebar's Playlists list after a create / rename / delete, but
    /// only when it's the section currently on screen (else it reloads on entry).
    pub(crate) fn reload_playlists_section(&mut self) {
        if self.layout == Layout::Dashboard && self.local.section == LocalSection::Playlists {
            self.local_load_section();
        }
    }

    /// Carry out a confirmed playlist deletion (the `⏎`/`y` path of the delete
    /// dialog). Clears the pending state either way.
    pub(crate) fn confirm_delete_playlist(&mut self) {
        if let Some(id) = self.input.confirm_delete.take() {
            self.library.delete_playlist(id);
            self.save_playlists();
            self.notify("Playlist deleted".into());
            self.reload_playlists_section();
        }
    }

    /// Confirm the new/rename playlist text-entry.
    pub(crate) fn confirm_name(&mut self) {
        let name = self.input.buffer.trim().to_string();
        if !name.is_empty() {
            match self.input.naming {
                Some(NameTarget::New) => {
                    let id = self.library.create_playlist(name);
                    // if this name flow was started from "add to playlist", add them
                    for track in std::mem::take(&mut self.input.add_targets) {
                        self.library.add_to_playlist(id, track);
                    }
                    self.save_playlists();
                    self.notify("Playlist created".into());
                    self.reload_playlists_section();
                }
                Some(NameTarget::Rename(id)) => {
                    self.library.rename_playlist(id, name);
                    self.save_playlists();
                    self.notify("Playlist renamed".into());
                    self.reload_playlists_section();
                }
                Some(NameTarget::AddMusicDir) => {
                    let path = expand_tilde(&name);
                    if path.is_dir() {
                        if !self.config.music_dirs.contains(&path) {
                            self.config.music_dirs.push(path);
                            self.config.save();
                            self.request_rescan();
                            self.notify("Added music directory".into());
                        }
                    } else {
                        self.notify(format!("Not a directory: {name}"));
                    }
                }
                Some(NameTarget::Bookmark) => {
                    let query = self.search.query.clone();
                    if !query.trim().is_empty() {
                        // overwrite a same-named bookmark, else append
                        if let Some(b) = self.bookmarks.iter_mut().find(|b| b.name == name) {
                            b.query = query;
                        } else {
                            self.bookmarks
                                .push(crate::library::store::Bookmark { name, query });
                        }
                        self.save_bookmarks();
                        self.notify("Bookmarked".into());
                    }
                }
                Some(NameTarget::SmartPlaylist) => {
                    let query = self.search.query.clone();
                    if !query.trim().is_empty() {
                        self.library.create_smart_playlist(name, query);
                        self.save_playlists();
                        self.notify("Smart playlist created".into());
                    }
                }
                Some(NameTarget::SpotifyClientId) => {
                    // Persist to the dedicated client-id file (atomic, survives
                    // config.toml churn / parse errors) AND apply it live; then a
                    // best-effort config.toml mirror. Start a fresh login with it.
                    self.config.spotify_client_id = name.clone();
                    crate::spotify::auth::persist_client_id(&self.config.dir, &name);
                    self.config.save();
                    self.stop_spotify_overlay();
                    self.spov.session_cmd = None;
                    self.spov.session_rx = None;
                    crate::spotify::Tokens::clear(&self.config.dir);
                    self.spotify.tokens = None;
                    self.spotify.items.clear();
                    self.spotify.conn = crate::spotify::ConnState::Connecting { url: None };
                    self.spotify.auth_rx =
                        Some(crate::spotify::spawn_login(self.config.dir.clone()));
                    self.notify("Saved your Client ID — logging in with your app…".into());
                }
                None => {}
            }
        }
        self.input.naming = None;
        self.input.buffer.clear();
    }

    /// Confirm the "add to playlist" picker: add to the chosen playlist, or open
    /// the new-playlist text-entry if the "New playlist" row is selected.
    pub(crate) fn confirm_add_to_playlist(&mut self) {
        if self.input.add_targets.is_empty() {
            return;
        }
        let n = self.library.playlists.len();
        if self.input.add_sel >= n {
            self.input.naming = Some(NameTarget::New); // keeps add_targets → confirm_name adds them
            self.input.buffer.clear();
            return;
        }
        let id = self
            .library
            .playlists_sorted()
            .get(self.input.add_sel)
            .map(|p| p.id);
        if let Some(id) = id {
            let tracks = std::mem::take(&mut self.input.add_targets);
            let count = tracks.len();
            for track in tracks {
                self.library.add_to_playlist(id, track);
            }
            self.save_playlists();
            let name = self
                .library
                .playlists
                .get(&id)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            self.notify(if count == 1 {
                format!("Added to {name}")
            } else {
                format!("Added {count} tracks to {name}")
            });
        }
        self.input.add_targets.clear();
    }

    /// Persist the current playlists to disk.
    pub(crate) fn save_playlists(&self) {
        crate::library::store::PlaylistStore::from_library(&self.library).save(&self.config.dir);
    }

    /// Persist the current bookmarks (saved searches) to disk.
    fn save_bookmarks(&self) {
        crate::library::store::BookmarkStore::save(&self.bookmarks, &self.config.dir);
    }
}
