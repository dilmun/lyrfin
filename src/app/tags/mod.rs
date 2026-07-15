//! Tag Edit modal core on `AppState`: the modal lifecycle (open/close/tab-switch),
//! the operator target-set + editor entry, and the modal's state types. The
//! editor field manipulation lives in `editor`, the online search in `search`.

use super::*;

mod editor;
mod search;

impl AppState {
    /// Whether the unified Tag Edit modal is open (any of its three tabs).
    pub fn tags_open(&self) -> bool {
        self.tags.edit.is_some() || self.tags.search.is_some() || self.tags.cover.is_some()
    }

    /// Whether a modal overlay/popup owns the screen: the per-view settings popup,
    /// the full Settings overlay, or the Info overlay. While one is open only its
    /// own navigation keys work — global one-key commands are suppressed in the
    /// keymap (see `crate::keymap::map`). Text-input modals (palette, tag editor,
    /// naming, search) and the Info overlay capture keys earlier in the chain.
    pub fn modal_overlay_open(&self) -> bool {
        self.settings.popup.is_some()
            || self.settings.overlay
            || self.info.is_some()
            || self.eq.open
    }

    /// The track the modal's Auto/Cover tabs operate on — the editor's first
    /// target, falling back to the now-playing track.
    pub(crate) fn tags_primary(&self) -> Option<crate::core::model::TrackId> {
        self.tags
            .edit
            .as_ref()
            .and_then(|te| te.targets.first().copied())
            .or(self.player.current)
    }

    /// Open the unified Tag Edit modal on the Edit tab (manual editor).
    pub fn open_tags(&mut self) {
        self.begin_tag_edit();
        if self.tags.edit.is_some() {
            self.tags.tab = 0;
        } else {
            self.notify("No track to edit".into());
        }
    }

    /// Switch the modal's active tab, lazily opening that tab's content the first
    /// time it's visited (Auto/Cover fetch on first switch).
    pub fn tags_tab_to(&mut self, tab: u8) {
        if !self.tags_open() {
            return;
        }
        self.tags.tab = tab.min(2);
        match self.tags.tab {
            0 if self.tags.edit.is_none() => self.begin_tag_edit(),
            1 if self.tags.search.is_none() => self.open_tag_search(),
            2 if self.tags.cover.is_none() => self.open_cover_search(),
            _ => {}
        }
    }

    /// Tab / Shift-Tab: step the modal's tab (Edit → Auto Tag → Cover, wrapping).
    pub fn tags_tab_step(&mut self, dir: i32) {
        if !self.tags_open() {
            return;
        }
        let n = 3i32;
        let cur = self.tags.tab as i32;
        self.tags_tab_to((((cur + dir) % n + n) % n) as u8);
    }

    /// Close the whole modal (all three tabs).
    pub fn close_tags(&mut self) {
        self.tags.edit = None;
        self.tags.search = None;
        self.tags.cover = None;
        self.tags.tab = 0;
    }

    /// The tracks an operator (edit / add-to-playlist) applies to: the marked
    /// set + live visual range in display order, or — when nothing is selected —
    /// the cursor track (falling back to the now-playing track).
    pub(crate) fn selected_track_ids(&self) -> Vec<crate::core::model::TrackId> {
        let ids = self.active_track_ids();
        if !self.marks.ids.is_empty() || self.marks.anchor.is_some() {
            let vis = self.visual_range();
            ids.into_iter()
                .enumerate()
                .filter(|(i, id)| {
                    self.marks.ids.contains(id) || vis.is_some_and(|(lo, hi)| *i >= lo && *i <= hi)
                })
                .map(|(_, id)| id)
                .collect()
        } else {
            self.active_track_cursor()
                .and_then(|c| ids.get(c).copied())
                .or(self.player.current)
                .into_iter()
                .collect()
        }
    }

    /// Open the tag editor on the marked tracks (bulk) — or, with no marks, the
    /// selected track (falling back to the now-playing track).
    pub(crate) fn begin_tag_edit(&mut self) {
        // tag editing is for locally-stored music — not the Radio / Spotify views,
        // where the "selection" is unrelated to any editable local file
        if matches!(self.layout, Layout::Radio | Layout::Spotify) {
            self.notify("Tag editing is for local music, not streams".into());
            return;
        }
        self.tags.tab = 0; // Edit tab
        let ids = self.selected_track_ids();
        self.marks.anchor = None; // consume visual mode on opening the editor
        let paths: Vec<std::path::PathBuf> = ids
            .iter()
            .filter_map(|id| self.library.track(*id))
            .map(|t| t.path.clone())
            .collect();
        // standard tags come from the scanned Track; lyrics aren't scanned, so
        // read them straight from each file.
        let drafts: Vec<crate::tags::EditableTags> = ids
            .iter()
            .filter_map(|id| self.library.track(*id))
            .map(|t| {
                let mut e = crate::tags::EditableTags::from_track(t);
                e.lyrics = crate::tags::read_lyrics(&t.path);
                e
            })
            .collect();
        let Some(first) = drafts.first().cloned() else {
            return;
        };

        // bulk: fields that differ across the selection become <keep>
        let mut draft = first.clone();
        let mut keep = [false; crate::tags::FIELDS.len()];
        if drafts.len() > 1 {
            for (i, k) in keep.iter_mut().enumerate() {
                let v0 = first.get(i);
                if !drafts.iter().all(|d| d.get(i) == v0) {
                    *k = true;
                    draft.set(i, String::new());
                }
            }
        }
        self.tags.edit = Some(TagEdit {
            targets: ids,
            paths,
            original: draft.clone(),
            draft,
            keep,
            touched: [false; crate::tags::FIELDS.len()],
            cursor: 0,
            caret: 0,
            convert: None,
            replace: None,
            editing: false,
            confirm_album: false,
        });
    }
}

/// A tag-editor session over one or more tracks (bulk). For bulk edits, fields
/// that differ across the selection start as `<keep>` (see `keep`) and are left
/// untouched unless the user edits them.
pub struct TagEdit {
    pub targets: Vec<crate::core::model::TrackId>,
    pub paths: Vec<std::path::PathBuf>,
    pub draft: crate::tags::EditableTags,
    /// Pre-edit snapshot (for a future revert/diff); not read yet.
    #[allow(dead_code)]
    pub original: crate::tags::EditableTags,
    /// Per-field: true = `<keep>` (mixed across the selection, not written).
    pub keep: [bool; crate::tags::FIELDS.len()],
    /// Per-field: the user changed this field (typed, cleared, or transformed),
    /// so it should be written — even when the new value is empty.
    pub touched: [bool; crate::tags::FIELDS.len()],
    pub cursor: usize, // focused field index into tags::FIELDS
    /// In-field text caret (char index) for the focused field while editing.
    pub caret: usize,
    /// Converter prompt: `(to_filename, pattern)`. `to_filename` true = rename
    /// files from the tags; false = parse tags from the filename.
    pub convert: Option<(bool, String)>,
    /// Find-&-replace prompt: `(find, replace, on_replace)`. Applies to the
    /// focused field across every target track. `on_replace` = caret is in the
    /// replacement box (Tab toggles).
    pub replace: Option<(String, String, bool)>,
    /// EDIT-mode sub-state: false = browsing fields (Enter starts editing, `s`
    /// saves), true = typing into the focused field.
    pub editing: bool,
    /// `true` while awaiting confirmation of an apply-to-album write (guards the
    /// bulk write — Enter/y confirms, Esc/n cancels).
    pub confirm_album: bool,
}

/// State of the online tag/metadata search popup (command palette only).
pub struct TagSearch {
    pub key: String,
    #[allow(dead_code)]
    pub label: String,
    pub query: String,
    pub editing: bool,
    pub status: CoverStatus,
    pub candidates: Vec<crate::tagsearch::TagCandidate>,
    pub sel: usize,
    /// The playing/selected file (gets every field).
    pub song: std::path::PathBuf,
    /// All album tracks (the album-level fields are applied to these too).
    pub album: Vec<std::path::PathBuf>,
    /// The song's current tags, for the old→new preview diff.
    pub current: crate::tags::EditableTags,
    // ---- album mode (compare the whole album against a fetched source) ----
    /// `false` = single song (default); `true` = whole-album diff.
    pub album_mode: bool,
    /// Matched album sources (one per source that found the album).
    pub albums: Vec<crate::tagsearch::AlbumMatch>,
    /// Selected album source (index into `albums`).
    pub album_sel: usize,
    /// Selected local track (index into `album_tracks`).
    pub track_sel: usize,
    /// Local album tracks: `(path, current tags)`, in track order.
    pub album_tracks: Vec<(std::path::PathBuf, crate::tags::EditableTags)>,
    /// A pending apply awaiting confirmation (guards accidental writes).
    pub pending: Option<PendingApply>,
    /// Caret position (char index) in the query while editing it.
    pub qcaret: usize,
    /// The auto-seeded query ("artist title"). If the user edits the query away
    /// from this, results are matched against the *query* (not the track's tags).
    pub seeded: String,
}

/// The unified "Tag Edit" modal state (slice 3 of the AppState split): the active
/// tab plus each tab's popup. `tags_open()` is true when any popup is `Some`.
#[derive(Default)]
pub struct TagModal {
    /// Active tab — 0 Edit, 1 Auto Tag, 2 Cover.
    pub tab: u8,
    /// Manual tag-editor session (Edit tab).
    pub edit: Option<TagEdit>,
    /// Online tag/metadata search (Auto Tag tab).
    pub search: Option<TagSearch>,
    /// Album-art search (Cover tab).
    pub cover: Option<CoverSearch>,
}

/// Which apply the tag popup is confirming.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PendingApply {
    /// Write all fields to the current song.
    Song,
    /// Write album-level fields to every album track (single-mode `a`).
    AlbumBasic,
    /// Write the matched per-track fetched tags (album mode).
    AlbumFull,
}
