//! Local library drill-in browse logic, over the demo library.

use super::*;
use crate::app::{Focus, LocalItem, LocalSection, Panel};

#[test]
fn section_loads_tracks_or_containers() {
    let mut a = demo();
    a.local.section = LocalSection::AllTracks;
    a.local_load_section();
    assert!(
        a.local.items.iter().all(|i| i.is_track()),
        "All Tracks → a flat track list"
    );
    assert!(!a.local.items.is_empty(), "demo has tracks");
    assert!(a.local.crumb.is_none(), "section level has no breadcrumb");

    a.local.section = LocalSection::Albums;
    a.local_load_section();
    assert!(
        a.local
            .items
            .iter()
            .all(|i| matches!(i, LocalItem::Album(_))),
        "Albums → drillable album containers"
    );
}

#[test]
fn drill_in_pushes_a_frame_and_back_restores_the_parent() {
    let mut a = demo();
    a.local.section = LocalSection::Albums;
    a.local_load_section();
    let n_albums = a.local.items.len();
    assert!(n_albums >= 1, "demo seeds an album");

    a.local.sel = 0;
    let album = a.local.items[0].clone();
    a.local_open(album);
    assert_eq!(a.local.nav.depth(), 1, "one frame pushed");
    assert!(
        a.local.crumb.as_deref().is_some_and(|c| c.starts_with('◉')),
        "album breadcrumb set"
    );
    assert!(
        a.local.items.iter().all(|i| i.is_track()),
        "drilled into the album's tracks"
    );
    assert_eq!(a.local.sel, 0, "cursor resets at the child level");

    // the user moves the cursor, then backs out
    a.local.sel = 2;
    assert!(a.local_back(), "Back pops the frame");
    assert_eq!(a.local.nav.depth(), 0, "frame popped");
    assert_eq!(
        a.local.items.len(),
        n_albums,
        "parent album list restored verbatim"
    );
    assert!(a.local.crumb.is_none(), "back at the section level");
    assert!(!a.local_back(), "no more frames at the top");
}

#[test]
fn artist_drill_is_a_grouped_page() {
    let mut a = demo();
    a.local.section = LocalSection::Artists;
    a.local_load_section();
    assert!(!a.local.items.is_empty(), "demo seeds an artist");

    let artist = a.local.items[0].clone();
    a.local_open(artist);
    assert!(
        a.local.crumb.as_deref().is_some_and(|c| c.starts_with('☻')),
        "artist breadcrumb set"
    );
    let headers: Vec<&str> = a
        .local
        .items
        .iter()
        .filter_map(|i| match i {
            LocalItem::Header(h) => Some(*h),
            _ => None,
        })
        .collect();
    assert!(
        headers.contains(&"ALBUMS"),
        "the artist page is grouped (has an ALBUMS section), got {headers:?}"
    );
}

#[test]
fn enter_on_focused_artist_pane_opens_the_now_playing_artist_page() {
    use crate::action::Action;
    use crate::app::{Focus, Panel};
    let mut a = demo();
    // the demo seeds a now-playing track; resolve the artist its pane describes
    let id = a
        .current_track()
        .and_then(|t| t.artist_id)
        .expect("the now-playing track is indexed to an artist");
    let name = a.library.artists[&id].name.clone();

    // focus the Artist info pane and press Enter
    a.focus = Focus::Pane(Panel::Artist);
    a.update(Action::Activate);

    assert_eq!(a.local.nav.depth(), 1, "a drill-in frame was pushed");
    assert_eq!(a.focus, Focus::Main, "focus moved onto the opened page");
    assert_eq!(
        a.local.crumb.as_deref(),
        Some(format!("☻ {name}").as_str()),
        "drilled into the now-playing artist's page"
    );
    assert!(
        a.local
            .items
            .iter()
            .any(|i| matches!(i, LocalItem::Header(_))),
        "the artist page is grouped (POPULAR / release sections), like ⏎ on a row"
    );
}

#[test]
fn enter_on_artist_pane_is_a_noop_with_nothing_playing() {
    use crate::action::Action;
    use crate::app::{Focus, Panel};
    let mut a = demo();
    a.player.current = None; // nothing playing → the pane describes no artist
    a.focus = Focus::Pane(Panel::Artist);
    a.update(Action::Activate);
    assert_eq!(
        a.local.nav.depth(),
        0,
        "no drill-in without a now-playing artist"
    );
}

#[test]
fn session_restores_the_local_section() {
    let mut a = demo();
    a.local.section = LocalSection::Albums;
    a.local_load_section();
    let saved = a.session();
    assert_eq!(
        saved.local_section.as_deref(),
        Some("albums"),
        "section key saved"
    );

    // a fresh launch restoring that session lands back on the Albums section,
    // not the default All Tracks
    let mut b = demo();
    b.scan.restore = Some(saved);
    b.restore_library_state();
    assert_eq!(b.local.section, LocalSection::Albums, "section restored");
    assert!(
        b.local
            .items
            .iter()
            .all(|i| matches!(i, LocalItem::Album(_))),
        "the Albums list is loaded, not All Tracks"
    );
}

#[test]
fn session_restores_the_drill_path() {
    let mut a = demo();
    a.local.section = LocalSection::Albums;
    a.local_load_section();
    // drill into the first album, move the cursor onto a track
    a.local.sel = 0;
    let album = a.local.items[0].clone();
    a.local_open(album);
    assert!(
        a.local.items.iter().all(|i| i.is_track()),
        "in the album's tracks"
    );
    a.local.sel = 1.min(a.local.items.len().saturating_sub(1));
    let saved = a.session();
    assert!(
        saved.local_open.as_ref().is_some_and(|p| !p.is_empty()),
        "the drill path is saved"
    );

    // reopen: we land back INSIDE the album's tracks, not on the Albums list
    let mut b = demo();
    b.scan.restore = Some(saved);
    b.restore_library_state();
    assert_eq!(b.local.nav.depth(), 1, "one drill level restored");
    assert!(
        b.local.crumb.as_deref().is_some_and(|c| c.starts_with('◉')),
        "re-drilled into the album (breadcrumb set)"
    );
    assert!(
        b.local.items.iter().all(|i| i.is_track()),
        "showing the album's tracks, not the album library"
    );
}

// ---- unified inline search row -------------------------------------------

#[test]
fn search_renders_the_inline_search_row() {
    use crate::action::Action;
    let mut a = demo();
    a.update(Action::BeginSearch);
    assert!(a.search.active, "BeginSearch activates the local search");
    a.update(Action::SearchInput("song".into()));
    assert_eq!(a.search.query, "song");

    let s = render_layout(&mut a, Layout::Dashboard, 120, 40);
    // the shared search_bar row (not the old title-embedded query): magnifier
    // glyph, focused caret, the source scope label, and the live result count.
    assert!(s.contains('⌕'), "the row shows the magnifier glyph");
    assert!(s.contains('▌'), "the focused row shows the text caret");
    assert!(
        s.contains("Library"),
        "the row shows the source scope label"
    );
    assert!(s.contains("results"), "the row shows the live result count");
}

#[test]
fn local_search_input_uses_the_shared_text_capture() {
    use crate::action::{Action, Motion};
    use crate::event::{Key, KeyCode, Mods};
    let key = |code| Key {
        code,
        mods: Mods::default(),
    };
    let mut a = demo();
    a.update(Action::BeginSearch);
    // typing edits the query
    assert!(matches!(
        crate::keymap::map(&a, key(KeyCode::Char('a'))),
        Action::SearchInput(q) if q == "a"
    ));
    // ↑/↓ now navigate the result list (the shared capture), where the old
    // bespoke handler swallowed them as no-ops
    assert!(matches!(
        crate::keymap::map(&a, key(KeyCode::Down)),
        Action::Move(Motion::Down)
    ));
    // Esc leaves search
    assert!(matches!(
        crate::keymap::map(&a, key(KeyCode::Esc)),
        Action::Back
    ));
}

#[test]
fn back_then_forward_round_trips_the_drill_in() {
    let mut a = demo();
    a.local.section = LocalSection::Albums;
    a.local_load_section();
    let n_albums = a.local.items.len();

    a.local.sel = 0;
    a.local_open(a.local.items[0].clone());
    let tracks = a.local.items.len();
    let crumb = a.local.crumb.clone();
    a.local.sel = 1;

    assert!(a.local_back(), "Back leaves the album");
    assert_eq!(a.local.items.len(), n_albums, "parent restored");

    assert!(a.local_forward(), "Forward re-enters the album");
    assert_eq!(a.local.nav.depth(), 1, "back at drill depth 1");
    assert_eq!(a.local.items.len(), tracks, "child list restored verbatim");
    assert_eq!(a.local.crumb, crumb, "breadcrumb restored");
    assert_eq!(a.local.sel, 1, "cursor restored where it was left");
    assert!(!a.local_forward(), "nothing further to redo");
}

#[test]
fn a_new_drill_in_truncates_the_forward_branch() {
    let mut a = demo();
    a.local.section = LocalSection::Albums;
    a.local_load_section();
    assert!(!a.local.items.is_empty(), "demo seeds an album");

    a.local.sel = 0;
    a.local_open(a.local.items[0].clone());
    assert!(a.local_back(), "Back stashes a forward frame");
    assert!(a.local.nav.can_forward(), "forward is available");

    // a fresh drill-in discards what we stepped out of, rather than leaving a
    // stale redo pointing at a branch the user has navigated away from
    a.local_open(a.local.items[0].clone());
    assert!(
        !a.local.nav.can_forward(),
        "new navigation truncated forward"
    );
    assert!(!a.local_forward(), "Forward is now inert");
}

#[test]
fn back_at_the_top_level_leaves_the_list_intact() {
    // Regression: building the outgoing history frame `mem::take`s the visible
    // list, so an eagerly-built frame would blank the pane on a no-op Back.
    let mut a = demo();
    a.local.section = LocalSection::Albums;
    a.local_load_section();
    let n = a.local.items.len();
    assert!(n > 0, "demo seeds albums");

    assert!(
        !a.local_back(),
        "nothing to back out of at the section level"
    );
    assert_eq!(a.local.items.len(), n, "the list survived the no-op Back");
    assert!(!a.local_forward(), "nothing to redo either");
    assert_eq!(
        a.local.items.len(),
        n,
        "the list survived the no-op Forward"
    );
}

#[test]
fn back_restores_the_focus_the_drill_in_started_from() {
    // Drilling in from the Artist pane used to dump you on Main on the way back,
    // because `local_open` clobbered focus and `local_back` never restored it.
    let mut a = demo();
    a.local.section = LocalSection::Artists;
    a.local_load_section();
    a.local.sel = 0;

    a.focus = Focus::Pane(Panel::Artist);
    a.local_open(a.local.items[0].clone());
    assert_eq!(a.focus, Focus::Main, "drill-in focuses the content");

    assert!(a.local_back(), "Back pops the drill-in");
    assert_eq!(
        a.focus,
        Focus::Pane(Panel::Artist),
        "focus returns to the pane the drill-in started from"
    );
}

#[test]
fn back_clamps_focus_when_the_saved_pane_was_hidden() {
    // The frame remembers the focus the drill-in started from, but that pane can
    // be hidden in between — restoring it verbatim would strand focus on something
    // that is no longer drawn.
    let mut a = demo();
    a.local.section = LocalSection::Artists;
    a.local_load_section();
    a.local.sel = 0;

    a.focus = Focus::Pane(Panel::Artist);
    a.local_open(a.local.items[0].clone());
    a.toggle_panel(Panel::Artist); // hide it while drilled in
    assert!(a.local_back(), "Back pops the drill-in");
    assert_ne!(
        a.focus,
        Focus::Pane(Panel::Artist),
        "focus must not land on a hidden pane"
    );
    assert!(
        a.focus_order().contains(&a.focus),
        "focus stays on this view's ring, got {:?}",
        a.focus
    );
}
