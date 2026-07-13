//! Spotify view snapshot/behaviour tests, split out of spotify_tests.rs.
//! A child of `tests`, so `use super::*` pulls in the shared demo()/render_layout.

use super::*;

#[test]
fn spotify_now_bar_shows_track_art_meta_like_local() {
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:abc".into(),
        name: "Shape of You".into(),
        subtitle: "Ed Sheeran".into(),
        album: "÷ (Divide)".into(),
        image: None,
        kind: Kind::Track,
        duration_ms: 240_000,
        artist_uri: None,
        ..Default::default()
    });
    a.spov.sp_dur = 240.0;
    a.spov.sp_pos = 42.0;
    a.spov.spotify_paused = true; // not buffering → the time shows
    a.spov.sp_saved = true;
    let s = render_layout(&mut a, Layout::Spotify, 120, 36);
    assert!(s.contains("Shape of You"), "title in the now-bar");
    assert!(s.contains("Ed Sheeran"), "artist in the now-bar");
    assert!(s.contains('♥'), "liked heart shows next to the title");
    assert!(s.contains("0:42"), "elapsed time under the bar");
    assert!(s.contains("4:00"), "total time under the bar");
}

#[test]
fn spotify_browser_uses_columnar_tracklist() {
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = crate::spotify::ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    a.config.track_columns = true; // this test pins the column-table layout
    a.config.columns.artist = true;
    a.config.columns.album = true;
    a.config.columns.time = true;
    let mk = |name: &str, artist: &str, album: &str| Item {
        uri: format!("spotify:track:{name}"),
        name: name.into(),
        subtitle: artist.into(),
        album: album.into(),
        image: None,
        kind: Kind::Track,
        duration_ms: 200_000,
        artist_uri: None,
        ..Default::default()
    };
    a.spotify.items = vec![
        mk("Shape of You", "Ed Sheeran", "Divide"),
        mk("Perfect", "Ed Sheeran", "Divide"),
    ];
    a.spotify.crumb = Some("◉ Divide".into());
    let s = render_layout(&mut a, Layout::Spotify, 120, 36);
    // column headers (columnar table, not the old "Title — Artist" inline)
    assert!(s.contains("TITLE"), "TITLE column header");
    assert!(s.contains("ARTIST"), "ARTIST column header");
    assert!(s.contains("ALBUM"), "ALBUM column header");
    assert!(
        s.contains("Shape of You") && s.contains("Divide"),
        "rows render"
    );
    assert!(s.contains("LIBRARY"), "bordered sidebar pane present");
}

#[test]
fn spotify_main_list_track_layout_toggles_rows_and_columns() {
    // Spotify's all-tracks list shares the unified layout: the column table by
    // default (like local), compact rows when `track_columns` is off.
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = crate::spotify::ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    a.spotify.crumb = Some("◉ Divide".into());
    a.spotify.items = vec![Item {
        name: "Shape of You".into(),
        subtitle: "Ed Sheeran".into(),
        album: "Divide".into(),
        year: Some(2017),
        duration_ms: 200_000,
        kind: Kind::Track, // no Group ⇒ the all-tracks (not artist-page) branch
        ..Default::default()
    }];
    // rows: track + artist · album meta on one line, no column header
    a.config.track_columns = false;
    let rows = render_layout(&mut a, Layout::Spotify, 120, 30);
    assert!(
        rows.contains("Shape of You") && rows.contains("Ed Sheeran") && rows.contains("Divide"),
        "the rows layout shows the track + artist · album meta"
    );
    assert!(
        !rows.contains("TITLE"),
        "the rows layout has no column header"
    );
    // flip to the column table (the default)
    a.config.track_columns = true;
    let cols = render_layout(&mut a, Layout::Spotify, 120, 30);
    assert!(
        cols.contains("TITLE") && cols.contains("ARTIST"),
        "the column header appears when track_columns is on"
    );
    assert!(cols.contains("Shape of You"), "the track still renders");
}

#[test]
fn spotify_tracklist_marks_the_now_playing_row() {
    // The middle-pane track list marks the currently-playing Spotify track with a
    // ▶ marker (matched by uri against `now_spotify`), like the local tracklist and
    // the queue — so what's playing is visible while browsing, not only in the
    // queue pane. Both layouts (compact rows + column table) mark it.
    use crate::spotify::api::{Item, Kind};
    let mk = |name: &str| Item {
        uri: format!("spotify:track:{name}"),
        name: name.into(),
        subtitle: "GAYLE".into(),
        album: name.into(),
        duration_ms: 139_000,
        kind: Kind::Track,
        ..Default::default()
    };
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = crate::spotify::ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    a.spotify.crumb = Some("≡ New Music Friday".into());
    a.spotify.items = vec![mk("Danceteria"), mk("junebug"), mk("THE CINEMA")];
    // playing (not paused): the transport shows ⏸, so the only ▶ on screen is the
    // tracklist's now-playing marker — the *paused* transport button is also a ▶.
    a.spov.spotify_paused = false;

    // a track that isn't in this list → no row is marked (proves uri matching, and
    // that no chrome renders a stray ▶ to confound the positive checks below)
    a.spov.now_spotify = Some(mk("something else"));
    a.config.track_columns = false;
    let miss = render_layout(&mut a, Layout::Spotify, 120, 30);
    assert!(
        !miss.contains('▶'),
        "no row is marked when the playing track isn't in the visible list"
    );

    // play a track that IS in the list → its row gets the ▶ marker (compact rows)
    a.spov.now_spotify = Some(mk("junebug"));
    let rows = render_layout(&mut a, Layout::Spotify, 120, 30);
    assert!(
        rows.contains('▶'),
        "the rows layout marks the now-playing track"
    );

    // the column table marks it too (▶ replaces the row number in the # column)
    a.config.track_columns = true;
    a.config.columns.index = true;
    let cols = render_layout(&mut a, Layout::Spotify, 120, 30);
    assert!(
        cols.contains('▶'),
        "the column table marks the now-playing track"
    );
}

#[test]
fn spotify_discovery_sections_present_and_loadable() {
    use crate::spotify::ConnState;
    use crate::spotify::Tokens;
    use crate::spotify::api::{Section, SpRequest};
    assert!(Section::ALL.contains(&Section::RecentlyPlayed));
    assert!(Section::ALL.contains(&Section::TopTracks));

    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    let s = render_layout(&mut a, Layout::Spotify, 120, 36);
    assert!(
        s.contains("Recently Played"),
        "recently-played in the sidebar"
    );
    assert!(s.contains("Top Tracks"), "top-tracks in the sidebar");

    // selecting one loads it from the matching endpoint
    let (tx, rx) = crossbeam_channel::unbounded::<SpRequest>();
    a.set_spotify_sender(tx);
    a.spotify.tokens = Some(Tokens {
        access_token: "t".into(),
        refresh_token: "r".into(),
        expires_at: 0,
        scopes: String::new(),
    });
    a.spotify.section = Section::RecentlyPlayed;
    a.spotify_load_section();
    assert!(
        matches!(
            rx.try_recv(),
            Ok(SpRequest::Library {
                section: Section::RecentlyPlayed,
                ..
            })
        ),
        "Recently Played triggers a Library load for that section"
    );
}

#[test]
fn spotify_search_groups_results_into_songs_list_and_card_carousels() {
    use crate::spotify::ConnState;
    use crate::spotify::api::{Item, Kind};
    let mk = |name: &str, kind: Kind| Item {
        uri: format!("spotify:x:{name}"),
        name: name.into(),
        subtitle: "x".into(),
        album: String::new(),
        image: None,
        kind,
        duration_ms: 0,
        artist_uri: None,
        ..Default::default()
    };
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    a.spotify.in_search = true;
    a.spotify.query = "x".into();
    // mixed kinds (not all-tracks) → SONGS list on top + card carousels below
    a.spotify.items = vec![
        mk("t1", Kind::Track),
        mk("al1", Kind::Album),
        mk("ar1", Kind::Artist),
        mk("pl1", Kind::Playlist),
        mk("sh1", Kind::Show), // a podcast show → its own PODCASTS carousel
    ];
    // search routes through the shared "track list + carousels" layout
    assert_eq!(
        a.spotify_carousels_from(),
        Some(1),
        "the leading SONGS end at the first non-track"
    );
    // tall enough that every card carousel fits (they're much taller than list rows)
    let s = render_layout(&mut a, Layout::Spotify, 120, 70);
    for header in ["SONGS", "ALBUMS", "ARTISTS", "PLAYLISTS", "PODCASTS"] {
        assert!(s.contains(header), "search shows the {header} header");
    }
    assert!(s.contains("t1"), "the song renders in the SONGS list");
    assert!(s.contains("al1"), "an album renders as a carousel card");
}

#[test]
fn spotify_item_meta_composes_per_type() {
    use crate::spotify::api::{Item, Kind};
    use crate::ui::components::item_meta;
    let track = Item {
        subtitle: "Artist".into(),
        album: "Alb".into(),
        year: Some(2023),
        kind: Kind::Track,
        ..Default::default()
    };
    assert_eq!(item_meta(&track), "Artist  ·  Alb  ·  2023");
    let album = Item {
        subtitle: "Artist".into(),
        year: Some(2022),
        kind: Kind::Album,
        ..Default::default()
    };
    assert_eq!(item_meta(&album), "Artist  ·  2022");
    let artist = Item {
        subtitle: String::new(),
        followers: Some(1000),
        kind: Kind::Artist,
        ..Default::default()
    };
    assert_eq!(item_meta(&artist), "", "artist = just name (followers off)");
    let pl = Item {
        subtitle: "Owner".into(),
        count: Some(12),
        kind: Kind::Playlist,
        ..Default::default()
    };
    assert_eq!(item_meta(&pl), "Owner  ·  12 tracks");
}

#[test]
fn spotify_search_input_lives_in_the_pane_border() {
    use crate::spotify::ConnState;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    a.spotify.searching = true;
    a.spotify.query = "radiohead".into();
    let s = render_layout(&mut a, Layout::Spotify, 120, 36);
    // the query is shown inline in the SPOTIFY pane's top border, led by a search
    // indicator (the magnifier when idle) — no separate row, no placeholder
    assert!(
        s.contains("SPOTIFY") && s.contains("⌕ radiohead"),
        "search input embedded in the SPOTIFY border with a magnifier indicator"
    );
}

#[test]
fn spotify_views_shape_arabic() {
    use crate::spotify::ConnState;
    use crate::spotify::api::{Item, Kind};
    let arabic = "رحمة رياض";
    // helper: reshapes/reorders when on + Arabic; a no-op otherwise
    assert_ne!(
        crate::arabic::shaped(arabic, true),
        arabic,
        "shaping changes Arabic"
    );
    assert_eq!(
        crate::arabic::shaped(arabic, false),
        arabic,
        "off → unchanged"
    );
    assert_eq!(
        crate::arabic::shaped("Rahma Riad", true),
        "Rahma Riad",
        "latin unchanged"
    );

    // the Spotify columnar tracklist renders the SHAPED form (not raw logical)
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.config.arabic_shaping = true;
    a.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    a.spotify.items = vec![Item {
        uri: "spotify:track:x".into(),
        name: arabic.into(),
        subtitle: "x".into(),
        album: String::new(),
        image: None,
        kind: Kind::Track,
        duration_ms: 1000,
        artist_uri: None,
        ..Default::default()
    }];
    // a drill-in crumb (distinct from the row name) drives the pane title — it must
    // be shaped too, otherwise an RTL playlist/album name shows reversed in the header
    let crumb = "أغاني الحب";
    a.spotify.crumb = Some(crumb.into());
    let s = render_layout(&mut a, Layout::Spotify, 120, 36);
    assert!(
        s.contains(&crate::arabic::shaped(arabic, true)),
        "tracklist shows shaped Arabic"
    );
    assert!(
        s.contains(&crate::arabic::shaped(&crumb.to_uppercase(), true)),
        "drill-in header title shows shaped Arabic (not raw/reversed)"
    );
}

#[test]
fn spotify_has_a_podcasts_section() {
    use crate::spotify::ConnState;
    use crate::spotify::api::Section;
    assert!(
        Section::ALL.contains(&Section::Podcasts),
        "Podcasts is a library section"
    );
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    let s = render_layout(&mut a, Layout::Spotify, 120, 36);
    assert!(s.contains("Podcasts"), "Podcasts shows in the sidebar");
}

#[test]
fn empty_podcasts_section_explains_region_availability() {
    // an empty Podcasts list shows the helpful region message, not "Empty"
    use crate::spotify::ConnState;
    use crate::spotify::api::Section;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    a.spotify.section = Section::Podcasts;
    a.spotify.items.clear();
    a.spotify.loading = false;
    let s = render_layout(&mut a, Layout::Spotify, 120, 36);
    assert!(s.contains("No podcasts here"), "friendly empty-state title");
    assert!(
        s.contains("region") && s.contains("countries"),
        "explains the country/region availability limitation"
    );
}

#[test]
fn podcasts_section_leads_with_top_podcasts_hub_entry() {
    // The Podcasts tab doubles as the podcast hub: the saved-shows list is led by a
    // `Kind::Category` entry into Spotify's editorial Top-Podcasts + categories browse
    // page, drilled via the same pathfinder path as the music Browse grid.
    use crate::spotify::api::{Item, Kind, Section, SpResult};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.section = Section::Podcasts;
    a.on_spotify_result(SpResult::Library {
        key: String::new(),
        items: vec![Item {
            uri: "spotify:show:1".into(),
            name: "My Saved Show".into(),
            kind: Kind::Show,
            ..Default::default()
        }],
    });
    assert_eq!(
        a.spotify.items.len(),
        2,
        "hub entry prepended to saved shows"
    );
    assert_eq!(
        a.spotify.items[0].kind,
        Kind::Category,
        "the hub entry is a browse-page category tile"
    );
    assert_eq!(
        a.spotify.items[0].uri,
        crate::spotify::pathfinder::PODCASTS_BROWSE_ROOT,
        "it points at the podcast browse page"
    );
    assert_eq!(
        a.spotify.items[1].name, "My Saved Show",
        "the user's saved shows follow the hub entry"
    );

    // other library sections are untouched — no hub entry leaks in
    a.spotify.section = Section::Albums;
    a.on_spotify_result(SpResult::Library {
        key: String::new(),
        items: vec![Item {
            uri: "spotify:album:1".into(),
            name: "An Album".into(),
            kind: Kind::Album,
            ..Default::default()
        }],
    });
    assert!(
        a.spotify
            .items
            .iter()
            .all(|i| i.uri != crate::spotify::pathfinder::PODCASTS_BROWSE_ROOT),
        "the hub entry is Podcasts-only"
    );
}

#[test]
fn drilled_podcast_charts_render_as_a_grid() {
    use crate::spotify::api::{Item, Kind, Section};
    let show = |n: &str| Item {
        uri: format!("spotify:show:{n}"),
        name: n.into(),
        kind: Kind::Show,
        ..Default::default()
    };
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.section = Section::Podcasts;
    a.spotify.grid = true;
    a.spotify.crumb = Some("▦ Podcast Charts".into());
    a.spotify.items = vec![show("1"), show("2")];
    assert!(
        a.spotify_podcast_grid_active(),
        "a drilled-in flat show list renders as a grid"
    );

    // follows the grid/list toggle
    a.spotify.grid = false;
    assert!(
        !a.spotify_podcast_grid_active(),
        "toggling to list turns the grid off"
    );

    // a sectioned page (shelf-tagged items) stays carousels, not a flat grid
    a.spotify.grid = true;
    a.spotify.items[0].section = Some("Popular".into());
    assert!(
        !a.spotify_podcast_grid_active(),
        "sectioned shelves stay carousels"
    );

    // the top-level saved-shows list (no crumb) is not a grid
    a.spotify.items[0].section = None;
    a.spotify.crumb = None;
    assert!(
        !a.spotify_podcast_grid_active(),
        "the top-level saved-shows list stays a list"
    );
}

#[test]
fn all_categories_page_is_always_a_grid_even_in_list_mode() {
    use crate::spotify::api::{Item, Kind, Section};
    let cat = |n: &str| Item {
        uri: format!("spotify:page:{n}"),
        name: n.into(),
        kind: Kind::Category,
        ..Default::default()
    };
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.section = Section::Podcasts;
    a.spotify.grid = false; // list-mode toggle is off
    a.spotify.crumb = Some("▦ Browse all categories".into());
    a.spotify.items = vec![cat("comedy"), cat("news"), cat("sports")];
    assert!(
        a.spotify_podcast_grid_active(),
        "a page of category tiles is a grid regardless of the grid/list toggle"
    );
}

#[test]
fn browse_grid_load_more_appends_and_detects_exhaustion() {
    use crate::spotify::api::{Item, Kind, Section};
    use crate::spotify::session::SessionEvent;
    let show = |n: usize| Item {
        uri: format!("spotify:show:{n}"),
        name: format!("Show {n}"),
        kind: Kind::Show,
        ..Default::default()
    };
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.section = Section::Podcasts;
    a.spotify.crumb = Some("▦ Podcast Charts".into());
    a.spotify.items = (0..50).map(show).collect();
    a.spotify.browse_page = Some("spotify:page:charts".into());
    a.spotify.browse_limit = 100; // a grow to 100 is in flight
    a.spotify.browse_loading_more = true;

    // the grow returns 80 items (fewer than the 100 asked) → appended, then marked
    // exhausted so scrolling stops re-asking. The empty key matches a fresh state.
    let (etx, erx) = crossbeam_channel::unbounded::<SessionEvent>();
    etx.send(SessionEvent::Browse {
        key: String::new(),
        items: (0..80).map(show).collect(),
        error: None,
    })
    .unwrap();
    a.spov.session_rx = Some(erx);
    a.pump_spotify_session();

    assert_eq!(
        a.spotify.items.len(),
        80,
        "the grid grew with the new batch"
    );
    assert!(!a.spotify.browse_loading_more, "the in-flight grow cleared");
    assert!(
        a.spotify.browse_exhausted,
        "a short batch (< the limit asked) means fully loaded"
    );
}

#[test]
fn followed_shows_cache_tracks_library_and_toggles() {
    use crate::spotify::api::{Item, Kind, Section, SpResult};
    let show = |n: &str| Item {
        uri: format!("spotify:show:{n}"),
        name: n.into(),
        kind: Kind::Show,
        ..Default::default()
    };
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.section = Section::Podcasts;
    // the saved-shows load seeds the followed set (these ARE the user's follows)
    a.on_spotify_result(SpResult::Library {
        key: String::new(),
        items: vec![show("1"), show("2")],
    });
    assert!(a.spotify.followed_shows.contains("spotify:show:1"));
    assert!(a.spotify.followed_shows.contains("spotify:show:2"));

    // a follow toggle updates the set live, before any list refresh
    a.on_spotify_result(SpResult::Follow {
        uri: "spotify:show:9".into(),
        followed: true,
    });
    assert!(
        a.spotify.followed_shows.contains("spotify:show:9"),
        "following adds it"
    );
    a.on_spotify_result(SpResult::Follow {
        uri: "spotify:show:1".into(),
        followed: false,
    });
    assert!(
        !a.spotify.followed_shows.contains("spotify:show:1"),
        "unfollowing removes it"
    );
}

#[test]
fn empty_podcast_hub_shows_a_region_note_not_a_stray_button() {
    use crate::spotify::api::{Item, Kind, Section};
    use crate::spotify::session::SessionEvent;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.section = Section::Podcasts;
    a.spotify.crumb = Some("▦ Browse Top Podcasts".into());

    // a region without podcasts returns only the "Browse all categories" link
    let (etx, erx) = crossbeam_channel::unbounded::<SessionEvent>();
    etx.send(SessionEvent::Browse {
        key: String::new(),
        items: vec![Item {
            uri: "spotify:page:all".into(),
            name: crate::spotify::pathfinder::ALL_CATEGORIES_LABEL.into(),
            kind: Kind::Category,
            ..Default::default()
        }],
        error: None,
    })
    .unwrap();
    a.spov.session_rx = Some(erx);
    a.pump_spotify_session();

    assert!(
        a.spotify.items.is_empty(),
        "the lone browse-all link is treated as an empty hub"
    );
    assert!(
        a.spotify.note.to_lowercase().contains("region"),
        "shows a clear region note instead: {:?}",
        a.spotify.note
    );
}

#[test]
fn hub_categories_render_as_a_flat_grid_not_a_carousel() {
    use crate::spotify::api::{Item, Kind};
    let show = |n: &str| Item {
        uri: format!("spotify:show:{n}"),
        name: n.into(),
        kind: Kind::Show,
        section: Some("New Show Releases".into()),
        ..Default::default()
    };
    let cat = |n: &str| Item {
        uri: format!("spotify:page:{n}"),
        name: n.into(),
        kind: Kind::Category,
        ..Default::default() // section None → the category shelf
    };
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.cols.set(3); // 3 columns
    a.spotify.items = vec![
        show("s1"),
        show("s2"),
        cat("c1"),
        cat("c2"),
        cat("c3"),
        cat("c4"),
        cat("c5"),
    ];
    // the show shelf stays one carousel; the 5 category tiles wrap into 3-wide grid
    // rows (3 + 2) instead of a single horizontal carousel
    let card_row_lens: Vec<usize> = a
        .spotify_browse_rows()
        .iter()
        .filter_map(|r| r.cards().map(<[usize]>::len))
        .collect();
    assert_eq!(
        card_row_lens,
        vec![2, 3, 2],
        "shows = 1 carousel of 2; categories = grid rows of 3 then 2"
    );
}

#[test]
fn hub_categories_grid_has_a_browse_all_button_row() {
    use crate::app::ReleaseRow;
    use crate::spotify::api::{Item, Kind};
    let cat = |n: &str| Item {
        uri: format!("spotify:page:{n}"),
        name: n.into(),
        kind: Kind::Category,
        ..Default::default()
    };
    let button = Item {
        uri: "spotify:page:all".into(),
        name: crate::spotify::pathfinder::ALL_CATEGORIES_LABEL.into(),
        kind: Kind::Category,
        ..Default::default()
    };
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.cols.set(3);
    a.spotify.items = vec![cat("c1"), cat("c2"), cat("c3"), cat("c4"), button];
    let rows = a.spotify_browse_rows();

    // the browse-all button (last item, index 4) is its own Banner row
    let banners: Vec<usize> = rows
        .iter()
        .filter_map(|r| match r {
            ReleaseRow::Banner(i) => Some(*i),
            _ => None,
        })
        .collect();
    assert_eq!(
        banners,
        vec![4],
        "the browse-all button is a Banner, not a tile"
    );

    // the 4 real category tiles wrap into cols-wide Cards rows, button excluded
    let tiles: usize = rows
        .iter()
        .filter_map(|r| match r {
            ReleaseRow::Cards(c) => Some(c.len()),
            _ => None,
        })
        .sum();
    assert_eq!(
        tiles, 4,
        "the 4 real categories tile up; the button isn't among them"
    );
}

#[test]
fn spotify_show_drills_into_episodes() {
    use crate::spotify::Tokens;
    use crate::spotify::api::{Item, Kind, SpRequest};
    let mut a = demo();
    a.layout = Layout::Spotify;
    // wire the worker while tokens are None (no resume), then add tokens
    let (tx, rx) = crossbeam_channel::unbounded::<SpRequest>();
    a.set_spotify_sender(tx);
    a.spotify.tokens = Some(Tokens {
        access_token: "t".into(),
        refresh_token: "r".into(),
        expires_at: 0,
        scopes: String::new(),
    });
    a.focus = crate::app::Focus::Main;
    a.spotify.items = vec![Item {
        uri: "spotify:show:1".into(),
        name: "My Show".into(),
        subtitle: "Publisher".into(),
        kind: Kind::Show,
        ..Default::default()
    }];
    a.spotify.sel = 0;

    a.spotify_activate(); // Enter on a show → drill into its episodes
    assert!(a.spotify.crumb.is_some(), "drilled into the show");
    assert!(
        matches!(rx.try_recv(), Ok(SpRequest::Open { kind: Kind::Show, uri, .. }) if uri == "spotify:show:1"),
        "fetches the show's episodes over the Web API"
    );
}

#[test]
fn enter_on_focused_spotify_artist_pane_opens_the_artist_page() {
    use crate::app::{Focus, Panel};
    use crate::spotify::Tokens;
    use crate::spotify::api::Item;
    use crate::spotify::session::SessionCommand;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.tokens = Some(Tokens {
        access_token: "t".into(),
        refresh_token: "r".into(),
        expires_at: u64::MAX, // valid → the session is reused, not respawned
        scopes: String::new(),
    });
    // a preset session channel so spotify_ensure_session reuses it instead of
    // spawning a real librespot thread; we read the command it emits
    let (cmd_tx, cmd_rx) = crossbeam_channel::unbounded::<SessionCommand>();
    a.spov.session_cmd = Some(cmd_tx);
    // now playing: the pane describes the track's primary artist
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:1".into(),
        name: "Perfect".into(),
        subtitle: "Ed Sheeran".into(),
        artist_uri: Some("spotify:artist:eds".into()),
        ..Default::default()
    });

    a.focus = Focus::Pane(Panel::Artist);
    a.spotify_activate(); // ⏎ on the Artist pane

    assert_eq!(
        a.spotify.crumb.as_deref(),
        Some("☻ Ed Sheeran"),
        "drilled into the now-playing artist's page"
    );
    assert_eq!(a.focus, Focus::Main, "focus moved onto the opened page");
    assert!(
        matches!(cmd_rx.try_recv(), Ok(SessionCommand::FetchArtistPage { uri, .. }) if uri == "spotify:artist:eds"),
        "fetches that artist's grouped page via the session"
    );
}

#[test]
fn enter_on_spotify_artist_pane_opens_the_show_for_a_podcast() {
    use crate::app::{Focus, Panel};
    use crate::spotify::Tokens;
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.tokens = Some(Tokens {
        access_token: "t".into(),
        refresh_token: "r".into(),
        expires_at: u64::MAX,
        scopes: String::new(),
    });
    // now playing: a podcast episode — no artist, but it knows its show
    a.spov.now_spotify = Some(Item {
        uri: "spotify:episode:1".into(),
        name: "#2517 - Taylor Sheridan".into(),
        album: "The Joe Rogan Experience".into(),
        show_uri: Some("spotify:show:jre".into()),
        kind: Kind::Track,
        ..Default::default()
    });

    a.focus = Focus::Pane(Panel::Artist);
    a.spotify_activate(); // ⏎ on the Artist pane

    assert_eq!(
        a.spotify.crumb.as_deref(),
        Some("▣ The Joe Rogan Experience"),
        "a podcast episode drills into its show's page"
    );
    assert_eq!(
        a.spotify
            .open_item
            .as_ref()
            .map(|i| (i.uri.as_str(), i.kind)),
        Some(("spotify:show:jre", Kind::Show)),
        "opens the show container, not an artist"
    );
    assert_eq!(a.focus, Focus::Main, "focus moved onto the opened page");
}

#[test]
fn enter_on_podcast_pane_without_show_uri_resolves_the_show() {
    use crate::app::{Focus, Panel};
    use crate::spotify::Tokens;
    use crate::spotify::api::{Item, Kind, SpRequest};
    let mut a = demo();
    a.layout = Layout::Spotify;
    let (tx, rx) = crossbeam_channel::unbounded::<SpRequest>();
    a.set_spotify_sender(tx); // tokens still None → no auto-resume spawn
    a.spotify.tokens = Some(Tokens {
        access_token: "t".into(),
        refresh_token: "r".into(),
        expires_at: u64::MAX,
        scopes: String::new(),
    });
    // an episode restored from an older session: no show_uri to open directly
    a.spov.now_spotify = Some(Item {
        uri: "spotify:episode:1".into(),
        name: "#2517 - Taylor Sheridan".into(),
        album: "The Joe Rogan Experience".into(),
        show_uri: None,
        kind: Kind::Track,
        ..Default::default()
    });

    a.focus = Focus::Pane(Panel::Artist);
    a.spotify_activate(); // ⏎ on the Artist pane

    assert!(
        matches!(
            rx.try_recv(),
            Ok(SpRequest::ResolveShow { episode_uri, name, .. })
                if episode_uri == "spotify:episode:1" && name == "The Joe Rogan Experience"
        ),
        "an episode without show_uri asks the Web API to resolve its show"
    );
}

#[test]
fn enter_on_spotify_artist_pane_is_a_noop_without_an_artist_uri() {
    use crate::app::{Focus, Panel};
    use crate::spotify::api::Item;
    let mut a = demo();
    a.layout = Layout::Spotify;
    // a now-playing track that carries no artist URI (e.g. a local/odd source)
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:1".into(),
        name: "Perfect".into(),
        subtitle: "Ed Sheeran".into(),
        artist_uri: None,
        ..Default::default()
    });
    a.focus = Focus::Pane(Panel::Artist);
    a.spotify_activate();
    assert!(
        a.spotify.crumb.is_none(),
        "no drill-in without a resolvable artist URI"
    );
}

#[test]
fn spotify_artist_pane_shows_bio() {
    use crate::app::{Dock, Panel};
    use crate::artistinfo::{ArtistInfo, InfoRequest, InfoResult};
    use crate::spotify::ConnState;
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    set_panel(&mut a, Layout::Spotify, Panel::Artist, true, Dock::Right);
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:x".into(),
        name: "Song".into(),
        subtitle: "The Band".into(),
        album: "Al".into(),
        image: None,
        kind: Kind::Track,
        duration_ms: 1000,
        artist_uri: Some("spotify:artist:1".into()),
        ..Default::default()
    });
    // wiring the info worker requests the bio for the Spotify artist by name
    let (tx, rx) = crossbeam_channel::unbounded::<InfoRequest>();
    a.set_info_sender(tx);
    let req = rx.try_recv().expect("requested artist info");
    assert_eq!(
        req.artist, "The Band",
        "bio fetched by the Spotify artist name"
    );
    // the bio arrives → adopted + rendered in the pane
    a.on_info_result(InfoResult {
        artist: "The Band".into(),
        info: Some(ArtistInfo {
            name: "The Band".into(),
            bio: "A legendary act from Baghdad.".into(),
            genre: None,
            formed: Some("2010".into()),
            country: Some("Iraq".into()),
            style: None,
        }),
    });
    assert!(a.meta.artist_info.is_some());
    let s = render_layout(&mut a, Layout::Spotify, 140, 36);
    assert!(s.contains("legendary"), "bio text shows in the artist pane");
    assert!(s.contains("Iraq"), "country shows in the artist pane");
}

#[test]
fn artist_bio_does_not_bleed_across_local_and_spotify() {
    // Regression: both artist panes read the single shared `meta.artist_info`.
    // Switching the view must re-target that slot at the now-active source's
    // artist, so the previous source's bio can't linger in the other pane.
    use crate::app::{Dock, Panel};
    use crate::artistinfo::{ArtistInfo, InfoRequest, InfoResult};
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    // both source panes visible; a local track + a Spotify track, different artists
    set_panel(&mut a, Layout::Dashboard, Panel::Artist, true, Dock::Right);
    set_panel(&mut a, Layout::Spotify, Panel::Artist, true, Dock::Right);
    a.player.current = Some(a.library.tracks.values().next().unwrap().id); // album_artist "Neon District"
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:x".into(),
        name: "Song".into(),
        subtitle: "The Band".into(),
        album: "Al".into(),
        kind: Kind::Track,
        duration_ms: 1000,
        artist_uri: Some("spotify:artist:1".into()),
        ..Default::default()
    });

    // start in the Spotify view; wire the worker and load the Spotify artist's bio
    a.layout = Layout::Spotify;
    let (tx, rx) = crossbeam_channel::unbounded::<InfoRequest>();
    a.set_info_sender(tx);
    assert_eq!(rx.try_recv().unwrap().artist, "The Band");
    a.on_info_result(InfoResult {
        artist: "The Band".into(),
        info: Some(ArtistInfo {
            bio: "Spotify artist bio.".into(),
            ..Default::default()
        }),
    });
    assert!(a.meta.artist_info.is_some(), "Spotify bio loaded");

    // switch to local → the stale Spotify bio is cleared and the local artist's
    // bio is requested instead (no bleed into the local pane)
    a.set_layout(Layout::Dashboard);
    assert!(
        a.meta.artist_info.is_none(),
        "Spotify bio cleared on the switch to local"
    );
    assert_eq!(a.meta.info_artist.as_deref(), Some("Neon District"));
    assert!(a.meta.info_pending);
    assert_eq!(
        rx.try_recv().unwrap().artist,
        "Neon District",
        "local artist bio requested after the switch"
    );

    // load the local bio, switch back → it must not bleed into the Spotify pane
    a.on_info_result(InfoResult {
        artist: "Neon District".into(),
        info: Some(ArtistInfo {
            bio: "Local artist bio.".into(),
            ..Default::default()
        }),
    });
    assert!(a.meta.artist_info.is_some(), "local bio loaded");
    a.set_layout(Layout::Spotify);
    assert!(
        a.meta.artist_info.is_none(),
        "local bio cleared on the switch back to Spotify"
    );
    assert_eq!(a.meta.info_artist.as_deref(), Some("The Band"));
    assert_eq!(rx.try_recv().unwrap().artist, "The Band");
}

#[test]
fn spotify_pane_never_renders_the_local_artists_bio() {
    // Render-level guard: even if the shared `meta.artist_info` slot still holds a
    // bio fetched for the local pane's artist, the Spotify pane must not show it.
    use crate::app::{Dock, Panel};
    use crate::artistinfo::ArtistInfo;
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    set_panel(&mut a, Layout::Spotify, Panel::Artist, true, Dock::Right);
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:x".into(),
        name: "Song".into(),
        subtitle: "The Band".into(),
        album: "Al".into(),
        kind: Kind::Track,
        duration_ms: 1000,
        artist_uri: Some("spotify:artist:1".into()),
        ..Default::default()
    });
    // a stale bio left over from the local pane (a *different* artist)
    a.meta.info_artist = Some("Neon District".into());
    a.meta.artist_info = Some(ArtistInfo {
        bio: "LOCALBIOMARKER".into(),
        ..Default::default()
    });
    let s = render_layout(&mut a, Layout::Spotify, 140, 36);
    assert!(
        !s.contains("LOCALBIOMARKER"),
        "the local artist's bio must not bleed into the Spotify pane"
    );
}

#[test]
fn local_pane_never_renders_the_spotify_artists_bio() {
    // The mirror guard: a bio fetched for the Spotify pane must not show in the
    // local artist pane.
    use crate::app::{Dock, Panel};
    use crate::artistinfo::ArtistInfo;
    let mut a = demo();
    set_panel(&mut a, Layout::Dashboard, Panel::Artist, true, Dock::Right);
    a.player.current = Some(a.library.tracks.values().next().unwrap().id); // album_artist "Neon District"
    // a stale bio left over from the Spotify pane (a *different* artist)
    a.meta.info_artist = Some("The Band".into());
    a.meta.artist_info = Some(ArtistInfo {
        bio: "SPOTIFYBIOMARKER".into(),
        ..Default::default()
    });
    let s = render_layout(&mut a, Layout::Dashboard, 140, 36);
    assert!(
        !s.contains("SPOTIFYBIOMARKER"),
        "the Spotify artist's bio must not bleed into the local pane"
    );
}

// Build the paused-in-background Spotify track used by the lyrics-bleed tests.
fn paused_spotify_track() -> crate::spotify::api::Item {
    use crate::spotify::api::{Item, Kind};
    Item {
        uri: "spotify:track:x".into(),
        name: "Song".into(),
        subtitle: "The Band".into(),
        kind: Kind::Track,
        duration_ms: 1000,
        ..Default::default()
    }
}

#[test]
fn spotify_pane_never_renders_the_local_tracks_lyrics() {
    // Regression (the real repro): starting a local track only *pauses* the Spotify
    // overlay (`pause_spotify_overlay`), so `now_spotify` stays Some. Local and
    // Spotify lyrics share one `meta.lyrics` slot, so the Spotify LYRICS pane must
    // still refuse the local track's lyrics — gated by the source the slot loaded for.
    use crate::app::{Dock, LyricsPane, Panel};
    use crate::spotify::ConnState;
    use std::time::Duration;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    set_panel(&mut a, Layout::Spotify, Panel::Lyrics, true, Dock::Right);
    // a local track is the active/playing source, a Spotify track lingers paused
    let tid = a.library.tracks.values().next().unwrap().id;
    a.player.current = Some(tid);
    a.spov.now_spotify = Some(paused_spotify_track());
    // the shared slot holds lyrics loaded for the LOCAL track
    let key = {
        let t = a.library.track(tid).unwrap();
        crate::lyrics::cache_key(&t.artist, &t.title)
    };
    a.meta.lyrics_for = Some(key);
    a.meta.lyrics = Some(crate::lyrics::Lyrics {
        lines: vec![(Duration::ZERO, "LOCALLYRICMARKER".into())],
        trans: vec![None],
        synced: false,
    });
    assert!(
        a.lyrics_for_pane(LyricsPane::Spotify).is_none(),
        "the local lyrics are not the Spotify pane's"
    );
    let s = render_layout(&mut a, Layout::Spotify, 140, 36);
    assert!(
        !s.contains("LOCALLYRICMARKER"),
        "the local track's lyrics must not bleed into the Spotify lyrics pane"
    );
}

#[test]
fn local_pane_never_renders_the_spotify_tracks_lyrics() {
    // The mirror guard: a Spotify track's lyrics must not show in a local view's
    // LYRICS pane when no local track is the active source.
    use crate::app::{Dock, LyricsPane, Panel};
    use std::time::Duration;
    let mut a = demo();
    set_panel(&mut a, Layout::Dashboard, Panel::Lyrics, true, Dock::Right);
    a.player.current = None; // nothing playing locally
    a.spov.now_spotify = Some(paused_spotify_track());
    a.meta.lyrics_for = Some(crate::lyrics::cache_key("The Band", "Song"));
    a.meta.lyrics = Some(crate::lyrics::Lyrics {
        lines: vec![(Duration::ZERO, "SPOTIFYLYRICMARKER".into())],
        trans: vec![None],
        synced: false,
    });
    assert!(a.lyrics_for_pane(LyricsPane::Local).is_none());
    let s = render_layout(&mut a, Layout::Dashboard, 140, 36);
    assert!(
        !s.contains("SPOTIFYLYRICMARKER"),
        "the Spotify track's lyrics must not bleed into the local lyrics pane"
    );
}

#[test]
fn spotify_pane_shows_the_spotify_tracks_lyrics() {
    // Positive control: with the slot loaded for the Spotify track, its lyrics *do*
    // render — proving the gate suppresses only the wrong source, not lyrics wholesale.
    use crate::app::{Dock, Panel};
    use crate::spotify::ConnState;
    use std::time::Duration;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    set_panel(&mut a, Layout::Spotify, Panel::Lyrics, true, Dock::Right);
    a.spov.now_spotify = Some(paused_spotify_track());
    a.meta.lyrics_for = Some(crate::lyrics::cache_key("The Band", "Song"));
    a.meta.lyrics = Some(crate::lyrics::Lyrics {
        lines: vec![(Duration::ZERO, "SPOTIFYLYRICMARKER".into())],
        trans: vec![None],
        synced: false,
    });
    let s = render_layout(&mut a, Layout::Spotify, 140, 36);
    assert!(
        s.contains("SPOTIFYLYRICMARKER"),
        "the active Spotify track's lyrics render in the pane"
    );
}

#[test]
fn lyrics_slot_is_source_gated_per_view() {
    // Unit-level proof of the gate the render tests rely on: one shared slot loaded
    // for the local track is visible to the Local pane but never the Spotify pane —
    // even with a Spotify track paused in the background (now_spotify Some).
    use crate::app::LyricsPane;
    use std::time::Duration;
    let mut a = demo();
    let tid = a.library.tracks.values().next().unwrap().id;
    a.player.current = Some(tid);
    a.spov.now_spotify = Some(paused_spotify_track());
    let key = {
        let t = a.library.track(tid).unwrap();
        crate::lyrics::cache_key(&t.artist, &t.title)
    };
    a.meta.lyrics_for = Some(key);
    a.meta.lyrics = Some(crate::lyrics::Lyrics {
        lines: vec![(Duration::ZERO, "x".into())],
        trans: vec![None],
        synced: false,
    });
    // in a local view the slot belongs to the Local pane…
    a.layout = Layout::Dashboard;
    assert!(a.lyrics_for_pane(LyricsPane::Local).is_some());
    assert!(a.lyrics_for_pane(LyricsPane::Spotify).is_none());
    // …and in the Spotify view it's still not the Spotify pane's
    a.layout = Layout::Spotify;
    assert!(a.lyrics_for_pane(LyricsPane::Spotify).is_none());
}

#[test]
fn spotify_panels_default_hidden_and_movable() {
    use crate::app::{Dock, Panel};
    // the Spotify view hosts the same movable-panel set the Dashboard does,
    // including the now-movable LIBRARY sidebar (shown on the left by default)
    assert_eq!(
        Layout::Spotify.panels(),
        &[Panel::Sidebar, Panel::Queue, Panel::Artist, Panel::Lyrics]
    );
    let side = Layout::Spotify.default_panel(Panel::Sidebar);
    assert!(side.shown, "the sidebar shows by default");
    assert_eq!(
        side.dock,
        Dock::Left,
        "the sidebar defaults to the left edge"
    );
    assert!(
        Layout::Spotify.panel_movable(Panel::Sidebar),
        "the sidebar is movable"
    );
    for p in [Panel::Queue, Panel::Artist, Panel::Lyrics] {
        let cfg = Layout::Spotify.default_panel(p);
        assert!(!cfg.shown, "{p:?} is opt-in (hidden by default)");
        assert_eq!(cfg.dock, Dock::Right, "{p:?} defaults right");
        assert!(Layout::Spotify.panel_movable(p), "{p:?} is movable");
    }
}

/// End-to-end render proof for the "Up Next follows the now-playing track" fix:
/// with the queue pane focused and the cursor parked on the last of 100 tracks,
/// the pane is scrolled to the bottom; after the last track ends and repeat-all
/// wraps to the top, the pane must scroll up to keep the now-playing row visible.
#[test]
fn spotify_up_next_scrolls_to_the_now_playing_track_after_wrap() {
    use crate::app::{Dock, Focus, Panel};
    use crate::core::player::Repeat;
    use crate::spotify::ConnState;
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::session::{SessionCommand, SessionEvent};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    let (tx, _rx) = crossbeam_channel::unbounded::<SessionCommand>();
    a.spov.session_cmd = Some(tx); // ensure_session reuses this (no spawn)
    a.spotify.tokens = Some(crate::spotify::Tokens {
        access_token: "a".into(),
        refresh_token: "r".into(),
        expires_at: crate::datetime::now_unix() + 3600,
        scopes: "streaming".into(),
    });
    let mk = |i: usize| Item {
        uri: format!("spotify:track:{i}"),
        name: format!("ZZZ{i:03}"), // unique, unambiguous substrings
        kind: Kind::Track,
        duration_ms: 1000,
        ..Default::default()
    };
    a.spov.sp_queue = (0..100).map(mk).collect();
    a.spov.sp_idx = 99;
    a.spov.now_spotify = Some(a.spov.sp_queue[99].clone());
    a.spov.sp_repeat = Repeat::All;
    a.spov.sp_started = true;
    a.spotify.queue_sel = 99;
    set_panel(&mut a, Layout::Spotify, Panel::Queue, true, Dock::Right);
    a.focus = Focus::Pane(Panel::Queue);

    let before = render_layout(&mut a, Layout::Spotify, 120, 40);
    assert!(
        before.contains("ZZZ099"),
        "the last track is visible at the bottom before the wrap"
    );
    assert!(
        !before.contains("ZZZ001"),
        "the top of the queue is scrolled off before the wrap"
    );

    // the last track ends → repeat-all wraps to the first and keeps playing
    let (etx, erx) = crossbeam_channel::unbounded::<SessionEvent>();
    etx.send(SessionEvent::EndOfTrack).unwrap();
    a.spov.session_rx = Some(erx);
    a.pump_spotify_session();
    assert_eq!(a.spov.sp_idx, 0, "wrapped to the first track");

    let after = render_layout(&mut a, Layout::Spotify, 120, 40);
    assert!(
        after.contains("ZZZ001"),
        "the Up Next pane scrolled up to the now-playing track"
    );
    assert!(
        !after.contains("ZZZ099"),
        "the old bottom row is no longer shown after the wrap"
    );
}

#[test]
fn spotify_view_hosts_movable_panels() {
    use crate::app::{Dock, Panel};
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = crate::spotify::ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    let mk = |name: &str, artist: &str| Item {
        uri: format!("spotify:track:{name}"),
        name: name.into(),
        subtitle: artist.into(),
        album: "Divide".into(),
        image: None,
        kind: Kind::Track,
        duration_ms: 200_000,
        artist_uri: None,
        ..Default::default()
    };
    a.spov.now_spotify = Some(mk("Perfect", "Ed Sheeran"));
    a.spov.sp_queue = vec![
        mk("Shape of You", "Ed Sheeran"),
        mk("Castle on the Hill", "Ed Sheeran"),
    ];
    // dock all three on the right (they stack vertically in one column)
    for p in [Panel::Queue, Panel::Artist, Panel::Lyrics] {
        set_panel(&mut a, Layout::Spotify, p, true, Dock::Right);
    }
    let s = render_layout(&mut a, Layout::Spotify, 120, 36);
    assert!(s.contains("QUEUE"), "queue pane present");
    assert!(s.contains("Shape of You"), "queue lists upcoming tracks");
    assert!(s.contains("ARTIST"), "artist pane present");
    assert!(s.contains("LYRICS"), "lyrics pane present");
    assert!(
        s.contains("LIBRARY"),
        "sidebar still renders alongside panels"
    );
}

#[test]
fn spotify_tab_cycles_focus_through_panes() {
    use crate::action::Action;
    use crate::app::{Dock, Focus, Panel};
    use crate::spotify::ConnState;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    set_panel(&mut a, Layout::Spotify, Panel::Queue, true, Dock::Right);
    set_panel(&mut a, Layout::Spotify, Panel::Artist, true, Dock::Right);
    // order is sidebar → list → queue → artist → (wrap)
    assert_eq!(a.focus, Focus::Main, "list focused by default");
    a.update(Action::SpotifyCycleFocus(1));
    assert_eq!(a.focus, Focus::Pane(Panel::Queue));
    a.update(Action::SpotifyCycleFocus(1));
    assert_eq!(a.focus, Focus::Pane(Panel::Artist));
    a.update(Action::SpotifyCycleFocus(1));
    assert_eq!(a.focus, Focus::Sidebar);
    a.update(Action::SpotifyCycleFocus(1));
    assert_eq!(a.focus, Focus::Main, "wraps back to the list");
    // Esc out of a focused pane returns to the list (no nav-back)
    a.update(Action::SpotifyCycleFocus(1));
    a.update(Action::SpotifyCancel);
    assert_eq!(a.focus, Focus::Main);
    // a hidden pane is skipped in the cycle
    set_panel(&mut a, Layout::Spotify, Panel::Queue, false, Dock::Right);
    a.update(Action::SpotifyCycleFocus(1));
    assert_eq!(a.focus, Focus::Pane(Panel::Artist), "hidden queue skipped");
}

#[test]
fn spotify_lyrics_pane_is_focusable_and_resizable() {
    use crate::action::Action;
    use crate::app::{Dock, Focus, Panel};
    use crate::spotify::ConnState;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    // show all three movable panes (right-docked, stacked)
    for p in [Panel::Queue, Panel::Artist, Panel::Lyrics] {
        set_panel(&mut a, Layout::Spotify, p, true, Dock::Right);
    }
    // the Lyrics pane is now part of the Tab cycle: list → queue → artist → lyrics
    assert_eq!(a.focus, Focus::Main);
    a.update(Action::SpotifyCycleFocus(1));
    a.update(Action::SpotifyCycleFocus(1));
    a.update(Action::SpotifyCycleFocus(1));
    assert_eq!(
        a.focus,
        Focus::Pane(Panel::Lyrics),
        "Tab reaches the Lyrics pane"
    );
    // `<`/`>` resize the focused Lyrics pane (was display-only before); it's
    // right-docked, so `<` grows it.
    let before = a.panel(Panel::Lyrics).size;
    a.update(Action::ResizeFocusedPane(-1));
    assert!(
        a.panel(Panel::Lyrics).size > before,
        "resize grows the focused Lyrics pane"
    );
    // hiding the focused pane drops focus to the list, not a hidden pane
    a.toggle_panel(Panel::Lyrics);
    assert_eq!(
        a.focus,
        Focus::Main,
        "hiding the focused Lyrics pane refocuses the list"
    );
}

#[test]
fn spotify_queue_pane_has_a_movable_cursor() {
    use crate::action::{Action, Motion};
    use crate::app::{Dock, Focus, Panel};
    use crate::spotify::ConnState;
    use crate::spotify::api::{Item, Kind};
    let mk = |n: &str| Item {
        uri: format!("spotify:track:{n}"),
        name: n.into(),
        subtitle: "A".into(),
        album: "Al".into(),
        image: None,
        kind: Kind::Track,
        duration_ms: 1000,
        artist_uri: None,
        ..Default::default()
    };
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    a.spov.sp_queue = vec![mk("a"), mk("b"), mk("c"), mk("d")];
    a.spov.sp_idx = 1;
    set_panel(&mut a, Layout::Spotify, Panel::Queue, true, Dock::Right);

    // Tab to the queue pane: the cursor parks on the now-playing row
    a.update(Action::SpotifyCycleFocus(1)); // List → Pane(Queue)
    assert_eq!(a.focus, Focus::Pane(Panel::Queue));
    assert_eq!(
        a.spotify.queue_sel, 1,
        "cursor starts on the now-playing track"
    );
    // j/k moves the cursor (clamped to the queue)
    a.update(Action::Move(Motion::Down));
    assert_eq!(a.spotify.queue_sel, 2, "j moves the queue cursor down");
    a.update(Action::Move(Motion::Up));
    a.update(Action::Move(Motion::Up));
    assert_eq!(a.spotify.queue_sel, 0, "k moves it up, clamped at the top");
}

#[test]
fn spotify_artist_pane_loads_rich_details() {
    use crate::spotify::Tokens;
    use crate::spotify::api::{Item, Kind, SpRequest, SpResult};
    let mut a = demo();
    a.layout = Layout::Spotify;
    // wire the Web-API worker (captures the artist request) + art worker while
    // tokens are still None, so set_spotify_sender doesn't spawn a resume thread
    let (tx, rx) = crossbeam_channel::unbounded::<SpRequest>();
    a.set_spotify_sender(tx);
    let (atx, arx) = crossbeam_channel::unbounded();
    a.set_spotify_art_sender(atx);
    a.spotify.tokens = Some(Tokens {
        access_token: "t".into(),
        refresh_token: "r".into(),
        expires_at: 0,
        scopes: String::new(),
    });
    // now-playing track carrying its artist's uri
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:x".into(),
        name: "Song".into(),
        subtitle: "The Band".into(),
        album: "Al".into(),
        image: None,
        kind: Kind::Track,
        duration_ms: 1000,
        artist_uri: Some("spotify:artist:1".into()),
        ..Default::default()
    });

    a.spotify_load_artist();
    assert!(
        matches!(rx.try_recv(), Ok(SpRequest::Artist { uri, .. }) if uri == "spotify:artist:1"),
        "fetches the artist's details over the Web API"
    );
    assert_eq!(a.spov.sp_artist_uri.as_deref(), Some("spotify:artist:1"));

    // the details arrive → pane state populated + the photo is requested
    a.on_spotify_result(SpResult::Artist {
        uri: "spotify:artist:1".into(),
        name: "The Band".into(),
        image: Some("http://img/a".into()),
        genres: "rock · indie".into(),
        followers: 1_800_000,
    });
    let art = a.spov.sp_artist.as_ref().expect("artist details set");
    assert_eq!(art.name, "The Band");
    assert_eq!(art.genres, "rock · indie");
    assert_eq!(art.followers, 1_800_000);
    assert!(arx.try_recv().is_ok(), "artist photo requested");
    assert_eq!(a.spov.sp_artist_cover_url.as_deref(), Some("http://img/a"));

    // the follower count renders on its own line in the artist pane, so a long
    // genre list can't clip this headline stat off a narrow pane
    use crate::app::{Dock, Panel};
    a.spotify.conn = crate::spotify::ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    set_panel(&mut a, Layout::Spotify, Panel::Artist, true, Dock::Right);
    let s = render_layout(&mut a, Layout::Spotify, 140, 36);
    let want = format!("{} followers", crate::spotify::api::fmt_count(1_800_000));
    assert!(
        s.contains(&want),
        "follower count shows in the pane: {want:?}"
    );
}

#[test]
fn artist_pane_shows_popularity_when_followers_unavailable() {
    // the shared dev-mode client id strips the follower count, but librespot
    // supplies a 0–100 popularity — the pane falls back to it as the stat.
    use crate::app::spotify::SpArtist;
    use crate::app::{Dock, Panel};
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = crate::spotify::ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    set_panel(&mut a, Layout::Spotify, Panel::Artist, true, Dock::Right);
    a.spov.now_spotify = Some(Item {
        name: "Diamonds".into(),
        subtitle: "Rihanna".into(),
        kind: Kind::Track,
        ..Default::default()
    });
    a.spov.sp_artist = Some(SpArtist {
        name: "Rihanna".into(),
        genres: String::new(),
        followers: 0, // dev-mode stripped it
        popularity: 95,
        bio: String::new(),
    });
    let s = render_layout(&mut a, Layout::Spotify, 140, 36);
    assert!(
        s.contains("Popularity 95/100"),
        "popularity is the fallback stat when followers are unavailable"
    );
    assert!(
        !s.contains("followers"),
        "no follower line when the count is unavailable"
    );
}

#[test]
fn spotify_artist_pane_prefers_spotifys_own_bio() {
    use crate::app::spotify::SpArtist;
    use crate::app::{Dock, Panel};
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = crate::spotify::ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    set_panel(&mut a, Layout::Spotify, Panel::Artist, true, Dock::Right);
    a.spov.now_spotify = Some(Item {
        name: "Track".into(),
        subtitle: "Dingding".into(),
        kind: Kind::Track,
        ..Default::default()
    });
    a.spov.sp_artist = Some(SpArtist {
        name: "Dingding".into(),
        bio: "SPOTIFYBIOTEXT — the official artist biography.".into(),
        ..SpArtist::default()
    });
    let s = render_layout(&mut a, Layout::Spotify, 140, 36);
    assert!(
        s.contains("ABOUT") && s.contains("SPOTIFYBIOTEXT"),
        "the pane shows Spotify's own bio (no Wikipedia language roulette)"
    );
}

#[test]
fn artist_page_renders_grouped_sections() {
    // opening an artist shows a Spotify-style page: numbered Popular tracks,
    // then Albums / Singles & EPs sections (items carry their Group tags).
    use crate::spotify::api::{Group, Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = crate::spotify::ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    a.spotify.crumb = Some("☻ Rihanna".into());
    a.spotify.items = vec![
        Item {
            name: "Diamonds".into(),
            kind: Kind::Track,
            duration_ms: 225_000,
            group: Group::Popular,
            ..Default::default()
        },
        Item {
            name: "We Found Love".into(),
            kind: Kind::Track,
            duration_ms: 215_000,
            group: Group::Popular,
            ..Default::default()
        },
        Item {
            name: "ANTI".into(),
            kind: Kind::Album,
            subtitle: "Robyn Fenty".into(),
            year: Some(2016),
            group: Group::Albums,
            ..Default::default()
        },
        Item {
            name: "Lift Me Up".into(),
            kind: Kind::Album,
            subtitle: "Robyn Fenty".into(),
            year: Some(2022),
            group: Group::Singles,
            ..Default::default()
        },
    ];
    let s = render_layout(&mut a, Layout::Spotify, 140, 36);
    assert!(s.contains("POPULAR"), "Popular section header");
    assert!(s.contains("ALBUMS"), "Albums grid section header");
    assert!(s.contains("SINGLES & EPs"), "Singles grid section header");
    assert!(
        s.contains("Diamonds") && s.contains("ANTI"),
        "popular track + album card render"
    );
    assert!(
        s.contains("Robyn Fenty"),
        "album card subtitle is the artist only (shared grid_card_subtitle)"
    );
    assert!(
        s.contains("2016"),
        "the album's year shows on the card (right-aligned on the title line)"
    );
}

#[test]
fn invalid_client_error_shows_the_client_id_setup_guide() {
    use crate::spotify::ConnState;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.config.spotify_client_id = String::new(); // no private id → guide applies
    a.spotify.conn = ConnState::Error {
        msg: "Session expired (Spotify error 400: invalid_client). The shared Spotify app \
              was rejected — set your own Client ID (press ; → Spotify), then log in."
            .into(),
    };
    let s = render_layout(&mut a, Layout::Spotify, 120, 40);
    assert!(
        s.contains("developer.spotify.com"),
        "the invalid_client error shows the step-by-step setup guide (dashboard link)"
    );
    assert!(
        s.contains("Copy the app's Client ID"),
        "the guide lists the steps"
    );

    // with a private client id already set, the bare error shows (no setup guide)
    a.config.spotify_client_id = "abc123".into();
    let s2 = render_layout(&mut a, Layout::Spotify, 120, 40);
    assert!(
        !s2.contains("Copy the app's Client ID"),
        "no setup guide once a client id is configured"
    );
}

#[test]
fn spotify_popular_row_shows_artist_album_year_meta() {
    // the POPULAR list uses the shared render_popular_row / track_meta — with the
    // column table off, a track row is "name … artist · album · year" (no
    // columns), same as the local page.
    use crate::spotify::api::{Group, Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.config.track_columns = false; // exercise the rows layout (columns is default)
    a.spotify.conn = crate::spotify::ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    a.spotify.crumb = Some("☻ Adele".into());
    a.spotify.items = vec![Item {
        name: "Hometown Glory".into(),
        subtitle: "Adele".into(),
        album: "Nineteen".into(),
        year: Some(2008),
        kind: Kind::Track,
        group: Group::Popular,
        ..Default::default()
    }];
    let s = render_layout(&mut a, Layout::Spotify, 140, 30);
    assert!(s.contains("Hometown Glory"), "track name renders");
    assert!(
        s.contains("Adele") && s.contains("Nineteen") && s.contains("2008"),
        "the popular row shows the artist · album · year meta"
    );
    assert!(
        !s.contains("TITLE"),
        "the rows layout has no column headers"
    );
}

#[test]
fn track_columns_setting_renders_a_column_table() {
    // config.track_columns flips the POPULAR list to a TITLE/ARTIST/ALBUM/YEAR/TIME
    // column table (shared render_popular_columns), for both local + Spotify pages.
    use crate::spotify::api::{Group, Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = crate::spotify::ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    a.spotify.crumb = Some("☻ Adele".into());
    a.config.track_columns = true;
    a.spotify.items = vec![Item {
        name: "Hometown Glory".into(),
        subtitle: "Adele".into(),
        album: "Nineteen".into(),
        year: Some(2008),
        duration_ms: 200_000,
        kind: Kind::Track,
        group: Group::Popular,
        ..Default::default()
    }];
    let s = render_layout(&mut a, Layout::Spotify, 140, 30);
    assert!(
        s.contains("TITLE") && s.contains("ARTIST") && s.contains("YEAR") && s.contains("TIME"),
        "column layout shows the TITLE/ARTIST/YEAR/TIME headers"
    );
    assert!(
        s.contains("Hometown Glory") && s.contains("2008"),
        "the track + year still render in column mode"
    );
}

#[test]
fn spotify_artist_page_grid_navigates_across_release_groups() {
    use crate::action::Action;
    use crate::app::Focus;
    use crate::spotify::api::{Group, Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.focus = Focus::Main;
    a.spotify.crumb = Some("☻ Artist".into());
    let album = |n: &str, g: Group| Item {
        name: n.into(),
        kind: Kind::Album,
        group: g,
        ..Default::default()
    };
    a.spotify.items = vec![
        Item {
            name: "Pop1".into(),
            kind: Kind::Track,
            group: Group::Popular,
            ..Default::default()
        }, // 0
        album("A1", Group::Albums),  // 1
        album("A2", Group::Albums),  // 2
        album("S1", Group::Singles), // 3
        album("S2", Group::Singles), // 4
    ];
    a.spotify.cols.set(2);
    // POPULAR is [0..1); the release region starts at the first album (idx 1)
    assert_eq!(a.spotify_releases_from(), 1);

    // each group is a horizontal carousel: j/k move BETWEEN carousels, h/l WITHIN one.
    a.spotify.sel = 1; // ALBUMS carousel, first card
    a.update(Action::GridMove(0, 1)); // j → down into the SINGLES carousel
    assert_eq!(
        a.spotify.sel, 3,
        "j moves to the next group's carousel at the same column"
    );
    a.update(Action::GridMove(0, -1)); // k → back up into ALBUMS
    assert_eq!(a.spotify.sel, 1);
    a.update(Action::GridMove(1, 0)); // l → scroll within the ALBUMS carousel
    assert_eq!(a.spotify.sel, 2);
    a.update(Action::GridMove(1, 0)); // l → clamps at the carousel end (no crossing)
    assert_eq!(a.spotify.sel, 2);
    // k off the top carousel → the last POPULAR track
    a.spotify.sel = 1;
    a.update(Action::GridMove(0, -1));
    assert_eq!(
        a.spotify.sel, 0,
        "k off the top carousel → the POPULAR track"
    );
}

#[test]
fn release_carousel_prefetches_offscreen_covers() {
    // Scrolling a carousel shouldn't show covers popping in: rendering a carousel
    // requests art for a few cards beyond the visible window (both directions), so
    // they're decoded before you reach them. Shared `release_grid`, so this guards
    // the behaviour for local + Spotify alike.
    use crate::artwork::ArtKey;
    use crate::spotify::api::{Group, Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = crate::spotify::ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    a.spotify.crumb = Some("☻ Artist".into());
    // one ALBUMS carousel of 30 covers, each with a distinct image URL.
    a.spotify.items = (0..30u32)
        .map(|i| Item {
            name: format!("Album {i}"),
            kind: Kind::Album,
            group: Group::Albums,
            image: Some(format!("http://img/{i}")),
            ..Default::default()
        })
        .collect();
    a.spotify.sel = 0; // first card visible

    let _ = render_layout(&mut a, Layout::Spotify, 120, 30);

    let cols = a.spotify.cols.get();
    let cache = a.grid_art.borrow();
    let cached = (0..30)
        .filter(|i| cache.contains_key(&ArtKey::remote(&format!("http://img/{i}"))))
        .count();
    assert!(cols >= 1, "carousel laid out at least one column");
    assert!(
        cached > cols,
        "prefetch requests covers beyond the {cols} visible (cached {cached})"
    );
    assert!(
        cached < 30,
        "prefetch is bounded — not the whole carousel (cached {cached})"
    );
}

#[test]
fn activating_an_artist_requests_the_grouped_page() {
    use crate::action::Action;
    use crate::spotify::Tokens;
    use crate::spotify::api::{Item, Kind};
    use crate::spotify::session::SessionCommand;
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.tokens = Some(Tokens {
        access_token: "t".into(),
        refresh_token: "r".into(),
        expires_at: crate::datetime::now_unix() + 3600, // valid → session is reused
        scopes: String::new(),
    });
    // a live session command channel so spotify_open routes to librespot
    let (tx, rx) = crossbeam_channel::unbounded::<SessionCommand>();
    a.spov.session_cmd = Some(tx);
    a.spotify.items = vec![Item {
        uri: "spotify:artist:1".into(),
        name: "Rihanna".into(),
        kind: Kind::Artist,
        ..Default::default()
    }];
    a.spotify.sel = 0;
    a.update(Action::SpotifyActivate);
    assert!(
        matches!(rx.try_recv(), Ok(SessionCommand::FetchArtistPage { uri, .. }) if uri == "spotify:artist:1"),
        "opening an artist asks librespot for a grouped artist page"
    );
}

#[test]
fn spotify_lyrics_fetched_and_synced_to_its_clock() {
    use crate::lyricsfetch::{LyricsRequest, LyricsResult};
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:x".into(),
        name: "Song".into(),
        subtitle: "The Band, Other".into(),
        album: "Al".into(),
        kind: Kind::Track,
        duration_ms: 180_000,
        ..Default::default()
    });
    a.spov.sp_dur = 180.0;

    // wiring the lyrics worker with a Spotify track playing targets ITS lyrics
    let (tx, rx) = crossbeam_channel::unbounded::<LyricsRequest>();
    a.set_lyrics_sender(tx);
    // the key load_spotify_lyrics binds (first listed artist + title)
    let key = crate::lyrics::cache_key("The Band", "Song");
    // an online request fires on a cache miss (the shared test dir may already
    // hold the entry, which skips it) — verify its fields when it does fire
    if let Ok(req) = rx.try_recv() {
        assert_eq!(req.title, "Song");
        assert_eq!(req.artist, "The Band", "uses the first listed artist");
        assert_eq!(req.key, key);
    }

    // synced lyrics arrive → adopted
    a.on_lyrics_result(LyricsResult {
        key,
        text: Some("[00:01.00]one\n[00:05.00]two".into()),
    });
    assert!(
        a.meta
            .lyrics
            .as_ref()
            .is_some_and(|l| l.synced && l.lines.len() == 2),
        "synced Spotify lyrics adopted"
    );

    // the karaoke clock follows the Spotify position, not the idle local player
    a.spov.sp_pos = 6.0;
    assert_eq!(a.playback_elapsed().as_secs(), 6);
}

#[test]
fn lyrics_offset_nudges_the_karaoke_clock() {
    use crate::spotify::api::Item;
    use std::time::Duration;
    let mut a = demo(); // Dashboard + no Spotify track → local (player.elapsed) path
    a.player.elapsed = Duration::from_secs(10);

    assert_eq!(a.config.lyrics_offset_ms, 0, "no nudge by default");
    assert_eq!(a.playback_elapsed(), Duration::from_secs(10));

    // +250ms delays the highlight → the lyric clock reads 250ms earlier
    a.config.lyrics_offset_ms = 250;
    assert_eq!(a.playback_elapsed(), Duration::from_millis(9_750));
    // negative advances it (for lyrics that lag)
    a.config.lyrics_offset_ms = -250;
    assert_eq!(a.playback_elapsed(), Duration::from_millis(10_250));
    // never runs negative near the start of a track
    a.player.elapsed = Duration::from_millis(100);
    a.config.lyrics_offset_ms = 500;
    assert_eq!(a.playback_elapsed(), Duration::ZERO);

    // the same nudge rides the Spotify clock when that's the active source
    a.layout = Layout::Spotify;
    a.spov.now_spotify = Some(Item {
        uri: "spotify:track:y".into(),
        name: "S".into(),
        duration_ms: 180_000,
        ..Default::default()
    });
    a.spov.sp_pos = 20.0;
    a.config.lyrics_offset_ms = 300;
    assert_eq!(a.playback_elapsed(), Duration::from_millis(19_700));
}

#[test]
fn lyrics_offset_shifts_plain_unsynced_progress() {
    use std::time::Duration;
    // plain (unsynced) lyrics scroll by progress; the `,`/`.` nudge must move
    // them too, via `lyrics_progress` folding in the offset.
    let mut a = demo(); // Dashboard + no Spotify track → local path
    a.player.duration = Duration::from_secs(50);
    a.player.elapsed = Duration::from_secs(25);

    assert_eq!(a.config.lyrics_offset_ms, 0, "no nudge by default");
    assert!(
        (a.lyrics_progress() - 0.5).abs() < 1e-4,
        "halfway with no nudge"
    );

    // +5s reads the clock earlier → plain lyrics shift back to 40%
    a.config.lyrics_offset_ms = 5_000;
    assert!(
        (a.lyrics_progress() - 0.4).abs() < 1e-4,
        "got {}",
        a.lyrics_progress()
    );
    // negative advances → 60%
    a.config.lyrics_offset_ms = -5_000;
    assert!(
        (a.lyrics_progress() - 0.6).abs() < 1e-4,
        "got {}",
        a.lyrics_progress()
    );
}

#[test]
fn machine_translation_fills_trans_and_skips_identical_lines() {
    use crate::translate::TranslateResult;
    let mut a = demo();
    a.config.lyrics_translate_to = "en".into();
    a.meta.lyrics = Some(crate::lyrics::Lyrics::parse(
        "[00:01.00]hola\n[00:02.00]hello\n[00:03.00]mundo",
    ));
    a.meta.lyrics_for = Some("k".into());

    a.on_translate_result(TranslateResult {
        key: "k".into(),
        target: "en".into(),
        lines: Some(vec!["hi".into(), "hello".into(), "world".into()]),
    });
    let tr = &a.meta.lyrics.as_ref().unwrap().trans;
    assert_eq!(tr[0].as_deref(), Some("hi"));
    assert_eq!(
        tr[1], None,
        "a line equal to the original shows no duplicate"
    );
    assert_eq!(tr[2].as_deref(), Some("world"));
}

#[test]
fn stale_translation_result_is_ignored() {
    use crate::translate::TranslateResult;
    let mut a = demo();
    a.config.lyrics_translate_to = "en".into();
    a.meta.lyrics = Some(crate::lyrics::Lyrics::parse("[00:01.00]uno"));
    a.meta.lyrics_for = Some("current".into());

    // a result for a different (already-navigated-away) track must not apply
    a.on_translate_result(TranslateResult {
        key: "other".into(),
        target: "en".into(),
        lines: Some(vec!["one".into()]),
    });
    assert_eq!(a.meta.lyrics.as_ref().unwrap().trans[0], None);

    // and a result for the wrong target language is ignored too
    a.on_translate_result(TranslateResult {
        key: "current".into(),
        target: "fr".into(),
        lines: Some(vec!["un".into()]),
    });
    assert_eq!(a.meta.lyrics.as_ref().unwrap().trans[0], None);
}

#[test]
fn nudge_lyrics_offset_accumulates_and_clamps() {
    use crate::action::Action;
    let mut a = demo(); // empty config dir → save() is a no-op (test-safe)
    a.update(Action::LyricsOffset(50));
    a.update(Action::LyricsOffset(50));
    assert_eq!(a.config.lyrics_offset_ms, 100, "steps accumulate");
    // bounded to ±5s so a stuck key can't push it absurdly far
    for _ in 0..200 {
        a.update(Action::LyricsOffset(50));
    }
    assert_eq!(a.config.lyrics_offset_ms, 5000, "clamped at +5s");
}

// Home groups its items into one carousel per shelf (like the artist page's
// release rows), and the shelf titles render as headers.
#[test]
fn spotify_home_groups_items_into_shelf_carousels() {
    use crate::app::ReleaseRow;
    use crate::spotify::api::{Item, Kind, Section};

    let shelf = |uri: &str, name: &str, kind, title: &str| Item {
        uri: uri.into(),
        name: name.into(),
        kind,
        section: Some(title.into()),
        ..Default::default()
    };
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = crate::spotify::ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    a.spotify.section = Section::Home;
    a.spotify.items = vec![
        shelf(
            "spotify:playlist:1",
            "Deep Focus",
            Kind::Playlist,
            "Made for you",
        ),
        shelf("spotify:album:2", "Divide", Kind::Album, "Made for you"),
        shelf("spotify:artist:3", "Adele", Kind::Artist, "Jump back in"),
    ];

    assert!(
        a.spotify_sectioned_active(),
        "Home shelves render as carousels"
    );

    // one Header + one Cards row per shelf; cards carry the shelf's item indices
    let rows = a.spotify_browse_rows();
    assert_eq!(rows.len(), 4, "two shelves → 2 headers + 2 carousels");
    assert!(matches!(&rows[0], ReleaseRow::Header(t) if t.as_ref() == "Made for you"));
    assert_eq!(
        rows[1].cards(),
        Some(&[0usize, 1][..]),
        "first shelf's two cards"
    );
    assert!(matches!(&rows[2], ReleaseRow::Header(t) if t.as_ref() == "Jump back in"));
    assert_eq!(
        rows[3].cards(),
        Some(&[2usize][..]),
        "second shelf's one card"
    );

    // shelf titles render as section headers
    let s = render_layout(&mut a, Layout::Spotify, 120, 40);
    assert!(s.contains("Made for you"), "first shelf header renders");
    assert!(s.contains("Jump back in"), "second shelf header renders");
}

/// The Spotify view is fully mouse-driven: the main track list selects on click
/// (double-click plays), the wheel scrolls it, and the sidebar sections are
/// clickable. (Regression: the Spotify column table passed `click: None` and the
/// rows/sidebar registered nothing, so the whole view ignored the mouse.)
#[test]
fn spotify_view_is_mouse_driven() {
    use crate::action::Motion;
    use crate::app::{Focus, MouseTarget};
    use crate::spotify::Tokens;
    use crate::spotify::api::{Item, Kind, Section};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = crate::spotify::ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    a.spotify.tokens = Some(Tokens {
        access_token: "t".into(),
        refresh_token: "r".into(),
        expires_at: 0,
        scopes: String::new(),
    });
    a.spotify.items = (0..40)
        .map(|i| Item {
            uri: format!("spotify:track:{i}"),
            name: format!("Song {i}"),
            subtitle: "Artist".into(),
            kind: Kind::Track,
            duration_ms: 200_000,
            ..Default::default()
        })
        .collect();
    let _ = render_layout(&mut a, Layout::Spotify, 160, 44);

    // helper: centre of the first registered rect matching a predicate
    let find = |a: &AppState, pred: &dyn Fn(&MouseTarget) -> bool| -> (u16, u16) {
        a.hit
            .borrow()
            .iter()
            .find(|(_, t)| pred(t))
            .map(|(r, _)| (r.x + 1, r.y))
            .expect("target registered")
    };

    // click the 3rd track row → selects it
    let (rx, ry) = find(&a, &|t| matches!(t, MouseTarget::SpotifyItem(2)));
    a.handle_click(rx, ry, false);
    assert_eq!(a.spotify.sel, 2, "clicking a Spotify row selects it");

    // wheel over the list advances the selection
    a.handle_scroll(rx, ry, Motion::Down);
    assert_eq!(a.spotify.sel, 3, "the wheel scrolls the Spotify list");

    // a sidebar section row is clickable → focuses the sidebar + loads that section
    let (sx, sy) = find(&a, &|t| matches!(t, MouseTarget::SpotifySection(2)));
    a.handle_click(sx, sy, false);
    assert_eq!(a.focus, Focus::Sidebar, "clicking the sidebar focuses it");
    assert_eq!(
        a.spotify.section,
        Section::ALL[2],
        "and selects that section"
    );
}

#[test]
fn spotify_columns_survive_long_values_instead_of_collapsing() {
    // Regression: a playlist with long "feat." artist lists / long album names used
    // to size those fixed columns to their full untruncated width, so the shared
    // `fit` would rather DROP them than show them that wide — collapsing the whole
    // Spotify main pane down to just TITLE + TIME. The columns are now capped
    // (col_max_w) + clipped with an ellipsis, so the default set stays visible.
    use crate::spotify::api::{Item, Kind};
    let mut a = demo();
    a.layout = Layout::Spotify;
    a.spotify.conn = crate::spotify::ConnState::Connected {
        name: "me".into(),
        premium: true,
    };
    a.config.track_columns = true; // defaults enable artist/album/year/time
    a.spotify.crumb = Some("≡ NEW MUSIC FRIDAY UAE".into());
    let mk = |name: &str, artist: &str, album: &str| Item {
        uri: format!("spotify:track:{name}"),
        name: name.into(),
        subtitle: artist.into(),
        album: album.into(),
        year: Some(2024),
        duration_ms: 235_000,
        kind: Kind::Track,
        ..Default::default()
    };
    a.spotify.items = vec![
        mk("Danceteria", "Ludmilla, Anitta", "Fragmentos"),
        mk(
            "Sick Ass Foo's",
            "Some Artist, Another One, A Third Feature",
            "The Very Long Album Name Deluxe Edition",
        ),
        mk(
            "Ishq Kameena 2.0",
            "Artist A, Artist B",
            "Ishq Kameena 2.0 (From \"Baby Do Die Do\")",
        ),
    ];
    // width where the un-capped columns used to drop out (only ARTIST survived at
    // 120, everything but TITLE/TIME below that): the whole default set must show.
    let s = render_layout(&mut a, Layout::Spotify, 120, 30);
    for col in ["TITLE", "ARTIST", "ALBUM", "YEAR", "TIME"] {
        assert!(s.contains(col), "the {col} column stays visible ({s})");
    }
    // and the long values are clipped with an ellipsis, not shown in full
    assert!(
        s.contains('…') && !s.contains("A Third Feature"),
        "long artist/album values truncate rather than sizing the column wide"
    );
}
