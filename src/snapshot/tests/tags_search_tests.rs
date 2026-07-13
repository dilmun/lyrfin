//! Tags_search snapshot/behaviour tests, split out of snapshot.rs.

use super::*;

#[test]
fn cover_search_popup_flow() {
    use crate::action::Action;
    use crate::app::CoverStatus;
    use crate::coversearch::{Candidate, CoverResult};
    use crate::event::{Key, KeyCode, Mods};
    let mut app = demo();
    let id = app.library.tracks.values().next().unwrap().id;
    app.player.current = Some(id);

    app.update(Action::OpenCoverSearch);
    let cs = app.tags.cover.as_ref().expect("popup open");
    assert!(!cs.query.is_empty(), "query seeded from tags");
    assert!(matches!(cs.status, CoverStatus::Searching));
    let key = cs.key.clone();

    // results arrive (picker is None in tests, so previews are empty)
    let mk = |s| Candidate {
        source: s,
        width: 1000,
        height: 1000,
        full_url: format!("http://x/{s}.jpg"),
        thumb: image::DynamicImage::new_rgb8(2, 2),
    };
    app.on_cover_result(CoverResult::Found {
        key: key.clone(),
        candidates: vec![mk("iTunes"), mk("Deezer")],
    });
    assert!(matches!(
        app.tags.cover.as_ref().unwrap().status,
        CoverStatus::Results
    ));
    // j navigates (not editing)
    let k = |c: char| {
        crate::keymap::map(
            &app,
            Key {
                code: KeyCode::Char(c),
                mods: Mods::default(),
            },
        )
    };
    assert!(matches!(k('j'), Action::CoverMove(_)));
    app.update(Action::CoverMove(crate::action::Motion::Down));
    assert_eq!(app.tags.cover.as_ref().unwrap().sel, 1);

    // '/' starts editing; then a char types into the query
    app.update(Action::CoverInput(
        app.tags.cover.as_ref().unwrap().query.clone(),
    ));
    assert!(app.tags.cover.as_ref().unwrap().editing);

    // embedding result closes the popup
    app.on_cover_result(CoverResult::Embedded {
        key,
        count: 3,
        msg: "Embedded cover in 3 track(s)".into(),
    });
    assert!(app.tags.cover.is_none(), "popup closes after embed");
}

#[test]
fn search_cache_invalidates_correctly() {
    use crate::action::{Action, Motion};
    let mut a = demo();
    a.update(Action::SearchInput("Tycho".into()));
    let r = a.display_ids();
    assert!(!r.is_empty(), "found Tycho");
    // a navigation action must NOT change the (cached) search result
    a.update(Action::Move(Motion::Down));
    assert_eq!(a.display_ids(), r, "scrolling doesn't re-search");
    // changing the query recomputes
    a.update(Action::SearchInput("Bonobo".into()));
    assert_ne!(a.display_ids(), r, "a new query yields new results");
}

#[test]
fn search_index_matches_prefix_and_fuzzy_falls_back() {
    use crate::action::Action;
    let mut a = demo();
    let tycho = a
        .library
        .tracks
        .values()
        .find(|t| t.artist == "Tycho")
        .map(|t| t.id)
        .expect("demo has a Tycho track");

    // "tych" is a prefix of the indexed "tycho" token → resolved by the index
    a.update(Action::SearchInput("tych".into()));
    assert!(
        a.display_ids().contains(&tycho),
        "index finds Tycho by prefix"
    );

    // "tyco" is a prefix of no token → index returns empty → the fuzzy
    // fallback still matches it as a subsequence of "Tycho".
    a.update(Action::SearchInput("tyco".into()));
    assert!(
        a.display_ids().contains(&tycho),
        "fuzzy fallback catches the typo the index can't"
    );
}

#[test]
fn tag_apply_requires_confirm() {
    use crate::action::Action;
    use crate::app::PendingApply;
    use crate::event::{Key, KeyCode, Mods};
    use crate::tagsearch::{TagCandidate, TagResult};
    let mut app = demo();
    let id = app.library.tracks.values().next().unwrap().id;
    app.player.current = Some(id);
    app.update(Action::OpenTagSearch);
    let key = app.tags.search.as_ref().unwrap().key.clone();
    app.on_tag_result(TagResult::Found {
        key,
        candidates: vec![TagCandidate {
            source: "iTunes",
            title: "X".into(),
            artist: "A".into(),
            ..Default::default()
        }],
    });
    // Enter stages a confirm — it does NOT write
    app.update(Action::TagActivate);
    assert_eq!(
        app.tags.search.as_ref().unwrap().pending,
        Some(PendingApply::Song)
    );
    // while pending, Enter maps to the confirm action
    let k = crate::keymap::map(
        &app,
        Key {
            code: KeyCode::Enter,
            mods: Mods::default(),
        },
    );
    assert!(matches!(k, Action::TagConfirm));
    // Esc cancels the confirm and keeps the popup open
    app.update(Action::Back);
    assert!(app.tags.search.as_ref().unwrap().pending.is_none());
    assert!(app.tags.search.is_some(), "cancel doesn't close the popup");
}

#[test]
fn tag_search_popup_flow() {
    use crate::action::Action;
    use crate::app::CoverStatus;
    use crate::tagsearch::{TagCandidate, TagResult};
    let mut app = demo();
    let id = app.library.tracks.values().next().unwrap().id;
    app.player.current = Some(id);
    app.update(Action::OpenTagSearch);
    let ts = app.tags.search.as_ref().expect("popup open");
    assert!(!ts.query.is_empty());
    let key = ts.key.clone();
    let mk = |s: &'static str, title: &str| TagCandidate {
        source: s,
        title: title.into(),
        artist: "Neon District".into(),
        album: "Afterglow".into(),
        year: Some(2025),
        genre: Some("Synthwave".into()),
        track_no: Some(3),
        track_total: Some(10),
        ..Default::default()
    };
    app.on_tag_result(TagResult::Found {
        key: key.clone(),
        candidates: vec![mk("iTunes", "Midnight Protocol"), mk("Deezer", "Midnight")],
    });
    assert!(matches!(
        app.tags.search.as_ref().unwrap().status,
        CoverStatus::Results
    ));
    app.update(Action::TagMove(crate::action::Motion::Down));
    assert_eq!(app.tags.search.as_ref().unwrap().sel, 1);
    // applying closes the popup
    app.on_tag_result(TagResult::Applied {
        key,
        count: 12,
        msg: "Applied tags to 12 track(s)".into(),
    });
    assert!(app.tags.search.is_none());
}
