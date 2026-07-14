//! `AppState` construction: the `new` constructor that wires config, library,
//! workers, and the initial view/focus into a ready-to-run state. Split out of
//! app/mod.rs so the core module holds the struct + accessors, not the long
//! initialization body.

use super::*;

impl AppState {
    pub fn new(config: Config) -> Self {
        config.seed_bundled_themes(); // drop the shipped custom themes into themes/
        let theme = Theme::resolve(&config.theme, &config.themes_dir());
        let icons = crate::icons::Icons::resolve(&config.icon_set, &config.icons);
        let bookmarks = crate::library::store::BookmarkStore::load(&config.dir).bookmarks;
        let play_history = crate::library::store::HistoryStore::load(&config.dir);
        let radio_favorites = crate::library::store::RadioFavorites::load(&config.dir);
        let radio_history = crate::library::store::RadioHistory::load(&config.dir);
        let spotify_tokens = crate::spotify::Tokens::load(&config.dir);
        let eq_presets = config.load_eq_presets(); // custom EQ presets (before `config` moves)
        let sort = parse_sort(&config.sort_order);
        // surface a config.toml parse error as a long-lived, copyable status-bar
        // toast (same lifetime as notify_error), without re-borrowing `config`.
        let config_err = config.config_error.clone();
        let err_toast = config_err.clone().map(|text| Notification {
            text,
            ttl_ticks: (config.fps as u16).saturating_mul(25).max(180),
        });
        Self {
            running: true,
            dirty: true,
            view: View::Dashboard,
            layout: Layout::Dashboard,
            focus: Focus::Main,
            library_view: LibraryView::Tracks,
            player: PlayerState::default(),
            library: Library::default(),
            theme,
            auto_theme: None,
            applied_sys_theme: None,
            appearance_at: std::time::Instant::now(),
            icons,
            config,
            selection: 0,
            queue_sel: 0,
            input: ModalInput::default(),
            browser: Browser::default(),
            local: LocalBrowse::default(),
            bookmarks,
            sort,
            scan: ScanState::default(),
            rng: 0,
            fx: PlaybackFx::default(),
            settings: SettingsUi::default(),
            eq: EqUi::default(),
            eq_presets,
            in_tmux: false,
            tags: TagModal::default(),
            radio: {
                let mut r = Radio::default();
                r.favorites = radio_favorites;
                r.history = radio_history;
                r.rebuild_history_views(); // derive Recent / Most Played on launch
                r
            },
            rnow: radio::RadioNow {
                now_station: None,
                now_station_title: None,
                radio_paused: false,
                dvr: None,
            },
            spotify: {
                let mut s = Spotify::default();
                s.tokens = spotify_tokens;
                s
            },
            spov: spotify::SpOverlay {
                now_spotify: None,
                spotify_paused: false,
                sp_pos: 0.0,
                sp_dur: 0.0,
                sp_queue: Vec::new(),
                sp_idx: 0,
                sp_started: false,
                sp_fail_streak: 0,
                sp_recovery: spotify::SpRecovery::Normal,
                sp_cooldown_until: 0,
                sp_played_ok: false,
                sp_key_denials: 0,
                sp_resume_at: None,
                sp_keyretry_at: None,
                sp_keyretry_n: 0,
                sp_stall_at: None,
                sp_stall_n: 0,
                sp_stall_pos: 0.0,
                sp_skip_target: None,
                sp_skip_at: None,
                sp_seek_at: None,
                sp_seek_target: None,
                sp_seek_streak: 0,
                sp_seek_streak_at: None,
                sp_cover: None,
                sp_cover_url: None,
                sp_saved: false,
                sp_follow_pending: None,
                sp_shuffle: false,
                sp_repeat: Repeat::Off,
                sp_artist: None,
                sp_artist_uri: None,
                sp_artist_cover: None,
                sp_artist_cover_url: None,
                sp_artist_top: Vec::new(),
                sp_show_meta: None,
                session_cmd: None,
                session_rx: None,
                sp_stream: false,
                sp_bridge: None,
            },
            views: ViewState::default(),
            workers: Workers::default(),
            hit: std::cell::RefCell::new(Vec::new()),
            overlay_hits: std::cell::Cell::new(0),
            resize_edges: std::cell::RefCell::new(Vec::new()),
            resize_drag: None,
            search: SearchState::default(),
            scroll: ScrollOff::default(),
            grid_scroll: GridScrollAccum::default(),
            playlist_name: "Synthwave Drive".into(),
            plays_since_flush: 0,
            play_history,
            info: None,
            palette: None,
            marks: MultiSelect::default(),
            viz: Visualizer::new(VIZ_BANDS),
            art: CoverArt::default(),
            grid_art: std::cell::RefCell::new(std::collections::HashMap::new()),
            grid_art_clock: std::cell::Cell::new(0),
            meta: TrackMeta::default(),
            error_log: config_err
                .iter()
                .map(|m| LogEntry {
                    ts: crate::datetime::now_unix(),
                    msg: m.clone(),
                })
                .collect(),
            notification: err_toast,
            last_error: config_err,
            tick: 0,
            engine: Box::new(NullEngine),
            engine_active: false,
            loaded_track: None,
            last_audio_progress: 0,
            frame_dt: std::time::Duration::from_secs_f64(1.0 / 60.0),
            media_cover: None,
            media_cover_slot: false,
        }
    }
}
