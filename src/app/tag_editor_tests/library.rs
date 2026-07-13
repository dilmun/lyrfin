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
    assert!(matches.iter().any(|&i| entries[i].1.contains("theme")));
    a.update(Action::PaletteActivate); // runs top match + closes
    assert!(a.palette.is_none());
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

    assert!(a.run_command("replaygain album").contains("Album"));
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
