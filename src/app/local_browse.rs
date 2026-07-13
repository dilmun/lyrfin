//! The local library's drill-in browse model — the Spotify-style flat-sections +
//! nav_stack navigation for the on-disk library, on the shared [`super::nav`]
//! engine. A flat [`LocalSection`] sidebar; selecting a section loads its items
//! (tracks, or drillable containers); Enter on a container drills in (pushes a
//! frame, sets a breadcrumb); Esc pops back. All lists are built synchronously
//! from the in-memory [`crate::library::Library`] (no async — the data is local).

use super::nav::NavStack;
use super::release::{POPULAR_HEADER, ReleaseRow, ReleaseSection, release_grid_step};
use super::*;
use crate::core::model::{AlbumId, ArtistId, PlaylistId, TrackId};

/// A row in the local browse list: a playable track, a drillable container, or a
/// non-selectable group header (used on the grouped artist page).
#[derive(Clone)]
pub enum LocalItem {
    Track(TrackId),
    Album(AlbumId),
    Artist(ArtistId),
    Playlist(PlaylistId),
    Genre(String),
    /// Non-selectable group header, e.g. "POPULAR" / "ALBUMS" on an artist page.
    Header(&'static str),
}

impl LocalItem {
    pub fn is_track(&self) -> bool {
        matches!(self, LocalItem::Track(_))
    }
    pub fn is_selectable(&self) -> bool {
        !matches!(self, LocalItem::Header(_))
    }
}

/// The flat sidebar sections of the local library (the local analogue of Spotify's
/// `Section`). The first group are leaf track-lists; the rest are container lists
/// that drill in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum LocalSection {
    #[default]
    AllTracks,
    Favorites,
    RecentlyAdded,
    RecentlyPlayed,
    MostPlayed,
    Playlists,
    Albums,
    Artists,
    Genres,
}

impl LocalSection {
    /// All sections in sidebar order.
    pub const ALL: [LocalSection; 9] = [
        LocalSection::AllTracks,
        LocalSection::Favorites,
        LocalSection::RecentlyAdded,
        LocalSection::RecentlyPlayed,
        LocalSection::MostPlayed,
        LocalSection::Playlists,
        LocalSection::Albums,
        LocalSection::Artists,
        LocalSection::Genres,
    ];
    pub fn label(self) -> &'static str {
        match self {
            LocalSection::AllTracks => "All Tracks",
            LocalSection::Favorites => "Favorites",
            LocalSection::RecentlyAdded => "Recently Added",
            LocalSection::RecentlyPlayed => "Recently Played",
            LocalSection::MostPlayed => "Most Played",
            LocalSection::Playlists => "Playlists",
            LocalSection::Albums => "Albums",
            LocalSection::Artists => "Artists",
            LocalSection::Genres => "Genres",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            LocalSection::AllTracks => "♫",
            LocalSection::Favorites => "♥",
            LocalSection::RecentlyAdded => "✦",
            LocalSection::RecentlyPlayed => "↻",
            LocalSection::MostPlayed => "▲",
            LocalSection::Playlists => "≡",
            LocalSection::Albums => "◉",
            LocalSection::Artists => "☻",
            LocalSection::Genres => "⊞",
        }
    }
    /// Stable key for session persistence (independent of label/order).
    pub fn key(self) -> &'static str {
        match self {
            LocalSection::AllTracks => "all_tracks",
            LocalSection::Favorites => "favorites",
            LocalSection::RecentlyAdded => "recently_added",
            LocalSection::RecentlyPlayed => "recently_played",
            LocalSection::MostPlayed => "most_played",
            LocalSection::Playlists => "playlists",
            LocalSection::Albums => "albums",
            LocalSection::Artists => "artists",
            LocalSection::Genres => "genres",
        }
    }
    pub fn from_key(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|sec| sec.key() == s)
    }
    /// Whether this section defaults to the cover-art grid (vs a list). Only the
    /// container sections with per-item art — Albums and Artists — start as a grid.
    pub fn default_grid(self) -> bool {
        matches!(self, LocalSection::Albums | LocalSection::Artists)
    }
}

/// The local library's drill-in browse state: the current list + cursor +
/// breadcrumb, on the shared [`NavStack`] back-stack. Mirrors the Spotify browse
/// model so both navigate identically.
#[derive(Default)]
pub struct LocalBrowse {
    pub section: LocalSection,
    pub items: Vec<LocalItem>,
    pub sel: usize,
    /// Breadcrumb when drilled into a container (e.g. "◉ Album"); `None` at the
    /// section level.
    pub crumb: Option<String>,
    pub nav: NavStack<LocalItem>,
    /// Render the current container section as a cover-art grid (vs a flat list).
    pub grid: bool,
    /// Column count from the last grid render — drives 2-D navigation. Interior
    /// mutability: set during render (`&AppState`), read during nav (`&mut self`).
    pub cols: std::cell::Cell<usize>,
    /// Persisted top-row scroll offset of the cover-art grid. Kept across frames so
    /// the grid viewport is sticky (scrolls only when the selection leaves it),
    /// instead of re-centring on every selection change. Set during render.
    pub row_off: std::cell::Cell<usize>,
    /// Persisted horizontal scroll offset of the artist page's SELECTED release
    /// carousel, plus the carousel it belongs to (`car_key`, its first item index) so
    /// the offset resets when the selection moves to another carousel. Sticky, same as
    /// `row_off` but on the horizontal axis. Set during render.
    pub car_off: std::cell::Cell<usize>,
    pub car_key: std::cell::Cell<usize>,
}

/// Move a grid cursor by `(dx, dy)` cards using `cols` as the row stride, clamped
/// to `[0, n)` in reading order. Shared by the local + Spotify grids.
pub(crate) fn grid_step(sel: usize, n: usize, cols: usize, dx: i32, dy: i32) -> usize {
    if n == 0 {
        return 0;
    }
    let cols = cols.max(1) as i32;
    let next = sel as i32 + dx + dy * cols;
    next.clamp(0, n as i32 - 1) as usize
}

/// Horizontal grid step **clamped to the current row** — unlike [`grid_step`], a
/// move past the row's end stays on the last card instead of wrapping onto the next
/// row. Drives touchpad sideways scroll so a two-finger swipe stays on the row it
/// started on (row changes need a deliberate vertical gesture).
pub(crate) fn grid_step_row_locked(sel: usize, n: usize, cols: usize, dx: i32) -> usize {
    if n == 0 {
        return 0;
    }
    let cols = cols.max(1);
    let row_start = (sel / cols) * cols;
    let row_end = (row_start + cols - 1).min(n - 1);
    (sel as i32 + dx).clamp(row_start as i32, row_end as i32) as usize
}

/// Classify an album into a [`ReleaseSection`] by title keywords + track count.
/// The local library has no release-type field, so this is best-effort. Singles and
/// EPs share one section ("SINGLES & EPs"), matching Spotify — the taxonomy lives in
/// [`super::release`], this only maps a local album onto it.
fn release_kind(a: &crate::core::model::Album) -> ReleaseSection {
    let t = a.title.to_lowercase();
    let n = a.track_ids.len();
    if t.contains("remix") {
        ReleaseSection::Remixes
    } else if [
        "greatest hits",
        "best of",
        "the best",
        "collection",
        "compilation",
        "anthology",
    ]
    .iter()
    .any(|k| t.contains(k))
    {
        ReleaseSection::Compilations
    } else if t.contains("single") || t.contains(" ep") || t.contains("(ep)") || n <= 6 {
        ReleaseSection::SinglesEps
    } else {
        ReleaseSection::Albums
    }
}

impl AppState {
    /// Load the current section's items at the top level (clears any drill-in).
    pub(crate) fn local_load_section(&mut self) {
        self.local.nav.clear();
        self.local.crumb = None;
        self.local.sel = 0;
        // grid/list: the user's persisted per-section choice, else the default
        let section = self.local.section;
        self.local.grid = self
            .views
            .grid
            .get(&section)
            .copied()
            .unwrap_or_else(|| section.default_grid());
        self.local.items = self.local_section_items(section);
    }

    /// Whether the Main pane should render as a cover-art grid right now: grid mode
    /// is on, we're at the top level of the Albums/Artists section (not drilled in),
    /// and there are items to show.
    pub(crate) fn local_grid_active(&self) -> bool {
        self.local.grid
            && self.local.crumb.is_none()
            && matches!(
                self.local.section,
                LocalSection::Albums | LocalSection::Artists
            )
            && !self.local.items.is_empty()
    }

    /// On a grouped artist page (a drill-in), the flat-items index of the FIRST
    /// release-section header (any header but POPULAR) — where the labelled album
    /// grid region begins. `None` for any other list, so the album grid is only
    /// mixed into the artist page (not the top-level Albums section).
    pub(crate) fn artist_releases_from(&self) -> Option<usize> {
        self.local.crumb.as_ref()?; // only on a drill-in (the artist page)
        let pos = self
            .local
            .items
            .iter()
            .position(|it| matches!(it, LocalItem::Header(h) if *h != POPULAR_HEADER))?;
        self.local.items[pos..]
            .iter()
            .any(|it| matches!(it, LocalItem::Album(_)))
            .then_some(pos)
    }

    /// The release region as visual rows: each section header is its own row, and
    /// each group's album run becomes ONE row — a horizontal carousel (the render
    /// windows it + scrolls left/right). Shared by the render and the 2-D nav.
    pub(crate) fn artist_release_rows(&self) -> Vec<ReleaseRow> {
        let mut rows = Vec::new();
        let Some(from) = self.artist_releases_from() else {
            return rows;
        };
        let items = &self.local.items;
        let mut i = from;
        while i < items.len() {
            if matches!(items[i], LocalItem::Album(_)) {
                let start = i;
                while i < items.len() && matches!(items[i], LocalItem::Album(_)) {
                    i += 1;
                }
                rows.push(ReleaseRow::Cards((start..i).collect())); // whole group → one carousel
            } else {
                if let LocalItem::Header(label) = items[i] {
                    rows.push(ReleaseRow::Header(label.into()));
                }
                i += 1;
            }
        }
        rows
    }

    /// Toggle the Albums/Artists cover-art grid on/off (the `#` key) and remember
    /// the choice for this section. `local_grid_active` gates where it applies.
    pub(crate) fn toggle_grid_view(&mut self) {
        self.local.grid = !self.local.grid;
        self.views.grid.insert(self.local.section, self.local.grid);
    }

    /// Toggle grid⇄list for whichever browser is on screen — the shared target of
    /// the `#` key and the "View" settings row, so the two stay identical.
    pub(crate) fn toggle_grid_current(&mut self) {
        if self.layout == Layout::Spotify {
            self.spotify_toggle_grid();
        } else {
            self.toggle_grid_view();
        }
    }

    /// Route a 2-D grid/carousel move to whichever source owns the Main view — the
    /// shared target of the keyboard (`h`/`l`/`j`/`k` → `GridMove`) and the touchpad
    /// (horizontal/vertical scroll), so both drive the same navigation.
    pub(crate) fn grid_move_current(&mut self, dx: i32, dy: i32) {
        if self.layout == Layout::Spotify {
            self.spotify_grid_move(dx, dy, false);
        } else {
            self.grid_move(dx, dy, false);
        }
    }

    /// Like [`grid_move_current`] but horizontal moves are **locked to the current
    /// row/carousel** (they clamp at the row's end instead of wrapping onto the next
    /// row). Drives touchpad sideways scroll; row changes come from a separate
    /// vertical gesture. Carousels are already row-locked, so `locked` only changes
    /// the flat top-level grid.
    pub(crate) fn grid_scroll_current(&mut self, dx: i32, dy: i32) {
        if self.layout == Layout::Spotify {
            self.spotify_grid_move(dx, dy, true);
        } else {
            self.grid_move(dx, dy, true);
        }
    }

    /// Move the grid selection by `(dx, dy)` cards. On the artist page the release
    /// region is several labelled card grids (Albums / Singles / …): `h`/`l` step
    /// ±1 across the whole album sequence (crossing group boundaries), `j`/`k` move
    /// between visual card rows (skipping headers), and `k` off the top row drops
    /// back into the POPULAR track list.
    pub(crate) fn grid_move(&mut self, dx: i32, dy: i32, locked: bool) {
        if let Some(from) = self.artist_releases_from() {
            if self.local.sel < from {
                return; // in the POPULAR list (keymap shouldn't route here)
            }
            let rows = self.artist_release_rows();
            match release_grid_step(&rows, self.local.sel, dx, dy) {
                Some(new) => self.local.sel = new,
                // off the top of the release grid → the last selectable item above it
                None => {
                    if let Some(p) = self.local.items[..from]
                        .iter()
                        .rposition(|it| it.is_selectable())
                    {
                        self.local.sel = p;
                    }
                }
            }
            return;
        }
        self.local.sel = if locked && dy == 0 {
            grid_step_row_locked(
                self.local.sel,
                self.local.items.len(),
                self.local.cols.get(),
                dx,
            )
        } else {
            grid_step(
                self.local.sel,
                self.local.items.len(),
                self.local.cols.get(),
                dx,
                dy,
            )
        };
    }

    /// Build a section's top-level items: a leaf track-list, or a list of drillable
    /// containers (playlists / albums / artists / genres).
    fn local_section_items(&self, section: LocalSection) -> Vec<LocalItem> {
        let tracks = |ids: Vec<TrackId>| ids.into_iter().map(LocalItem::Track).collect();
        match section {
            LocalSection::AllTracks => tracks(self.library.all_tracks_sorted()),
            LocalSection::Favorites => tracks(self.smart_ids(SmartList::Favorites)),
            LocalSection::RecentlyAdded => tracks(self.smart_ids(SmartList::RecentlyAdded)),
            LocalSection::RecentlyPlayed => tracks(self.smart_ids(SmartList::RecentlyPlayed)),
            LocalSection::MostPlayed => tracks(self.smart_ids(SmartList::MostPlayed)),
            LocalSection::Playlists => self
                .library
                .playlists_sorted()
                .iter()
                .map(|p| LocalItem::Playlist(p.id))
                .collect(),
            LocalSection::Albums => self
                .library
                .albums_sorted()
                .iter()
                .map(|a| LocalItem::Album(a.id))
                .collect(),
            LocalSection::Artists => self
                .library
                .artists_sorted()
                .iter()
                .map(|a| LocalItem::Artist(a.id))
                .collect(),
            LocalSection::Genres => self
                .library
                .genres
                .iter()
                .cloned()
                .map(LocalItem::Genre)
                .collect(),
        }
    }

    /// Drill into a container: push the current list onto the back-stack, set a
    /// breadcrumb, and show the container's children (tracks, or — for an artist —
    /// the grouped artist page). Tracks/headers don't drill.
    pub(crate) fn local_open(&mut self, item: LocalItem) {
        let track_items = |ids: Vec<TrackId>| ids.into_iter().map(LocalItem::Track).collect();
        let (crumb, children) = match &item {
            LocalItem::Album(id) => {
                let title = self
                    .library
                    .albums
                    .get(id)
                    .map(|a| a.title.clone())
                    .unwrap_or_default();
                let ids = self.library.tracks_of(*id).iter().map(|t| t.id).collect();
                (format!("◉ {title}"), track_items(ids))
            }
            LocalItem::Artist(id) => {
                let name = self
                    .library
                    .artists
                    .get(id)
                    .map(|a| a.name.clone())
                    .unwrap_or_default();
                (format!("☻ {name}"), self.local_artist_page(*id))
            }
            LocalItem::Playlist(id) => {
                let name = self
                    .library
                    .playlists
                    .get(id)
                    .map(|p| p.name.clone())
                    .unwrap_or_default();
                (
                    format!("≡ {name}"),
                    track_items(self.library.playlist_tracks(*id)),
                )
            }
            LocalItem::Genre(g) => (
                format!("⊞ {g}"),
                track_items(self.library.tracks_with_genre(g)),
            ),
            LocalItem::Track(_) | LocalItem::Header(_) => return,
        };
        self.local.nav.push(
            std::mem::take(&mut self.local.items),
            self.local.sel,
            self.local.crumb.take(),
            (),
        );
        self.local.crumb = Some(crumb);
        self.local.items = children;
        self.local.sel = 0;
        self.focus = Focus::Main;
    }

    /// The grouped artist page (Spotify-style): the artist's most-played tracks
    /// ("POPULAR"), then their releases bucketed by kind (Albums / Singles / EPs /
    /// Remixes / Compilations), newest-first within each, only non-empty groups.
    fn local_artist_page(&self, artist: ArtistId) -> Vec<LocalItem> {
        let mut items = Vec::new();
        let mut tracks = self.library.tracks_of_artist(artist);
        tracks.sort_by_key(|&id| {
            std::cmp::Reverse(self.library.track(id).map(|t| t.play_count).unwrap_or(0))
        });
        let top: Vec<TrackId> = tracks.into_iter().take(5).collect();
        if !top.is_empty() {
            items.push(LocalItem::Header(POPULAR_HEADER));
            items.extend(top.into_iter().map(LocalItem::Track));
        }
        // bucket releases by section, keeping the newest-first order within each
        let albums = self.library.albums_of_by_year(artist);
        for section in ReleaseSection::ORDER {
            let mut any = false;
            for a in albums.iter().filter(|a| release_kind(a) == section) {
                if !any {
                    items.push(LocalItem::Header(section.label()));
                    any = true;
                }
                items.push(LocalItem::Album(a.id));
            }
        }
        items
    }

    /// Jump from the focused Artist info pane to the now-playing artist's full
    /// page — the same grouped drill-in (POPULAR + releases) reached by ⏎ on an
    /// Artist row. The pane always shows the *album* artist of the current track,
    /// and indexing keys each track's `artist_id` on that album artist, so the id
    /// is exactly the artist node the pane describes — no name lookup needed.
    /// No-op when nothing local is playing or the track predates artist indexing.
    pub(crate) fn open_local_artist_page(&mut self) {
        if let Some(id) = self.current_track().and_then(|t| t.artist_id) {
            self.local_open(LocalItem::Artist(id));
        }
    }

    /// Open the now-playing artist's page from the Artist pane. The Spotify view's
    /// pane opens the Spotify artist; every other view opens the local artist. Shared
    /// by the keyboard (⏎) and mouse (double-click) paths so both behave the same.
    pub(crate) fn open_artist_page(&mut self) {
        if self.layout == Layout::Spotify {
            self.open_spotify_artist_page();
        } else {
            self.open_local_artist_page();
        }
    }

    /// Back out of a drill-in: pop a frame, restoring the parent list + cursor +
    /// breadcrumb verbatim. Returns `true` if it popped (else already at the top).
    pub(crate) fn local_back(&mut self) -> bool {
        if let Some(f) = self.local.nav.pop() {
            self.local.items = f.items;
            self.local.sel = f.sel;
            self.local.crumb = f.crumb;
            true
        } else {
            false
        }
    }

    /// Enter in the local library: a section (sidebar) loads its list; a container
    /// drills in; a track plays (queueing from the surrounding tracks in the list).
    pub(crate) fn local_activate(&mut self) {
        if self.focus == Focus::Sidebar {
            self.local_load_section();
            self.focus = Focus::Main;
            return;
        }
        let Some(item) = self.local.items.get(self.local.sel).cloned() else {
            return;
        };
        match item {
            LocalItem::Track(id) => {
                // queue = every track in the current list, starting at the selection
                let queue: Vec<TrackId> = self
                    .local
                    .items
                    .iter()
                    .filter_map(|it| match it {
                        LocalItem::Track(t) => Some(*t),
                        _ => None,
                    })
                    .collect();
                let idx = queue.iter().position(|&t| t == id).unwrap_or(0);
                self.player.queue.items = queue;
                self.player.queue.position = idx;
                self.player.current = Some(id);
                self.search.active = false;
                self.play_current();
            }
            LocalItem::Header(_) => {}
            container => self.local_open(container),
        }
    }

    /// A stable, rescan-proof string id for a container (by name/title, since
    /// Album/Artist/Playlist ids are reassigned on rescan). Used to persist a
    /// drill-in across restarts. `None` for tracks/headers.
    fn local_ref(&self, item: &LocalItem) -> Option<String> {
        match item {
            LocalItem::Album(id) => self
                .library
                .albums
                .get(id)
                .map(|a| format!("a\t{}\t{}", a.title, a.artist)),
            LocalItem::Artist(id) => self
                .library
                .artists
                .get(id)
                .map(|a| format!("r\t{}", a.name)),
            LocalItem::Playlist(id) => self
                .library
                .playlists
                .get(id)
                .map(|p| format!("p\t{}", p.name)),
            LocalItem::Genre(g) => Some(format!("g\t{g}")),
            LocalItem::Track(_) | LocalItem::Header(_) => None,
        }
    }

    /// The current drill path (opened containers, top → bottom) as stable refs —
    /// persisted in the session so a reopen lands on the same drilled view.
    pub(crate) fn local_open_path(&self) -> Vec<String> {
        self.local
            .nav
            .opened()
            .filter_map(|it| self.local_ref(it))
            .collect()
    }

    /// Re-drill a saved path (from `local_open_path`) after the section is loaded:
    /// at each level find the container matching the ref and open it, then place
    /// the cursor. Stops early if a ref no longer resolves (e.g. deleted album).
    pub(crate) fn local_restore_drill(&mut self, path: &[String], sel: usize) {
        for r in path {
            let Some(idx) = self
                .local
                .items
                .iter()
                .position(|it| self.local_ref(it).as_deref() == Some(r.as_str()))
            else {
                break;
            };
            self.local.sel = idx;
            let item = self.local.items[idx].clone();
            self.local_open(item);
        }
        self.local.sel = sel.min(self.local.items.len().saturating_sub(1));
    }
}

#[cfg(test)]
mod tests {
    use super::{ReleaseSection, grid_step_row_locked, release_kind};
    use crate::core::model::{Album, AlbumId, TrackId};

    #[test]
    fn grid_step_row_locked_clamps_to_the_row() {
        // 8 items, 3 cols → rows [0,1,2] [3,4,5] [6,7]
        assert_eq!(grid_step_row_locked(1, 8, 3, 1), 2, "steps within the row");
        assert_eq!(
            grid_step_row_locked(2, 8, 3, 1),
            2,
            "clamps at the row end (no wrap to 3)"
        );
        assert_eq!(
            grid_step_row_locked(3, 8, 3, -1),
            3,
            "clamps at the row start (no wrap to 2)"
        );
        assert_eq!(
            grid_step_row_locked(7, 8, 3, 1),
            7,
            "short last row clamps at the last item"
        );
        assert_eq!(grid_step_row_locked(0, 0, 3, 1), 0, "empty grid is a no-op");
    }

    fn album(title: &str, tracks: usize) -> Album {
        Album {
            id: AlbumId::new(1),
            title: title.into(),
            artist: String::new(),
            artist_id: None,
            year: None,
            genre: None,
            track_ids: vec![TrackId::new(1); tracks],
            cover_path: None,
        }
    }

    #[test]
    fn release_kind_classifies_by_title_and_track_count() {
        // title keywords win over track count, checked remix → compilation → …
        assert_eq!(
            release_kind(&album("Song (Remix)", 1)),
            ReleaseSection::Remixes
        );
        assert_eq!(
            release_kind(&album("Greatest Hits", 18)),
            ReleaseSection::Compilations
        );
        // singles and EPs share one section (matching Spotify's "SINGLES & EPs")
        assert_eq!(
            release_kind(&album("Lead Single", 9)),
            ReleaseSection::SinglesEps
        );
        assert_eq!(release_kind(&album("My EP", 9)), ReleaseSection::SinglesEps);
        // else by track count: ≤6 single/EP, more is an album
        assert_eq!(
            release_kind(&album("One Off", 1)),
            ReleaseSection::SinglesEps
        );
        assert_eq!(
            release_kind(&album("Short Set", 5)),
            ReleaseSection::SinglesEps
        );
        assert_eq!(release_kind(&album("Debut", 12)), ReleaseSection::Albums);
    }
}
