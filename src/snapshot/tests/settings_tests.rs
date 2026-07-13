//! Settings snapshot/behaviour tests, split out of snapshot.rs.

use super::*;

#[test]
fn playback_visualizer_mode_persists_to_config_on_cycle() {
    use crate::action::Action;
    // a unique throwaway dir (never the real ~/.config/lyrfin)
    let dir = std::env::temp_dir().join("lyrfin-playback-viz-persist");
    let _ = std::fs::remove_dir_all(&dir);
    let mut a = crate::app::AppState::new(Config {
        dir: dir.clone(),
        ..Default::default()
    });
    a.seed_demo();
    // Dashboard has no big visualizer, so CycleVisualizer cycles the *playback-bar*
    // mode (`player_viz_mode`, config-backed) — the one that was resetting to Bars.
    a.layout = Layout::Dashboard;
    let before = a.config.player_viz_mode;
    a.update(Action::CycleVisualizer);
    let after = a.config.player_viz_mode;
    assert_ne!(before, after, "the playback viz mode advanced on cycle");

    // it must hit config.toml on the spot (config.save runs on cycle), so it
    // survives a restart — and, with the test-isolation fix, this write lands in
    // the temp dir, never the user's real config.
    let text = std::fs::read_to_string(dir.join("config.toml")).expect("cycling wrote config.toml");
    assert!(
        text.contains(&format!("player_viz_mode = {after}")),
        "the cycled playback viz mode is persisted to config.toml"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn spotify_logout_or_reset_lives_in_settings() {
    use crate::action::Action;
    use crate::app::Setting;
    use crate::spotify::{ConnState, Tokens};
    let mut a = demo();
    a.layout = Layout::Spotify;
    // disconnected with no cached token: nothing to log out of / reset
    a.update(Action::OpenViewSettings);
    assert!(
        !a.popup_all_settings().contains(&Setting::SpotifyLogout),
        "no row without a cached token"
    );
    a.update(Action::OpenViewSettings); // close popup
    // disconnected but a token is cached: offer a reset (replaces old `L`)
    a.spotify.tokens = Some(Tokens {
        access_token: "a".into(),
        refresh_token: "r".into(),
        expires_at: 0,
        scopes: String::new(),
    });
    a.update(Action::OpenViewSettings);
    assert!(
        a.popup_all_settings().contains(&Setting::SpotifyLogout),
        "reset row present when a token is cached"
    );
    a.update(Action::OpenViewSettings); // close popup
    // connected: offer log out
    a.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    a.update(Action::OpenViewSettings);
    assert!(
        a.popup_all_settings().contains(&Setting::SpotifyLogout),
        "logout row present once connected"
    );
}

#[test]
fn spotify_popup_excludes_unrelated_general_settings() {
    use crate::action::Action;
    use crate::app::Setting;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.update(Action::OpenViewSettings);
    let items = a.popup_all_settings();
    // General-group rows (Radio refresh, mouse, …) are not Spotify settings
    assert!(
        !items.contains(&Setting::RadioRefresh),
        "Radio refresh dropped from the Spotify popup"
    );
    assert!(
        !items.contains(&Setting::Mouse),
        "General rows dropped from the Spotify popup"
    );
    // the relevant chrome stays: movable panels + the playback visualizer
    assert!(
        items.contains(&Setting::PlayerViz),
        "playback visualizer stays"
    );
    assert!(
        items.iter().any(|s| matches!(s, Setting::PanelShow(_))),
        "panel layout rows stay"
    );
}

#[test]
fn spotify_logout_confirms_before_acting() {
    use crate::action::Action;
    use crate::spotify::ConnState;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    a.update(Action::OpenViewSettings);
    // switch to the Spotify tab, then select the logout row
    let sp = a
        .popup_tab_names()
        .iter()
        .position(|t| *t == "Spotify")
        .unwrap();
    a.set_overlay_tab(sp);
    a.settings.sel = a
        .settings_group_items()
        .iter()
        .position(|s| *s == crate::app::Setting::SpotifyLogout)
        .unwrap();
    // first Enter only arms the confirmation — it does not log out
    a.update(Action::Activate);
    assert!(a.settings.confirm_logout, "Enter arms the confirm prompt");
    assert!(matches!(a.spotify.conn, ConnState::Connected { .. }));
    // Esc cancels: still connected, popup stays open
    a.update(Action::Back);
    assert!(!a.settings.confirm_logout, "Esc cancels the prompt");
    assert!(matches!(a.spotify.conn, ConnState::Connected { .. }));
    assert!(a.settings.popup.is_some(), "popup stays open after cancel");
    // arm again, then confirm: logs out + closes the popup
    a.update(Action::Activate);
    a.update(Action::Activate);
    assert!(!a.settings.confirm_logout);
    assert!(matches!(a.spotify.conn, ConnState::Disconnected));
    assert!(a.settings.popup.is_none(), "popup closes after logout");
}

#[test]
fn dashboard_popup_library_tab_has_add_directory() {
    // The first-run onboarding tells users "; → Library → ＋ Add folder", so the
    // Dashboard's `;` popup must carry a Library tab offering Add-directory.
    use crate::action::Action;
    let mut a = demo();
    a.layout = Layout::Dashboard;
    a.update(Action::OpenViewSettings);
    let li = a
        .popup_tab_names()
        .iter()
        .position(|t| *t == "Library")
        .expect("Dashboard popup has a Library tab");
    a.set_overlay_tab(li);
    assert!(
        a.settings_group_items()
            .contains(&crate::app::Setting::AddDir),
        "the Library tab offers ＋ Add music directory"
    );
}

#[test]
fn view_popup_is_tabbed_panes_content_playback() {
    use crate::action::Action;
    let mut a = demo();
    a.layout = Layout::Dashboard;
    a.update(Action::OpenViewSettings);
    // a roomy overlay so every tab's rows are visible without scrolling (the
    // compact default would scroll the longer tabs — that's by design)
    a.config.overlay_size = crate::config::OVERLAY_SIZE_COUNT - 1;
    assert_eq!(
        a.popup_tab_names(),
        vec![
            "Panes",
            "Grid",
            "Tracklist",
            "Audio",
            "Visualizer",
            "Library"
        ],
        "Dashboard popup groups into Panes / Grid / Tracklist / Audio / Visualizer / Library"
    );
    // Panes tab shows panel rows
    let panes = render_layout(&mut a, Layout::Dashboard, 80, 26);
    assert!(panes.contains("Panes") && panes.contains("Show Sidebar"));
    // Grid tab = grid display options, NOT the tracklist columns
    a.update(Action::OverlayTab(1));
    let grid = render_layout(&mut a, Layout::Dashboard, 80, 26);
    assert!(
        grid.contains("Grid card shape"),
        "Grid tab has the grid options:\n{grid}"
    );
    assert!(
        !grid.contains("Track number"),
        "tracklist columns are NOT on the Grid tab"
    );
    // Tracklist tab = the track layout + column toggles, separated from the grid
    a.update(Action::OverlayTab(1));
    let cols = render_layout(&mut a, Layout::Dashboard, 80, 26);
    assert!(
        cols.contains("Track number") && cols.contains("Comment"),
        "Tracklist tab has the tracklist column toggles:\n{cols}"
    );
    assert!(
        !cols.contains("Grid card shape"),
        "grid options are NOT on the Tracklist tab"
    );
}

#[test]
fn spotify_tracklist_tab_only_offers_supported_columns() {
    // Spotify carries only #/Artist/Album/Year/Time metadata, so its `;` popup's
    // Tracklist tab must offer the shared layout toggle + that subset only — never
    // the columns it can't populate (Genre/Composer/Bitrate/…).
    use crate::action::Action;
    use crate::app::Setting;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.update(Action::OpenViewSettings);
    let ci = a
        .popup_tab_names()
        .iter()
        .position(|t| *t == "Tracklist")
        .expect("the Spotify popup has a Tracklist tab");
    a.set_overlay_tab(ci);
    let items = a.settings_group_items();
    // the shared rows/columns layout toggle + the supported column subset
    assert!(
        items.contains(&Setting::TrackColumns),
        "the rows/columns layout toggle leads the Tracklist tab"
    );
    for s in [
        Setting::ColIndex,
        Setting::ColArtist,
        Setting::ColAlbum,
        Setting::ColYear,
        Setting::ColTime,
    ] {
        assert!(
            items.contains(&s),
            "supported column {s:?} present on Spotify"
        );
    }
    // the unsupported ones are hidden (offering them would mislead — no data)
    for s in [
        Setting::ColGenre,
        Setting::ColComposer,
        Setting::ColBitrate,
        Setting::ColFormat,
        Setting::ColPlays,
        Setting::ColComment,
        Setting::ColRating,
        Setting::ColAlbumArtist,
    ] {
        assert!(
            !items.contains(&s),
            "unsupported column {s:?} hidden on Spotify"
        );
    }
}

#[test]
fn grid_view_row_toggles_the_current_section_grid() {
    // the "View" (grid/list) row on the Grid tab is the settings twin of `#`:
    // present only on a grid-capable section, and activating it flips the grid.
    use crate::action::Action;
    use crate::app::Setting;
    use crate::spotify::api::Section;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.section = Section::Playlists;
    a.spotify_load_section(); // Playlists default to a grid
    assert!(a.spotify.grid, "Playlists open as a grid");

    a.update(Action::OpenViewSettings);
    let grid_tab = a
        .popup_tab_names()
        .iter()
        .position(|t| *t == "Grid")
        .expect("the Spotify popup has a Grid tab");
    a.set_overlay_tab(grid_tab);
    let items = a.settings_group_items();
    assert!(
        items.contains(&Setting::GridList),
        "the grid-capable Playlists section shows the View (grid/list) row"
    );
    assert!(
        items.contains(&Setting::GridShape) && items.contains(&Setting::GridSize),
        "the Grid tab also carries shape + size"
    );

    // activating the View row flips the grid off (→ list), same as `#`
    a.settings.sel = items.iter().position(|s| *s == Setting::GridList).unwrap();
    a.update(Action::Activate);
    assert!(!a.spotify.grid, "the View row toggled grid → list");

    // on a non-grid section the row is absent (shape/size stay — they're global)
    a.spotify.section = Section::LikedSongs;
    a.spotify_load_section();
    let names = a.popup_tab_names();
    assert!(names.contains(&"Grid"), "Grid tab still present");
    let gi = names.iter().position(|t| *t == "Grid").unwrap();
    a.set_overlay_tab(gi);
    let items = a.settings_group_items();
    assert!(
        !items.contains(&Setting::GridList),
        "no View row on a non-grid section (Liked Songs)"
    );
    assert!(
        items.contains(&Setting::GridShape),
        "grid shape/size are global, so they remain"
    );
}

#[test]
fn dashboard_tracklist_tab_offers_every_column() {
    // contrast with Spotify: the local library has all metadata, so its Tracklist
    // tab offers the full set (including the ones Spotify hides).
    use crate::action::Action;
    use crate::app::Setting;
    let mut a = demo();
    a.layout = Layout::Dashboard;
    a.update(Action::OpenViewSettings);
    let ci = a
        .popup_tab_names()
        .iter()
        .position(|t| *t == "Tracklist")
        .expect("the Dashboard popup has a Tracklist tab");
    a.set_overlay_tab(ci);
    let items = a.settings_group_items();
    assert!(
        items.contains(&Setting::TrackColumns),
        "layout toggle present"
    );
    for s in [
        Setting::ColGenre,
        Setting::ColComposer,
        Setting::ColBitrate,
        Setting::ColComment,
    ] {
        assert!(
            items.contains(&s),
            "the local library offers every column, incl. {s:?}"
        );
    }
}

#[test]
fn concert_settings_popup_closes_on_esc() {
    use crate::action::Action;
    let mut app = demo();
    app.layout = Layout::Concert;
    app.update(Action::OpenViewSettings);
    assert!(app.settings.popup.is_some());
    app.update(Action::Back);
    assert!(app.settings.popup.is_none(), "esc closes the concert popup");
}

#[test]
fn music_dir_removed_with_del_not_enter() {
    use crate::action::Action;
    use crate::app::Setting;
    let mut app = demo();
    app.config.music_dirs = vec![std::path::PathBuf::from("/tmp/music")];
    app.open_settings_group("Library"); // the group that hosts music dirs
    let items = app.settings_group_items();
    let idx = items
        .iter()
        .position(|s| matches!(s, Setting::MusicDir(_)))
        .unwrap();
    app.settings.sel = idx;
    app.update(Action::Activate); // Enter must NOT delete
    assert_eq!(app.config.music_dirs.len(), 1, "Enter doesn't delete");
    app.update(Action::SettingsRemove); // Del / ^d deletes
    assert_eq!(app.config.music_dirs.len(), 0, "del removes the dir");
}

#[test]
fn keybinding_rebind_via_settings() {
    use crate::action::Action;
    use crate::app::Setting;
    use crate::event::{Key, KeyCode, Mods};
    let mut app = demo();
    let idx = crate::keymap::configurable_actions()
        .iter()
        .position(|a| *a == "next")
        .unwrap();
    app.open_settings_group("Keys");
    let row = app
        .settings_group_items()
        .iter()
        .position(|s| matches!(s, Setting::Keybind(i) if *i == idx))
        .unwrap();
    app.settings.sel = row;
    app.update(Action::Activate); // start capturing
    assert_eq!(app.settings.rebinding.as_deref(), Some("next"));
    let press = |app: &AppState, c: char| {
        crate::keymap::map(
            app,
            Key {
                code: KeyCode::Char(c),
                mods: Mods::default(),
            },
        )
    };
    let captured = press(&app, 'z');
    app.update(captured);
    assert!(app.settings.rebinding.is_none(), "rebind completes");
    // close Settings so global keys resolve again — an open overlay deliberately
    // swallows them (see the overlay-gate test). Esc closes the tabbed overlay.
    app.update(Action::Back);
    assert!(!app.modal_overlay_open(), "Settings closed");
    assert!(matches!(press(&app, 'z'), Action::Next), "z now = next");
    assert!(matches!(press(&app, 'n'), Action::Noop), "old n unbound");
}

#[test]
fn restore_keybinds_resets_to_defaults() {
    use crate::action::Action;
    use crate::event::{Key, KeyCode, Mods};
    let mut app = demo();
    app.config.keymap.rebind("next", "z");
    let press = |app: &AppState, c: char, ctrl: bool| {
        crate::keymap::map(
            app,
            Key {
                code: KeyCode::Char(c),
                mods: Mods {
                    ctrl,
                    ..Default::default()
                },
            },
        )
    };
    assert!(matches!(press(&app, 'r', true), Action::RestoreKeybinds));
    app.update(Action::RestoreKeybinds);
    assert!(matches!(press(&app, 'n', false), Action::Next));
    // the custom next→z rebind is gone, so z is back to its default action
    assert!(matches!(press(&app, 'z', false), Action::FitLayout));
}

#[test]
fn bundled_themes_parse_and_join_the_cycle() {
    use crate::config::{BUNDLED_THEMES, Config};
    use crate::ui::theme::Theme;
    // every shipped theme is valid and names itself
    for (name, body) in BUNDLED_THEMES {
        let t = Theme::from_toml(name, body).expect("valid theme TOML");
        assert_eq!(&t.name, name, "{name} self-names");
    }
    // a fresh config seeds them; the cycle = built-ins + customs
    let dir = std::env::temp_dir().join("lyrfin-bundled-themes-test");
    let _ = std::fs::remove_dir_all(&dir);
    let app = crate::app::AppState::new(Config {
        dir,
        ..Config::default()
    });
    let all = app.all_themes();
    assert_eq!(
        all.len(),
        1 + 5 + BUNDLED_THEMES.len(),
        "auto + built-ins + customs"
    );
    assert!(all.contains(&"auto".to_string()));
    assert!(all.contains(&"aurora".to_string()));
    assert!(all.contains(&"tokyonight-night".to_string()));
    assert!(all.contains(&"foxnight-dawn".to_string()));
}

#[test]
fn settings_toggle_persists_in_config() {
    use crate::app::Setting;
    let mut app = demo();
    let before = app.config.album_art;
    app.open_settings_group("Theme");
    let idx = app
        .settings_group_items()
        .iter()
        .position(|s| *s == Setting::AlbumArt)
        .unwrap();
    app.settings.sel = idx;
    app.update(crate::action::Action::Activate);
    assert_eq!(app.config.album_art, !before);
}

#[test]
fn follow_system_reveals_light_dark_pickers() {
    // The Theme group shows the single Theme picker by default; turning on
    // "Follow system light/dark" hides it and reveals the Light + Dark pickers, so
    // only the rows that matter for the current mode are ever visible.
    use crate::app::Setting;
    let mut app = demo();
    app.open_settings_group("Theme");
    let items = app.settings_group_items();
    assert!(
        items.contains(&Setting::ThemeFollowSystem),
        "the follow-system toggle is present"
    );
    assert!(
        items.contains(&Setting::Theme),
        "single Theme picker shows when not following"
    );
    assert!(
        !items.contains(&Setting::LightTheme) && !items.contains(&Setting::DarkTheme),
        "the per-appearance pickers are hidden when not following"
    );

    // toggle follow-system on
    let idx = items
        .iter()
        .position(|s| *s == Setting::ThemeFollowSystem)
        .unwrap();
    app.settings.sel = idx;
    app.update(crate::action::Action::Activate);
    assert!(app.config.theme_follows_system, "toggled on");

    let items = app.settings_group_items();
    assert!(
        items.contains(&Setting::LightTheme) && items.contains(&Setting::DarkTheme),
        "both per-appearance pickers are revealed while following"
    );
    assert!(
        !items.contains(&Setting::Theme),
        "the single Theme picker is hidden while following"
    );
}

#[test]
fn apply_appearance_switches_between_light_and_dark_themes() {
    // The core follow-system behaviour, with the OS detection injected: a Dark
    // appearance activates the dark theme, Light the light theme, and an
    // undetectable appearance (e.g. non-macOS) falls back to the dark theme.
    use crate::appearance::Appearance;
    let mut app = demo();
    app.config.theme_follows_system = true;
    // built-in names so resolution is deterministic without a themes/ dir
    app.config.light_theme = "glacier".into();
    app.config.dark_theme = "cyberpunk".into();

    app.apply_appearance(Some(Appearance::Dark));
    assert_eq!(app.theme.name, "cyberpunk", "dark appearance → dark theme");
    app.apply_appearance(Some(Appearance::Light));
    assert_eq!(app.theme.name, "glacier", "light appearance → light theme");
    app.apply_appearance(None);
    assert_eq!(
        app.theme.name, "cyberpunk",
        "undetectable appearance falls back to the dark theme"
    );
}

#[test]
fn disabling_follow_system_restores_the_single_theme() {
    let mut app = demo();
    app.set_theme("cyberpunk"); // the user's single-theme choice
    assert_eq!(app.theme.name, "cyberpunk");
    app.config.light_theme = "glacier".into();
    app.config.dark_theme = "monolith".into();

    app.set_theme_follows_system(true);
    assert!(app.config.theme_follows_system);
    assert!(
        matches!(app.theme.name.as_str(), "glacier" | "monolith"),
        "following the system switches to a light/dark slot theme, not the single one"
    );

    app.set_theme_follows_system(false);
    assert!(!app.config.theme_follows_system);
    assert_eq!(
        app.theme.name, "cyberpunk",
        "disabling follow-system restores the single theme"
    );
}

#[test]
fn reload_cover_keeps_the_active_theme_under_follow_system() {
    // Regression: a local track change calls `reload_cover`, which used to reset
    // the theme to the dormant `config.theme` (often "auto"), snapping the UI to
    // the terminal palette on every track while follow-system mode drove a
    // light/dark slot. Spotify playback doesn't call `reload_cover`, so only local
    // playback showed the flicker. It must reset to the *active* theme instead.
    use crate::appearance::Appearance;
    let mut app = demo();
    app.set_theme("auto"); // the dormant single-theme choice
    app.config.light_theme = "glacier".into();
    app.config.dark_theme = "cyberpunk".into();
    app.set_theme_follows_system(true);
    app.apply_appearance(Some(Appearance::Dark));
    assert_eq!(app.theme.name, "cyberpunk", "following system → dark slot");

    app.reload_cover(); // what loading a local track triggers
    assert_eq!(
        app.theme.name, "cyberpunk",
        "reload_cover keeps the active follow-system theme, not config.theme (\"auto\")"
    );
}

#[test]
fn touchpad_settings_are_editable_in_the_general_tab() {
    use crate::app::Setting;
    use crate::config::TouchpadSpeed;
    let mut app = demo();
    app.open_settings_group("General");

    // the two rows are present in the General group
    let items = app.settings_group_items();
    assert!(items.contains(&Setting::TouchpadScroll), "speed row shows");
    assert!(
        items.contains(&Setting::GridScrollLock),
        "row-lock row shows"
    );

    // cycling the speed row (Activate steps slow→normal→fast) mutates + persists
    let idx = items
        .iter()
        .position(|s| *s == Setting::TouchpadScroll)
        .unwrap();
    app.settings.sel = idx;
    assert_eq!(app.config.touchpad_speed, TouchpadSpeed::Normal);
    app.update(crate::action::Action::Activate);
    assert_eq!(
        app.config.touchpad_speed,
        TouchpadSpeed::Fast,
        "Activate cycles the touchpad speed"
    );

    // toggling the row-lock flips the bool
    let before = app.config.grid_scroll_lock;
    let idx = app
        .settings_group_items()
        .iter()
        .position(|s| *s == Setting::GridScrollLock)
        .unwrap();
    app.settings.sel = idx;
    app.update(crate::action::Action::Activate);
    assert_eq!(app.config.grid_scroll_lock, !before);
}

#[test]
fn touchpad_setting_labels_render_in_the_overlay() {
    let mut a = demo();
    a.open_settings();
    a.config.overlay_size = crate::config::OVERLAY_SIZE_COUNT - 1; // roomy → rows fit
    let out = render_layout(&mut a, Layout::Dashboard, 140, 30);
    assert!(
        out.contains("Touchpad scroll speed"),
        "the touchpad speed row renders:\n{out}"
    );
    assert!(
        out.contains("Lock grid scroll to row"),
        "the row-lock row renders:\n{out}"
    );
}

#[test]
fn boolean_rows_render_as_toggle_switches() {
    use crate::action::Action;
    let mut app = demo();
    app.layout = Layout::Dashboard;
    // the per-view popup on Dashboard surfaces panel toggles + the Tracklist
    // column toggles — all boolean rows
    app.update(Action::OpenViewSettings);
    let out = render_layout(&mut app, Layout::Dashboard, 100, 40);
    assert!(
        out.contains("(──●)") || out.contains("(●──)"),
        "boolean settings render as toggle switches:\n{out}"
    );
    // the old `●  on / ○  off` text is gone
    assert!(
        !out.contains("○  off") && !out.contains("●  on"),
        "the old on/off pip text was replaced"
    );
}

#[test]
fn global_settings_overlay_is_tabbed() {
    use crate::action::Action;
    let mut a = demo();
    a.open_settings();
    a.config.overlay_size = crate::config::OVERLAY_SIZE_COUNT - 1; // widest → every tab on one row
    let out = render_layout(&mut a, Layout::Dashboard, 140, 30);
    // Dashboard exposes every group, so all of SETTINGS_GROUPS are tabbed here
    for g in crate::app::SETTINGS_GROUPS {
        assert!(out.contains(g), "tab `{g}` is shown:\n{out}");
    }
    assert!(out.contains("Mouse support"), "the General tab's rows show");
    // Tab to the Tracklist group (General→Panes→Grid→Tracklist) swaps the rows
    a.update(Action::OverlayTab(3));
    let out2 = render_layout(&mut a, Layout::Dashboard, 120, 30);
    assert!(
        out2.contains("Track number"),
        "Tracklist rows after tabbing:\n{out2}"
    );
    assert!(
        !out2.contains("Mouse support"),
        "General rows are gone after tabbing"
    );
}

#[test]
fn global_overlay_is_a_superset_of_every_popup() {
    // The core guarantee: nothing is reachable only from a view's `;` popup. Every
    // setting a popup exposes also lives in the global overlay's master list
    // (`settings_items`), under a real `SETTINGS_GROUPS` tab — and every popup tab
    // name is one of those group names (one shared vocabulary). Guards against a
    // setting being wired into a popup tab but forgotten in the global overlay.
    use crate::action::Action;
    let layouts = [
        Layout::FullPlayer,
        Layout::LyricsFocus,
        Layout::Dashboard,
        Layout::LibraryFocus,
        Layout::Concert,
        Layout::Radio,
        Layout::Spotify,
    ];
    for layout in layouts {
        let mut a = demo();
        a.layout = layout;
        a.update(Action::OpenViewSettings); // open the `;` popup for this view
        let global = a.settings_items();
        for s in a.popup_all_settings() {
            assert!(
                global.contains(&s),
                "{s:?} is in the {layout:?} popup but missing from the global overlay"
            );
        }
        for tab in a.popup_tab_names() {
            assert!(
                crate::app::SETTINGS_GROUPS.contains(&tab),
                "popup tab `{tab}` on {layout:?} is not a SETTINGS_GROUPS name"
            );
        }
    }
}

#[test]
fn settings_tabs_drop_only_empty_panes() {
    // The global overlay skips a group only when the current view has no rows for
    // it — in practice just "Panes" (Radio/Concert host no movable panels). Every
    // other group is always present, so the overlay never shows a blank tab.
    for layout in [Layout::Radio, Layout::Concert] {
        let mut a = demo();
        a.layout = layout;
        let tabs = a.settings_tabs();
        assert!(
            !tabs.contains(&"Panes"),
            "{layout:?} has no movable panels → no Panes tab: {tabs:?}"
        );
        // the non-pane groups still show
        for g in ["General", "Grid", "Tracklist", "Theme", "Spotify", "Keys"] {
            assert!(tabs.contains(&g), "{g} tab present on {layout:?}");
        }
    }
    for layout in [Layout::Dashboard, Layout::Spotify] {
        let mut a = demo();
        a.layout = layout;
        assert!(
            a.settings_tabs().contains(&"Panes"),
            "{layout:?} hosts panels → shows the Panes tab"
        );
    }
}

#[test]
fn overlay_size_cycles_grows_and_persists() {
    use crate::action::Action;
    use crate::ui::components::overlay_dims;
    let mut a = demo();
    let area = ratatui::layout::Rect::new(0, 0, 120, 40);
    // opens at the compact default (step 0)
    assert_eq!(a.config.overlay_size, 0, "overlays open compact by default");
    let mut prev = overlay_dims(&a, area);
    // `f` (CycleOverlaySize) steps the size up, persisted, each step larger
    for step in 1..crate::config::OVERLAY_SIZE_COUNT {
        a.update(Action::CycleOverlaySize);
        assert_eq!(a.config.overlay_size, step, "size stepped up + persisted");
        let cur = overlay_dims(&a, area);
        assert!(
            cur.0 >= prev.0 && cur.1 >= prev.1 && cur != prev,
            "step {step} grows the overlay ({prev:?} → {cur:?})"
        );
        prev = cur;
    }
    // cycling past the top wraps back to the compact default
    a.update(Action::CycleOverlaySize);
    assert_eq!(
        a.config.overlay_size, 0,
        "cycling past the top wraps to Small"
    );
}

#[test]
fn popup_geometry_is_stable_and_resizable() {
    // Two guarantees for the `;` popup: (1) its size never changes as you switch
    // tabs — so the centred card never re-centers and jumps up/down (the "dizzy"
    // report); and (2) `f` grows it, consistent with the other overlays.
    use crate::action::Action;
    let mut a = demo();
    a.layout = Layout::Dashboard;
    a.update(Action::OpenViewSettings);
    let area = ratatui::layout::Rect::new(0, 0, 120, 40);

    // (1) identical size on every tab (Dashboard's tabs differ in row count)
    let n = a.popup_tab_names().len();
    assert!(
        n >= 2,
        "Dashboard's popup has several tabs to switch between"
    );
    let base = crate::ui::views::popup_dims(&a, area);
    for i in 0..n {
        a.set_overlay_tab(i);
        assert_eq!(
            crate::ui::views::popup_dims(&a, area),
            base,
            "popup size is identical on tab {i}, so it never jumps"
        );
    }

    // (2) `f` (CycleOverlaySize) grows it — the same resize the global overlay has
    a.update(Action::CycleOverlaySize);
    let (mw, mh) = crate::ui::views::popup_dims(&a, area);
    assert!(
        mw >= base.0 && mh >= base.1 && (mw, mh) != base,
        "growing the size grows the popup ({}x{} → {mw}x{mh})",
        base.0,
        base.1
    );
}

#[test]
fn overlay_content_scrolls_in_a_small_window() {
    // "tabs/content should be scrollable in smaller windows": a compact overlay
    // can't show a long tab all at once, so the cursor scrolls the list; a grown
    // overlay fits the same rows without scrolling. `settings.off` is the sticky
    // scroll offset the renderer computes (0 = everything fits).
    use crate::action::{Action, Motion};
    let mut a = demo();
    a.layout = Layout::Dashboard;
    a.update(Action::OpenViewSettings);
    let tl = a
        .popup_tab_names()
        .iter()
        .position(|t| *t == "Tracklist")
        .expect("Dashboard popup has a Tracklist tab");
    a.set_overlay_tab(tl);
    a.update(Action::Move(Motion::Bottom)); // cursor on the last (Comment) row

    // compact size + short window → the long tab must scroll to keep the cursor
    a.config.overlay_size = 0;
    render_layout(&mut a, Layout::Dashboard, 90, 18);
    assert!(
        a.settings.off.get() > 0,
        "a long tab scrolls in a small overlay (offset {})",
        a.settings.off.get()
    );

    // grown to the top step + a tall window → the same rows fit, no scroll
    a.config.overlay_size = crate::config::OVERLAY_SIZE_COUNT - 1;
    render_layout(&mut a, Layout::Dashboard, 90, 40);
    assert_eq!(
        a.settings.off.get(),
        0,
        "the same rows fit without scrolling once the overlay is grown"
    );
}

#[test]
fn settings_overlay_tab_keys_and_gating() {
    use crate::action::Action;
    use crate::event::{Key, KeyCode, Mods};
    let mut a = demo();
    a.open_settings();
    let press = |a: &AppState, code: KeyCode| {
        crate::keymap::map(
            a,
            Key {
                code,
                mods: Mods::default(),
            },
        )
    };
    assert!(matches!(press(&a, KeyCode::Tab), Action::OverlayTab(1)));
    assert!(matches!(
        press(&a, KeyCode::BackTab),
        Action::OverlayTab(-1)
    ));
    // a global one-key command is swallowed while the overlay owns the screen
    assert!(matches!(press(&a, KeyCode::Char('v')), Action::Noop));
}

#[test]
fn settings_toggle_column() {
    use crate::app::Setting;
    let mut app = demo();
    let before = app.config.columns.genre;
    app.open_settings_group("Tracklist");
    let idx = app
        .settings_group_items()
        .iter()
        .position(|s| *s == Setting::ColGenre)
        .unwrap();
    app.settings.sel = idx;
    app.update(crate::action::Action::Activate);
    assert_eq!(app.config.columns.genre, !before);
}
