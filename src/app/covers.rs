//! Album-art cover load + search methods on `AppState` (extracted from app/mod.rs).

use super::*;

/// Inline album-art rendering state, grouped out of `AppState` (was four loose
/// fields). The terminal image-protocol `picker` (set once the terminal is
/// queried), the current track's `full` + transport-bar `bar` image protocols
/// (separate so the two sizes don't thrash one state each frame), and the cover's
/// pixel `dims` for aspect-correct layout. `None` → gradient placeholder.
#[derive(Default)]
pub struct CoverArt {
    pub picker: Option<ratatui_image::picker::Picker>,
    pub full: crate::ui::components::CoverState,
    pub bar: crate::ui::components::CoverState,
    pub dims: Option<(u32, u32)>,
}

impl AppState {
    /// (Re)load the current track's embedded cover into the inline-image
    /// protocols, honouring the `album_art` setting. `None` → gradient fallback.
    /// Called on track change and when the album-art toggle flips.
    pub fn reload_cover(&mut self) {
        self.art.full = None;
        self.art.bar = None;
        self.art.dims = None;
        // Re-derive the base theme (dropping the previous track's dynamic accent);
        // it's re-tinted below. Reset to the *active* theme by its own name — NOT
        // `config.theme`, which is dormant while follow-system mode drives the
        // light/dark slot, so using it would snap every local track change back to
        // the single (often `auto`) theme. Goes through `resolve_theme` so `auto`
        // keeps the terminal-detected palette.
        let name = self.theme.name.clone();
        self.theme = self.resolve_theme(&name);
        if !self.config.album_art && !self.config.dynamic_accent {
            return;
        }
        let Some(path) = self.current_track().map(|t| t.path.clone()) else {
            return;
        };
        // decode the cover once, then use it for both the accent and the image
        let Some(img) = crate::cover::load_cover(&path) else {
            return;
        };
        if self.config.dynamic_accent {
            self.theme.set_accent(crate::cover::dominant_color(&img));
        }
        if self.config.album_art
            && let Some(p) = self.art.picker.as_ref()
        {
            self.art.dims = Some((img.width(), img.height()));
            self.art.full = Some(crate::ui::components::Cover::new(p, img.clone()));
            self.art.bar = Some(crate::ui::components::Cover::new(p, img));
        }
    }

    /// Rebuild the persistent inline-image covers (now-playing art, transport-bar
    /// cover, Spotify track cover, Spotify artist photo) with a *fresh* protocol id.
    /// Called on the modal-close edge: a modal overlay that partly covered a cover
    /// leaves stale glyphs on the covered edge, because ratatui-image v11's Kitty
    /// renderer reuses the image id and Ghostty won't repaint occluded placeholder
    /// cells for an unchanged id (only a new id — as a new track's cover gets — does).
    /// Rebuilding from each cover's retained image mints that new id, so the next
    /// render re-places the whole image cleanly. Synchronous (no worker) → the swap
    /// is instant with no blank frame. See [`crate::ui::components::Cover`].
    pub fn rebuild_persistent_covers(&mut self) {
        let Some(picker) = self.art.picker.as_ref() else {
            return;
        };
        for cover in [
            self.art.full.as_ref(),
            self.art.bar.as_ref(),
            self.spov.sp_cover.as_ref(),
            self.spov.sp_artist_cover.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            cover.rebuild(picker);
        }
        self.dirty = true;
    }

    /// Attach the cover-search worker's request channel.
    pub fn set_cover_sender(
        &mut self,
        tx: crossbeam_channel::Sender<crate::coversearch::CoverRequest>,
    ) {
        self.workers.cover = Some(tx);
    }

    /// Open the album-art picker for the current track's album.
    pub fn open_cover_search(&mut self) {
        self.tags.tab = 2; // Cover tab
        let Some(t) = self.tags_primary().and_then(|id| self.library.track(id)) else {
            self.notify("No track playing".into());
            return;
        };
        let artist = if !t.album_artist.is_empty() {
            t.album_artist.clone()
        } else {
            t.artist.clone()
        };
        let album = t.album.clone();
        let paths: Vec<std::path::PathBuf> = match t.album_id {
            Some(a) => self
                .library
                .tracks_of(a)
                .iter()
                .map(|t| t.path.clone())
                .collect(),
            None => vec![t.path.clone()],
        };
        let label = match (album.is_empty(), artist.is_empty()) {
            (false, false) => format!("{album} · {artist}"),
            (false, true) => album.to_string(),
            (true, false) => artist.to_string(),
            (true, true) => "this track".into(),
        };
        let query = format!("{artist} {album}").trim().to_string();
        let song = t.path.clone();
        // default to album-wide only when it's actually a multi-track album
        let album_wide = paths.len() > 1;
        self.tags.cover = Some(CoverSearch {
            key: String::new(),
            label,
            query,
            editing: false,
            status: CoverStatus::Searching,
            candidates: Vec::new(),
            previews: Vec::new(),
            sel: 0,
            paths,
            song,
            album_wide,
            confirm: false,
            qcaret: 0,
        });
        self.run_cover_search();
    }

    /// Move the candidate selection (no-op while editing the query).
    pub fn cover_move(&mut self, m: Motion) {
        if let Some(cs) = self.tags.cover.as_mut()
            && !cs.editing
            && !cs.candidates.is_empty()
        {
            let n = cs.candidates.len();
            cs.sel = match m {
                Motion::Up => (cs.sel + n - 1) % n,
                Motion::Down => (cs.sel + 1) % n,
                Motion::Top => 0,
                Motion::Bottom => n - 1,
                _ => cs.sel,
            };
        }
    }

    /// Enter inside the popup: run the search (editing) or embed the selection.
    /// Enter: re-search (editing) or *request* the embed (shows a confirm).
    pub fn cover_activate(&mut self) {
        let Some(cs) = self.tags.cover.as_mut() else {
            return;
        };
        if cs.editing {
            cs.editing = false;
            self.run_cover_search();
        } else if cs.candidates.get(cs.sel).is_some() {
            cs.confirm = true; // guard accidental writes
        }
    }

    /// Toggle the embed scope: whole album ⇄ just the current song.
    pub fn cover_toggle_scope(&mut self) {
        if let Some(cs) = self.tags.cover.as_mut() {
            cs.album_wide = !cs.album_wide;
        }
    }

    /// Confirm + perform the cover embed (to the album or just the current song).
    pub fn cover_confirm(&mut self) {
        let Some(cs) = self.tags.cover.as_mut() else {
            return;
        };
        cs.confirm = false;
        let Some(cand) = cs.candidates.get(cs.sel) else {
            return;
        };
        let url = cand.full_url.clone();
        let paths = if cs.album_wide {
            cs.paths.clone()
        } else {
            vec![cs.song.clone()]
        };
        let key = cs.key.clone();
        cs.status = CoverStatus::Embedding;
        if let Some(tx) = &self.workers.cover {
            let _ = tx.send(crate::coversearch::CoverRequest::Embed { url, paths, key });
        }
    }

    /// Type into / edit the popup's query line.
    pub fn cover_input(&mut self, s: String) {
        if let Some(cs) = self.tags.cover.as_mut() {
            cs.editing = true;
            cs.qcaret = s.chars().count();
            cs.query = s;
        }
    }

    /// Receive a cover-search worker result.
    pub fn on_cover_result(&mut self, res: crate::coversearch::CoverResult) {
        use crate::coversearch::CoverResult::*;
        match res {
            Found { key, candidates } => {
                if self.tags.cover.as_ref().map(|cs| cs.key.clone()) != Some(key) {
                    return; // stale / closed
                }
                // build a preview protocol per candidate (picker borrowed first)
                let previews: Vec<crate::ui::components::CoverState> = candidates
                    .iter()
                    .map(|c| {
                        self.art
                            .picker
                            .as_ref()
                            .map(|p| crate::ui::components::Cover::new(p, c.thumb.clone()))
                    })
                    .collect();
                if let Some(cs) = self.tags.cover.as_mut() {
                    cs.status = if candidates.is_empty() {
                        CoverStatus::Empty
                    } else {
                        CoverStatus::Results
                    };
                    cs.previews = previews;
                    cs.candidates = candidates;
                    cs.sel = 0;
                }
            }
            Error { key, msg } => {
                if let Some(cs) = self.tags.cover.as_mut()
                    && cs.key == key
                {
                    cs.status = CoverStatus::Error(msg);
                }
            }
            Embedded { key, count, msg } => {
                let matches = self.tags.cover.as_ref().is_some_and(|cs| cs.key == key);
                if matches {
                    self.close_tags();
                    if count > 0 {
                        self.reload_cover();
                    }
                    self.notify(msg);
                }
            }
        }
    }

    /// (Re)run the album-art search with the cover popup's current query.
    pub(crate) fn run_cover_search(&mut self) {
        self.workers.cover_seq += 1;
        let key = format!("cs{}", self.workers.cover_seq);
        let Some(cs) = self.tags.cover.as_mut() else {
            return;
        };
        cs.key = key.clone();
        cs.status = CoverStatus::Searching;
        cs.candidates.clear();
        cs.previews.clear();
        cs.sel = 0;
        let query = cs.query.clone();
        if query.is_empty() {
            cs.status = CoverStatus::Empty;
            return;
        }
        if let Some(tx) = &self.workers.cover {
            let _ = tx.send(crate::coversearch::CoverRequest::Search { query, key });
        }
    }
}

/// State of the album-art search popup (opened from the command palette).
pub struct CoverSearch {
    /// Identifies the in-flight search; results with a stale key are ignored.
    pub key: String,
    /// Title for the popup, e.g. "Neon Dreams · Neon District". Not rendered yet.
    #[allow(dead_code)]
    pub label: String,
    /// Editable search query (seeded from the album/artist tags).
    pub query: String,
    /// `true` while the query line has focus (typing edits it).
    pub editing: bool,
    pub status: CoverStatus,
    pub candidates: Vec<crate::coversearch::Candidate>,
    /// One lazily-built preview protocol per candidate (parallel to `candidates`).
    pub previews: Vec<crate::ui::components::CoverState>,
    pub sel: usize,
    /// Album track paths the chosen art will be embedded into (album scope).
    pub paths: Vec<std::path::PathBuf>,
    /// The current single track (song scope).
    pub song: std::path::PathBuf,
    /// `true` = embed to the whole album (`paths`); `false` = just `song`.
    pub album_wide: bool,
    /// `true` while awaiting confirmation of the embed (guards accidental writes).
    pub confirm: bool,
    /// Caret position (char index) in the query while editing it.
    pub qcaret: usize,
}

#[derive(PartialEq)]
pub enum CoverStatus {
    Searching,
    Results,
    Empty,
    Embedding,
    Error(String),
}
