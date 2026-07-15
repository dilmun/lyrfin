//! Editing behaviour tests (split from tag_editor_tests). `use super::*`
//! reaches the shared app() fixture + AppState privates.

use super::*;

#[test]
fn edit_mode_opens_browsing_then_types() {
    let mut a = app();
    assert_eq!(a.mode(), Mode::View);
    a.update(Action::BeginTagEdit);
    assert!(a.tags.edit.is_some(), "editor opens");
    assert_eq!(a.mode(), Mode::Edit);
    assert!(!a.tags.edit.as_ref().unwrap().editing, "opens in browse");

    // move to Artist (field 1), enter edit, type
    a.update(Action::TagEditMove(Motion::Down));
    a.update(Action::TagEditBeginEdit);
    a.update(Action::TagEditType("New Name".into()));
    assert_eq!(a.tags.edit.as_ref().unwrap().draft.artist, "New Name");
}

#[test]
fn tab_switches_modal_tabs_not_digits() {
    let mut a = app();
    a.update(Action::BeginTagEdit); // Edit tab, browsing
    assert_eq!(a.tags.tab, 0);
    let press = |a: &AppState, code: KeyCode| {
        crate::keymap::map(
            a,
            Key {
                code,
                mods: Mods::default(),
            },
        )
    };
    // Tab / ⇧Tab switch the modal's tabs — consistent with every other overlay
    assert!(matches!(press(&a, KeyCode::Tab), Action::OverlayTab(1)));
    assert!(matches!(
        press(&a, KeyCode::BackTab),
        Action::OverlayTab(-1)
    ));
    a.update(Action::OverlayTab(1));
    assert_eq!(a.tags.tab, 1, "Tab advances to the Auto Tag tab");
    // the old 1/2/3 shortcuts are gone — back on the Edit tab, '1' is inert
    a.tags_tab_to(0);
    assert!(matches!(press(&a, KeyCode::Char('1')), Action::Noop));
}

#[test]
fn keymap_enter_edits_s_saves_in_browse() {
    let mut a = app();
    a.update(Action::BeginTagEdit); // browsing
    let enter = Key {
        code: KeyCode::Enter,
        mods: Mods::default(),
    };
    // Enter starts editing the field — it does NOT save
    assert!(matches!(
        crate::keymap::map(&a, enter),
        Action::TagEditBeginEdit
    ));
    let s = Key {
        code: KeyCode::Char('s'),
        mods: Mods::default(),
    };
    assert!(matches!(crate::keymap::map(&a, s), Action::TagEditSave));
    // once editing, letters insert and Enter returns to browse
    a.update(Action::TagEditBeginEdit);
    let z = Key {
        code: KeyCode::Char('z'),
        mods: Mods::default(),
    };
    assert!(matches!(
        crate::keymap::map(&a, z),
        Action::TagEditInsert('z')
    ));
    assert!(matches!(
        crate::keymap::map(&a, enter),
        Action::TagEditStopEdit
    ));
}

#[test]
fn visual_mode_reported() {
    let mut a = app();
    a.focus = Focus::Main;
    a.update(Action::VisualSelect);
    assert_eq!(a.mode(), Mode::Visual);
}

#[test]
fn visual_select_and_marks_track_the_local_cursor() {
    use crate::action::Motion;
    let mut a = app();
    a.layout = Layout::Dashboard;
    a.focus = Focus::Main;
    // seed_demo's default (AllTracks) list is a flat run of tracks — the case that
    // used to silently mis-target because the operators read `self.selection`, not
    // the Dashboard's `local.sel`.
    assert!(a.local.items.len() >= 4 && a.local.items.iter().all(|i| i.is_track()));
    let want: Vec<_> = (1..=3)
        .map(|i| match a.local.items[i] {
            crate::app::LocalItem::Track(t) => t,
            _ => unreachable!("all tracks"),
        })
        .collect();
    a.local.sel = 1;
    // a stale, unrelated `self.selection` must NOT be what visual selection uses.
    a.selection = 9;

    // V anchors at the Dashboard cursor; extending down covers rows 1..=3.
    a.update(Action::VisualSelect);
    assert_eq!(a.mode(), Mode::Visual);
    a.update(Action::Move(Motion::Down));
    a.update(Action::Move(Motion::Down));
    assert_eq!(
        a.visual_range(),
        Some((1, 3)),
        "range follows local.sel, not selection"
    );
    assert_eq!(
        a.selected_track_ids(),
        want,
        "bulk-op target is the live range"
    );

    // x commits the range to the marked set and leaves visual mode.
    a.update(Action::ToggleMark);
    assert_eq!(a.mode(), Mode::View);
    assert_eq!(a.marks.ids.len(), 3);
    assert!(
        want.iter()
            .all(|t| a.marks.ids.contains(&crate::app::MarkKey::Track(*t)))
    );

    // and a bulk operator (favourite) hits exactly the marked set.
    a.update(Action::ToggleFavoriteSel);
    assert!(
        want.iter()
            .all(|t| a.library.track(*t).is_some_and(|tk| tk.favorite))
    );
}

#[test]
fn caret_edits_mid_string() {
    use crate::action::Caret;
    let mut a = app();
    a.update(Action::BeginTagEdit); // Title focused
    a.update(Action::TagEditBeginEdit);
    a.update(Action::TagEditType("'Round".into())); // caret at end
    // delete the leading quote: Home, then forward-delete
    a.update(Action::TagEditCaret(Caret::Home));
    a.update(Action::TagEditDelete);
    assert_eq!(a.tags.edit.as_ref().unwrap().draft.get(0), "Round");
    // insert at the caret (now position 0)
    a.update(Action::TagEditInsert('A'));
    assert_eq!(a.tags.edit.as_ref().unwrap().draft.get(0), "ARound");
}

#[test]
fn clearing_a_field_marks_it_for_writing() {
    let mut a = app();
    a.update(Action::BeginTagEdit);
    a.update(Action::TagEditClear); // clears the focused field (Title)
    let te = a.tags.edit.as_ref().unwrap();
    assert!(te.touched[0], "cleared field is written even though empty");
    assert_eq!(te.draft.get(0), "");
}

#[test]
fn manual_edit_album_apply_confirms_first() {
    let mut a = app();
    a.player.current = a.player.queue.items.first().copied();
    a.selection = 0;
    a.update(Action::BeginTagEdit);
    assert!(a.tags.edit.is_some(), "tag editor opened");

    // no edits yet → arming the album apply does nothing
    a.update(Action::TagEditAlbumPrompt);
    assert!(
        !a.tags.edit.as_ref().unwrap().confirm_album,
        "no changes → no confirmation armed"
    );

    // touch a field, then arm → confirmation is shown (no write yet)
    a.tags.edit.as_mut().unwrap().touched[2] = true; // album field
    a.update(Action::TagEditAlbumPrompt);
    assert!(
        a.tags.edit.as_ref().unwrap().confirm_album,
        "a change arms the album confirmation"
    );

    // Esc/n cancels without writing — the editor stays open
    a.update(Action::TagEditAlbumCancel);
    assert!(a.tags.edit.is_some(), "cancel keeps the editor open");
    assert!(
        !a.tags.edit.as_ref().unwrap().confirm_album,
        "cancel clears the confirmation"
    );
}

#[test]
fn remove_field_clears_focused_draft_field() {
    let mut a = app();
    a.update(Action::BeginTagEdit); // Title focused, non-empty
    assert!(!a.tags.edit.as_ref().unwrap().draft.get(0).is_empty());

    // ^d in browse mode is the remove-field action
    let ctrl_d = Key {
        code: KeyCode::Char('d'),
        mods: Mods {
            ctrl: true,
            ..Mods::default()
        },
    };
    assert!(matches!(
        crate::keymap::map(&a, ctrl_d),
        Action::TagRemoveField
    ));
    a.update(Action::TagRemoveField);
    // the field is emptied in the draft and the editor stays open
    assert_eq!(a.tags.edit.as_ref().unwrap().draft.get(0), "");
    assert!(a.tags.edit.is_some());
}

#[test]
fn find_replace_prompt_routes_typing_to_active_box() {
    let mut a = app();
    a.update(Action::BeginTagEdit);
    a.update(Action::TagReplaceBegin);
    assert!(
        a.tags.edit.as_ref().unwrap().replace.is_some(),
        "prompt opens"
    );

    // typing while the prompt is open targets the FIND box first
    let key = |c: char| Key {
        code: KeyCode::Char(c),
        mods: Mods::default(),
    };
    let act = crate::keymap::map(&a, key('x'));
    assert!(matches!(act, Action::TagReplaceType(ref s) if s == "x"));
    a.update(act);
    assert_eq!(
        a.tags.edit.as_ref().unwrap().replace.as_ref().unwrap().0,
        "x"
    );

    // Tab switches to the REPLACE box; typing now fills it
    a.update(Action::TagReplaceToggle);
    let act = crate::keymap::map(&a, key('y'));
    assert!(matches!(act, Action::TagReplaceType(ref s) if s == "y"));
    a.update(act);
    let r = a.tags.edit.as_ref().unwrap().replace.as_ref().unwrap();
    assert_eq!((r.0.as_str(), r.1.as_str(), r.2), ("x", "y", true));

    // Esc cancels the prompt but keeps the editor open
    a.update(Action::TagReplaceCancel);
    assert!(a.tags.edit.as_ref().unwrap().replace.is_none());
    assert!(a.tags.edit.is_some());
}
