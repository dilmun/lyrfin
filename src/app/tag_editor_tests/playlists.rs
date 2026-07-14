//! Playlists behaviour tests (split from tag_editor_tests). `use super::*`
//! reaches the shared app() fixture + AppState privates.

use super::*;

#[test]
fn add_popup_creates_new_playlist_with_tracks() {
    let mut a = app();
    a.selection = 0;
    let track = a.display_ids()[0];
    a.update(Action::AddToPlaylistPrompt);
    a.input.add_sel = a.library.playlists.len(); // the "+ New playlist" row
    a.update(Action::Activate); // → naming prompt
    assert!(
        a.input.naming.is_some(),
        "Enter on +New opens the name prompt"
    );
    a.update(Action::NameInput("My List".into()));
    a.update(Action::Activate); // confirm_name → create + add the track
    let pl = a.library.playlists.values().find(|p| p.name == "My List");
    assert!(pl.is_some(), "playlist created");
    assert!(pl.unwrap().track_ids.contains(&track), "track added");
}

#[test]
fn playlist_actions_target_the_selected_local_playlist() {
    let mut a = app();
    a.config.dir = std::env::temp_dir().join("lyrfin_pl_actions_test");

    // browse the Playlists section: the selected row resolves the action target
    a.layout = Layout::Dashboard;
    a.local.section = LocalSection::Playlists;
    a.local_load_section();
    a.focus = Focus::Main;
    let p_demo = match a.local.items.first() {
        Some(LocalItem::Playlist(id)) => *id,
        _ => panic!("expected a playlist row in the Playlists section"),
    };
    a.local.sel = 0;
    assert_eq!(a.selected_local_playlist(), Some(p_demo));

    // create: the new flow doesn't need a selection + refreshes the section list
    let before = a.local.items.len();
    a.update(Action::BeginNewPlaylist);
    a.update(Action::NameInput("Fresh".into()));
    a.update(Action::Activate);
    assert!(a.library.playlists.values().any(|p| p.name == "Fresh"));
    assert_eq!(a.local.items.len(), before + 1, "Playlists list refreshed");

    // rename: targets the selected playlist
    a.local.sel = a
        .local
        .items
        .iter()
        .position(|it| matches!(it, LocalItem::Playlist(id) if *id == p_demo))
        .unwrap();
    a.update(Action::BeginRenamePlaylist);
    assert!(matches!(a.input.naming, Some(NameTarget::Rename(id)) if id == p_demo));
    a.update(Action::NameInput("Renamed".into()));
    a.update(Action::Activate);
    assert_eq!(a.library.playlists[&p_demo].name, "Renamed");

    // delete: removes the selected playlist + refreshes the list. Re-select it,
    // since the rename reloaded + re-sorted the list (cursor may have shifted).
    a.local.sel = a
        .local
        .items
        .iter()
        .position(|it| matches!(it, LocalItem::Playlist(id) if *id == p_demo))
        .unwrap();
    // cancel path: opening then dismissing the dialog leaves the playlist intact
    a.update(Action::DeletePlaylist);
    a.update(Action::Back); // esc cancels
    assert!(a.input.confirm_delete.is_none(), "delete dialog dismissed");
    assert!(
        a.library.playlists.contains_key(&p_demo),
        "playlist survives a cancelled delete"
    );

    let n = a.local.items.len();
    a.update(Action::DeletePlaylist);
    assert_eq!(
        a.input.confirm_delete,
        Some(p_demo),
        "delete opens the confirm dialog rather than deleting outright"
    );
    a.update(Action::Activate); // ⏎ confirms
    assert!(
        !a.library.playlists.contains_key(&p_demo),
        "playlist deleted after confirm"
    );
    assert!(a.input.confirm_delete.is_none(), "confirm state cleared");
    assert_eq!(a.local.items.len(), n - 1, "Playlists list refreshed");

    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("lyrfin_pl_actions_test"));
}

#[test]
fn remove_track_from_a_drilled_in_playlist() {
    let mut a = app();
    a.config.dir = std::env::temp_dir().join("lyrfin_pl_remove_test");

    // a normal playlist holding two known tracks
    let tracks: Vec<_> = a.library.all_tracks_sorted().into_iter().take(2).collect();
    assert_eq!(tracks.len(), 2);
    let pid = a.library.create_playlist("Removable".into());
    for &t in &tracks {
        a.library.add_to_playlist(pid, t);
    }

    // browse Playlists → select that playlist → drill into its tracks
    a.layout = Layout::Dashboard;
    a.local.section = LocalSection::Playlists;
    a.local_load_section();
    a.focus = Focus::Main;
    a.local.sel = a
        .local
        .items
        .iter()
        .position(|it| matches!(it, LocalItem::Playlist(id) if *id == pid))
        .expect("our playlist is listed");
    a.update(Action::Activate); // open → its tracks
    assert_eq!(
        a.current_local_playlist(),
        Some(pid),
        "drilled into the playlist"
    );
    assert_eq!(a.local.items.len(), 2, "both tracks shown");

    // remove the first track: it leaves the stored playlist + the open list refreshes
    a.local.sel = 0;
    a.update(Action::RemoveFromPlaylist);
    assert_eq!(a.local.items.len(), 1, "open track list refreshed in place");
    assert_eq!(
        a.library.playlist_tracks(pid),
        vec![tracks[1]],
        "only the second track remains"
    );

    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("lyrfin_pl_remove_test"));
}

#[test]
fn smart_playlist_is_rule_based_and_live() {
    let mut a = app(); // demo: only track 1 is a favorite

    // create from the current search query via the name prompt
    a.search.query = "fav".into();
    a.update(Action::NewSmartPlaylist);
    assert!(matches!(a.input.naming, Some(NameTarget::SmartPlaylist)));
    a.update(Action::NameInput("Loved".into()));
    a.update(Action::Activate); // confirm_name → create_smart_playlist

    let pl = a
        .library
        .playlists
        .values()
        .find(|p| p.name == "Loved")
        .expect("smart playlist created");
    let id = pl.id;
    assert_eq!(pl.query.as_deref(), Some("fav"));
    assert!(a.library.is_smart_playlist(id));

    // membership is computed live from the query
    let tracks = a.library.playlist_tracks(id);
    assert_eq!(tracks, vec![TrackId::new(1)]);

    // favoriting another track immediately changes membership (it's dynamic)
    a.library.tracks.get_mut(&TrackId::new(2)).unwrap().favorite = true;
    let tracks = a.library.playlist_tracks(id);
    assert!(tracks.contains(&TrackId::new(1)) && tracks.contains(&TrackId::new(2)));

    // adding tracks to a smart playlist is a no-op (rule-based)
    a.library.add_to_playlist(id, TrackId::new(5));
    assert!(a.library.playlists[&id].track_ids.is_empty());

    // empty search → no prompt
    a.search.query.clear();
    a.update(Action::NewSmartPlaylist);
    assert!(a.input.naming.is_none());
}

#[test]
fn bookmark_create_jump_and_reject_empty() {
    let mut a = app();
    a.config.dir = std::env::temp_dir().join("lyrfin_bm_test");

    a.search.query = "rating>=4 fav".into();
    a.update(Action::BookmarkSearch);
    assert!(matches!(a.input.naming, Some(NameTarget::Bookmark)));
    assert_eq!(a.input.buffer, "rating>=4 fav", "prompt prefills the query");
    a.update(Action::NameInput("Top Rated".into()));
    a.update(Action::Activate); // confirm_name → saves the bookmark
    assert_eq!(a.bookmarks.len(), 1);
    assert_eq!(a.bookmarks[0].name, "Top Rated");
    assert_eq!(a.bookmarks[0].query, "rating>=4 fav");

    // it surfaces as a quick-jump entry in the command palette
    let entries = a.palette_entries();
    assert!(entries.iter().any(|e| e.label == "★ Top Rated"
        && matches!(&e.action, Action::RunSearch(q) if q.as_str() == "rating>=4 fav")));

    // jumping applies the saved query and leaves input mode
    a.search.query.clear();
    a.update(Action::RunSearch("rating>=4 fav".into()));
    assert_eq!(a.search.query, "rating>=4 fav");
    assert!(!a.search.active);

    // re-bookmarking the same name overwrites rather than duplicating
    a.search.query = "year>=2020".into();
    a.update(Action::BookmarkSearch);
    a.update(Action::NameInput("Top Rated".into()));
    a.update(Action::Activate);
    assert_eq!(a.bookmarks.len(), 1, "same name updates in place");
    assert_eq!(a.bookmarks[0].query, "year>=2020");

    // nothing to bookmark when there's no search
    a.search.query.clear();
    a.update(Action::BookmarkSearch);
    assert!(a.input.naming.is_none());

    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("lyrfin_bm_test"));
}
