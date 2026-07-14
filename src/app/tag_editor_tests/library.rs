//! Library behaviour tests (split from tag_editor_tests). `use super::*`
//! reaches the shared app() fixture + AppState privates.

use super::*;

#[test]
fn command_palette_filters_and_runs() {
    let mut a = app();
    a.update(Action::OpenPalette);
    assert!(a.palette.is_some());
    a.update(Action::PaletteInput("theme".into()));
    let matches = a.palette_matches();
    assert!(!matches.is_empty(), "'theme' matches a command");
    let entries = a.palette_entries();
    assert!(matches.iter().any(|&i| entries[i].label.contains("theme")));
    a.update(Action::PaletteActivate); // runs top match + closes
    assert!(a.palette.is_none());
}

#[test]
fn shift_f_cycles_lyrics_format_not_favorite() {
    // Regression: on kitty-keyboard-protocol terminals (Ghostty/Kitty) Shift+f
    // arrives as the base char 'f' + SHIFT, not the uppercase 'F'. With the Lyrics
    // pane focused it must resolve to F = cycle-lyrics-format, never f = favourite.
    let mut a = app();
    a.focus = Focus::Pane(Panel::Lyrics);
    let shift_f = Key {
        code: KeyCode::Char('f'),
        mods: Mods {
            shift: true,
            ..Mods::default()
        },
    };
    assert_eq!(crate::keymap::map(&a, shift_f), Action::CycleLyricsFormat);

    // plain 'f' is still favourite (targets the current track, or no-ops if none)
    let plain_f = Key {
        code: KeyCode::Char('f'),
        mods: Mods::default(),
    };
    assert!(matches!(
        crate::keymap::map(&a, plain_f),
        Action::ToggleFavorite(_) | Action::Noop
    ));
}

#[test]
fn lyrics_format_key_is_pane_scoped() {
    let mut a = app();
    let shift_f = Key {
        code: KeyCode::Char('F'),
        mods: Mods::default(),
    };

    // Outside the lyrics view/pane, `F` is not a global binding any more — it does
    // nothing (and certainly never toggles favourite).
    a.layout = Layout::Dashboard;
    a.focus = Focus::Main;
    assert_eq!(crate::keymap::map(&a, shift_f), Action::Noop);

    // The Lyrics side-pane focused → format cycles.
    a.focus = Focus::Pane(Panel::Lyrics);
    assert_eq!(crate::keymap::map(&a, shift_f), Action::CycleLyricsFormat);

    // The dedicated Lyrics view (whose content is the main area) → also cycles.
    a.layout = Layout::LyricsFocus;
    a.focus = Focus::Main;
    assert_eq!(crate::keymap::map(&a, shift_f), Action::CycleLyricsFormat);
}

#[test]
fn queue_pane_owns_reorder_keys_but_nav_falls_through() {
    use crate::action::Motion;
    let mut a = app();
    a.layout = Layout::Dashboard;
    a.focus = Focus::Pane(Panel::Queue);
    let k = |c| Key {
        code: KeyCode::Char(c),
        mods: Mods::default(),
    };
    assert_eq!(
        crate::keymap::map(&a, k('K')),
        Action::QueueMove(Motion::Up)
    );
    assert_eq!(
        crate::keymap::map(&a, k('J')),
        Action::QueueMove(Motion::Down)
    );
    assert_eq!(crate::keymap::map(&a, k('x')), Action::QueueRemove);
    assert_eq!(crate::keymap::map(&a, k('D')), Action::QueueClearUpcoming);
    // list navigation is universal — it still reaches the global table.
    assert_eq!(crate::keymap::map(&a, k('j')), Action::Move(Motion::Down));
}

#[test]
fn focused_pane_shadows_stray_globals_but_keeps_universal_keys() {
    let mut a = app();
    a.layout = Layout::Dashboard;
    let k = |c| Key {
        code: KeyCode::Char(c),
        mods: Mods::default(),
    };

    // A focused side-pane exposes only its own options: view-content globals are
    // swallowed rather than leaking in.
    a.focus = Focus::Pane(Panel::Lyrics);
    for c in ['f', 't', 'e', 'v', 'b', 'i', 'a'] {
        assert_eq!(
            crate::keymap::map(&a, k(c)),
            Action::Noop,
            "'{c}' must be shadowed while a pane is focused"
        );
    }
    // …while universal keys (transport, nav, chrome) always pass through.
    assert_ne!(
        crate::keymap::map(&a, k('n')),
        Action::Noop,
        "next is transport"
    );
    assert_ne!(
        crate::keymap::map(&a, k(' ')),
        Action::Noop,
        "play/pause is transport"
    );
    assert_ne!(
        crate::keymap::map(&a, k(':')),
        Action::Noop,
        "the palette is always reachable"
    );
    assert_ne!(
        crate::keymap::map(&a, k('1')),
        Action::Noop,
        "view switch is always reachable"
    );

    // The shadow is not layout-specific — the Queue pane behaves the same way.
    a.focus = Focus::Pane(Panel::Queue);
    assert_eq!(crate::keymap::map(&a, k('t')), Action::Noop);
    assert_ne!(crate::keymap::map(&a, k('p')), Action::Noop);

    // The dedicated Lyrics *view* (main focus, not a pane) is NOT shadowed — it's a
    // full view, so its globals still work.
    a.layout = Layout::LyricsFocus;
    a.focus = Focus::Main;
    assert_ne!(crate::keymap::map(&a, k('t')), Action::Noop);
}

#[test]
fn palette_lists_settings_with_values_and_drills_to_apply() {
    let mut a = app();
    a.config.dir = std::env::temp_dir().join("lyrfin_palette_guided");
    let _ = std::fs::remove_dir_all(&a.config.dir);

    // the root list surfaces settings, each with its current value
    let entries = a.palette_entries();
    let theme_row = entries
        .iter()
        .find(|e| e.label == "Theme")
        .expect("a Theme setting row is reachable from the palette");
    assert_eq!(theme_row.value.as_deref(), Some(a.config.theme.as_str()));
    assert!(matches!(
        theme_row.action,
        Action::PaletteOpenSetting(Setting::Theme)
    ));

    // its choices enumerate the themes, with exactly one marked current
    let SettingChoices::Discrete(choices) = a.setting_choices(Setting::Theme) else {
        panic!("theme is a discrete picker");
    };
    assert_eq!(choices.iter().filter(|c| c.current).count(), 1);
    assert!(choices.iter().any(|c| c.label == "cyberpunk"));

    // open → drill into Theme → filter → apply: it sets + persists + closes
    a.update(Action::OpenPalette);
    a.update(Action::PaletteOpenSetting(Setting::Theme));
    assert!(matches!(
        a.palette.as_ref().unwrap().ctx,
        PaletteCtx::Setting(Setting::Theme)
    ));
    a.update(Action::PaletteInput("cyberpunk".into()));
    a.update(Action::PaletteActivate);
    assert!(a.palette.is_none());
    assert_eq!(a.theme.name, "cyberpunk");

    let _ = std::fs::remove_dir_all(&a.config.dir);
}

#[test]
fn formerly_config_only_setting_is_reachable_and_toggles() {
    let mut a = app();
    a.config.dir = std::env::temp_dir().join("lyrfin_gap_settings");
    let _ = std::fs::remove_dir_all(&a.config.dir);

    // Arabic shaping used to be config.toml-only; it's now a reachable General setting
    let entries = a.palette_entries();
    assert!(entries.iter().any(|e| e.label == "Arabic text shaping"
        && matches!(e.action, Action::PaletteOpenSetting(Setting::ArabicShaping))));

    // and it flips in place from the palette (a plain toggle) + persists
    let before = a.config.arabic_shaping;
    a.update(Action::OpenPalette);
    a.update(Action::PaletteOpenSetting(Setting::ArabicShaping));
    assert!(a.palette.is_none());
    assert_eq!(a.config.arabic_shaping, !before);

    let _ = std::fs::remove_dir_all(&a.config.dir);
}

#[test]
fn palette_esc_pops_drill_to_root_then_closes() {
    let mut a = app();
    a.update(Action::OpenPalette);
    a.update(Action::PaletteOpenSetting(Setting::GridSize));
    assert!(matches!(
        a.palette.as_ref().unwrap().ctx,
        PaletteCtx::Setting(_)
    ));
    a.update(Action::Back); // pops the value picker back to the root list
    assert!(matches!(a.palette.as_ref().unwrap().ctx, PaletteCtx::Root));
    a.update(Action::Back); // closes the palette
    assert!(a.palette.is_none());
}

#[test]
fn palette_toggle_setting_flips_in_place_without_drilling() {
    let mut a = app();
    a.config.dir = std::env::temp_dir().join("lyrfin_palette_toggle");
    let _ = std::fs::remove_dir_all(&a.config.dir);
    let before = a.config.gapless;
    a.update(Action::OpenPalette);
    a.update(Action::PaletteOpenSetting(Setting::Gapless));
    // a plain toggle doesn't open a picker — it flips and the palette closes
    assert!(a.palette.is_none());
    assert_eq!(a.config.gapless, !before);
    let _ = std::fs::remove_dir_all(&a.config.dir);
}

#[test]
fn apply_setting_value_sets_exact_value_and_persists() {
    let mut a = app();
    a.config.dir = std::env::temp_dir().join("lyrfin_apply_value");
    let _ = std::fs::remove_dir_all(&a.config.dir);

    // crossfade is bounded — a chosen value applies exactly
    a.apply_setting_value(Setting::Crossfade, &ChoiceValue::Int(4000));
    assert_eq!(a.config.crossfade_ms, 4000);
    // replaygain is indexed discrete (0 off / 1 track / 2 album)
    a.apply_setting_value(Setting::ReplayGain, &ChoiceValue::Int(2));
    assert_eq!(a.config.replaygain, 2);

    // it hit config.toml (in the temp dir, never the real config)
    let text = std::fs::read_to_string(a.config.dir.join("config.toml")).expect("config written");
    assert!(text.contains("crossfade_ms = 4000"));

    let _ = std::fs::remove_dir_all(&a.config.dir);
}

#[test]
fn sort_parse_directions() {
    use SortField::*;
    assert_eq!(
        parse_sort("artist,album,year,track"),
        vec![
            (Artist, false),
            (Album, false),
            (Year, true),
            (Track, false)
        ],
        "year defaults to descending (latest→old), others ascending"
    );
    assert_eq!(
        parse_sort("title:asc,year:asc"),
        vec![(Title, false), (Year, false)]
    );
    assert_eq!(parse_sort("-album"), vec![(Album, true)]);
    assert!(parse_sort("bogus").is_empty());
}

#[test]
fn sort_command_orders_and_persists_default() {
    let mut a = app();
    a.config.dir = std::env::temp_dir().join("lyrfin_sort_test");

    // the default sort comes from config: artist, year↓, album
    assert_eq!(
        a.sort,
        vec![
            (SortField::Artist, false),
            (SortField::Year, true),
            (SortField::Album, false),
        ]
    );

    let ids = a.player.queue.items.clone();
    let (older, newer) = (ids[0], ids[1]);
    a.library.track_mut(older).unwrap().year = Some(1990);
    a.library.track_mut(newer).unwrap().year = Some(2020);

    // `sort:artist,album,year,track` via the colon form
    assert!(a.run_command("sort:year").starts_with("Sorted by"));
    assert_eq!(a.sort, vec![(SortField::Year, true)]);
    assert_eq!(
        a.sort_ids(vec![older, newer]),
        vec![newer, older],
        "year sorts latest first"
    );

    // `sort:off` reverts to natural order
    a.run_command("sort:off");
    assert!(a.sort.is_empty());
    assert_eq!(a.sort_ids(vec![older, newer]), vec![older, newer]);

    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("lyrfin_sort_test"));
}

#[test]
fn command_line_runs_typed_commands() {
    let mut a = app();
    a.config.dir = std::env::temp_dir().join("lyrfin_cmd_test");

    assert!(a.run_command("theme cyberpunk").contains("cyberpunk"));
    assert_eq!(a.theme.name, "cyberpunk");

    a.run_command("set volume 42");
    assert_eq!(a.player.volume, 42);

    let g0 = a.config.gapless;
    a.run_command("toggle gapless");
    assert_eq!(a.config.gapless, !g0);

    // routed through the unified apply path, so the toast uses the UI's value label
    assert!(a.run_command("replaygain album").contains("album"));
    assert_eq!(a.config.replaygain, 2);

    a.run_command("sleep 20");
    assert!(a.sleep_remaining_secs().is_some());

    a.run_command("repeat one");
    assert_eq!(a.player.repeat, Repeat::One);

    // errors are reported as messages, never panics
    assert!(
        a.run_command("bogus xyz")
            .to_lowercase()
            .contains("unknown")
    );
    assert!(a.run_command("set volume abc").contains("0"));
    assert!(
        a.run_command("theme nope")
            .to_lowercase()
            .contains("no theme")
    );

    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("lyrfin_cmd_test"));
}

#[test]
fn typed_setting_commands_route_through_unified_apply() {
    let mut a = app();
    a.config.dir = std::env::temp_dir().join("lyrfin_cmd_collapse");
    let _ = std::fs::remove_dir_all(&a.config.dir);

    // `set <toggle> on/off` now sets an explicit state (previously only `toggle` flipped)
    a.run_command("set gapless off");
    assert!(!a.config.gapless);
    a.run_command("set gapless on");
    assert!(a.config.gapless);

    // a discrete value matches by label…
    a.run_command("set icons outline");
    assert_eq!(a.config.icon_set, "outline");
    // …and a bounded value clamps to the setting's range
    a.run_command("set crossfade 999999");
    assert_eq!(a.config.crossfade_ms, 12000);

    // an invalid value returns a hint listing the options — no panic, no change
    let before = a.config.replaygain;
    let msg = a.run_command("replaygain bogus");
    assert!(msg.contains("off") && msg.contains("track"), "hint: {msg}");
    assert_eq!(a.config.replaygain, before);

    let _ = std::fs::remove_dir_all(&a.config.dir);
}

#[test]
fn palette_runs_command_and_prefills_templates() {
    let mut a = app();
    a.config.dir = std::env::temp_dir().join("lyrfin_cmd_test2");

    // a typed "verb arg" line runs as a command and closes the palette
    a.update(Action::OpenPalette);
    a.update(Action::PaletteInput("theme glacier".into()));
    a.update(Action::PaletteActivate);
    assert!(a.palette.is_none());
    assert_eq!(a.theme.name, "glacier");

    // a template entry pre-fills the query and keeps the palette open
    a.update(Action::OpenPalette);
    a.update(Action::PalettePrefill("set ".into()));
    assert_eq!(a.palette.as_ref().unwrap().query, "set ");

    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("lyrfin_cmd_test2"));
}

#[test]
fn recently_played_moves_to_top_without_duplicates() {
    let mut a = app();
    let ids: Vec<_> = a.player.queue.items.iter().take(3).copied().collect();
    let (sa, sb, sc) = (ids[0], ids[1], ids[2]); // A, B, C
    // history = [C, B, A]
    a.library.recently_played = vec![sc, sb, sa];

    // replay A → it moves to the top, the old A is dropped: [A, C, B]
    a.record_play(sa);
    assert_eq!(a.library.recently_played, vec![sa, sc, sb]);

    // and never any duplicates
    let mut uniq = a.library.recently_played.clone();
    uniq.sort_by_key(|id| id.get());
    uniq.dedup();
    assert_eq!(
        uniq.len(),
        a.library.recently_played.len(),
        "recently-played has no duplicate tracks"
    );
}

#[test]
fn keybinding_search_filters() {
    let mut a = app();
    a.update(Action::ToggleHelp);
    assert!(matches!(
        a.info.as_ref().map(|i| i.tab),
        Some(crate::app::InfoTab::Keys)
    ));
    let all = a.help_matches().len();

    // typing while help is open routes into the filter
    let key = Key {
        code: KeyCode::Char('v'),
        mods: Mods::default(),
    };
    assert!(matches!(crate::keymap::map(&a, key), Action::HelpInput(ref s) if s == "v"));

    a.update(Action::HelpInput("volume".into()));
    let filtered = a.help_matches();
    assert!(filtered.len() < all, "filter narrows the list");
    assert!(
        filtered.iter().all(
            |(k, d)| k.to_lowercase().contains("volume") || d.to_lowercase().contains("volume")
        )
    );

    // toggling the same tab closes the Info overlay (and drops the filter)
    a.update(Action::ToggleHelp);
    assert!(a.info.is_none());
}

#[test]
fn advanced_query_filters_display_ids() {
    let mut a = app(); // demo: track 1 is the only favorite; 2 tracks by "Tycho"
    a.search.query = "fav".into();
    let fav = a.display_ids();
    assert_eq!(
        fav,
        vec![TrackId::new(1)],
        "structured `fav` filters to favorites"
    );

    a.search.query = "artist:tycho".into();
    let tycho = a.display_ids();
    assert_eq!(tycho.len(), 2, "two Tycho tracks in the demo");
    assert!(
        tycho
            .iter()
            .all(|id| a.library.track(*id).unwrap().artist == "Tycho")
    );

    // a plain-word query is left to fuzzy search (still returns hits)
    a.search.query = "tycho".into();
    assert!(!a.display_ids().is_empty());
}

#[test]
fn random_album_browses_and_plays() {
    let mut a = app();
    a.update(Action::RandomAlbum);
    assert!(!a.browser.list.is_empty(), "browses the picked album");
    assert!(a.player.current.is_some(), "starts playback");
    assert_eq!(
        a.player.queue.items, a.browser.list,
        "queue matches the album"
    );
    // the PRNG always yields an in-range index
    for n in 1..50usize {
        assert!(a.next_rand_below(n) < n);
    }
    assert_eq!(a.next_rand_below(0), 0, "n=0 is handled");
}

#[test]
fn forgotten_and_on_this_day_lists() {
    let mut a = app();
    let now = crate::datetime::now_unix();
    let day = crate::datetime::DAY;
    let (cy, cm, cd) = crate::datetime::ymd_from_unix(now);
    let ids: Vec<_> = a.library.tracks.keys().copied().collect();

    // a track played long ago → Forgotten; a track added on this day a year
    // ago → On This Day; a freshly-played track is in neither.
    let forgotten_id = ids[0];
    let memory_id = ids[1];
    let fresh_id = ids[2];
    {
        let t = a.library.track_mut(forgotten_id).unwrap();
        t.play_count = 3;
        t.last_played = now.saturating_sub(200 * day) as u32;
    }
    {
        let t = a.library.track_mut(memory_id).unwrap();
        t.added_at = (crate::datetime::unix_from_ymd(cy - 1, cm, cd) + 100) as u32;
    }
    {
        let t = a.library.track_mut(fresh_id).unwrap();
        t.play_count = 1;
        t.last_played = now as u32; // played just now → not forgotten
        t.added_at = now as u32; // added today (this year) → not "on this day"
    }

    let forgotten = a.smart_ids(SmartList::Forgotten);
    assert!(forgotten.contains(&forgotten_id), "old play is forgotten");
    assert!(
        !forgotten.contains(&fresh_id),
        "recent play is not forgotten"
    );

    let otd = a.smart_ids(SmartList::OnThisDay);
    assert!(otd.contains(&memory_id), "prior-year same-day add shows up");
    assert!(!otd.contains(&fresh_id), "added-today is not a memory");
}

#[test]
fn rescan_preserves_live_playback() {
    let mut a = app();
    let id = a.player.queue.items[2];
    a.player.current = Some(id);
    a.player.status = Status::Playing;
    a.player.elapsed = Duration::from_secs(42);
    a.loaded_track = Some(id);
    let path = a.library.track(id).unwrap().path.clone();
    // simulate a rescan: same files, fresh TrackIds
    let tracks: Vec<Track> = a.library.tracks.values().cloned().collect();
    a.set_library(tracks);
    let cur = a.player.current.expect("still has a current track");
    assert_eq!(
        a.library.track(cur).unwrap().path,
        path,
        "current track follows the same file across a rescan"
    );
    assert_eq!(a.player.status, Status::Playing, "playback isn't paused");
    assert_eq!(a.player.elapsed, Duration::from_secs(42), "position kept");
    assert_eq!(
        a.loaded_track,
        Some(cur),
        "loaded track re-pinned to the new id (no reload)"
    );
}

#[test]
fn genres_section_lists_and_drills_in() {
    let mut a = app(); // demo: every track genre "Synthwave"

    // the Genres section lists one genre row (a drillable container)
    a.local.section = LocalSection::Genres;
    a.local_load_section();
    assert_eq!(a.local.items.len(), 1, "one genre in the demo library");
    assert!(
        matches!(&a.local.items[0], LocalItem::Genre(g) if g == "Synthwave"),
        "the row is the Synthwave genre"
    );

    // drilling into it shows that genre's tracks (every demo track)
    let genre = a.local.items[0].clone();
    a.local_open(genre);
    assert!(a.local.crumb.as_deref() == Some("⊞ Synthwave"));
    assert_eq!(
        a.local.items.len(),
        a.library.all_tracks_sorted().len(),
        "all Synthwave tracks under the genre"
    );
    assert!(
        a.local.items.iter().all(|it| it.is_track()),
        "the drilled-in rows are tracks"
    );
    // Esc pops back to the section level
    assert!(a.local_back());
    assert!(a.local.crumb.is_none());
    assert!(matches!(&a.local.items[0], LocalItem::Genre(_)));
}
