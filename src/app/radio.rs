//! Internet-radio methods on `AppState` (extracted from app/mod.rs).

use super::*;
use crate::audio::AudioCommand;
use crate::core::player::Status;

/// Radio playback-overlay state, grouped out of [`AppState`]. Accessed as
/// `app.rnow.*`; the local player is preserved/frozen while a station streams.
pub struct RadioNow {
    pub now_station: Option<crate::radio::Station>,
    pub now_station_title: Option<String>,
    pub radio_paused: bool,
    /// Timeshift (DVR) state for a buffered live stream, in seconds since tune-in.
    /// `None` when the stream isn't timeshifted (DVR off / not yet reported), which
    /// keeps such a stream forward-only.
    pub dvr: Option<DvrState>,
}

/// The timeshift window + play position for a buffered live stream (seconds since
/// tune-in): the stream can be sought anywhere in `[start, live]`.
#[derive(Debug, Clone, Copy, Default)]
pub struct DvrState {
    /// Current play position.
    pub pos: f64,
    /// Oldest seekable position (buffer tail).
    pub start: f64,
    /// Live edge (newest buffered audio).
    pub live: f64,
}

impl DvrState {
    /// Seconds the play position sits behind the live edge (0 when at/near live).
    pub fn behind_live(&self) -> f64 {
        (self.live - self.pos).max(0.0)
    }
}

impl RadioNow {
    /// Whether a live radio stream is the *current* engine audio source: a station
    /// is tuned in and not paused. When paused (`radio_paused`), the engine has
    /// been handed back to local playback and the station is just a preserved
    /// overlay — so this is `false` and local seek/controls apply.
    pub fn is_live(&self) -> bool {
        self.now_station.is_some() && !self.radio_paused
    }
}

impl AppState {
    /// Jump a timeshifted live stream back to the live edge (newest buffered audio).
    /// No-op unless a DVR-buffered station is loaded (works playing or paused).
    pub(crate) fn radio_go_live(&mut self) {
        if self.rnow.dvr.is_some() {
            self.engine.send(AudioCommand::GoLive);
            if let Some(d) = self.rnow.dvr.as_mut() {
                d.pos = d.live;
            }
        }
    }

    /// Jump a timeshifted live stream to the oldest buffered audio (the start of the
    /// rewind window). No-op unless a DVR-buffered station is loaded.
    pub(crate) fn radio_go_start(&mut self) {
        if let Some(d) = self.rnow.dvr {
            self.engine
                .send(AudioCommand::Seek(std::time::Duration::from_secs_f64(
                    d.start,
                )));
            if let Some(d) = self.rnow.dvr.as_mut() {
                d.pos = d.start;
            }
        }
    }
}

impl AppState {
    // ---- internet radio --------------------------------------------------
    /// Switch to the Radio view and load the default station list on first open.
    pub(crate) fn open_radio(&mut self) {
        self.set_layout(Layout::Radio);
        // land on the station list (not the section sidebar) so j/k moves stations
        // the moment the view opens, matching the pre-sidebar muscle memory.
        self.set_focus(Focus::Main);
        // pull the full directory (cache or download) so everything is local;
        // until it's ready the live API search serves as the fallback.
        self.load_directory(false);
        if self.radio.stations.is_empty() && !self.radio.loading && !self.radio.local_ready {
            let q = self.radio.query.clone();
            self.radio_search(q);
        }
    }

    /// Request the station directory (cached if fresh per the configured refresh
    /// interval, else downloaded). No-op if already loaded or a load is running.
    pub(crate) fn load_directory(&mut self, force: bool) {
        if self.radio.directory_loading || (self.radio.local_ready && !force) {
            return;
        }
        let max_age_secs = self.config.radio_refresh_days as u64 * 24 * 3600;
        if let Some(tx) = &self.workers.radio {
            self.radio.directory_loading = true;
            self.radio.directory_progress = 0;
            let _ = tx.send(crate::radio::RadioRequest::LoadDirectory {
                force,
                max_age_secs,
            });
        }
    }

    /// `R` in the Radio view: force a fresh directory download.
    pub(crate) fn refresh_directory(&mut self) {
        self.notify("Refreshing station directory…".into());
        self.load_directory(true);
    }

    /// The station list the active sidebar section shows: the saved favorites, a
    /// dedicated history/playlist list, or the shared search-results set.
    pub fn radio_view_list(&self) -> &[crate::radio::Station] {
        match self.radio.section {
            RadioSection::Favorites => &self.radio.favorites,
            RadioSection::Recent => &self.radio.recent,
            RadioSection::MostPlayed => &self.radio.most_played,
            // Drilled into a playlist → its stations; on the flat playlist list the
            // main pane renders the names itself (not a station table), so → empty.
            RadioSection::Playlists => match self.radio.pl.open {
                Some(id) => self
                    .radio
                    .playlists
                    .iter()
                    .find(|p| p.id == id)
                    .map_or(&[][..], |p| &p.stations),
                None => &[],
            },
            // Everything else views the shared search-results set (see `shows_results`).
            _ => &self.radio.stations,
        }
    }

    /// Is `st` one of the saved favorites? (matched by uuid, falling back to url.)
    pub fn radio_is_fav(&self, st: &crate::radio::Station) -> bool {
        let id = station_key(st);
        self.radio.favorites.iter().any(|f| station_key(f) == id)
    }

    /// The filtered (label, choice) entries for the open picker. The first entry
    /// clears the filter (`None`); the rest carry `(display name, api value)`,
    /// ranked closest-match-first when a query is typed so the auto-selected row
    /// (index 1) is the best hit (e.g. "china" → China, not a substring match).
    pub fn radio_picker_options(&self) -> Vec<PickRow> {
        let Some(p) = &self.radio.picker else {
            return Vec::new();
        };
        let q = p.query.to_lowercase();
        let clear = match p.kind {
            PickerKind::Country => "✗ All countries",
            PickerKind::Genre => "✗ Any genre",
        };
        let mut out: Vec<PickRow> = vec![(clear.into(), None)];
        // (row, score) — score ranks closeness; lists arrive sorted by
        // stationcount, and the sort below is stable, so popularity breaks ties.
        let mut matches: Vec<(PickRow, u8)> = Vec::new();
        match p.kind {
            PickerKind::Country => {
                for c in &self.radio.all_countries {
                    if let Some(s) = match_score(&c.name.to_lowercase(), &c.code.to_lowercase(), &q)
                    {
                        let label = format!("{} ({})", c.name, c.count);
                        matches.push(((label, Some((c.name.clone(), c.code.clone()))), s));
                    }
                }
            }
            PickerKind::Genre => {
                // scope to the selected country's genres if we have them, else global
                let scoped = self
                    .radio
                    .country
                    .as_ref()
                    .and_then(|(_, code)| self.radio.genres_by_country.get(code));
                let tags = scoped.unwrap_or(&self.radio.all_tags);
                for t in tags {
                    if let Some(s) = match_score(&t.name.to_lowercase(), "", &q) {
                        let label = format!("{} ({})", t.name, t.count);
                        matches.push(((label, Some((t.name.clone(), t.name.clone()))), s));
                    }
                }
            }
        }
        if !q.is_empty() {
            matches.sort_by_key(|(_, s)| *s);
        }
        out.extend(matches.into_iter().map(|(row, _)| row));
        out
    }

    /// Row count the open picker would show (clear row + matches) without
    /// building the allocating label list — for cursor bounds / auto-select.
    pub(crate) fn radio_picker_match_count(&self) -> usize {
        let Some(p) = &self.radio.picker else {
            return 0;
        };
        let q = p.query.to_lowercase();
        let matches = match p.kind {
            PickerKind::Country => self
                .radio
                .all_countries
                .iter()
                .filter(|c| {
                    match_score(&c.name.to_lowercase(), &c.code.to_lowercase(), &q).is_some()
                })
                .count(),
            PickerKind::Genre => {
                let scoped = self
                    .radio
                    .country
                    .as_ref()
                    .and_then(|(_, code)| self.radio.genres_by_country.get(code));
                scoped
                    .unwrap_or(&self.radio.all_tags)
                    .iter()
                    .filter(|t| match_score(&t.name.to_lowercase(), "", &q).is_some())
                    .count()
            }
        };
        matches + 1 // + the leading "clear filter" row
    }

    /// Run a radio search for `query` with the active filters. Once the local
    /// directory is loaded this filters in-memory (instant, no API call); until
    /// then it falls back to a coalesced live API search.
    pub(crate) fn radio_search(&mut self, query: String) {
        self.radio.query = query.clone();
        // Typing implies you want results — leave a dedicated-list section
        // (Favorites/Recent/…) for the results view so the matches are visible.
        if !self.radio.section.shows_results() {
            self.radio.section = RadioSection::AllStations;
        }
        self.radio.sel = 0;
        if self.radio.local_ready {
            self.run_local_search();
            return;
        }
        self.workers.radio_seq += 1;
        let key = format!("r{}", self.workers.radio_seq);
        self.radio.key = key.clone();
        self.radio.loading = true;
        self.radio.note = "Searching…".into();
        if let Some(tx) = &self.workers.radio {
            let _ = tx.send(crate::radio::RadioRequest::Search {
                query,
                country: self.radio.country.as_ref().map(|(_, c)| c.clone()),
                tag: self.radio.tag.clone(),
                sort: self.radio.sort,
                key,
            });
        }
    }

    /// Re-run the search with the current query + filters (after a filter change).
    pub(crate) fn refresh_radio(&mut self) {
        let q = self.radio.query.clone();
        self.radio_search(q);
    }

    /// Enter: apply the open picker, activate the highlighted sidebar section, or
    /// play the selected station (depending on what has focus).
    pub(crate) fn radio_activate(&mut self) {
        if self.radio.picker.is_some() {
            self.radio_apply_picker();
            return;
        }
        if self.radio.pl.modal_open() {
            self.radio_modal_confirm();
            return;
        }
        if self.focus == Focus::Sidebar {
            self.radio_activate_section();
            return;
        }
        // Playlists section, flat list: Enter drills into the highlighted playlist.
        if self.radio.section == RadioSection::Playlists && self.radio.pl.open.is_none() {
            self.radio_playlist_open();
            return;
        }
        let pick = self.radio_view_list().get(self.radio.sel).cloned();
        if let Some(st) = pick {
            self.radio.editing = false; // playing returns focus to the list
            self.play_station(st);
        }
    }

    /// Enter (or click) on a sidebar section: open its picker (Countries/Genres),
    /// focus the query box (Search), apply its sort (Popular/Trending), else just
    /// hand focus to the station list. Shared by the keymap and the mouse handler.
    pub(crate) fn radio_activate_section(&mut self) {
        use crate::radio::Sort;
        match self.radio.section {
            RadioSection::Countries => self.radio_open_picker(PickerKind::Country),
            RadioSection::Genres => self.radio_open_picker(PickerKind::Genre),
            RadioSection::Popular => {
                self.radio.sort = Sort::Popular;
                self.refresh_radio();
                self.set_focus(Focus::Main);
            }
            // No dedicated "trending" API field yet — rank by votes as an interim
            // signal until `clicktrend` lands in the browse-polish phase.
            RadioSection::Trending => {
                self.radio.sort = Sort::Votes;
                self.refresh_radio();
                self.set_focus(Focus::Main);
            }
            _ => self.set_focus(Focus::Main),
        }
    }

    /// Esc: close the picker, leave search edit, or return to All Stations. In
    /// plain browse it does nothing — Esc never drops you back to the local player
    /// (switch views with 1–6 for that), so it can't be hit by accident.
    pub(crate) fn radio_cancel(&mut self) {
        if self.radio.picker.is_some() {
            self.radio.picker = None;
        } else if self.radio.pl.modal_open() {
            self.radio_modal_cancel();
        } else if self.radio.editing {
            self.radio.editing = false;
        } else if self.radio.section == RadioSection::Playlists && self.radio.pl.open.is_some() {
            self.radio_playlist_back();
        } else if self.radio.section != RadioSection::AllStations {
            self.radio.section = RadioSection::AllStations;
            self.radio.sel = 0;
        }
    }

    /// Open the country/genre picker, fetching its source list on first use.
    pub(crate) fn radio_open_picker(&mut self, kind: PickerKind) {
        self.radio.editing = false;
        // the genre picker is scoped to the selected country (if any), so the
        // source list it caches against depends on that country's ISO code
        let country_code = self
            .radio
            .country
            .as_ref()
            .map(|(_, c)| c.clone())
            .filter(|_| kind == PickerKind::Genre);
        // local mode: derive the picker list from the in-memory directory (no API
        // call) — country-scoped genres are computed on demand and cached.
        if self.radio.local_ready {
            if kind == PickerKind::Genre
                && let Some(code) = &country_code
                && !self.radio.genres_by_country.contains_key(code)
            {
                let items = derive_genres(&self.radio.all_stations, Some(code));
                self.radio.genres_by_country.insert(code.clone(), items);
            }
            self.radio.picker = Some(RadioPicker {
                kind,
                ..RadioPicker::default()
            });
            return;
        }
        let cached = match kind {
            PickerKind::Country => !self.radio.all_countries.is_empty(),
            PickerKind::Genre => match &country_code {
                Some(code) => self.radio.genres_by_country.contains_key(code),
                None => !self.radio.all_tags.is_empty(),
            },
        };
        let mut p = RadioPicker {
            kind,
            ..RadioPicker::default()
        };
        if !cached {
            self.workers.radio_seq += 1;
            let key = format!("p{}", self.workers.radio_seq);
            p.key = key.clone();
            p.loading = true;
            if let Some(tx) = &self.workers.radio {
                let req = match (kind, &country_code) {
                    (PickerKind::Country, _) => crate::radio::RadioRequest::Countries { key },
                    (PickerKind::Genre, Some(code)) => crate::radio::RadioRequest::CountryGenres {
                        code: code.clone(),
                        key,
                    },
                    (PickerKind::Genre, None) => crate::radio::RadioRequest::Tags { key },
                };
                let _ = tx.send(req);
            }
        }
        self.radio.picker = Some(p);
    }

    /// Apply the highlighted picker entry as the active filter, then re-search.
    pub(crate) fn radio_apply_picker(&mut self) {
        let opts = self.radio_picker_options();
        let Some(p) = &self.radio.picker else {
            return;
        };
        let kind = p.kind;
        let chosen = opts.get(p.sel.min(opts.len().saturating_sub(1))).cloned();
        self.radio.picker = None;
        let Some((_, choice)) = chosen else {
            return;
        };
        match kind {
            PickerKind::Country => self.radio.country = choice,
            PickerKind::Genre => self.radio.tag = choice.map(|(name, _)| name),
        }
        self.refresh_radio();
    }

    /// 'f': star/unstar the highlighted station and persist the list.
    pub(crate) fn radio_star(&mut self) {
        let Some(st) = self.radio_view_list().get(self.radio.sel).cloned() else {
            return;
        };
        let id = station_key(&st);
        if let Some(pos) = self
            .radio
            .favorites
            .iter()
            .position(|f| station_key(f) == id)
        {
            self.radio.favorites.remove(pos);
            self.notify(format!("Unstarred: {}", st.name));
            if self.radio.section == RadioSection::Favorites {
                let len = self.radio.favorites.len();
                if self.radio.sel >= len {
                    self.radio.sel = len.saturating_sub(1);
                }
            }
        } else {
            self.radio.favorites.push(st.clone());
            self.notify(format!("Starred: {}", st.name));
        }
        crate::library::store::RadioFavorites::save(&self.radio.favorites, &self.config.dir);
    }

    /// n/p: tune the next/previous station in the current list (wrap-around).
    pub(crate) fn radio_station(&mut self, delta: i32) {
        let n = self.radio_view_list().len();
        if n == 0 {
            return;
        }
        let cur = self.radio.sel.min(n - 1) as i32;
        let next = (cur + delta).rem_euclid(n as i32) as usize;
        self.radio.sel = next;
        let st = self.radio_view_list()[next].clone();
        self.play_station(st);
    }

    /// 'o': cycle the result sort order and re-search (only the results sections
    /// are sorted; favorites / history keep their own order).
    pub(crate) fn radio_cycle_sort(&mut self) {
        self.radio.sort = self.radio.sort.next();
        self.notify(format!("Sort: {}", self.radio.sort.label()));
        if self.radio.section.shows_results() {
            self.refresh_radio();
        }
    }

    /// Whether the now-playing bar should show the radio station (only in the
    /// Radio view while a station is tuned). Local views keep showing `player`.
    pub fn showing_radio(&self) -> bool {
        self.layout == Layout::Radio && self.rnow.now_station.is_some()
    }

    pub(crate) fn play_station(&mut self, st: crate::radio::Station) {
        let (url, name) = (st.url.clone(), st.name.clone());
        self.record_station_play(&st); // history: bump count + last-played, persist
        self.rnow.now_station = Some(st);
        self.rnow.now_station_title = None; // ICY title arrives once the stream is up
        self.rnow.radio_paused = false;
        self.rnow.dvr = None; // fresh tune → the engine re-reports the timeshift window
        // radio is an OVERLAY: the local player (current track, queue, position,
        // album art) is preserved and merely paused, so switching to a local
        // view still shows your music. The engine streams the radio meanwhile.
        if self.player.status == Status::Playing {
            self.player.status = Status::Paused;
        }
        self.engine.send(AudioCommand::SetSpeed(1.0)); // never time-stretch a live stream
        // Timeshift (DVR) window from config: a live stream buffered this far back
        // can be paused, rewound, and caught up to live. `None` → forward-only.
        let dvr = (self.config.radio_dvr && self.config.radio_dvr_minutes > 0)
            .then(|| std::time::Duration::from_secs(self.config.radio_dvr_minutes as u64 * 60));
        self.engine.send(AudioCommand::LoadStream { url, dvr });
        self.engine.send(AudioCommand::Play);
        self.notify(format!("Tuning in: {name}"));
    }

    /// Record a tune-in: bump the station's play count + last-played in the history
    /// (move-to-front, deduped by [`station_key`]), rebuild the Recent / Most Played
    /// views, and persist to `radio_history.json`. Called from every play path so
    /// n/p channel-hopping and Enter both count.
    pub(crate) fn record_station_play(&mut self, st: &crate::radio::Station) {
        let now = crate::datetime::now_unix();
        let key = station_key(st).to_string();
        if let Some(e) = self
            .radio
            .history
            .iter_mut()
            .find(|e| station_key(&e.station) == key)
        {
            e.play_count = e.play_count.saturating_add(1);
            e.last_played = now;
            e.station = st.clone(); // refresh metadata (name/bitrate may have changed)
        } else {
            self.radio.history.push(crate::radio::HistoryEntry {
                station: st.clone(),
                play_count: 1,
                last_played: now,
            });
        }
        // bound: keep the most-recently-played CAP entries
        let cap = crate::library::store::RadioHistory::CAP;
        if self.radio.history.len() > cap {
            self.radio
                .history
                .sort_by_key(|e| std::cmp::Reverse(e.last_played));
            self.radio.history.truncate(cap);
        }
        self.radio.rebuild_history_views();
        crate::library::store::RadioHistory::save(&self.radio.history, &self.config.dir);
    }

    /// Apply a radio worker result (search results, picker lists, errors),
    /// ignoring stale ones tagged with a superseded key.
    pub fn on_radio_result(&mut self, res: crate::radio::RadioResult) {
        use crate::radio::RadioResult::*;
        match res {
            Found { key, stations } => {
                if self.radio.key != key {
                    return; // a newer search superseded this one
                }
                self.radio.loading = false;
                self.radio.note = if stations.is_empty() {
                    "No stations".into()
                } else {
                    String::new()
                };
                self.radio.stations = stations;
                // land the cursor on the now-playing / last-tuned station if it's
                // in the results (e.g. after a session restore), else the top
                self.radio.sel = self
                    .rnow
                    .now_station
                    .as_ref()
                    .and_then(|cur| {
                        let id = station_key(cur);
                        self.radio
                            .stations
                            .iter()
                            .position(|s| station_key(s) == id)
                    })
                    .unwrap_or(0);
            }
            Countries { items, .. } => {
                self.radio.all_countries = items;
                if let Some(p) = &mut self.radio.picker
                    && p.kind == PickerKind::Country
                {
                    p.loading = false;
                }
            }
            Tags { items, .. } => {
                self.radio.all_tags = items;
                if let Some(p) = &mut self.radio.picker
                    && p.kind == PickerKind::Genre
                {
                    p.loading = false;
                }
            }
            CountryGenres { code, items, .. } => {
                self.radio.genres_by_country.insert(code, items);
                if let Some(p) = &mut self.radio.picker
                    && p.kind == PickerKind::Genre
                {
                    p.loading = false;
                }
            }
            DirectoryProgress { read } => {
                self.radio.directory_progress = read;
            }
            Directory {
                stations,
                from_cache,
            } => {
                self.ingest_directory(stations, from_cache);
            }
            Error { key, msg } => {
                if key.is_empty() {
                    // a directory load that failed with no cache to fall back on
                    self.radio.directory_loading = false;
                    self.radio.directory_progress = 0;
                    self.radio.note = format!("Directory: {msg}");
                } else if self.radio.key == key {
                    self.radio.loading = false;
                    self.radio.note = format!("Error: {msg}");
                } else if let Some(p) = &mut self.radio.picker
                    && p.key == key
                {
                    p.loading = false;
                }
            }
        }
    }

    pub fn set_radio_sender(&mut self, tx: crossbeam_channel::Sender<crate::radio::RadioRequest>) {
        self.workers.radio = Some(tx);
        // if a session restored us straight into the Radio view, load the
        // directory (cache/download) and populate the list without a keypress.
        if self.layout == Layout::Radio {
            self.load_directory(false);
            if self.radio.stations.is_empty() && !self.radio.loading {
                let q = self.radio.query.clone();
                self.radio_search(q);
            }
        }
    }

    /// A radio search / picker / directory fetch is in flight — poll faster so
    /// its result (and the download progress bar) updates within a frame instead
    /// of on the slow idle heartbeat.
    pub(crate) fn radio_busy(&self) -> bool {
        self.radio.loading
            || self.radio.directory_loading
            || self.radio.picker.as_ref().is_some_and(|p| p.loading)
    }

    /// Adopt a freshly loaded station directory: build the lowercase search
    /// index, derive the country + genre picker lists, switch to local mode, and
    /// re-run the current query locally.
    pub(crate) fn ingest_directory(
        &mut self,
        mut stations: Vec<crate::radio::Station>,
        from_cache: bool,
    ) {
        // keep the set ordered by popularity so the default sort is order-free
        stations.sort_by_key(|b| std::cmp::Reverse(b.clickcount));
        self.radio.name_lc = stations.iter().map(|s| s.name.to_lowercase()).collect();
        self.radio.tags_lc = stations.iter().map(|s| s.tags.to_lowercase()).collect();
        self.radio.all_countries = derive_countries(&stations);
        self.radio.all_tags = derive_genres(&stations, None);
        self.radio.genres_by_country.clear(); // re-derive per country on demand
        self.radio.all_stations = stations;
        self.radio.local_ready = true;
        self.radio.directory_loading = false;
        self.radio.directory_progress = 0;
        let n = self.radio.all_stations.len();
        if !from_cache {
            self.notify(format!("Station directory updated — {n} stations"));
        }
        // refresh the view with local results now that the data is here
        let q = self.radio.query.clone();
        self.run_local_search_with(q);
    }
}

/// The Radio view's sidebar sections — the radio analogue of [`LocalSection`].
/// A flat list navigated in the sidebar: some are station lists (All Stations,
/// Favorites, …); Countries/Genres drill into a filter picker. Searching is the
/// always-present query box atop the main pane, not a section. The active section
/// drives [`AppState::radio_view_list`]. Icons are monochrome glyphs (theme-tinted
/// by `section_list`), matching the local library sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum RadioSection {
    #[default]
    AllStations,
    Favorites,
    Playlists,
    Recent,
    MostPlayed,
    Countries,
    Genres,
    Popular,
    Trending,
}

impl RadioSection {
    /// All sections in sidebar order.
    pub const ALL: [RadioSection; 9] = [
        RadioSection::AllStations,
        RadioSection::Favorites,
        RadioSection::Playlists,
        RadioSection::Recent,
        RadioSection::MostPlayed,
        RadioSection::Countries,
        RadioSection::Genres,
        RadioSection::Popular,
        RadioSection::Trending,
    ];
    pub fn label(self) -> &'static str {
        match self {
            RadioSection::AllStations => "All Stations",
            RadioSection::Favorites => "Favorites",
            RadioSection::Playlists => "Playlists",
            RadioSection::Recent => "Recent",
            RadioSection::MostPlayed => "Most Played",
            RadioSection::Countries => "Countries",
            RadioSection::Genres => "Genres",
            RadioSection::Popular => "Popular",
            RadioSection::Trending => "Trending",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            RadioSection::AllStations => "♫",
            RadioSection::Favorites => "♥",
            RadioSection::Playlists => "≡",
            RadioSection::Recent => "↻",
            RadioSection::MostPlayed => "▲",
            RadioSection::Countries => "⊕",
            RadioSection::Genres => "⊞",
            RadioSection::Popular => "★",
            RadioSection::Trending => "↗",
        }
    }
    /// Stable key for session persistence (independent of label/order).
    pub fn key(self) -> &'static str {
        match self {
            RadioSection::AllStations => "all",
            RadioSection::Favorites => "favorites",
            RadioSection::Playlists => "playlists",
            RadioSection::Recent => "recent",
            RadioSection::MostPlayed => "most_played",
            RadioSection::Countries => "countries",
            RadioSection::Genres => "genres",
            RadioSection::Popular => "popular",
            RadioSection::Trending => "trending",
        }
    }
    pub fn from_key(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|sec| sec.key() == s)
    }
    /// Whether this section's main list is the shared search-results set
    /// (`radio.stations`) rather than a dedicated list (favorites / history /
    /// playlists) — so a live search never has to clear the section to show hits.
    pub fn shows_results(self) -> bool {
        matches!(
            self,
            RadioSection::AllStations
                | RadioSection::Countries
                | RadioSection::Genres
                | RadioSection::Popular
                | RadioSection::Trending
        )
    }
}

/// Internet-radio view state: the active sidebar section, the live search query,
/// the fetched stations, the active filters (country / genre / sort), the saved
/// favorites, and any open filter picker. `key` tags the in-flight search so
/// stale results are ignored.
#[derive(Default)]
pub struct Radio {
    /// The selected sidebar section (drives which list `radio_view_list` returns).
    pub section: RadioSection,
    pub query: String,
    pub sel: usize,
    /// Persisted sticky scroll offset of the station list — so clicking a visible
    /// row selects it in place instead of recentring under the cursor. Set during
    /// render.
    pub list_off: std::cell::Cell<usize>,
    pub stations: Vec<crate::radio::Station>,
    pub loading: bool,
    /// Status line under the search box ("", "Searching…", "No stations", error).
    pub note: String,
    key: String,
    /// `true` while the search box has focus (typing edits the query).
    pub editing: bool,
    /// Active country filter as (display name, ISO code); `None` = all countries.
    pub country: Option<(String, String)>,
    /// Active genre/tag filter; `None` = any genre.
    pub tag: Option<String>,
    /// Result ordering.
    pub sort: crate::radio::Sort,
    /// Starred stations (persisted to `radio_favorites.json`).
    pub favorites: Vec<crate::radio::Station>,
    /// Listening history (persisted to `radio_history.json`) — the source of truth
    /// for the Recent + Most Played sections; `recent`/`most_played` are derived
    /// station lists rebuilt from it (see [`Radio::rebuild_history_views`]).
    pub history: Vec<crate::radio::HistoryEntry>,
    /// Derived: stations newest-played first (the Recent section).
    pub recent: Vec<crate::radio::Station>,
    /// Derived: stations by descending play count (the Most Played section).
    pub most_played: Vec<crate::radio::Station>,
    /// User-created named station collections (persisted to `radio_playlists.json`).
    pub playlists: Vec<crate::radio::Playlist>,
    /// Playlists-section drill + modal state (which list is open, name entry, the
    /// add-to-playlist picker, delete confirm).
    pub pl: RadioPlaylistUi,
    /// Open country/genre picker, if any.
    pub picker: Option<RadioPicker>,
    /// Cached picker source lists (fetched once, reused on reopen).
    pub all_countries: Vec<crate::radio::Country>,
    pub all_tags: Vec<crate::radio::TagItem>,
    /// Genres present per country (ISO code → tags), so the genre picker can be
    /// scoped to the selected country. Derived from that country's stations.
    pub genres_by_country: std::collections::HashMap<String, Vec<crate::radio::TagItem>>,

    /// The full station directory, downloaded once and cached, so all searching /
    /// filtering / sorting happens locally with no per-keystroke API calls.
    pub all_stations: Vec<crate::radio::Station>,
    /// Lowercased name / tags parallel to `all_stations` (built once) so local
    /// filtering never allocates per row.
    pub(crate) name_lc: Vec<String>,
    pub(crate) tags_lc: Vec<String>,
    /// The local directory is loaded and in use (else we use live API search).
    pub local_ready: bool,
    /// A directory load/download is in flight.
    pub directory_loading: bool,
    /// Bytes downloaded so far for the in-progress directory fetch (0 = none).
    pub directory_progress: u64,
}

/// What a radio playlist name-entry is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioNameTarget {
    /// Create a new playlist.
    New,
    /// Rename the playlist with this id.
    Rename(u32),
}

/// Playlists-section UI: the drill-in cursor plus the transient modal states (name
/// entry, the "add station to a playlist" picker, and the delete confirm). All
/// `None`/empty means the flat playlist list is showing.
#[derive(Default)]
pub struct RadioPlaylistUi {
    /// The playlist drilled into (its stations show in the main pane); `None` = the
    /// flat list of playlists is showing.
    pub open: Option<u32>,
    /// Cursor within the flat playlist list.
    pub sel: usize,
    /// Sticky scroll offset of the flat playlist list (set during render).
    pub list_off: std::cell::Cell<usize>,
    /// Active name-entry (create / rename) + its live buffer; `None` = not naming.
    pub naming: Option<RadioNameTarget>,
    pub buffer: String,
    /// A station being added to a playlist: the "pick a playlist" overlay is open.
    pub adding: Option<crate::radio::Station>,
    /// Cursor in the add-to-playlist picker (an extra trailing row = "New playlist").
    pub add_sel: usize,
    /// A playlist id pending the two-step delete confirm.
    pub confirm_delete: Option<u32>,
}

impl RadioPlaylistUi {
    /// Whether any radio-playlist modal (name entry, add picker, delete confirm) is
    /// open — used to gate global keys and route input to the modal.
    pub fn modal_open(&self) -> bool {
        self.naming.is_some() || self.adding.is_some() || self.confirm_delete.is_some()
    }
}

impl Radio {
    /// Rebuild the derived `recent` (newest-played first) and `most_played` (by
    /// descending play count) station lists from `history`. Cheap — the history is
    /// bounded — so it runs on load and after every tune-in rather than per frame.
    pub fn rebuild_history_views(&mut self) {
        let mut recent: Vec<&crate::radio::HistoryEntry> = self.history.iter().collect();
        recent.sort_by_key(|e| std::cmp::Reverse(e.last_played));
        self.recent = recent.into_iter().map(|e| e.station.clone()).collect();

        let mut most: Vec<&crate::radio::HistoryEntry> =
            self.history.iter().filter(|e| e.play_count > 0).collect();
        most.sort_by_key(|e| {
            (
                std::cmp::Reverse(e.play_count),
                std::cmp::Reverse(e.last_played),
            )
        });
        self.most_played = most.into_iter().map(|e| e.station.clone()).collect();
    }
}

/// A modal filter picker overlaying the Radio view (country or genre): a list
/// you navigate by default, with a filter box focused on demand (`/`).
#[derive(Default)]
pub struct RadioPicker {
    pub kind: PickerKind,
    pub query: String,
    pub sel: usize,
    /// Persisted sticky scroll offset of the picker list (fresh 0 on each open —
    /// the picker is recreated). Set during render.
    pub off: std::cell::Cell<usize>,
    pub loading: bool,
    /// `true` while the filter box is focused (typing edits the query); `false`
    /// = list navigation (j/k/arrows move).
    pub editing: bool,
    key: String,
}

/// Which filter list a `RadioPicker` is showing.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    #[default]
    Country,
    Genre,
}

/// Tally distinct countries (by ISO code) across the local directory, most
/// stations first — drives the country picker without any API call.
fn derive_countries(stations: &[crate::radio::Station]) -> Vec<crate::radio::Country> {
    use std::collections::HashMap;
    let mut map: HashMap<String, (String, u32)> = HashMap::new();
    for s in stations {
        if s.countrycode.is_empty() {
            continue;
        }
        let e = map
            .entry(s.countrycode.clone())
            .or_insert_with(|| (s.country.clone(), 0));
        if e.0.is_empty() && !s.country.is_empty() {
            e.0 = s.country.clone();
        }
        e.1 += 1;
    }
    let mut out: Vec<crate::radio::Country> = map
        .into_iter()
        .map(|(code, (name, count))| crate::radio::Country {
            name: if name.is_empty() { code.clone() } else { name },
            code,
            count,
        })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    out
}

/// Tally genre tags across the local directory (optionally only stations whose
/// ISO code is `country`), most-common first and capped — drives the genre
/// picker (and its country-scoped variant) without any API call.
fn derive_genres(
    stations: &[crate::radio::Station],
    country: Option<&str>,
) -> Vec<crate::radio::TagItem> {
    use std::collections::HashMap;
    let mut map: HashMap<String, u32> = HashMap::new();
    for s in stations {
        if let Some(c) = country
            && !s.countrycode.eq_ignore_ascii_case(c)
        {
            continue;
        }
        for t in s.tags.split(',') {
            let t = t.trim();
            if !t.is_empty() {
                *map.entry(t.to_string()).or_insert(0) += 1;
            }
        }
    }
    let mut out: Vec<crate::radio::TagItem> = map
        .into_iter()
        .map(|(name, count)| crate::radio::TagItem { name, count })
        .collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    out.truncate(500);
    out
}
