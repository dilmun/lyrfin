//! Palette snapshot/behaviour tests, split out of snapshot.rs.

use super::*;

#[test]
fn palette_multiword_label_and_setting_by_name() {
    use crate::action::Action;
    use crate::app::Setting;
    let mut app = demo();
    let id = app.library.tracks.values().next().unwrap().id;
    app.player.current = Some(id);
    // a multi-word ENTRY label opens its entry
    app.update(Action::OpenPalette);
    app.update(Action::PaletteInput("Tag Edit".into()));
    app.update(Action::PaletteActivate);
    assert!(app.tags_open(), "'Tag Edit' opened the unified tag modal");
    assert!(app.palette.is_none());
    app.update(Action::Back); // close modal

    // a setting is reached by typing its name (no command verb)
    app.update(Action::OpenPalette);
    app.update(Action::PaletteInput("theme".into()));
    let entries = app.palette_entries();
    let m = app.palette_matches();
    assert!(
        m.iter().any(|&i| matches!(
            entries[i].action,
            Action::PaletteOpenSetting(Setting::Theme)
        )),
        "the Theme setting is reachable by name"
    );
}

#[test]
fn palette_ctrl_jk_navigates() {
    use crate::action::{Action, Motion};
    use crate::event::{Key, KeyCode, Mods};
    let mut app = demo();
    app.update(Action::OpenPalette);
    let ctrl = Mods {
        ctrl: true,
        ..Default::default()
    };
    let down = crate::keymap::map(
        &app,
        Key {
            code: KeyCode::Char('j'),
            mods: ctrl,
        },
    );
    assert!(matches!(down, Action::PaletteMove(Motion::Down)));
    let up = crate::keymap::map(
        &app,
        Key {
            code: KeyCode::Char('k'),
            mods: ctrl,
        },
    );
    assert!(matches!(up, Action::PaletteMove(Motion::Up)));
}

#[test]
fn palette_commands_are_grouped() {
    use crate::app::{PALETTE_GROUPS, palette_commands};
    for (cat, _, _) in palette_commands() {
        assert!(
            PALETTE_GROUPS.contains(&cat),
            "category {cat} is a known group"
        );
    }
}
