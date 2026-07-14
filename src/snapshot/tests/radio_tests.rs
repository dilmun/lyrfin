//! Radio-view snapshot/behaviour tests, split out of snapshot.rs's mod tests
//! to keep that file navigable. A child of `tests`, so `use super::*` pulls in
//! the shared `demo()` helper + render_layout.

use super::*;

#[test]
fn key_6_opens_radio_view() {
    use crate::event::{Key, KeyCode, Mods};
    let mut a = demo();
    let k = Key {
        code: KeyCode::Char('6'),
        mods: Mods::default(),
    };
    let action = crate::keymap::map(&a, k);
    assert!(
        matches!(action, crate::action::Action::OpenRadio),
        "6 should map to OpenRadio, got {action:?}"
    );
    a.update(action);
    assert_eq!(a.layout, Layout::Radio, "layout switched to Radio");
}

#[test]
fn radio_view_lists_and_plays_stations() {
    use crate::radio::Station;
    let st = |name: &str, cc: &str, tag: &str, br: u32, uuid: &str| Station {
        name: name.into(),
        url: format!("http://example/{uuid}"),
        countrycode: cc.into(),
        tags: tag.into(),
        bitrate: br,
        uuid: uuid.into(),
        ..Default::default()
    };
    let mut a = demo();
    a.radio.stations = vec![
        st("Mega FM", "EG", "arabic", 128, "u1"),
        st("CNR Music", "CN", "pop", 64, "u2"),
    ];

    let s = render_layout(&mut a, Layout::Radio, 100, 20);
    assert!(s.contains("RADIO"), "radio panel renders");
    assert!(
        s.contains("STATION") && s.contains("KBPS"),
        "columnar table header present (shared columns_table)"
    );
    assert!(
        s.contains("Mega FM") && s.contains("CNR Music"),
        "stations listed"
    );
    assert!(
        s.contains("EG") && s.contains("arabic"),
        "station metadata shown"
    );

    // tuning in the selected station makes it the now-playing station
    a.radio.sel = 0;
    a.update(crate::action::Action::RadioActivate);
    assert!(
        a.rnow
            .now_station
            .as_ref()
            .is_some_and(|p| p.name == "Mega FM"),
        "selected station becomes now-playing"
    );
    let s = render_layout(&mut a, Layout::Radio, 100, 20);
    assert!(s.contains("📻"), "the now-bar shows the streaming station");
}

#[test]
fn radio_search_is_not_focused_by_default() {
    use crate::action::Action;
    use crate::event::{Key, KeyCode, Mods};
    let mut a = demo();
    a.layout = Layout::Radio;
    // a letter in browse mode is a command (it falls through to the global
    // binding), never query text — typing must not edit the search box
    let x = Key {
        code: KeyCode::Char('x'),
        mods: Mods::default(),
    };
    assert!(
        !matches!(crate::keymap::map(&a, x), Action::RadioInput(_)),
        "letters don't edit the query until the search box is focused"
    );
    // '/' focuses the search box; only then does typing edit the query
    let slash = Key {
        code: KeyCode::Char('/'),
        mods: Mods::default(),
    };
    assert!(matches!(
        crate::keymap::map(&a, slash),
        Action::RadioFocusSearch
    ));
    a.update(Action::RadioFocusSearch);
    assert!(a.radio.editing, "search box focused after /");
    let j = Key {
        code: KeyCode::Char('j'),
        mods: Mods::default(),
    };
    match crate::keymap::map(&a, j) {
        Action::RadioInput(q) => assert_eq!(q, "j"),
        other => panic!("expected RadioInput while editing, got {other:?}"),
    }
    // Esc leaves search edit but stays in the view
    a.update(Action::RadioCancel);
    assert!(!a.radio.editing && a.layout == Layout::Radio);
}

#[test]
fn radio_dedicated_keys_drive_filters() {
    use crate::action::Action;
    use crate::event::{Key, KeyCode, Mods};
    let key = |c| Key {
        code: KeyCode::Char(c),
        mods: Mods::default(),
    };
    let mut a = demo();
    a.layout = Layout::Radio;
    // the station-list keys are scoped to the Main pane (the section sidebar shadows
    // them), so focus the list before asserting them.
    a.focus = crate::app::Focus::Main;
    assert!(matches!(
        crate::keymap::map(&a, key('c')),
        Action::RadioOpenCountry
    ));
    assert!(matches!(
        crate::keymap::map(&a, key('g')),
        Action::RadioOpenGenre
    ));
    // `f` stars the highlighted station (unified with favourite elsewhere).
    assert!(matches!(
        crate::keymap::map(&a, key('f')),
        Action::RadioStar
    ));
    assert!(matches!(
        crate::keymap::map(&a, key('o')),
        Action::RadioCycleSort
    ));
    // opening a picker, then Esc closes it without leaving the view
    a.update(Action::RadioOpenCountry);
    assert!(a.radio.picker.is_some(), "country picker opened");
    a.update(Action::RadioCancel);
    assert!(a.radio.picker.is_none() && a.layout == Layout::Radio);
}

#[test]
fn radio_favorites_star_persists_and_toggles() {
    use crate::action::Action;
    use crate::radio::Station;
    let mut a = demo();
    a.config.dir = std::env::temp_dir().join("lyrfin_radio_fav_test");
    let _ = std::fs::remove_dir_all(&a.config.dir);
    a.layout = Layout::Radio;
    a.radio.stations = vec![Station {
        name: "Mega FM".into(),
        url: "http://x/1".into(),
        uuid: "u1".into(),
        ..Default::default()
    }];
    a.radio.sel = 0;
    a.update(Action::RadioStar);
    assert_eq!(a.radio.favorites.len(), 1, "starred");
    // persisted to disk
    let loaded = crate::library::store::RadioFavorites::load(&a.config.dir);
    assert_eq!(loaded.len(), 1, "favorite written to radio_favorites.json");
    // the Favorites section shows the saved list
    a.radio.section = crate::app::RadioSection::Favorites;
    assert_eq!(a.radio_view_list().len(), 1);
    // star again (from the favorites list) removes it
    a.update(Action::RadioStar);
    assert!(a.radio.favorites.is_empty(), "unstarred");
    let _ = std::fs::remove_dir_all(&a.config.dir);
}

#[test]
fn radio_picker_filters_and_applies_country() {
    use crate::action::Action;
    use crate::radio::Country;
    let mut a = demo();
    a.layout = Layout::Radio;
    a.radio.all_countries = vec![
        Country {
            name: "Egypt".into(),
            code: "EG".into(),
            count: 50,
        },
        Country {
            name: "China".into(),
            code: "CN".into(),
            count: 80,
        },
    ];
    a.update(Action::RadioOpenCountry);
    let opts = a.radio_picker_options();
    assert_eq!(opts.len(), 3, "clear entry + two countries");
    assert!(opts[0].1.is_none(), "first entry clears the filter");
    // narrow to Egypt
    a.update(Action::RadioPickerInput("egy".into()));
    let opts = a.radio_picker_options();
    assert_eq!(opts.len(), 2, "clear + Egypt");
    assert!(opts[1].0.contains("Egypt"));
    if let Some(p) = a.radio.picker.as_mut() {
        p.sel = 1;
    }
    a.update(Action::RadioActivate);
    assert!(a.radio.picker.is_none(), "picker closed after apply");
    assert_eq!(
        a.radio.country.as_ref().map(|(n, _)| n.clone()),
        Some("Egypt".to_string()),
        "country filter applied"
    );
}

#[test]
fn radio_picker_navigates_by_default_slash_to_filter() {
    use crate::action::{Action, Motion};
    use crate::event::{Key, KeyCode, Mods};
    let ch = |c| Key {
        code: KeyCode::Char(c),
        mods: Mods::default(),
    };
    let mut a = demo();
    a.layout = Layout::Radio;
    a.update(Action::RadioOpenCountry);
    assert!(
        a.radio.picker.as_ref().is_some_and(|p| !p.editing),
        "picker opens focused on the list, not the filter box"
    );
    // j/k navigate the list (they do NOT type into the filter)
    assert!(matches!(
        crate::keymap::map(&a, ch('j')),
        Action::Move(Motion::Down)
    ));
    assert!(matches!(
        crate::keymap::map(&a, ch('k')),
        Action::Move(Motion::Up)
    ));
    // '/' focuses the filter box; only then does typing edit the query
    assert!(matches!(
        crate::keymap::map(&a, ch('/')),
        Action::RadioPickerStartSearch
    ));
    a.update(Action::RadioPickerStartSearch);
    assert!(a.radio.picker.as_ref().unwrap().editing);
    match crate::keymap::map(&a, ch('j')) {
        Action::RadioPickerInput(q) => assert_eq!(q, "j"),
        other => panic!("expected filter input while editing, got {other:?}"),
    }
    // Tab returns to navigation without closing the picker
    let tab = Key {
        code: KeyCode::Tab,
        mods: Mods::default(),
    };
    assert!(matches!(
        crate::keymap::map(&a, tab),
        Action::RadioPickerEndSearch
    ));
    a.update(Action::RadioPickerEndSearch);
    assert!(a.radio.picker.as_ref().is_some_and(|p| !p.editing));
    // Esc in navigation closes the picker (still in the Radio view)
    let esc = Key {
        code: KeyCode::Esc,
        mods: Mods::default(),
    };
    assert!(matches!(crate::keymap::map(&a, esc), Action::RadioCancel));
    a.update(Action::RadioCancel);
    assert!(a.radio.picker.is_none() && a.layout == Layout::Radio);
}

#[test]
fn radio_genre_picker_scopes_to_selected_country() {
    use crate::action::Action;
    use crate::radio::TagItem;
    let tag = |n: &str, c: u32| TagItem {
        name: n.into(),
        count: c,
    };
    let mut a = demo();
    a.layout = Layout::Radio;
    // global genres (used when no country is selected)
    a.radio.all_tags = vec![tag("musica romantica", 161), tag("pop", 5000)];
    // China-specific genres, already cached (so opening the picker won't fetch)
    a.radio
        .genres_by_country
        .insert("CN".into(), vec![tag("news", 148), tag("oldies", 35)]);
    a.radio.country = Some(("China".into(), "CN".into()));
    a.update(Action::RadioOpenGenre);
    let labels: Vec<String> = a
        .radio_picker_options()
        .iter()
        .map(|(l, _)| l.clone())
        .collect();
    assert!(
        labels.iter().any(|l| l.contains("news")),
        "genre picker shows the country's genres"
    );
    assert!(
        !labels.iter().any(|l| l.contains("musica romantica")),
        "global genres irrelevant to the country are hidden"
    );
}

#[test]
fn radio_picker_autoselects_closest_match() {
    use crate::action::Action;
    use crate::radio::Country;
    let mut a = demo();
    a.layout = Layout::Radio;
    a.radio.all_countries = vec![
        Country {
            name: "Chile".into(),
            code: "CL".into(),
            count: 30,
        },
        Country {
            name: "China".into(),
            code: "CN".into(),
            count: 80,
        },
        // a substring-only match that must NOT outrank the exact "China"
        Country {
            name: "French Indochina".into(),
            code: "FI".into(),
            count: 5,
        },
    ];
    a.update(Action::RadioOpenCountry);
    a.update(Action::RadioPickerInput("china".into()));
    // the cursor auto-lands on the first real row (the clear row is index 0)…
    assert_eq!(
        a.radio.picker.as_ref().map(|p| p.sel),
        Some(1),
        "highlight skips the clear row"
    );
    // …and that row is the exact match, not the substring "French Indochina"
    let opts = a.radio_picker_options();
    assert!(opts[1].0.starts_with("China"), "exact match ranked first");
    // so a single Enter applies China
    a.update(Action::RadioActivate);
    assert_eq!(
        a.radio.country.as_ref().map(|(n, _)| n.clone()),
        Some("China".to_string()),
        "Enter applies the auto-selected match"
    );
}

#[test]
fn radio_tab_cycles_sidebar_and_list_focus() {
    use crate::action::{Action, Motion};
    use crate::app::Focus;
    use crate::event::{Key, KeyCode, Mods};
    use crate::radio::Station;
    let tab = Key {
        code: KeyCode::Tab,
        mods: Mods::default(),
    };
    let slash = Key {
        code: KeyCode::Char('/'),
        mods: Mods::default(),
    };
    let j = Key {
        code: KeyCode::Char('j'),
        mods: Mods::default(),
    };
    let mut a = demo();
    a.layout = Layout::Radio;
    a.focus = Focus::Main;
    a.radio.stations = vec![
        Station {
            name: "A".into(),
            url: "u1".into(),
            uuid: "u1".into(),
            ..Default::default()
        },
        Station {
            name: "B".into(),
            url: "u2".into(),
            uuid: "u2".into(),
            ..Default::default()
        },
    ];
    a.radio.sel = 0;
    // list focused: j moves the station list, never the query
    assert!(matches!(
        crate::keymap::map(&a, j),
        Action::Move(Motion::Down)
    ));
    // Tab cycles focus to the section sidebar; j then steps sections (still a Move,
    // routed to the sidebar by navigation)
    assert!(matches!(crate::keymap::map(&a, tab), Action::CyclePane));
    a.update(Action::CyclePane);
    assert_eq!(a.focus, Focus::Sidebar);
    assert!(matches!(
        crate::keymap::map(&a, j),
        Action::Move(Motion::Down)
    ));
    // Tab again returns to the station list
    a.update(Action::CyclePane);
    assert_eq!(a.focus, Focus::Main);
    // '/' focuses the search box; only then does typing edit the query
    assert!(matches!(
        crate::keymap::map(&a, slash),
        Action::RadioFocusSearch
    ));
    a.update(Action::RadioFocusSearch);
    assert!(a.radio.editing);
    match crate::keymap::map(&a, j) {
        Action::RadioInput(q) => assert_eq!(q, "j"),
        other => panic!("expected query edit while search-focused, got {other:?}"),
    }
    // Esc leaves search edit but stays in the view
    a.update(Action::RadioCancel);
    assert!(!a.radio.editing);
}

#[test]
fn radio_sort_cycles_and_chips_render() {
    use crate::action::Action;
    let mut a = demo();
    a.layout = Layout::Radio;
    assert_eq!(a.radio.sort.label(), "popular");
    // the top pane shows the active filters; the status bar shows the radio
    // shortcuts (clear the demo notification, which would otherwise mask them)
    a.radio.country = Some(("Egypt".into(), "EG".into()));
    a.radio.tag = Some("jazz".into());
    a.notification = None;
    // wide enough that the full minimal status-bar hint isn't truncated by the
    // centre "Next:" hint competing for the left zone
    let s = render_layout(&mut a, Layout::Radio, 140, 20);
    assert!(
        s.contains("Country:") && s.contains("Egypt"),
        "country chip"
    );
    assert!(s.contains("Genre:") && s.contains("jazz"), "genre chip");
    assert!(s.contains("Sort:"), "sort chip");
    assert!(
        s.contains("search") && s.contains("settings") && s.contains("keys"),
        "status bar shows the minimal radio shortcuts + gateways"
    );
    // cycling changes the order
    a.update(Action::RadioCycleSort);
    assert_eq!(a.radio.sort.label(), "votes");
}

#[test]
fn radio_view_blocks_the_tag_editor() {
    use crate::action::Action;
    let mut a = demo();
    // in the Radio view, Tag Edit must not open the editor on a local track
    a.layout = Layout::Radio;
    a.update(Action::BeginTagEdit);
    assert!(!a.tags_open(), "tag editor stays closed in the Radio view");
    // but it still works in a local view
    a.layout = Layout::Dashboard;
    a.update(Action::BeginTagEdit);
    assert!(a.tags_open(), "tag editor opens for local music");
}

#[test]
fn radio_drives_the_visualizer() {
    use crate::action::Action;
    use crate::core::player::Status;
    use crate::radio::Station;
    let mut a = demo();
    a.layout = Layout::Radio; // the radio viz lives in the Radio view
    // local player is paused (overlay), radio is the live audio
    a.player.status = Status::Paused;
    a.rnow.now_station = Some(Station {
        name: "X".into(),
        url: "u".into(),
        uuid: "u".into(),
        ..Default::default()
    });
    a.rnow.radio_paused = false;
    a.player.spectrum = vec![1.0; 48]; // a loud frame from the radio audio
    for _ in 0..40 {
        a.update(Action::Tick); // let the smoothed viz rise
    }
    assert!(
        a.viz.levels.iter().any(|&l| l > 0.1),
        "the radio viz reacts to radio audio in the Radio view"
    );
}

#[test]
fn radio_overlay_keeps_local_context_in_local_views() {
    use crate::action::Action;
    use crate::radio::Station;
    let mut a = demo();
    let local_title = a
        .player
        .current
        .and_then(|id| a.library.track(id))
        .map(|t| t.title.clone())
        .expect("demo has a current local track");
    a.layout = Layout::Radio;
    a.radio.stations = vec![Station {
        name: "Cairo Jazz".into(),
        url: "u".into(),
        uuid: "u".into(),
        ..Default::default()
    }];
    a.radio.sel = 0;
    a.update(Action::RadioActivate); // radio overlay on; local preserved
    // Radio view → the bar belongs to the station (only the station)
    let s = render_layout(&mut a, Layout::Radio, 120, 22);
    assert!(s.contains("Cairo Jazz"), "Radio view shows the station");
    // switch to a local view → the bar shows YOUR track and NOTHING radio:
    // the two contexts never leak into each other (clear the transient
    // "Tuning in…" toast first, which is not a playback display)
    a.notification = None;
    let s = render_layout(&mut a, Layout::Dashboard, 120, 22);
    assert!(
        s.contains(&local_title),
        "Dashboard shows the local track, not the radio"
    );
    assert!(
        !s.contains("📻"),
        "no radio bleeds into the music-player view"
    );
    assert!(
        !s.contains("Cairo Jazz"),
        "the station name never appears in a local view"
    );
}

#[test]
fn radio_channel_change_updates_overlay() {
    use crate::action::Action;
    use crate::radio::Station;
    let mk = |name: &str, uuid: &str| Station {
        name: name.into(),
        url: format!("http://x/{uuid}"),
        uuid: uuid.into(),
        ..Default::default()
    };
    let mut a = demo();
    a.layout = Layout::Radio;
    a.radio.stations = vec![mk("Alpha", "u1"), mk("Beta", "u2"), mk("Gamma", "u3")];
    a.radio.sel = 0;
    // radio is an OVERLAY: tuning preserves the local track (it isn't nulled)
    let local_before = a.player.current;
    a.update(Action::RadioActivate); // tune Alpha
    assert!(
        a.rnow
            .now_station
            .as_ref()
            .is_some_and(|s| s.name == "Alpha")
    );
    assert!(!a.rnow.radio_paused, "tuning starts streaming");
    assert_eq!(
        a.player.current, local_before,
        "local track preserved under radio"
    );
    // n/p change station (RadioStation) — still an overlay, local untouched
    a.update(Action::RadioStation(1));
    assert!(
        a.rnow
            .now_station
            .as_ref()
            .is_some_and(|s| s.name == "Beta")
    );
    a.update(Action::RadioStation(-1));
    assert!(
        a.rnow
            .now_station
            .as_ref()
            .is_some_and(|s| s.name == "Alpha"),
        "p wraps back"
    );
    assert_eq!(a.player.current, local_before, "local still preserved");
    // the Radio view's now-bar shows the station (no album-art block)
    let s = render_layout(&mut a, Layout::Radio, 100, 20);
    assert!(s.contains("📻"), "radio now-bar");
    assert!(!s.contains("████████████"), "no album-art block for radio");
}

#[test]
fn radio_empty_filter_combo_explains_itself() {
    let mut a = demo();
    a.layout = Layout::Radio;
    a.radio.country = Some(("China".into(), "CN".into()));
    a.radio.tag = Some("musica romantica".into());
    a.radio.stations = vec![]; // the combo legitimately matches nothing
    a.radio.loading = false;
    let s = render_layout(&mut a, Layout::Radio, 90, 22);
    assert!(
        s.contains("No stations for China + musica romantica"),
        "empty state names the active filters"
    );
    assert!(
        s.contains("change filters"),
        "empty state suggests how to recover"
    );
}

#[test]
fn radio_local_directory_filters_in_memory() {
    use crate::action::Action;
    use crate::radio::{RadioResult, Station};
    let mk = |name: &str, cc: &str, tag: &str, br: u32, clicks: u32| Station {
        name: name.into(),
        countrycode: cc.into(),
        country: cc.into(),
        tags: tag.into(),
        bitrate: br,
        clickcount: clicks,
        url: format!("u-{name}"),
        uuid: name.into(),
        ..Default::default()
    };
    let mut a = demo();
    a.layout = Layout::Radio;
    // feed a downloaded directory (as the worker would) — adopts local mode
    a.on_radio_result(RadioResult::Directory {
        from_cache: true,
        stations: vec![
            mk("Alpha", "GB", "pop", 128, 100),
            mk("Beta", "EG", "jazz", 96, 50),
            mk("Gamma", "GB", "jazz rock", 320, 200),
        ],
    });
    assert!(a.radio.local_ready, "directory adopted, now local");
    assert!(
        a.radio
            .all_countries
            .iter()
            .any(|c| c.code == "GB" && c.count == 2),
        "country list derived locally"
    );
    // default popular order preserved (clickcount desc): Gamma, Alpha, Beta
    let names: Vec<String> = a.radio_view_list().iter().map(|s| s.name.clone()).collect();
    assert_eq!(names, vec!["Gamma", "Alpha", "Beta"]);
    // genre filter is a local substring over tags: "jazz" → Beta + Gamma
    a.radio.tag = Some("jazz".into());
    a.update(Action::RadioInput(String::new()));
    let names: Vec<String> = a.radio_view_list().iter().map(|s| s.name.clone()).collect();
    assert!(
        names.contains(&"Beta".to_string()) && names.contains(&"Gamma".to_string()),
        "jazz matches Beta + Gamma"
    );
    assert!(
        !names.contains(&"Alpha".to_string()),
        "pop station excluded"
    );
}

#[test]
fn radio_icy_title_headlines_the_now_bar() {
    use crate::radio::Station;
    let mut a = demo();
    a.layout = Layout::Radio;
    a.rnow.now_station = Some(Station {
        name: "Cairo Jazz".into(),
        countrycode: "EG".into(),
        tags: "jazz".into(),
        bitrate: 128,
        url: "u".into(),
        uuid: "u".into(),
        ..Default::default()
    });
    a.player.current = None;
    // before any ICY metadata, the station name is the headline
    let s = render_layout(&mut a, Layout::Radio, 100, 20);
    assert!(s.contains("Cairo Jazz"));
    // once the stream reports a song, it headlines and the station drops below
    a.rnow.now_station_title = Some("Miles Davis - So What".into());
    let s = render_layout(&mut a, Layout::Radio, 100, 20);
    assert!(
        s.contains("Miles Davis - So What"),
        "ICY song is the headline"
    );
    assert!(
        s.contains("📻") && s.contains("Cairo Jazz"),
        "station shown below"
    );
}

#[test]
fn radio_esc_does_not_exit_to_local_player() {
    use crate::action::Action;
    let mut a = demo();
    a.layout = Layout::Radio;
    // plain browse: Esc is a no-op, never drops back to the local player
    a.update(Action::RadioCancel);
    assert_eq!(a.layout, Layout::Radio, "Esc stays in the Radio view");
    // Esc still closes an open picker (without leaving the view)
    a.update(Action::RadioOpenCountry);
    a.update(Action::RadioCancel);
    assert!(a.radio.picker.is_none() && a.layout == Layout::Radio);
}

#[test]
fn radio_table_shows_votes_column() {
    use crate::radio::Station;
    let mut a = demo();
    a.layout = Layout::Radio;
    a.radio.stations = vec![Station {
        name: "Voted FM".into(),
        country: "Spain".into(),
        tags: "pop".into(),
        bitrate: 128,
        clickcount: 5000,
        votes: 4200,
        url: "u".into(),
        uuid: "u".into(),
        ..Default::default()
    }];
    let s = render_layout(&mut a, Layout::Radio, 120, 24);
    assert!(s.contains("VOTES"), "votes column header");
    assert!(s.contains("4.2k"), "votes value formatted");
}

#[test]
fn radio_directory_download_shows_progress_bar() {
    let mut a = demo();
    a.layout = Layout::Radio;
    a.radio.directory_loading = true;
    a.radio.directory_progress = 20 * 1024 * 1024; // 20 MB in
    let s = render_layout(&mut a, Layout::Radio, 160, 20);
    assert!(
        s.contains("Updating directory"),
        "progress label in status bar"
    );
    assert!(s.contains("MB"), "shows MB downloaded");
}

#[test]
fn radio_station_table_shows_metadata_columns() {
    use crate::radio::Station;
    let mk = |name: &str, co: &str, tag: &str, br: u32, clicks: u32| Station {
        name: name.into(),
        country: co.into(),
        tags: tag.into(),
        bitrate: br,
        clickcount: clicks,
        url: "u".into(),
        uuid: name.into(),
        ..Default::default()
    };
    let mut a = demo();
    a.layout = Layout::Radio;
    a.radio.stations = vec![
        mk("BBC Radio 1", "United Kingdom", "pop", 128, 254_000),
        mk("Cairo Jazz FM", "Egypt", "jazz", 96, 1_820),
    ];
    let s = render_layout(&mut a, Layout::Radio, 120, 24);
    assert!(
        s.contains("STATION")
            && s.contains("COUNTRY")
            && s.contains("GENRE")
            && s.contains("KBPS")
            && s.contains("PLAYS"),
        "column headers render"
    );
    assert!(
        s.contains("United Kingdom") && s.contains("Egypt"),
        "country column"
    );
    assert!(s.contains("254k"), "play count 254000 → 254k");
    assert!(s.contains("1.8k"), "play count 1820 → 1.8k");
    assert!(
        s.contains("BBC Radio 1") && s.contains("Cairo Jazz FM"),
        "station names"
    );
}

#[test]
fn radio_view_settings_popup_esc_closes_popup_not_view() {
    use crate::action::Action;
    use crate::event::{Key, KeyCode, Mods};
    let mut a = demo();
    a.layout = Layout::Radio;
    // ';' opens the per-view settings popup over the Radio view
    a.update(Action::OpenViewSettings);
    assert!(a.settings.popup.is_some(), "popup opened");
    // Esc must go to the popup (close it), not exit the Radio view
    let esc = Key {
        code: KeyCode::Esc,
        mods: Mods::default(),
    };
    let action = crate::keymap::map(&a, esc);
    assert!(
        matches!(action, Action::Back),
        "Esc routes to Back while the popup is open, got {action:?}"
    );
    a.update(action);
    assert!(a.settings.popup.is_none(), "popup closed");
    assert_eq!(a.layout, Layout::Radio, "still in the Radio view");
}

#[test]
fn radio_state_survives_a_session_round_trip() {
    use crate::action::Action;
    use crate::radio::Station;
    let mut a = demo();
    a.layout = Layout::Radio;
    a.radio.query = "jazz".into();
    a.radio.country = Some(("Egypt".into(), "EG".into()));
    a.radio.tag = Some("pop".into());
    a.update(Action::RadioCycleSort); // popular → votes
    // tune a station so it's the "last playing" channel
    a.radio.stations = vec![Station {
        name: "Cairo Jazz".into(),
        url: "http://x/1".into(),
        uuid: "u1".into(),
        ..Default::default()
    }];
    a.radio.sel = 0;
    a.update(Action::RadioActivate);
    assert!(a.rnow.now_station.is_some());

    // capture the session and reapply it into a fresh instance
    let sess = a.session();
    let mut b = demo();
    b.apply_session(sess);
    assert_eq!(b.radio.query, "jazz", "query restored");
    assert_eq!(
        b.radio.country.as_ref().map(|(n, _)| n.clone()),
        Some("Egypt".to_string()),
        "country filter restored"
    );
    assert_eq!(b.radio.tag.as_deref(), Some("pop"), "genre filter restored");
    assert_eq!(b.radio.sort.label(), "votes", "sort restored");
    assert!(
        b.rnow
            .now_station
            .as_ref()
            .is_some_and(|s| s.name == "Cairo Jazz"),
        "last-tuned station restored"
    );
    assert!(
        b.rnow.radio_paused,
        "restored station is paused, not auto-streamed"
    );
}
