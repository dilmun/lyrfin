//! Spotify browse / navigation methods on `AppState` (extracted from
//! app/spotify): loading sections + search, the sidebar/list/pane focus
//! cycle, drill-in (`spotify_open`) + back (`spotify_cancel`) + activate,
//! and the session-restore cursor placement.

use super::*;
use crate::app::Focus;

/// Items-per-shelf step for a drilled-in browse grid: the initial `browsePage`
/// fetch, and how much each scroll-triggered "load more" grows it by.
const BROWSE_PAGE_STEP: usize = 50;

impl AppState {
    /// Initial browse load on connect: refresh whatever view is showing. When the
    /// optimistic cache has already put a drilled-in container on screen, refresh THAT
    /// in place (no section round-trip, no flash) instead of reloading from scratch;
    /// otherwise re-run the (restored) search or load the (restored) section — which,
    /// for a session-restored drill with no cache, re-opens the container once the
    /// section lands (`spotify_apply_initial_restore`).
    pub(crate) fn spotify_load_initial(&mut self) {
        if self.spotify.open_item.is_some() {
            self.spotify.restore_open = None; // the shown container is authoritative
            self.spotify_refresh_open();
            return;
        }
        if self.spotify.in_search && !self.spotify.query.trim().is_empty() {
            let q = self.spotify.query.clone();
            self.spotify_search(q);
        } else {
            self.spotify_load_section();
        }
    }

    /// Load the current library section from the Web API (no-op until connected).
    pub(crate) fn spotify_load_section(&mut self) {
        use crate::spotify::api::Section;
        // Home + Browse are served by the pathfinder GraphQL gateway over librespot,
        // not the Web API — route them separately.
        match self.spotify.section {
            Section::Home => return self.spotify_load_home(),
            Section::Browse => return self.spotify_load_browse(),
            _ => {}
        }
        self.spotify_reset_browse();
        // grid vs list for this section: the persisted override, else on for the
        // container sections (Albums/Artists). `spotify_grid_active` gates render.
        let section = self.spotify.section;
        self.spotify.grid = self
            .views
            .spotify_grid
            .get(&section)
            .copied()
            .unwrap_or(matches!(
                section,
                Section::Albums | Section::Artists | Section::Playlists | Section::Podcasts
            ));
        let (Some(tokens), Some(tx)) =
            (self.spotify.tokens.as_ref(), self.workers.spotify.as_ref())
        else {
            return;
        };
        self.workers.spotify_seq += 1;
        let key = format!("s{}", self.workers.spotify_seq);
        self.spotify.key = key.clone();
        self.spotify.loading = true;
        self.spotify.in_search = false;
        self.spotify.note = "Loading…".into();
        let _ = tx.send(crate::spotify::api::SpRequest::Library {
            section: self.spotify.section,
            token: tokens.access_token.clone(),
            key,
        });
    }

    /// Load Spotify's editorial home feed via the librespot session (pathfinder
    /// GraphQL — the Web API no longer serves browse to dev-mode apps). Needs a
    /// session for its login5/client tokens; `spotify_ensure_session` spawns a
    /// metadata-only one (no audio bridge) if none is up yet.
    pub(crate) fn spotify_load_home(&mut self) {
        self.spotify_reset_browse();
        // Home renders as sectioned carousels (one per shelf), not the flat grid.
        self.spotify.grid = false;
        if !self.spotify_ensure_session() {
            self.spotify.note = "Connect Spotify to load Home".into();
            return;
        }
        let Some(cmd) = self.spov.session_cmd.as_ref() else {
            self.spotify.note = "Home unavailable (no session)".into();
            return;
        };
        self.workers.spotify_seq += 1;
        let key = format!("s{}", self.workers.spotify_seq);
        self.spotify.key = key.clone();
        self.spotify.loading = true;
        self.spotify.in_search = false;
        self.spotify.note = "Loading…".into();
        let _ = cmd.send(crate::spotify::session::SessionCommand::FetchHome { key });
    }

    /// Load the "Browse all" categories grid (the pathfinder browse root) via the
    /// librespot session — same session requirement + path as Home.
    pub(crate) fn spotify_load_browse(&mut self) {
        self.spotify_reset_browse();
        self.spotify.grid = false;
        if !self.spotify_ensure_session() {
            self.spotify.note = "Connect Spotify to browse".into();
            return;
        }
        let Some(cmd) = self.spov.session_cmd.as_ref() else {
            self.spotify.note = "Browse unavailable (no session)".into();
            return;
        };
        self.workers.spotify_seq += 1;
        let key = format!("s{}", self.workers.spotify_seq);
        self.spotify.key = key.clone();
        self.spotify.loading = true;
        self.spotify.in_search = false;
        self.spotify.note = "Loading…".into();
        let _ = cmd.send(crate::spotify::session::SessionCommand::FetchBrowsePage {
            uri: crate::spotify::pathfinder::BROWSE_ROOT.to_string(),
            key,
            limit: BROWSE_PAGE_STEP,
        });
    }

    /// Run a Spotify search (empty query → back to the section list).
    pub(crate) fn spotify_search(&mut self, query: String) {
        self.spotify.query = query.clone();
        if query.trim().is_empty() {
            self.spotify_load_section();
            return;
        }
        self.spotify_reset_browse();
        let (Some(tokens), Some(tx)) =
            (self.spotify.tokens.as_ref(), self.workers.spotify.as_ref())
        else {
            return;
        };
        self.workers.spotify_seq += 1;
        let key = format!("s{}", self.workers.spotify_seq);
        self.spotify.key = key.clone();
        self.spotify.loading = true;
        self.spotify.in_search = true;
        self.spotify.note = "Searching…".into();
        let _ = tx.send(crate::spotify::api::SpRequest::Search {
            query,
            token: tokens.access_token.clone(),
            key,
        });
    }

    /// Whether the Spotify main pane should render as a cover-art grid right now:
    /// grid mode is on, we're at the top level of the Albums/Artists section (not
    /// searching, not drilled in), and there are items. Mirrors `local_grid_active`.
    pub(crate) fn spotify_grid_active(&self) -> bool {
        use crate::spotify::api::Section;
        self.spotify.grid
            && !self.spotify.in_search
            && !self.spotify.searching
            && self.spotify.crumb.is_none()
            && matches!(
                self.spotify.section,
                Section::Albums | Section::Artists | Section::Playlists
            )
            && !self.spotify.items.is_empty()
    }

    /// Toggle the Spotify cover-art grid (`#`) and remember the choice for this
    /// section. `spotify_grid_active` gates where it applies.
    pub(crate) fn spotify_toggle_grid(&mut self) {
        self.spotify.grid = !self.spotify.grid;
        self.views
            .spotify_grid
            .insert(self.spotify.section, self.spotify.grid);
    }

    /// True when the result list is a grouped artist page (items tagged with a
    /// release `Group`), vs a flat section/search list.
    pub(crate) fn spotify_on_artist_page(&self) -> bool {
        use crate::spotify::api::Group;
        self.spotify.items.iter().any(|i| i.group != Group::None)
    }

    /// True when the current list should render as sectioned shelf carousels — any
    /// pathfinder browse content whose items carry a shelf `section` (the Home feed,
    /// and a Browse category's playlist shelves). Not searching. The artist page
    /// (grouped by `Group`) and flat lists/grids are handled elsewhere.
    pub(crate) fn spotify_sectioned_active(&self) -> bool {
        !self.spotify.in_search
            && !self.spotify.searching
            && self.spotify.items.iter().any(|i| i.section.is_some())
    }

    /// True when the "Browse all" categories should render as a flat cover grid: on
    /// the Browse section, at the top level, with category tiles loaded. (A category
    /// drilled into shows playlist shelves via `spotify_sectioned_active` instead.)
    pub(crate) fn spotify_browse_grid_active(&self) -> bool {
        use crate::spotify::api::{Kind, Section};
        self.spotify.section == Section::Browse
            && !self.spotify.in_search
            && !self.spotify.searching
            && self.spotify.crumb.is_none()
            && self.spotify.items.iter().any(|i| i.kind == Kind::Category)
    }

    /// A drilled-in podcast browse page shown as a flat cover grid: **Podcast Charts**
    /// (a flat list of shows) or the **Categories** page (category tiles). Distinct
    /// from the music `spotify_grid_active` (top-level Albums/Artists) — this fires
    /// *inside* a drill-in (`crumb`), for a flat, non-sectioned list of shows or
    /// categories, and follows the grid/list toggle (`#`). A sectioned page (shelves
    /// with titles) stays carousels; only a single flat list becomes a grid.
    pub(crate) fn spotify_podcast_grid_active(&self) -> bool {
        use crate::spotify::api::Kind;
        if self.spotify.crumb.is_none()
            || self.spotify.in_search
            || self.spotify.searching
            || self.spotify.items.is_empty()
            || self.spotify_sectioned_active()
        {
            return false;
        }
        let items = &self.spotify.items;
        // A page of category tiles is ALWAYS a grid (like the music Browse root — a
        // flat list of 50+ genre names reads poorly); a page of shows (Podcast Charts)
        // follows the grid/list toggle.
        if items.iter().all(|i| i.kind == Kind::Category) {
            return true;
        }
        self.spotify.grid && items.iter().all(|i| i.kind == Kind::Show)
    }

    /// Sectioned browse content as carousels: a header + one carousel per shelf,
    /// grouping the flat `items` by their `section` title. Mirrors
    /// `spotify_artist_release_rows` (the same `ReleaseRow`/`release_grid` model),
    /// but labels come from the shelves themselves rather than a fixed taxonomy.
    pub(crate) fn spotify_browse_rows(&self) -> Vec<crate::app::ReleaseRow> {
        use crate::app::ReleaseRow;
        use crate::spotify::api::Kind;
        let items = &self.spotify.items;
        // set by `release_grid` right before it calls this (rows_at), so it's the
        // live column count for chunking the category grid.
        let cols = self.spotify.cols.get().max(1);
        let mut rows = Vec::new();
        let mut i = 0;
        while i < items.len() {
            let start = i;
            let title = items[start].section.clone().unwrap_or_default();
            while i < items.len() && items[i].section == items[start].section {
                i += 1;
            }
            // A shelf of category tiles renders as a FLAT grid, not a horizontal
            // carousel: chunk it into `cols`-wide rows (each fits without scroll
            // arrows, so it reads as a grid, and the shared row-stepper already
            // navigates between rows keeping the column). Show shelves stay one-row
            // carousels.
            let is_categories = items[start..i].iter().all(|it| it.kind == Kind::Category);
            // pathfinder delivers category tiles untitled — give the grid a header so
            // it reads like the titled show shelves above/below it.
            let header = if !title.is_empty() {
                title
            } else if is_categories {
                "Categories".to_string()
            } else {
                String::new()
            };
            rows.push(ReleaseRow::Header(header.into()));
            if is_categories {
                // split out the "Browse all categories" button — it gets its own
                // full-width row below the grid of real category tiles.
                let (buttons, tiles): (Vec<usize>, Vec<usize>) = (start..i).partition(|&j| {
                    items[j].name == crate::spotify::pathfinder::ALL_CATEGORIES_LABEL
                });
                for chunk in tiles.chunks(cols) {
                    rows.push(ReleaseRow::Cards(chunk.to_vec()));
                }
                for idx in buttons {
                    rows.push(ReleaseRow::Banner(idx));
                }
            } else {
                rows.push(ReleaseRow::Cards((start..i).collect()));
            }
        }
        rows
    }

    /// On an artist page, the index of the first release item (first non-POPULAR) —
    /// where the grouped album-grid region begins. `items.len()` if there are no
    /// releases (only popular tracks). The leading `[0..from)` are the POPULAR list.
    pub(crate) fn spotify_releases_from(&self) -> usize {
        use crate::spotify::api::Group;
        self.spotify
            .items
            .iter()
            .position(|i| i.group != Group::Popular)
            .unwrap_or(self.spotify.items.len())
    }

    /// The index where the card carousels begin on a "track list + carousels" page —
    /// an artist page (releases after the POPULAR tracks) or search results (the card
    /// kinds after the SONGS). `Some(from)` selects that layout; `None` = neither.
    pub(crate) fn spotify_carousels_from(&self) -> Option<usize> {
        use crate::spotify::api::Kind;
        if self.spotify_on_artist_page() {
            Some(self.spotify_releases_from())
        } else if self.spotify.in_search && self.spotify.items.iter().any(|i| i.kind != Kind::Track)
        {
            // search: the leading SONGS are the tracks; cards (albums/artists/…) follow
            Some(
                self.spotify
                    .items
                    .iter()
                    .position(|i| i.kind != Kind::Track)
                    .unwrap_or(self.spotify.items.len()),
            )
        } else {
            None
        }
    }

    /// The carousel rows for a "track list + carousels" page: the artist's release
    /// groups, or search's card kinds. One header + one carousel per group.
    pub(crate) fn spotify_carousel_rows(&self) -> Vec<crate::app::ReleaseRow> {
        if self.spotify_on_artist_page() {
            self.spotify_artist_release_rows()
        } else {
            self.spotify_search_rows()
        }
    }

    /// Search results' card region as carousel rows: the non-track items grouped by
    /// kind (Albums / Artists / Playlists / Podcasts), which arrive already contiguous
    /// by kind (see `on_spotify_result`).
    fn spotify_search_rows(&self) -> Vec<crate::app::ReleaseRow> {
        use crate::app::ReleaseRow;
        use crate::spotify::api::Kind;
        let items = &self.spotify.items;
        let mut rows = Vec::new();
        let mut i = items
            .iter()
            .position(|it| it.kind != Kind::Track)
            .unwrap_or(items.len());
        while i < items.len() {
            let k = items[i].kind;
            let label = match k {
                Kind::Album => "ALBUMS",
                Kind::Artist => "ARTISTS",
                Kind::Playlist => "PLAYLISTS",
                Kind::Show => "PODCASTS",
                Kind::Track | Kind::Category => "",
            };
            rows.push(ReleaseRow::Header(label.into()));
            let start = i;
            while i < items.len() && items[i].kind == k {
                i += 1;
            }
            rows.push(ReleaseRow::Cards((start..i).collect()));
        }
        rows
    }

    /// The artist page's release region as visual rows (a header per release group +
    /// one carousel row per group), mirroring the local `artist_release_rows`.
    pub(crate) fn spotify_artist_release_rows(&self) -> Vec<crate::app::ReleaseRow> {
        use crate::app::ReleaseRow;
        use crate::app::release::ReleaseSection;
        let items = &self.spotify.items;
        let mut rows = Vec::new();
        let mut i = self.spotify_releases_from();
        while i < items.len() {
            let g = items[i].group;
            // map the API group onto the shared section taxonomy for the header label
            let label = ReleaseSection::from_group(g).map_or("", ReleaseSection::label);
            rows.push(ReleaseRow::Header(label.into()));
            let start = i;
            while i < items.len() && items[i].group == g {
                i += 1;
            }
            rows.push(ReleaseRow::Cards((start..i).collect())); // whole group → one carousel
        }
        rows
    }

    /// Move the Spotify grid selection by `(dx, dy)` cards. On a flat section grid
    /// it's a plain 2-D step; on a grouped artist page it navigates the release rows
    /// (shared `release_grid_step`), dropping into the POPULAR tracks off the top.
    pub(crate) fn spotify_grid_move(&mut self, dx: i32, dy: i32, locked: bool) {
        // Sectioned browse (Home / a category's shelves): navigate the carousels
        // (shared `release_grid_step`); an up-move off the top carousel just clamps
        // (no track list above, unlike the artist page).
        if self.spotify_sectioned_active() {
            let rows = self.spotify_browse_rows();
            if let Some(new) =
                crate::app::release::release_grid_step(&rows, self.spotify.sel, dx, dy)
            {
                self.spotify.sel = new;
            }
            return;
        }
        // "track list + carousels" (artist page or search): navigate the card
        // carousels; an up-move off the top carousel drops into the leading track list.
        if let Some(from) = self.spotify_carousels_from() {
            if self.spotify.sel < from {
                return; // in the leading track list (keymap shouldn't route here)
            }
            let rows = self.spotify_carousel_rows();
            match crate::app::release::release_grid_step(&rows, self.spotify.sel, dx, dy) {
                Some(new) => self.spotify.sel = new,
                None => self.spotify.sel = from.saturating_sub(1), // → last leading track
            }
            return;
        }
        self.spotify.sel = if locked && dy == 0 {
            crate::app::local_browse::grid_step_row_locked(
                self.spotify.sel,
                self.spotify.items.len(),
                self.spotify.cols.get(),
                dx,
            )
        } else {
            crate::app::local_browse::grid_step(
                self.spotify.sel,
                self.spotify.items.len(),
                self.spotify.cols.get(),
                dx,
                dy,
            )
        };
        // reaching the last rows of a drilled browse grid pages the next batch in
        self.spotify_maybe_load_more();
    }

    /// Grow a drilled-in **flat** browse grid (Podcast Charts / a Categories page) as
    /// the cursor nears its end — "load more as you scroll". Re-fetches the same
    /// `browsePage` with a larger items-per-shelf limit (one known-good query, no
    /// separate op), appending on arrival. Only the flat grid pages this way; the
    /// sectioned hub caps its shelves (reach the rest via the "Browse all categories"
    /// button), so it's excluded. No-op while a grow is in flight or it's fully loaded.
    pub(crate) fn spotify_maybe_load_more(&mut self) {
        if !self.spotify_podcast_grid_active()
            || self.spotify.browse_loading_more
            || self.spotify.browse_exhausted
        {
            return;
        }
        let Some(uri) = self.spotify.browse_page.clone() else {
            return;
        };
        // only when the cursor is within the last couple of rows
        let cols = self.spotify.cols.get().max(1);
        if self.spotify.sel + cols * 2 < self.spotify.items.len() {
            return;
        }
        let Some(cmd) = self.spov.session_cmd.clone() else {
            return;
        };
        self.spotify.browse_limit += BROWSE_PAGE_STEP;
        self.spotify.browse_loading_more = true;
        self.workers.spotify_seq += 1;
        let key = format!("s{}", self.workers.spotify_seq);
        self.spotify.key = key.clone();
        let _ = cmd.send(crate::spotify::session::SessionCommand::FetchBrowsePage {
            uri,
            key,
            limit: self.spotify.browse_limit,
        });
    }

    /// `b`: jump focus between the sidebar and the result list.
    pub(crate) fn spotify_toggle_sidebar(&mut self) {
        self.focus = if self.focus == Focus::Sidebar {
            Focus::Main
        } else {
            Focus::Sidebar
        };
    }

    /// Back out one Spotify level — the Spotify analogue of `local_back`, shared by
    /// `SpotifyCancel` (Esc / leaving the search box) and the global back chain
    /// (`go_back`, reached by Esc + ctrl-o). Returns `true` if it popped a level (so
    /// `q`/back knows there was something to back out of, vs quitting at the top).
    pub(crate) fn spotify_cancel(&mut self) -> bool {
        // a focused pane: Esc returns to the result list (don't navigate back)
        if matches!(self.focus, Focus::Pane(_)) {
            self.focus = Focus::Main;
            return true;
        }
        if self.spotify.searching {
            self.spotify.searching = false;
            true
        } else if let Some(frame) = self.spotify.nav.pop() {
            // restore the parent list verbatim (no refetch) + its search/open context
            self.spotify.items = frame.items;
            self.spotify.sel = frame.sel;
            self.spotify.crumb = frame.crumb;
            self.spotify.open_item = frame.ctx.open_item;
            self.spotify.in_search = frame.ctx.in_search;
            self.spotify.query = frame.ctx.query;
            self.spotify.note = frame.ctx.note;
            self.spotify.loading = false;
            // the restored parent is shown verbatim (no refetch), so its load-more
            // paging can't be resumed — disable it until the page is re-drilled
            self.spotify.browse_page = None;
            self.spotify.browse_loading_more = false;
            self.spotify.browse_exhausted = false;
            true
        } else if self.spotify.open_item.is_some() {
            // a cache-restored drill-in has no cached back-stack (only the visible list
            // is cached) — Esc returns to its section, loaded fresh, rather than
            // dead-ending. `spotify_load_section` clears the drill + reloads.
            self.spotify_load_section();
            true
        } else if self.spotify.in_search {
            self.spotify.query.clear();
            self.spotify_load_section();
            true
        } else {
            false
        }
    }

    /// ⏎ in the Spotify view. Sidebar focus → load that section. List focus →
    /// play the selected track (building a queue from the surrounding tracks).
    pub(crate) fn spotify_activate(&mut self) {
        if self.spotify.searching {
            self.spotify.searching = false;
            return;
        }
        // QUEUE pane focused: ⏎ jumps to (plays) the selected upcoming track
        if self.focus == Focus::Pane(Panel::Queue) {
            if !self.spov.sp_queue.is_empty() {
                let idx = self.spotify.queue_sel.min(self.spov.sp_queue.len() - 1);
                self.spotify_play(self.spov.sp_queue.clone(), idx);
            }
            return;
        }
        // Artist pane: ⏎ opens the now-playing artist's full page
        if self.focus == Focus::Pane(Panel::Artist) {
            self.open_spotify_artist_page();
            return;
        }
        // other panes (Lyrics) have no Enter action
        if matches!(self.focus, Focus::Pane(_)) {
            return;
        }
        if self.focus == Focus::Sidebar {
            self.spotify_load_section();
            self.focus = Focus::Main;
            return;
        }
        use crate::spotify::api::Kind;
        let Some(item) = self.spotify.items.get(self.spotify.sel).cloned() else {
            return;
        };
        match item.kind {
            Kind::Track => {
                // queue = all tracks in the current list, starting at the selection
                let queue: Vec<_> = self
                    .spotify
                    .items
                    .iter()
                    .filter(|it| it.kind == Kind::Track)
                    .cloned()
                    .collect();
                let idx = queue.iter().position(|t| t.uri == item.uri).unwrap_or(0);
                self.spov.sp_fail_streak = 0; // a fresh user-initiated attempt
                self.spotify_play(queue, idx);
            }
            // albums/playlists/artists/shows/categories → drill in (tracks, or a
            // category's playlist shelves)
            Kind::Album | Kind::Playlist | Kind::Artist | Kind::Show | Kind::Category => {
                self.spotify_open(item)
            }
        }
    }

    /// Jump from the focused Artist info pane to the now-playing item's full page,
    /// reusing the same [`Self::spotify_open`] drill-in as ⏎ on a row: a music track
    /// opens its primary artist's page, a podcast episode opens its show's page (its
    /// episode list). No-op when nothing is playing or a music track has no artist URI.
    pub(crate) fn open_spotify_artist_page(&mut self) {
        use crate::spotify::api::Kind;
        let Some(tr) = self.spov.now_spotify.clone() else {
            return;
        };
        if Self::is_episode_uri(&tr.uri) {
            // podcast → open its show (episode list). Newly-played episodes carry a
            // `show_uri`; one restored from an older session doesn't, so resolve it
            // via the Web API and open when the result lands (`ShowResolved`).
            match tr.show_uri.clone() {
                Some(uri) => self.open_spotify_container(uri, tr.album.clone(), Kind::Show),
                None => {
                    if let Some((token, tx)) = self.spotify_worker() {
                        let _ = tx.send(crate::spotify::api::SpRequest::ResolveShow {
                            episode_uri: tr.uri.clone(),
                            name: tr.album.clone(),
                            token,
                        });
                    }
                }
            }
        } else if let Some(uri) = tr.artist_uri.clone() {
            // music track → open the primary artist's page
            self.open_spotify_container(uri, tr.primary_artist().to_string(), Kind::Artist);
        }
    }

    /// Drill into a container from the Artist pane and hand focus to it (so j/k
    /// navigate the opened page — the local drill-in does this inside `local_open`;
    /// the Spotify drill-in doesn't). Shared by the direct and async open paths.
    pub(crate) fn open_spotify_container(
        &mut self,
        uri: String,
        name: String,
        kind: crate::spotify::api::Kind,
    ) {
        self.spotify_open(crate::spotify::api::Item {
            uri,
            name,
            kind,
            ..Default::default()
        });
        self.focus = Focus::Main;
    }

    /// Drill into a container: push the current list, request its tracks, and show
    /// a breadcrumb. Esc (`spotify_cancel`) pops back.
    pub(crate) fn spotify_open(&mut self, item: crate::spotify::api::Item) {
        use crate::spotify::api::Kind;
        if self.spotify.tokens.is_none() {
            return;
        }
        // save the current list (+ its search/open context) to restore on back
        self.spotify.nav.push(
            std::mem::take(&mut self.spotify.items),
            self.spotify.sel,
            self.spotify.crumb.take(),
            super::SpCtx {
                open_item: self.spotify.open_item.take(),
                in_search: self.spotify.in_search,
                query: self.spotify.query.clone(),
                note: self.spotify.note.clone(),
            },
        );
        // remember the container we're entering, so a session can re-open it
        self.spotify.open_item = Some(item.clone());
        let icon = match item.kind {
            Kind::Album => "◉",
            Kind::Artist => "☻",
            Kind::Playlist => "≡",
            Kind::Show => "▣",
            Kind::Category => "▦",
            Kind::Track => "♪",
        };
        self.spotify.crumb = Some(format!("{icon} {}", item.name));
        self.spotify.sel = 0;
        self.spotify_fetch_container(item);
    }

    /// Dispatch the fetch for a container's contents (its tracks / grouped artist page
    /// / browse page) on a fresh request key. Isolated from the drill-in bookkeeping
    /// (nav / breadcrumb / cursor) so it serves BOTH the initial open
    /// ([`Self::spotify_open`]) and an in-place background refresh
    /// ([`Self::spotify_refresh_open`]).
    fn spotify_fetch_container(&mut self, item: crate::spotify::api::Item) {
        use crate::spotify::api::Kind;
        if self.spotify.tokens.is_none() {
            return;
        }
        self.workers.spotify_seq += 1;
        let key = format!("s{}", self.workers.spotify_seq);
        self.spotify.key = key.clone();
        self.spotify.loading = true;
        self.spotify.note = "Loading…".into();
        // a fresh container isn't a pageable grid until proven one (the Category arm
        // sets it); resets any prior page's load-more paging.
        self.spotify.browse_page = None;
        self.spotify.browse_limit = BROWSE_PAGE_STEP;
        self.spotify.browse_loading_more = false;
        self.spotify.browse_exhausted = false;
        match item.kind {
            // albums + podcast shows resolve via the Web API (no session needed)
            Kind::Album | Kind::Show => {
                let token = self.spotify.tokens.as_ref().unwrap().access_token.clone();
                if let Some(tx) = &self.workers.spotify {
                    let _ = tx.send(crate::spotify::api::SpRequest::Open {
                        uri: item.uri,
                        kind: item.kind,
                        token,
                        key,
                    });
                }
            }
            // playlists + artists are blocked on the Web API for dev-mode apps →
            // fetch via librespot metadata (needs the session). An artist opens a
            // grouped page (popular + albums + singles); a playlist its tracks.
            Kind::Playlist | Kind::Artist => {
                if self.spotify_ensure_session() {
                    if let Some(cmd) = &self.spov.session_cmd {
                        let msg = if item.kind == Kind::Artist {
                            crate::spotify::session::SessionCommand::FetchArtistPage {
                                uri: item.uri,
                                key,
                            }
                        } else {
                            crate::spotify::session::SessionCommand::FetchTracks {
                                uri: item.uri,
                                artist: false,
                                key,
                            }
                        };
                        let _ = cmd.send(msg);
                    }
                } else {
                    self.spotify.loading = false;
                    self.spotify.note = "Log in to open this".into();
                }
            }
            // a "Browse all" category → its browse page (playlist shelves) over
            // pathfinder, same session path as playlists/artists
            Kind::Category => {
                if self.spotify_ensure_session() {
                    // a drilled category page is a load-more-able grid: remember its
                    // uri so `spotify_maybe_load_more` can grow it on scroll
                    self.spotify.browse_page = Some(item.uri.clone());
                    if let Some(cmd) = &self.spov.session_cmd {
                        let _ =
                            cmd.send(crate::spotify::session::SessionCommand::FetchBrowsePage {
                                uri: item.uri,
                                key,
                                limit: self.spotify.browse_limit,
                            });
                    }
                } else {
                    self.spotify.loading = false;
                    self.spotify.note = "Log in to open this".into();
                }
            }
            Kind::Track => {}
        }
    }

    /// Refresh the currently-open container's contents in place — no nav push, no
    /// breadcrumb change, so the drill-in stays put. The optimistic view cache shows
    /// the container immediately on launch; this re-fetches it in the background so it
    /// updates without a blank or a jump. No-op when nothing is drilled in.
    pub(crate) fn spotify_refresh_open(&mut self) {
        if let Some(item) = self.spotify.open_item.clone() {
            self.spotify_fetch_container(item);
        }
    }

    /// Leave any drill-in and clear the back stack (used when jumping to a
    /// top-level section or starting a search).
    pub(crate) fn spotify_reset_browse(&mut self) {
        self.spotify.crumb = None;
        self.spotify.open_item = None;
        self.spotify.nav.clear();
        self.spotify.browse_page = None;
        self.spotify.browse_loading_more = false;
        self.spotify.browse_exhausted = false;
    }

    /// The cursor to place on a freshly loaded list: a one-shot session-restored
    /// position (clamped to the list), else the top.
    pub(super) fn spotify_restored_sel(&mut self) -> usize {
        match self.spotify.restore_sel.take() {
            Some(sel) => sel.min(self.spotify.items.len().saturating_sub(1)),
            None => 0,
        }
    }

    /// Called once the initial section/search list lands on reconnect: if a
    /// drill-in was saved, re-open it (its `Opened` result then restores the
    /// in-container cursor); otherwise place the restored cursor on this list.
    pub(super) fn spotify_apply_initial_restore(&mut self) {
        if let Some(container) = self.spotify.restore_open.take() {
            // Prefer the freshly-loaded list's full `Item` (richest metadata) when the
            // container is present, and park the section cursor on it so Esc returns
            // there. Otherwise open the reconstructed descriptor as-is, so a drill-in
            // still restores when its container isn't in this list — an artist opened
            // from the now-playing track, a playlist not under your Playlists, etc.
            let full = self
                .spotify
                .items
                .iter()
                .find(|it| it.uri == container.uri)
                .cloned();
            if let Some(pos) = self
                .spotify
                .items
                .iter()
                .position(|it| it.uri == container.uri)
            {
                self.spotify.sel = pos;
            }
            self.spotify_open(full.unwrap_or(container)); // re-enters (async refetch)
        } else {
            self.spotify.sel = self.spotify_restored_sel();
        }
    }
}
