//! The Spotify view's playlist management on `AppState`: the "add to playlist"
//! picker plus create / rename / remove-track / unfollow, all talking to the Web
//! API worker (`SpRequest`). This is the Spotify analogue of `app::playlists`
//! (which manages the *local* library), kept separate because the two never share
//! ids — Spotify works in URIs, the local library in `TrackId`/`PlaylistId`.
//!
//! The flow mirrors the local one: a key opens a modal (`Spotify::pl_modal`),
//! navigation/typing mutate it, `Activate` confirms, and the worker result
//! (`on_spotify_playlist_write`) drives the toast + any refresh. The picker's
//! playlist list is fetched on demand (`SpRequest::MyPlaylists`) so it works from
//! anywhere in the view, not only the Playlists section.

use super::*;
use crate::spotify::api::{Item, Kind, PlaylistOp, SpRequest};
use crate::spotify::session::SessionCommand;

/// Sentinel key routing a `FetchPlaylistUris` result to the remove-a-track path
/// (vs the browse list), mirroring `ARTIST_PANE_KEY`. Read by the sibling
/// `events` module that drains session events.
pub(super) const REMOVE_FETCH_KEY: &str = "@pl-remove";

/// Largest playlist lyrfin will rebuild for a track remove. Spotify's replace
/// endpoint caps at 100 uris and librespot's raw fetch is capped at 100 (+1 to
/// detect overflow), so a remove on a bigger playlist is refused rather than risk
/// dropping the tracks beyond the cap. Keep in sync with `session::MAX_TRACKS`.
const SP_MAX_REBUILD: usize = 100;

/// The Spotify "add to playlist" picker + create/rename text-entry, when open.
/// Parallels the local `ModalInput`, but the rows are the account's Spotify
/// playlists and the pending targets are track URIs. Absent/`Default` when closed.
#[derive(Default)]
pub struct SpPlaylistModal {
    /// Track URI(s) to add once a playlist is chosen/created. Empty when the modal
    /// was opened only to create or rename (no pending add).
    pub add_uris: Vec<String>,
    /// Subject line for the picker header: a track title, or "N tracks".
    pub subject: String,
    /// The account's writable (owned/collaborative) playlists — the picker rows,
    /// fetched asynchronously after the modal opens.
    pub playlists: Vec<Item>,
    /// Fetching `playlists` — the overlay shows "Loading…" until the result lands.
    pub loading: bool,
    /// Matches the in-flight `MyPlaylists` request, so a stale result from a
    /// previously-closed modal is ignored.
    pub load_key: String,
    /// A note shown in the picker (e.g. a fetch error), else empty.
    pub note: String,
    /// Picker cursor: `0..playlists.len()` selects a playlist; the last row is
    /// "New playlist".
    pub sel: usize,
    /// Text-entry sub-mode (create / rename) with its live buffer; `None` = picker.
    pub naming: Option<SpNaming>,
    pub buffer: String,
}

/// Which text prompt the Spotify playlist modal is showing.
pub enum SpNaming {
    /// Create a new playlist; the modal's `add_uris` (if any) are added right after
    /// creation.
    New,
    /// Rename an existing playlist (by `uri`) to the typed name. The buffer is
    /// pre-filled with the current name. Opened directly from a playlist row.
    Rename { uri: String },
}

/// What a remove-a-track should do, decided from the playlist's fresh RAW uri
/// list. Pure output of [`plan_remove`] so the safety math is unit-testable
/// without a session/worker.
#[derive(Debug, PartialEq)]
enum RemovePlan {
    /// The playlist is larger than `max` (the raw fetch returned `> max` uris, so
    /// we can't prove we have all of them) → refuse; change nothing.
    TooLarge,
    /// None of the targets are in the live list (already gone / stale cursor) → do
    /// nothing.
    NotFound,
    /// Safe: replace the playlist's contents with exactly this list (the current
    /// tracks minus every occurrence of the target).
    Replace(Vec<String>),
}

/// Decide the remove action from the fresh RAW track uris. The ONLY safety logic
/// for the destructive replace, kept pure so it can be exhaustively tested:
/// - `> max` uris → [`RemovePlan::TooLarge`] (never rebuild a truncated list).
/// - none of `targets` present → [`RemovePlan::NotFound`] (never a surprise no-op
///   replace).
/// - otherwise → [`RemovePlan::Replace`] with every occurrence of any target
///   filtered out. The result is always strictly shorter than the input (we only
///   ever remove), never longer.
fn plan_remove(
    uris: Vec<String>,
    targets: &std::collections::HashSet<String>,
    max: usize,
) -> RemovePlan {
    if uris.len() > max {
        return RemovePlan::TooLarge;
    }
    let before = uris.len();
    let remaining: Vec<String> = uris.into_iter().filter(|u| !targets.contains(u)).collect();
    if remaining.len() == before {
        RemovePlan::NotFound
    } else {
        RemovePlan::Replace(remaining)
    }
}

/// The action `spotify_playlist_confirm` decides on while holding the modal
/// borrow, then performs after releasing it (so it can call other `self` methods).
enum Confirm {
    BeginNew,
    Create(String, Vec<String>),
    Rename(String, String),
    Add(String, String, Vec<String>),
}

impl AppState {
    // ---- opening the modals ---------------------------------------------

    /// `a`: open the picker to add the selected track — or, failing a track
    /// selection, the now-playing Spotify track — to one of the account's
    /// playlists. No-op with a hint when there's nothing addable.
    pub(crate) fn spotify_add_to_playlist_prompt(&mut self) {
        // a multi-selection (marked set / visual range) → all its tracks; otherwise
        // the cursor track — resolved directly so `a` works from any focus, like
        // before — falling back to the now-playing track.
        let mut uris = self.selected_spotify_uris();
        if uris.is_empty() {
            let cursor = self
                .spotify
                .items
                .get(self.spotify.sel)
                .filter(|it| it.kind == Kind::Track)
                .or(self.spov.now_spotify.as_ref())
                .map(|it| it.uri.clone());
            uris.extend(cursor);
        }
        if uris.is_empty() {
            self.notify("Select a track to add to a playlist".into());
            return;
        }
        let subject = if uris.len() == 1 {
            self.spotify
                .items
                .iter()
                .chain(self.spov.now_spotify.iter())
                .find(|it| it.uri == uris[0])
                .map(|it| it.name.clone())
                .unwrap_or_default()
        } else {
            format!("{} tracks", uris.len())
        };
        self.spotify.pl_modal = Some(SpPlaylistModal {
            add_uris: uris,
            subject,
            ..Default::default()
        });
        self.spotify_fetch_my_playlists();
        self.clear_marks();
    }

    /// `n` in the Playlists section: open the create-a-new-playlist prompt (no
    /// pending add — the playlist starts empty).
    pub(crate) fn spotify_new_playlist(&mut self) {
        self.spotify.pl_modal = Some(SpPlaylistModal {
            naming: Some(SpNaming::New),
            ..Default::default()
        });
    }

    /// `n` inside the picker: switch it into the new-playlist name prompt, keeping
    /// any pending tracks to add after the playlist is created.
    pub(crate) fn spotify_playlist_begin_new(&mut self) {
        if let Some(m) = &mut self.spotify.pl_modal {
            m.naming = Some(SpNaming::New);
            m.buffer.clear();
        }
    }

    /// `e`/`r` on a playlist row: open the rename prompt seeded with its name.
    pub(crate) fn spotify_begin_rename_playlist(&mut self) {
        let Some((uri, name)) = self
            .spotify
            .items
            .get(self.spotify.sel)
            .filter(|it| it.kind == Kind::Playlist)
            .map(|it| (it.uri.clone(), it.name.clone()))
        else {
            return;
        };
        self.spotify.pl_modal = Some(SpPlaylistModal {
            naming: Some(SpNaming::Rename { uri }),
            buffer: name,
            ..Default::default()
        });
    }

    /// `d` on a playlist row: arm the unfollow ("delete") confirmation.
    pub(crate) fn spotify_delete_playlist_prompt(&mut self) {
        if let Some((uri, name)) = self
            .spotify
            .items
            .get(self.spotify.sel)
            .filter(|it| it.kind == Kind::Playlist)
            .map(|it| (it.uri.clone(), it.name.clone()))
        {
            self.spotify.pl_confirm_delete = Some((uri, name));
        }
    }

    // ---- editing the open modal -----------------------------------------

    /// Set the create/rename text buffer as the user types.
    pub(crate) fn spotify_playlist_name_input(&mut self, s: String) {
        if let Some(m) = &mut self.spotify.pl_modal {
            m.buffer = s;
        }
    }

    /// Move the picker cursor (a no-op while the name prompt is up).
    pub(crate) fn spotify_playlist_move(&mut self, m: crate::action::Motion) {
        if let Some(modal) = &mut self.spotify.pl_modal
            && modal.naming.is_none()
        {
            let n = modal.playlists.len() + 1; // + the "New playlist" row
            modal.sel = crate::app::step(modal.sel, m, n);
        }
    }

    /// Esc within the modal: a name prompt reached from the picker steps back to
    /// the picker (keeping the pending add); otherwise the modal closes. Returns
    /// whether it handled the key (so `go_back` knows to stop).
    pub(crate) fn spotify_playlist_modal_back(&mut self) -> bool {
        let Some(m) = &mut self.spotify.pl_modal else {
            return false;
        };
        // a rename or a "new empty playlist" prompt has no picker to return to
        let return_to_picker = matches!(m.naming, Some(SpNaming::New)) && !m.add_uris.is_empty();
        if return_to_picker {
            m.naming = None;
            m.buffer.clear();
        } else {
            self.spotify.pl_modal = None;
        }
        true
    }

    // ---- confirming ------------------------------------------------------

    /// `⏎` within the modal: create/rename from the name prompt, or (in the picker)
    /// add to the highlighted playlist / open the new-playlist prompt on the last
    /// row.
    pub(crate) fn spotify_playlist_confirm(&mut self) {
        let decision = {
            let Some(m) = self.spotify.pl_modal.as_mut() else {
                return;
            };
            match &m.naming {
                Some(SpNaming::New) => {
                    let name = m.buffer.trim().to_string();
                    if name.is_empty() {
                        return; // keep the prompt open until a name is typed
                    }
                    Confirm::Create(name, std::mem::take(&mut m.add_uris))
                }
                Some(SpNaming::Rename { uri, .. }) => {
                    let name = m.buffer.trim().to_string();
                    if name.is_empty() {
                        return;
                    }
                    Confirm::Rename(uri.clone(), name)
                }
                None => {
                    if m.loading {
                        return; // playlists still loading — ignore the pick
                    }
                    if m.sel >= m.playlists.len() {
                        Confirm::BeginNew
                    } else {
                        let pl = &m.playlists[m.sel];
                        Confirm::Add(
                            pl.uri.clone(),
                            pl.name.clone(),
                            std::mem::take(&mut m.add_uris),
                        )
                    }
                }
            }
        };
        match decision {
            Confirm::BeginNew => self.spotify_playlist_begin_new(),
            Confirm::Create(name, uris) => {
                self.spotify.pl_modal = None;
                self.spotify_send_create(name, uris);
            }
            Confirm::Rename(uri, name) => {
                self.spotify.pl_modal = None;
                self.spotify_send_rename(uri, name);
            }
            Confirm::Add(uri, name, uris) => {
                self.spotify.pl_modal = None;
                self.spotify_send_add(uri, name, uris);
            }
        }
    }

    /// `⏎`/`y` on the unfollow confirmation: send the unfollow.
    pub(crate) fn spotify_confirm_delete_playlist(&mut self) {
        let Some((uri, name)) = self.spotify.pl_confirm_delete.take() else {
            return;
        };
        if let Some((token, tx)) = self.spotify_worker() {
            let _ = tx.send(SpRequest::UnfollowPlaylist {
                token,
                playlist_uri: uri,
                name,
            });
            self.notify("Deleting playlist…".into());
        }
    }

    /// `d`/`x` while drilled into a Spotify playlist: remove the selected track.
    ///
    /// Spotify blocks `DELETE`-ing playlist items for personal apps, so removal is
    /// done by REPLACING the playlist with its current contents minus this track.
    /// A replace overwrites everything, so this must never run against a partial
    /// list. We therefore don't trust the on-screen tracks (which cap at 100 and
    /// silently drop unresolvable tracks) — instead we fetch the playlist's RAW,
    /// live track uris fresh (via librespot) and finish in [`Self::spotify_apply_remove`],
    /// which refuses anything it can't prove is complete.
    pub(crate) fn spotify_remove_from_playlist(&mut self) {
        // the selection (marked set / visual range, else the cursor track)
        let track_uris = self.selected_spotify_uris();
        let playlist = self
            .spotify
            .open_item
            .as_ref()
            .filter(|it| it.kind == Kind::Playlist)
            .map(|it| (it.uri.clone(), it.name.clone()));
        let (false, Some((pl_uri, pl_name))) = (track_uris.is_empty(), playlist) else {
            return;
        };
        // need the librespot session to read the raw list (Web API strips the uris)
        if !self.spotify_ensure_session() {
            self.notify_error("Connect Spotify to edit playlists".into());
            return;
        }
        let Some(cmd) = self.spov.session_cmd.as_ref() else {
            return;
        };
        let n = track_uris.len();
        self.spotify.pl_pending_remove = Some((pl_uri.clone(), track_uris, pl_name));
        let _ = cmd.send(SessionCommand::FetchPlaylistUris {
            uri: pl_uri,
            key: REMOVE_FETCH_KEY.into(),
        });
        self.clear_marks();
        self.notify(format!(
            "Removing {n} {}…",
            if n == 1 { "track" } else { "tracks" }
        ));
    }

    /// The playlist's fresh RAW track uris arrived — do the completeness check and,
    /// if safe, replace the playlist with the list minus the removed track. This is
    /// the ONLY place that issues the destructive replace, so all the guards live
    /// here. `ok` is false when the fetch itself failed (never treated as "empty").
    pub(crate) fn spotify_apply_remove(&mut self, uris: Vec<String>, ok: bool) {
        let Some((pl_uri, track_uris, name)) = self.spotify.pl_pending_remove.take() else {
            return;
        };
        if !ok {
            self.notify_error(format!(
                "Couldn't read “{name}” to remove tracks — try again"
            ));
            return;
        }
        // All the safety guards live in `plan_remove` (pure + unit-tested): it
        // refuses a playlist too large to rebuild (>100 — a 120-track playlist
        // lands here and NOTHING is changed) or targets that aren't in the live
        // list, and only otherwise yields the exact current-minus-targets contents.
        let targets: std::collections::HashSet<String> = track_uris.into_iter().collect();
        match plan_remove(uris, &targets, SP_MAX_REBUILD) {
            RemovePlan::TooLarge => self.notify_error(format!(
                "“{name}” has over {SP_MAX_REBUILD} tracks — removing isn't supported \
                 there (Spotify blocks track deletion for personal apps; lyrfin's \
                 rebuild path is capped at {SP_MAX_REBUILD}). Nothing was changed."
            )),
            RemovePlan::NotFound => {
                self.notify("Those tracks aren't in the playlist anymore".into())
            }
            RemovePlan::Replace(remaining) => {
                if let Some((token, tx)) = self.spotify_worker() {
                    let _ = tx.send(SpRequest::ReplacePlaylistItems {
                        token,
                        playlist_uri: pl_uri,
                        uris: remaining,
                        name,
                    });
                }
            }
        }
    }

    // ---- worker requests -------------------------------------------------

    /// `(access_token, sender)` when both a token and the worker are available.
    pub(super) fn spotify_worker(&self) -> Option<(String, crossbeam_channel::Sender<SpRequest>)> {
        let token = self.spotify.tokens.as_ref()?.access_token.clone();
        let tx = self.workers.spotify.as_ref()?.clone();
        Some((token, tx))
    }

    /// Fetch the account's writable playlists into the open modal.
    fn spotify_fetch_my_playlists(&mut self) {
        let Some((token, tx)) = self.spotify_worker() else {
            return;
        };
        let user_id = self.spotify.account_id.clone().unwrap_or_default();
        self.workers.spotify_seq += 1;
        let key = format!("pl{}", self.workers.spotify_seq);
        if let Some(m) = &mut self.spotify.pl_modal {
            m.loading = true;
            m.load_key = key.clone();
            m.note.clear();
        }
        let _ = tx.send(SpRequest::MyPlaylists {
            token,
            user_id,
            key,
        });
    }

    fn spotify_send_create(&mut self, name: String, uris: Vec<String>) {
        let Some((token, tx)) = self.spotify_worker() else {
            return;
        };
        let _ = tx.send(SpRequest::CreatePlaylist { token, name, uris });
        self.notify("Creating playlist…".into());
    }

    fn spotify_send_rename(&mut self, uri: String, name: String) {
        if let Some((token, tx)) = self.spotify_worker() {
            let _ = tx.send(SpRequest::RenamePlaylist {
                token,
                playlist_uri: uri,
                name,
            });
            self.notify("Renaming playlist…".into());
        }
    }

    fn spotify_send_add(&mut self, uri: String, name: String, uris: Vec<String>) {
        if uris.is_empty() {
            return;
        }
        if let Some((token, tx)) = self.spotify_worker() {
            let _ = tx.send(SpRequest::AddToPlaylist {
                token,
                playlist_uri: uri,
                uris,
                name,
            });
            self.notify("Adding to playlist…".into());
        }
    }

    // ---- worker results --------------------------------------------------

    /// The account's writable playlists arrived — fill the still-open modal.
    pub(crate) fn on_spotify_my_playlists(
        &mut self,
        key: String,
        items: Vec<Item>,
        error: Option<String>,
    ) {
        let Some(m) = &mut self.spotify.pl_modal else {
            return;
        };
        if m.load_key != key {
            return; // a stale result from a previously-closed modal
        }
        m.loading = false;
        m.note = error.unwrap_or_default();
        m.playlists = items;
        m.sel = m.sel.min(m.playlists.len()); // clamp; last row = "New playlist"
    }

    /// A playlist write finished: toast it, then refresh whatever it changed.
    pub(crate) fn on_spotify_playlist_write(&mut self, op: PlaylistOp, ok: bool, msg: String) {
        if !ok {
            self.notify_error(msg);
            return;
        }
        self.notify(msg);
        match op {
            // structural change to the Playlists list → reload it if it's showing
            PlaylistOp::Create | PlaylistOp::Rename | PlaylistOp::Unfollow => {
                self.spotify_reload_playlists_section();
            }
            // a track left the open playlist → refetch its tracks
            PlaylistOp::Remove => self.spotify_reload_open_playlist(),
            PlaylistOp::Add => {} // nothing structural on screen to refresh
        }
    }

    /// Reload the Playlists section iff it's the list currently on screen (top
    /// level, not searching); otherwise it reloads on next entry anyway.
    fn spotify_reload_playlists_section(&mut self) {
        use crate::spotify::api::Section;
        let showing = self.layout == Layout::Spotify
            && self.spotify.section == Section::Playlists
            && self.spotify.crumb.is_none()
            && !self.spotify.in_search;
        if showing {
            self.spotify_load_section();
        }
    }

    /// Refetch the tracks of the playlist we're drilled into (after a remove),
    /// reusing the existing librespot metadata path + `Tracks` result handler.
    fn spotify_reload_open_playlist(&mut self) {
        let Some(item) = self
            .spotify
            .open_item
            .clone()
            .filter(|it| it.kind == Kind::Playlist)
        else {
            return;
        };
        if !self.spotify_ensure_session() {
            return;
        }
        let Some(cmd) = self.spov.session_cmd.as_ref() else {
            return;
        };
        self.workers.spotify_seq += 1;
        let key = format!("s{}", self.workers.spotify_seq);
        self.spotify.key = key.clone();
        self.spotify.loading = true;
        let _ = cmd.send(SessionCommand::FetchTracks {
            uri: item.uri,
            artist: false,
            key,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spotify::api::{Item, Kind};

    fn track(uri: &str, name: &str) -> Item {
        Item {
            uri: uri.into(),
            name: name.into(),
            kind: Kind::Track,
            ..Default::default()
        }
    }

    fn playlist(uri: &str, name: &str) -> Item {
        Item {
            uri: uri.into(),
            name: name.into(),
            kind: Kind::Playlist,
            ..Default::default()
        }
    }

    fn app() -> AppState {
        // a throwaway dir so nothing here can touch the user's real ~/.config/lyrfin
        let cfg = crate::config::Config {
            dir: std::env::temp_dir().join("lyrfin-sp-playlist-test"),
            ..crate::config::Config::default()
        };
        AppState::new(cfg)
    }

    #[test]
    fn add_prompt_targets_the_selected_track() {
        let mut a = app();
        a.spotify.items = vec![track("spotify:track:1", "One")];
        a.spotify.sel = 0;
        a.spotify_add_to_playlist_prompt();
        let m = a.spotify.pl_modal.expect("modal opens");
        assert_eq!(m.add_uris, vec!["spotify:track:1".to_string()]);
        assert_eq!(m.subject, "One");
        assert!(
            m.naming.is_none(),
            "opens on the picker, not the name prompt"
        );
    }

    #[test]
    fn add_prompt_needs_a_track() {
        let mut a = app();
        a.spotify.items = vec![playlist("spotify:playlist:1", "P")]; // not a track
        a.spotify.sel = 0;
        a.spotify_add_to_playlist_prompt();
        assert!(a.spotify.pl_modal.is_none(), "no track selected → no modal");
    }

    #[test]
    fn picker_last_row_opens_the_new_prompt() {
        let mut a = app();
        a.spotify.pl_modal = Some(SpPlaylistModal {
            add_uris: vec!["spotify:track:1".into()],
            playlists: vec![playlist("spotify:playlist:9", "Mix")],
            sel: 1, // past the one playlist → the "New playlist" row
            ..Default::default()
        });
        a.spotify_playlist_confirm();
        let m = a.spotify.pl_modal.expect("modal stays open in name mode");
        assert!(matches!(m.naming, Some(SpNaming::New)));
        assert_eq!(
            m.add_uris,
            vec!["spotify:track:1".to_string()],
            "pending add kept"
        );
    }

    #[test]
    fn empty_name_keeps_the_prompt_open() {
        let mut a = app();
        a.spotify.pl_modal = Some(SpPlaylistModal {
            naming: Some(SpNaming::New),
            buffer: "   ".into(),
            ..Default::default()
        });
        a.spotify_playlist_confirm();
        assert!(
            a.spotify.pl_modal.is_some(),
            "blank name doesn't create/close"
        );
    }

    #[test]
    fn esc_from_a_seeded_new_prompt_returns_to_the_picker() {
        let mut a = app();
        a.spotify.pl_modal = Some(SpPlaylistModal {
            add_uris: vec!["spotify:track:1".into()],
            naming: Some(SpNaming::New),
            ..Default::default()
        });
        assert!(a.spotify_playlist_modal_back());
        let m = a.spotify.pl_modal.expect("modal still open");
        assert!(m.naming.is_none(), "stepped back to the picker");
    }

    #[test]
    fn esc_from_a_rename_prompt_closes() {
        let mut a = app();
        a.spotify.pl_modal = Some(SpPlaylistModal {
            naming: Some(SpNaming::Rename {
                uri: "spotify:playlist:1".into(),
            }),
            ..Default::default()
        });
        assert!(a.spotify_playlist_modal_back());
        assert!(
            a.spotify.pl_modal.is_none(),
            "rename has no picker to return to"
        );
    }

    // ---- remove-a-track safety (plan_remove) ----------------------------

    fn uris(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("spotify:track:{i}")).collect()
    }

    fn targets(ts: &[&str]) -> std::collections::HashSet<String> {
        ts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn remove_over_100_tracks_is_refused_and_changes_nothing() {
        // the 120-track case: the raw fetch returns 101 (capped at max+1), so we
        // can't prove completeness → refuse, never replace.
        assert_eq!(
            plan_remove(uris(101), &targets(&["spotify:track:5"]), 100),
            RemovePlan::TooLarge
        );
    }

    #[test]
    fn remove_at_exactly_100_is_safe() {
        // exactly 100 means the raw fetch (take 101) returned 100 → complete list.
        let plan = plan_remove(uris(100), &targets(&["spotify:track:0"]), 100);
        match plan {
            RemovePlan::Replace(r) => {
                assert_eq!(r.len(), 99, "removed exactly one");
                assert!(!r.iter().any(|u| u == "spotify:track:0"), "target gone");
            }
            other => panic!("expected Replace, got {other:?}"),
        }
    }

    #[test]
    fn remove_drops_only_the_target_and_keeps_order() {
        let plan = plan_remove(uris(5), &targets(&["spotify:track:2"]), 100);
        assert_eq!(
            plan,
            RemovePlan::Replace(vec![
                "spotify:track:0".into(),
                "spotify:track:1".into(),
                "spotify:track:3".into(),
                "spotify:track:4".into(),
            ])
        );
    }

    #[test]
    fn remove_drops_every_selected_target_and_keeps_order() {
        // bulk: several targets removed in one pass, remaining order preserved.
        let plan = plan_remove(
            uris(5),
            &targets(&["spotify:track:1", "spotify:track:3"]),
            100,
        );
        assert_eq!(
            plan,
            RemovePlan::Replace(vec![
                "spotify:track:0".into(),
                "spotify:track:2".into(),
                "spotify:track:4".into(),
            ])
        );
    }

    #[test]
    fn remove_ignores_targets_not_present_but_removes_the_rest() {
        // a mix of present + stale targets still removes the present ones (Replace),
        // never a surprise no-op.
        let plan = plan_remove(
            uris(3),
            &targets(&["spotify:track:1", "spotify:track:99"]),
            100,
        );
        assert_eq!(
            plan,
            RemovePlan::Replace(vec!["spotify:track:0".into(), "spotify:track:2".into()])
        );
    }

    #[test]
    fn remove_missing_target_is_a_noop_not_a_replace() {
        // a stale cursor / already-removed track must NOT trigger a replace
        assert_eq!(
            plan_remove(uris(3), &targets(&["spotify:track:99"]), 100),
            RemovePlan::NotFound
        );
    }

    #[test]
    fn remove_last_track_yields_empty_replace() {
        assert_eq!(
            plan_remove(
                vec!["spotify:track:0".into()],
                &targets(&["spotify:track:0"]),
                100
            ),
            RemovePlan::Replace(vec![])
        );
    }

    #[test]
    fn remove_result_is_always_shorter_never_longer() {
        // safety invariant: a remove can only shrink the playlist
        for n in 0..=100 {
            if let RemovePlan::Replace(r) =
                plan_remove(uris(n), &targets(&["spotify:track:0"]), 100)
            {
                assert!(r.len() < n, "replace never grows the playlist");
            }
        }
    }

    #[test]
    fn delete_prompt_arms_the_confirmation() {
        let mut a = app();
        a.spotify.items = vec![playlist("spotify:playlist:7", "Trash")];
        a.spotify.sel = 0;
        a.spotify_delete_playlist_prompt();
        assert_eq!(
            a.spotify.pl_confirm_delete,
            Some(("spotify:playlist:7".into(), "Trash".into()))
        );
    }
}
