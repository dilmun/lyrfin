//! The mini (narrow-terminal) card layout: when it engages, what it shows, and
//! that every view survives a window too small for docked panes.

use super::*;
use crate::action::Action;
use crate::app::{Focus, LocalSection, Panel};
use crate::ui::breakpoint::MINI_W;

/// A width comfortably inside the mini regime, and one comfortably outside.
const NARROW: u16 = 50;
const WIDE: u16 = 100;

#[test]
fn mini_engages_below_the_width_threshold_only() {
    let mut app = demo();
    // one column under → a single card, so the docked side panes are gone
    let s = render_layout(&mut app, Layout::Dashboard, MINI_W - 1, 30);
    // match the pane's border title, not the word — "ARTIST" is also a tracklist
    // column header, and "QUEUE" could appear in body text
    assert!(!s.contains("╭ QUEUE"), "no docked queue in the mini layout");
    assert!(
        !s.contains("╭ ARTIST"),
        "no docked artist pane in the mini layout"
    );
    assert!(
        !s.contains("╭ LIBRARY"),
        "no docked sidebar in the mini layout"
    );

    // exactly at the threshold → the normal docked layout
    let s = render_layout(&mut app, Layout::Dashboard, MINI_W, 30);
    assert!(
        s.contains("LIBRARY"),
        "at the threshold the sidebar still docks"
    );
}

#[test]
fn a_short_but_wide_window_keeps_the_full_layout() {
    // Regression: the breakpoint is width-only. Home at 100×12 and a docked queue
    // at 90×18 both render correctly with panes, and must not collapse to cards.
    let mut app = demo();
    let s = render_layout(&mut app, Layout::Dashboard, WIDE, 12);
    assert!(s.contains("LIBRARY"), "short and wide keeps the sidebar");
    assert!(s.contains("MUSIC"), "short and wide keeps the main pane");
}

#[test]
fn the_mini_card_shows_one_pane_and_a_compact_bar() {
    let mut app = demo();
    let s = render_layout(&mut app, Layout::Dashboard, NARROW, 22);
    assert!(s.contains("MUSIC"), "the main card is on screen");
    // the 2-row bar: track line + progress line, no transport row or volume meter
    assert!(
        s.contains("Midnight Protocol · Neon District"),
        "compact bar names the track and artist on one line"
    );
    assert!(!s.contains("VOL"), "no volume meter on the compact bar");
    assert!(
        s.lines().any(|l| l.contains('●') && l.contains(':')),
        "compact bar has a progress line with times"
    );
}

#[test]
fn the_trail_marks_only_the_directions_that_go_somewhere() {
    let mut app = demo();
    app.local.section = LocalSection::AllTracks;
    app.local_load_section();

    // on Main at depth 0: Back reaches the sidebar, Forward has nowhere to go
    app.focus = Focus::Main;
    let s = render_layout(&mut app, Layout::Dashboard, NARROW, 22);
    let trail = s.lines().next().unwrap_or_default().to_string();
    assert!(trail.contains('‹'), "Back is available (→ the sidebar)");
    assert!(!trail.contains('›'), "nothing to go forward to yet");

    // on the sidebar: the mirror image
    app.focus = Focus::Sidebar;
    let s = render_layout(&mut app, Layout::Dashboard, NARROW, 22);
    let trail = s.lines().next().unwrap_or_default().to_string();
    assert!(!trail.contains('‹'), "the sidebar is the first card");
    assert!(trail.contains('›'), "Forward reaches the main list");
}

#[test]
fn the_miller_view_shows_one_column_as_the_card() {
    let mut app = demo();
    app.focus = Focus::Main;

    app.browser.col = 0;
    let s = render_layout(&mut app, Layout::LibraryFocus, NARROW, 22);
    assert!(s.contains("ARTISTS"), "column 0 is the ARTISTS card");
    assert!(!s.contains("ALBUMS"), "the other columns are not drawn");

    app.browser.col = 2;
    let s = render_layout(&mut app, Layout::LibraryFocus, NARROW, 22);
    assert!(s.contains("TRACKS"), "column 2 is the TRACKS card");
    assert!(!s.contains("ARTISTS"), "the other columns are not drawn");
}

#[test]
fn a_dock_pane_becomes_a_full_width_card() {
    let mut app = demo();
    app.focus = Focus::Pane(Panel::Queue);
    let s = render_layout(&mut app, Layout::Dashboard, NARROW, 22);
    assert!(s.contains("QUEUE"), "the queue is the card on screen");
    assert!(
        !s.contains("MUSIC"),
        "the main pane is not drawn beside it — one card at a time"
    );
}

#[test]
fn every_view_renders_in_a_window_too_small_for_panes() {
    // The mini layout covers four views; the other three keep their own path.
    // Either way none may panic or come out blank at a size this cramped.
    let mut app = demo();
    for layout in [
        Layout::Dashboard,
        Layout::LibraryFocus,
        Layout::FullPlayer,
        Layout::LyricsFocus,
        Layout::Concert,
        Layout::Radio,
        Layout::Spotify,
    ] {
        for (w, h) in [(NARROW, 22), (36, 20), (24, 8)] {
            let s = render_layout(&mut app, layout, w, h);
            assert!(
                s.lines().any(|l| !l.trim().is_empty()),
                "{layout:?} at {w}×{h} rendered blank"
            );
            assert!(
                s.lines().all(|l| l.chars().count() <= w as usize),
                "{layout:?} at {w}×{h} overflowed the frame width"
            );
        }
    }
}

// ---- input routing --------------------------------------------------------

/// Put the app in the mini regime the way a real frame would, then map a key.
fn mini_key(app: &mut AppState, w: u16, code: crate::event::KeyCode) -> Action {
    use crate::event::{Key, Mods};
    let _ = render_layout(app, app.layout, w, 24); // records the frame size
    crate::keymap::map(
        app,
        Key {
            code,
            mods: Mods::default(),
        },
    )
}

#[test]
fn h_pops_a_drill_level_in_the_mini_layout() {
    use crate::event::KeyCode;
    let mut app = demo();
    app.layout = Layout::Dashboard;
    app.local.section = LocalSection::Albums;
    app.local_load_section();
    app.local.sel = 0;
    app.local_open(app.local.items[0].clone());
    app.focus = Focus::Main;

    assert!(
        matches!(mini_key(&mut app, NARROW, KeyCode::Char('h')), Action::Back),
        "drilled in + narrow → h goes back a level"
    );
    // ...and at the top level it falls through to focus movement instead
    app.local_back();
    assert!(
        !matches!(mini_key(&mut app, NARROW, KeyCode::Char('h')), Action::Back),
        "at depth 0 h reverts to moving focus (→ the sidebar)"
    );
}

#[test]
fn l_redoes_a_popped_level_in_the_mini_layout() {
    use crate::event::KeyCode;
    let mut app = demo();
    app.layout = Layout::Dashboard;
    app.local.section = LocalSection::Albums;
    app.local_load_section();
    app.local.sel = 0;
    app.local_open(app.local.items[0].clone());
    app.local_back();
    app.focus = Focus::Main;
    app.local.grid = false; // as a list; the grid case is asserted below

    assert!(
        matches!(
            mini_key(&mut app, NARROW, KeyCode::Char('l')),
            Action::Forward
        ),
        "a stashed forward frame + narrow → l redoes it"
    );
}

#[test]
fn a_cover_grid_keeps_h_and_l_for_card_movement() {
    // Deliberate precedence: inside a grid, h/l move the selection as they do in
    // the wide layout, rather than becoming history navigation. Forward is still
    // reachable there via ctrl-]. Guards the ordering of `grid_context` before
    // `mini_view` in the keymap chain.
    use crate::event::KeyCode;
    let mut app = demo();
    app.layout = Layout::Dashboard;
    app.local.section = LocalSection::Albums;
    app.local_load_section();
    app.local.sel = 0;
    app.local_open(app.local.items[0].clone());
    app.local_back();
    app.focus = Focus::Main;
    assert!(app.local_grid_active(), "Albums defaults to the cover grid");

    assert!(
        matches!(
            mini_key(&mut app, NARROW, KeyCode::Char('l')),
            Action::GridMove(1, 0)
        ),
        "in a grid, l moves a card rather than redoing a navigation"
    );
}

#[test]
fn the_wide_layout_leaves_h_and_l_as_focus_movement() {
    use crate::event::KeyCode;
    let mut app = demo();
    app.layout = Layout::Dashboard;
    app.local.section = LocalSection::Albums;
    app.local_load_section();
    app.local.sel = 0;
    app.local_open(app.local.items[0].clone());
    app.focus = Focus::Main;

    // same state, wide frame: h must NOT hijack pane focus movement
    assert!(
        !matches!(mini_key(&mut app, WIDE, KeyCode::Char('h')), Action::Back),
        "the wide layout keeps h as focus movement — Back is esc / ctrl-o there"
    );
}

#[test]
fn ctrl_bracket_maps_to_forward_everywhere() {
    use crate::event::{Key, KeyCode, Mods};
    let app = demo();
    let k = Key {
        code: KeyCode::Char(']'),
        mods: Mods {
            ctrl: true,
            ..Mods::default()
        },
    };
    assert!(
        matches!(crate::keymap::map(&app, k), Action::Forward),
        "ctrl-] is the wide-layout Forward key"
    );
}

// ---- directional focus (ctrl+h/j/k/l) -------------------------------------

/// Render, then map a ctrl-modified key — the registry is populated during render,
/// so the two must go together for directional focus to resolve.
fn ctrl_key(app: &mut AppState, w: u16, h: u16, code: crate::event::KeyCode) -> Action {
    use crate::event::{Key, Mods};
    let _ = render_layout(app, app.layout, w, h);
    crate::keymap::map(
        app,
        Key {
            code,
            mods: Mods {
                ctrl: true,
                ..Mods::default()
            },
        },
    )
}

#[test]
fn ctrl_direction_maps_to_a_focus_jump() {
    use crate::event::KeyCode;
    let mut app = demo();
    app.layout = Layout::Dashboard;
    for (code, expect) in [
        (KeyCode::Char('h'), Action::FocusToward(-1, 0)),
        (KeyCode::Char('l'), Action::FocusToward(1, 0)),
        (KeyCode::Char('k'), Action::FocusToward(0, -1)),
        (KeyCode::Char('j'), Action::FocusToward(0, 1)),
    ] {
        let got = ctrl_key(&mut app, WIDE, 30, code);
        assert_eq!(
            std::mem::discriminant(&got),
            std::mem::discriminant(&expect),
            "ctrl+{code:?} should be a directional focus jump, got {got:?}"
        );
    }
}

#[test]
fn ctrl_down_reaches_the_pane_stacked_below() {
    // The end-to-end path: render populates the focus registry, then the action
    // resolves against it.
    //
    // Deliberately a VERTICAL move between two panes stacked on the same edge.
    // Horizontal moves fall back to the focus ring when geometry finds nothing, so
    // a ctrl+h test passes whether or not the registry works — it proves nothing.
    // There is no vertical fallback, so landing on Lyrics can only come from the
    // rects recorded during render.
    use crate::app::Dock;
    let mut app = demo();
    set_panel(&mut app, Layout::Dashboard, Panel::Queue, true, Dock::Right);
    set_panel(
        &mut app,
        Layout::Dashboard,
        Panel::Lyrics,
        true,
        Dock::Right,
    );
    app.layout = Layout::Dashboard;
    app.focus = Focus::Pane(Panel::Queue);
    let _ = render_layout(&mut app, Layout::Dashboard, WIDE, 30);
    assert!(
        !app.focus_rects.borrow().is_empty(),
        "render registers focusable regions"
    );

    app.update(Action::FocusToward(0, 1));
    assert_eq!(
        app.focus,
        Focus::Pane(Panel::Lyrics),
        "ctrl+j from the queue reaches the pane stacked under it"
    );

    let _ = render_layout(&mut app, Layout::Dashboard, WIDE, 30);
    app.update(Action::FocusToward(0, -1));
    assert_eq!(app.focus, Focus::Pane(Panel::Queue), "ctrl+k comes back up");
}

#[test]
fn ctrl_direction_still_moves_in_the_mini_layout() {
    // Only one card is drawn, so geometry finds no neighbour — the focus-ring
    // fallback is what keeps ctrl+h/l walking the card sequence.
    use crate::event::KeyCode;
    let mut app = demo();
    app.layout = Layout::Dashboard;
    app.focus = Focus::Main;
    let _ = render_layout(&mut app, Layout::Dashboard, NARROW, 22);
    assert!(
        matches!(
            ctrl_key(&mut app, NARROW, 22, KeyCode::Char('h')),
            Action::FocusToward(-1, 0)
        ),
        "the binding is layout-independent"
    );

    let before = app.focus;
    app.update(Action::FocusToward(-1, 0));
    assert_ne!(app.focus, before, "ctrl+h still changes card when narrow");
}

#[test]
fn a_vertical_jump_with_nothing_there_does_nothing() {
    // Unlike the horizontal axis there is no ring fallback: a nudge into empty
    // space must not teleport focus somewhere unrelated.
    let mut app = demo();
    app.layout = Layout::Dashboard;
    app.focus = Focus::Main;
    let _ = render_layout(&mut app, Layout::Dashboard, WIDE, 30);
    app.update(Action::FocusToward(0, -1));
    assert_eq!(app.focus, Focus::Main, "nothing above the main pane");
}

#[test]
fn ctrl_direction_survives_panes_that_bind_the_plain_key() {
    // Regression: every view/pane handler matches `KeyCode::Char('h')` without
    // inspecting modifiers, so they swallowed ctrl+h before it reached the key
    // table — the directional keys worked *only* in a plain list, which is the one
    // place they are least needed. The original test covered exactly that case and
    // so passed while the feature was broken everywhere else.
    use crate::app::LocalSection;
    use crate::event::{Key, KeyCode, Mods};
    let ctrl_h = Key {
        code: KeyCode::Char('h'),
        mods: Mods {
            ctrl: true,
            ..Mods::default()
        },
    };

    // a cover grid binds plain h/l to card movement
    let mut a = demo();
    a.layout = Layout::Dashboard;
    a.focus = Focus::Main;
    a.local.section = LocalSection::Albums;
    a.local_load_section();
    a.local.grid = true;
    assert!(a.grid_nav_active(), "the grid claims plain h/l");
    assert!(
        matches!(crate::keymap::map(&a, ctrl_h), Action::FocusToward(-1, 0)),
        "ctrl+h still jumps panes from inside a grid"
    );

    // the Library columns bind plain h/l to column switching
    let mut a = demo();
    a.layout = Layout::LibraryFocus;
    a.focus = Focus::Main;
    assert!(
        matches!(crate::keymap::map(&a, ctrl_h), Action::FocusToward(-1, 0)),
        "ctrl+h still jumps panes in the Library columns"
    );

    // ...but a modal still owns the keyboard
    let mut a = demo();
    a.layout = Layout::Dashboard;
    a.update(Action::OpenEqualizer);
    assert!(
        !matches!(crate::keymap::map(&a, ctrl_h), Action::FocusToward(..)),
        "an open modal keeps the keyboard — no jumping out of it"
    );
}

#[test]
fn history_keys_survive_a_legacy_terminal() {
    // The control-code ranges decide which ctrl+<key> combinations a terminal can
    // even express, and they are not uniform:
    //
    //   0x01..=0x1A → ctrl+a … ctrl+z   (every terminal)
    //   0x1C..=0x1F → ctrl+4 … ctrl+7   (so ctrl+] = 0x1D arrives as ctrl-5)
    //   0x09 = Tab, 0x0D = Enter, 0x1B = Esc — unusable as distinct bindings
    //
    // ctrl-b / ctrl-f sit in the first range, so they work everywhere. Regression
    // guard: ctrl+] once looked symmetric with ctrl+[ and silently did nothing in
    // iTerm2, because only ctrl+[ (= Esc) happened to land on a bound action.
    use crate::event::{Key, KeyCode, Mods};
    let ctrl = |c| Key {
        code: KeyCode::Char(c),
        mods: Mods {
            ctrl: true,
            ..Mods::default()
        },
    };
    let a = demo();

    assert!(matches!(crate::keymap::map(&a, ctrl('b')), Action::Back));
    assert!(matches!(crate::keymap::map(&a, ctrl('f')), Action::Forward));

    // ctrl+] as a legacy terminal actually sends it: ']' (0x5D) & 0x1F = 0x1D,
    // which crossterm reports as ctrl-5. Binding that byte is what makes the key
    // work outside the kitty protocol.
    assert!(matches!(crate::keymap::map(&a, ctrl('5')), Action::Forward));

    // Esc is back in its own right, which is why ctrl+[ needs no such entry —
    // '[' (0x5B) & 0x1F = 0x1B, which *is* Escape.
    assert!(matches!(
        crate::keymap::map(
            &a,
            Key {
                code: KeyCode::Esc,
                mods: Mods::default()
            }
        ),
        Action::Back
    ));
}

#[test]
fn bound_ctrl_keys_are_never_swallowed_by_a_view() {
    // Every view/pane handler matches bare characters, so each one silently ate the
    // ctrl combinations sharing a letter with its keys: in the Spotify view ctrl+f
    // "liked" the track and ctrl+b toggled the sidebar instead of moving through
    // history. Fixing letters one at a time only ever fixes the ones already
    // tripped over, so this asserts the rule.
    use crate::event::{Key, KeyCode, Mods};
    let ctrl = |c| Key {
        code: KeyCode::Char(c),
        mods: Mods {
            ctrl: true,
            ..Mods::default()
        },
    };

    for layout in [Layout::Spotify, Layout::Radio, Layout::Dashboard] {
        let mut a = demo();
        a.layout = layout;
        a.focus = Focus::Main;
        assert!(
            matches!(crate::keymap::map(&a, ctrl('f')), Action::Forward),
            "{layout:?}: ctrl+f is forward, not the view's `f`"
        );
        assert!(
            matches!(crate::keymap::map(&a, ctrl('b')), Action::Back),
            "{layout:?}: ctrl+b is back, not the view's `b`"
        );
        // ...while the PLAIN key still belongs to the view
        let plain_f = Key {
            code: KeyCode::Char('f'),
            mods: Mods::default(),
        };
        assert!(
            !matches!(crate::keymap::map(&a, plain_f), Action::Forward),
            "{layout:?}: plain f keeps the view's own meaning"
        );
    }

    // an unbound ctrl combination still falls through to the view
    let mut a = demo();
    a.layout = Layout::Dashboard;
    assert!(
        !matches!(crate::keymap::map(&a, ctrl('w')), Action::Forward),
        "an unbound ctrl key is not hijacked"
    );
}
