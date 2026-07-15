//! Key → Action mapping. Bindings are data-driven: [`DEFAULT_BINDINGS`] are the
//! built-in defaults, overlaid by the user's `keybindings.toml` (see
//! `crate::config::Keymap`). A key press is turned into a label (e.g. `"ctrl-c"`,
//! `"space"`, `"G"`), looked up, and the resulting action string is parsed.
//!
//! [`map`] resolves a press by walking a fixed priority of *context handlers*,
//! each a small `fn(&AppState, Key) -> Option<Action>`: the first that claims the
//! key wins (`Some`), otherwise it falls through (`None`) to the next, ending at
//! the global keybinding table. Capturing handlers (text inputs, popups) return
//! `Some(Noop)` for keys they don't use, so nothing leaks past an open modal.

use crate::action::{Action, Caret, Motion};
use crate::app::{AppState, Focus, InfoTab, Layout, LocalSection, Panel};
use crate::event::{Key, KeyCode, Mods};

mod catalog;
pub use catalog::*;

/// Translate a key press into an [`Action`] given current app context. Handlers
/// are tried in priority order; the first to return `Some` wins.
pub fn map(app: &AppState, key: Key) -> Action {
    // Fold Shift into the char case up front so every downstream handler (the
    // direct `Char('F')` matches AND the global-table `key_label`) sees one
    // encoding regardless of the terminal's keyboard protocol (see
    // [`normalize_shift`]).
    let key = normalize_shift(key);
    capture_rebinding(app, key)
        .or_else(|| capture_confirm_logout(app, key))
        .or_else(|| eq_overlay(app, key))
        .or_else(|| universal_nav(key))
        .or_else(|| tags_tab_switch(app, key))
        .or_else(|| cover_popup(app, key))
        .or_else(|| tagsearch_popup(app, key))
        .or_else(|| palette(app, key))
        .or_else(|| info_overlay(app, key))
        .or_else(|| search_input(app, key))
        .or_else(|| add_targets_picker(app, key))
        .or_else(|| sp_playlist_modal(app, key))
        .or_else(|| sp_confirm_delete_playlist(app, key))
        .or_else(|| tag_editor(app, key))
        .or_else(|| naming_input(app, key))
        .or_else(|| confirm_input(app, key))
        .or_else(|| modal_overlay(app, key))
        .or_else(|| pane_context(app, key))
        .or_else(|| spotify_view(app, key))
        .or_else(|| radio_view(app, key))
        .or_else(|| sidebar_playlists_context(app, key))
        .or_else(|| grid_context(app, key))
        .or_else(|| library_view(app, key))
        .or_else(|| visual_select(app, key))
        .unwrap_or_else(|| global_binding(app, key))
}

/// Fold a held Shift into the character's case so key handling is identical across
/// terminal keyboard protocols. Legacy terminals deliver `Shift+f` as the uppercase
/// char `'F'` (Shift consumed into the case); the kitty keyboard protocol (Ghostty,
/// Kitty) instead delivers the base char `'f'` with the SHIFT modifier set. Without
/// normalisation the two encodings disagree and every uppercase binding (`F`, `G`,
/// `Q`, …) silently degrades to its lowercase meaning under the kitty protocol — e.g.
/// `F` (cycle lyrics format) would fire `f` (toggle favourite). We converge on the
/// legacy form: uppercase an ASCII letter when Shift is held and clear the now-
/// redundant bit. Symbols and digits are left untouched — their shifted glyph is
/// keyboard-layout dependent, so we don't synthesise it here.
fn normalize_shift(key: Key) -> Key {
    if let KeyCode::Char(c) = key.code
        && key.mods.shift
        && c.is_ascii_lowercase()
    {
        return Key {
            code: KeyCode::Char(c.to_ascii_uppercase()),
            mods: Mods {
                shift: false,
                ..key.mods
            },
        };
    }
    key
}

/// Library (#2) 3-column browser: while the columns are focused, h/l/←/→ switch the
/// active column (otherwise they'd shift focus between panes). Vertical motion,
/// Enter, and every global binding fall through unchanged.
fn library_view(app: &AppState, key: Key) -> Option<Action> {
    if app.layout != Layout::LibraryFocus || app.focus != Focus::Main || app.is_searching() {
        return None;
    }
    Some(match key.code {
        KeyCode::Char('h') | KeyCode::Left => Action::Move(Motion::Left),
        KeyCode::Char('l') | KeyCode::Right => Action::Move(Motion::Right),
        _ => return None,
    })
}

/// Rebinding capture: the next key press (re)binds the pending action.
fn capture_rebinding(app: &AppState, key: Key) -> Option<Action> {
    app.settings.rebinding.as_ref()?;
    Some(match key.code {
        KeyCode::Esc => Action::Back,
        _ => Action::RebindKey(key_label(key)),
    })
}

/// Spotify log-out confirmation (status-bar prompt): modal until answered —
/// y/⏎ confirm, n/Esc cancel, everything else is ignored.
fn capture_confirm_logout(app: &AppState, key: Key) -> Option<Action> {
    if !app.settings.confirm_logout {
        return None;
    }
    Some(match key.code {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => Action::Activate,
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Action::Back,
        _ => Action::Noop,
    })
}

/// The Equalizer overlay is a self-contained modal: while it's open it owns the
/// whole keyboard so nothing leaks to the view behind it. ←/→ (h/l) select a band
/// (the preamp is the last control), ↑/↓ (k/j) adjust it ±1 dB, Tab / [ ] cycle
/// presets, ⏎/space toggle the EQ on/off, r resets to flat, 0 zeros the selected
/// control, s saves a custom preset, del/x deletes the active custom one, f/=/+
/// resize, Esc closes. While naming a preset every key edits the name.
fn eq_overlay(app: &AppState, key: Key) -> Option<Action> {
    if !app.eq_open() {
        return None;
    }
    // naming a custom preset: capture typing (⏎ saves, Esc cancels)
    if let Some(buf) = app.eq.naming.as_ref() {
        return Some(match key.code {
            KeyCode::Esc => Action::Back,
            KeyCode::Enter => Action::EqSavePreset,
            KeyCode::Backspace => {
                let mut b = buf.clone();
                b.pop();
                Action::EqNameInput(b)
            }
            KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => {
                Action::EqNameInput(format!("{buf}{c}"))
            }
            _ => Action::Noop,
        });
    }
    Some(match key.code {
        KeyCode::Esc => Action::Back,
        KeyCode::Left | KeyCode::Char('h') => Action::EqSelect(-1),
        KeyCode::Right | KeyCode::Char('l') => Action::EqSelect(1),
        KeyCode::Up | KeyCode::Char('k') => Action::EqAdjust(1),
        KeyCode::Down | KeyCode::Char('j') => Action::EqAdjust(-1),
        KeyCode::Enter | KeyCode::Char(' ') => Action::EqTogglePower,
        KeyCode::Tab | KeyCode::Char(']') => Action::EqCyclePreset(1),
        KeyCode::BackTab | KeyCode::Char('[') => Action::EqCyclePreset(-1),
        KeyCode::Char('r') => Action::EqReset,
        KeyCode::Char('0') => Action::EqResetBand,
        KeyCode::Char('s') => Action::EqBeginSave,
        KeyCode::Char('x') | KeyCode::Delete => Action::EqDeletePreset,
        KeyCode::Char('f') | KeyCode::Char('=') | KeyCode::Char('+') => Action::CycleOverlaySize,
        // the key that opens the EQ (default `e`) also closes it
        _ if matches!(global_binding(app, key), Action::OpenEqualizer) => Action::OpenEqualizer,
        _ => Action::Noop,
    })
}

/// Universal item navigation: ctrl-n / ctrl-p move down / up in whatever list,
/// menu, or song list is focused (routed by `AppState::nav`) — works in every
/// context, including overlays that capture the rest of the keyboard.
fn universal_nav(key: Key) -> Option<Action> {
    if key.mods.ctrl && !key.mods.alt {
        match key.code {
            KeyCode::Char('n') => return Some(Action::NavDown),
            KeyCode::Char('p') => return Some(Action::NavUp),
            _ => {}
        }
    }
    None
}

/// Unified Tag Edit modal: Tab / ⇧Tab switch tabs (Edit · Auto Tag · Cover),
/// consistent with every other tabbed overlay. Falls through for every other key
/// so the per-tab handlers below see it.
fn tags_tab_switch(app: &AppState, key: Key) -> Option<Action> {
    if !app.tags_open() {
        return None;
    }
    // Tab / ⇧Tab switch the modal's tabs (consistent with every other overlay).
    // The Edit tab's find/replace prompt owns Tab (toggles its find⇄replace
    // boxes), so don't steal it there.
    let replace_active = app
        .tags
        .edit
        .as_ref()
        .is_some_and(|te| te.replace.is_some());
    match key.code {
        KeyCode::Tab if !replace_active => Some(Action::OverlayTab(1)),
        KeyCode::BackTab if !replace_active => Some(Action::OverlayTab(-1)),
        _ => None,
    }
}

/// Album-art search popup captures input while open (Cover tab).
fn cover_popup(app: &AppState, key: Key) -> Option<Action> {
    let cs = match &app.tags.cover {
        Some(cs) if app.tags.tab == 2 => cs,
        _ => return None,
    };
    if cs.confirm {
        return Some(match key.code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => Action::CoverConfirm,
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => Action::Back,
            _ => Action::Noop,
        });
    }
    if cs.editing {
        return Some(match key.code {
            KeyCode::Esc => Action::Back,            // stop editing
            KeyCode::Enter => Action::CoverActivate, // re-search
            KeyCode::Backspace => Action::QueryBackspace,
            KeyCode::Delete => Action::QueryDelete,
            KeyCode::Left => Action::QueryCaret(Caret::Left),
            KeyCode::Right => Action::QueryCaret(Caret::Right),
            KeyCode::Home => Action::QueryCaret(Caret::Home),
            KeyCode::End => Action::QueryCaret(Caret::End),
            KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => Action::QueryInsert(c),
            _ => Action::Noop,
        });
    }
    Some(match key.code {
        KeyCode::Esc => Action::Back,            // close
        KeyCode::Enter => Action::CoverActivate, // embed selection
        // navigate the cover candidates: arrows, j/k, or [ ]
        KeyCode::Up | KeyCode::Char('k') | KeyCode::Left | KeyCode::Char('[') => {
            Action::CoverMove(Motion::Up)
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Right | KeyCode::Char(']') => {
            Action::CoverMove(Motion::Down)
        }
        KeyCode::Char('s') => Action::CoverToggleScope, // album ⇄ this song
        // '/' or 'e' focuses the query (start editing without changing it)
        KeyCode::Char('/') | KeyCode::Char('e') => Action::CoverInput(cs.query.clone()),
        _ => Action::Noop,
    })
}

/// Online tag-search popup captures input while open (Auto Tag tab).
fn tagsearch_popup(app: &AppState, key: Key) -> Option<Action> {
    let ts = match &app.tags.search {
        Some(ts) if app.tags.tab == 1 => ts,
        _ => return None,
    };
    if ts.pending.is_some() {
        return Some(match key.code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => Action::TagConfirm,
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => Action::Back,
            _ => Action::Noop,
        });
    }
    if ts.editing {
        return Some(match key.code {
            KeyCode::Esc => Action::Back,
            KeyCode::Enter => Action::TagActivate, // re-search
            KeyCode::Backspace => Action::QueryBackspace,
            KeyCode::Delete => Action::QueryDelete,
            KeyCode::Left => Action::QueryCaret(Caret::Left),
            KeyCode::Right => Action::QueryCaret(Caret::Right),
            KeyCode::Home => Action::QueryCaret(Caret::Home),
            KeyCode::End => Action::QueryCaret(Caret::End),
            KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => Action::QueryInsert(c),
            _ => Action::Noop,
        });
    }
    Some(match key.code {
        KeyCode::Esc => Action::Back,                 // close
        KeyCode::Enter => Action::TagActivate,        // apply (song / album)
        KeyCode::Char('a') => Action::TagApplyAlbum,  // apply album-wide
        KeyCode::Char('s') => Action::TagToggleAlbum, // single ⇄ album
        KeyCode::Up | KeyCode::Char('k') => Action::TagMove(Motion::Up),
        KeyCode::Down | KeyCode::Char('j') => Action::TagMove(Motion::Down),
        // ←/→ cycles the matched source/edition
        KeyCode::Left => Action::TagSource(-1),
        KeyCode::Right => Action::TagSource(1),
        KeyCode::Char('/') => Action::TagInput(ts.query.clone()),
        _ => Action::Noop,
    })
}

/// Command palette captures input while open.
fn palette(app: &AppState, key: Key) -> Option<Action> {
    let p = app.palette.as_ref()?;
    Some(match key.code {
        KeyCode::Esc => Action::Back,
        KeyCode::Enter => Action::PaletteActivate,
        KeyCode::Up | KeyCode::BackTab => Action::PaletteMove(Motion::Up),
        KeyCode::Down | KeyCode::Tab => Action::PaletteMove(Motion::Down),
        // → reveals the highlighted setting in the full Settings overlay
        KeyCode::Right => Action::PaletteReveal,
        // ctrl-j / ctrl-k navigate without leaving the home row
        KeyCode::Char('k') if key.mods.ctrl => Action::PaletteMove(Motion::Up),
        KeyCode::Char('j') if key.mods.ctrl => Action::PaletteMove(Motion::Down),
        KeyCode::Backspace => {
            let mut q = p.query.clone();
            q.pop();
            Action::PaletteInput(q)
        }
        KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => {
            Action::PaletteInput(format!("{}{c}", p.query))
        }
        _ => Action::Noop,
    })
}

/// The unified Info overlay captures input while open: Tab / Shift-Tab switch
/// tabs, Esc closes, arrows/page scroll the active tab. The Keys tab also types
/// to filter; the other tabs take vi-style scroll + `?`/`I` to jump tabs (and `y`
/// to copy the latest error on Health).
fn info_overlay(app: &AppState, key: Key) -> Option<Action> {
    let info = app.info.as_ref()?;
    Some(match key.code {
        KeyCode::Tab => Action::OverlayTab(1),
        KeyCode::BackTab => Action::OverlayTab(-1),
        KeyCode::Esc => Action::Back,
        // size: `=`/`+` work on every tab (incl. Keys, where typing owns `f`)
        KeyCode::Char('=') | KeyCode::Char('+') => Action::CycleOverlaySize,
        KeyCode::Up => Action::Move(Motion::Up),
        KeyCode::Down => Action::Move(Motion::Down),
        KeyCode::PageUp => Action::Move(Motion::PageUp),
        KeyCode::PageDown => Action::Move(Motion::PageDown),
        // Keys tab: typing filters the keybinding list
        _ if info.tab == InfoTab::Keys => match key.code {
            // `?` closes when the filter is empty (matches the old help toggle)
            KeyCode::Char('?') if info.keys_query.is_empty() => Action::Back,
            KeyCode::Backspace => {
                let mut q = info.keys_query.clone();
                q.pop();
                Action::HelpInput(q)
            }
            KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => {
                Action::HelpInput(format!("{}{c}", info.keys_query))
            }
            _ => Action::Noop,
        },
        // other tabs: vi-scroll + jump-tab muscle memory + `f` size (free here,
        // since only the Keys tab types)
        KeyCode::Char('f') => Action::CycleOverlaySize,
        KeyCode::Char('j') => Action::Move(Motion::Down),
        KeyCode::Char('k') => Action::Move(Motion::Up),
        KeyCode::Char('g') => Action::Move(Motion::Top),
        KeyCode::Char('G') => Action::Move(Motion::Bottom),
        KeyCode::Char('?') => Action::ToggleHelp, // jump to Keys
        KeyCode::Char('I') => Action::ToggleStats, // jump to / toggle Stats
        KeyCode::Char('y') if info.tab == InfoTab::Health => Action::CopyError,
        _ => Action::Noop,
    })
}

/// Local library search: the same shared text-capture every source view uses, so
/// typing/⏎/esc and ↑/↓ result navigation behave identically here and in
/// Spotify/Radio (previously this was a weaker bespoke handler with no list nav).
fn search_input(app: &AppState, key: Key) -> Option<Action> {
    if !app.search.active {
        return None;
    }
    Some(text_capture(
        key,
        &app.search.query,
        Action::SearchInput,
        Action::Activate,
        Action::Back,
    ))
}

/// "Add to playlist" picker captures navigation keys.
fn add_targets_picker(app: &AppState, key: Key) -> Option<Action> {
    if app.input.add_targets.is_empty() || app.input.naming.is_some() {
        return None;
    }
    Some(match key.code {
        KeyCode::Esc => Action::Back,
        KeyCode::Enter => Action::Activate,
        KeyCode::Char('j') | KeyCode::Down => Action::Move(Motion::Down),
        KeyCode::Char('k') | KeyCode::Up => Action::Move(Motion::Up),
        KeyCode::Char('n') => Action::BeginNewPlaylist,
        _ => Action::Noop,
    })
}

/// The Spotify "add / new / rename playlist" modal captures all input while open:
/// the create/rename sub-mode types a name; the picker navigates the playlist list
/// (`n` opens the new-playlist prompt, ⏎ adds, Esc backs out).
fn sp_playlist_modal(app: &AppState, key: Key) -> Option<Action> {
    let m = app.spotify.pl_modal.as_ref()?;
    if m.naming.is_some() {
        return Some(match key.code {
            KeyCode::Esc => Action::Back,
            KeyCode::Enter => Action::Activate,
            KeyCode::Backspace => {
                let mut b = m.buffer.clone();
                b.pop();
                Action::SpotifyNameInput(b)
            }
            KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => {
                Action::SpotifyNameInput(format!("{}{c}", m.buffer))
            }
            _ => Action::Noop,
        });
    }
    Some(match key.code {
        KeyCode::Esc => Action::Back,
        KeyCode::Enter => Action::Activate,
        KeyCode::Char('j') | KeyCode::Down => Action::Move(Motion::Down),
        KeyCode::Char('k') | KeyCode::Up => Action::Move(Motion::Up),
        KeyCode::Char('n') => Action::SpotifyBeginNewPlaylist,
        _ => Action::Noop,
    })
}

/// The Spotify playlist unfollow ("delete") confirmation owns the keyboard while
/// open: ⏎/y confirm, Esc/n cancel, everything else is swallowed.
fn sp_confirm_delete_playlist(app: &AppState, key: Key) -> Option<Action> {
    app.spotify.pl_confirm_delete.as_ref()?;
    Some(match key.code {
        KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => Action::Activate,
        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => Action::Back,
        _ => Action::Noop,
    })
}

/// Playlist-management keys inside the Spotify browse view (mirrors the local
/// Dashboard Playlists keys). Only claims a key when it actually applies — a
/// playlist/track is selected in the right context — so `n`/`d`/`e` keep their
/// other meanings (next-track, etc.) everywhere else. `a` (add the selected/
/// now-playing track to a playlist) applies anywhere a track can be resolved.
fn spotify_playlist_keys(app: &AppState, key: Key) -> Option<Action> {
    use crate::spotify::api::{Kind, Section};
    if !matches!(app.focus, Focus::Main | Focus::Sidebar) {
        return None;
    }
    let selected = app.spotify.items.get(app.spotify.sel);
    // Drilled into a playlist (its tracks are shown): d/x removes the selected one.
    let in_playlist = app
        .spotify
        .open_item
        .as_ref()
        .is_some_and(|it| it.kind == Kind::Playlist);
    if in_playlist
        && matches!(key.code, KeyCode::Char('d') | KeyCode::Char('x'))
        && selected.is_some_and(|it| it.kind == Kind::Track)
    {
        return Some(Action::SpotifyRemoveFromPlaylist);
    }
    // Top-level Playlists section: manage the selected playlist / create a new one.
    let on_playlists = app.spotify.section == Section::Playlists
        && app.spotify.crumb.is_none()
        && !app.spotify.in_search;
    if on_playlists {
        let on_pl = selected.is_some_and(|it| it.kind == Kind::Playlist);
        match key.code {
            KeyCode::Char('n') => return Some(Action::SpotifyNewPlaylist),
            KeyCode::Char('e') | KeyCode::Char('r') if on_pl => {
                return Some(Action::SpotifyRenamePlaylist);
            }
            KeyCode::Char('d') if on_pl => return Some(Action::SpotifyDeletePlaylist),
            _ => {}
        }
    }
    // 'a' adds the selected (or now-playing) track to a Spotify playlist — anywhere.
    if key.code == KeyCode::Char('a') {
        return Some(Action::SpotifyAddToPlaylist);
    }
    None
}

/// Tag editor (Edit tab): type to edit the focused field, arrows/Tab to move.
/// Captures all input while open (sub-modes: album-confirm, converter, replace).
fn tag_editor(app: &AppState, key: Key) -> Option<Action> {
    let te = match &app.tags.edit {
        Some(te) if app.tags.tab == 0 => te,
        _ => return None,
    };
    // apply-to-album confirmation captures Enter/Esc first
    if te.confirm_album {
        return Some(match key.code {
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => Action::TagEditSaveAlbum,
            _ => Action::TagEditAlbumCancel,
        });
    }
    // converter pattern prompt (filename↔tags) captures all input
    if let Some((_, buf)) = &te.convert {
        return Some(match key.code {
            KeyCode::Esc => Action::TagConvertCancel,
            KeyCode::Enter => Action::TagConvertApply,
            KeyCode::Backspace => {
                let mut s = buf.clone();
                s.pop();
                Action::TagConvertType(s)
            }
            KeyCode::Char(c) => Action::TagConvertType(format!("{buf}{c}")),
            _ => Action::Noop,
        });
    }
    // find-&-replace prompt (focused field) captures all input. Tab toggles
    // between the find and replace boxes; the active one receives typing.
    if let Some((find, repl, on_repl)) = &te.replace {
        let buf = if *on_repl { repl } else { find };
        return Some(match key.code {
            KeyCode::Esc => Action::TagReplaceCancel,
            KeyCode::Enter => Action::TagReplaceApply,
            KeyCode::Tab | KeyCode::BackTab => Action::TagReplaceToggle,
            KeyCode::Backspace => {
                let mut s = buf.clone();
                s.pop();
                Action::TagReplaceType(s)
            }
            KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => {
                Action::TagReplaceType(format!("{buf}{c}"))
            }
            _ => Action::Noop,
        });
    }
    if te.editing {
        // typing into the focused field. ←/→/Home/End move the caret,
        // Backspace/Delete edit at the caret, ↑↓ change field, Enter/Esc return
        // to browsing. (Tab/⇧Tab switch the modal's tabs — handled above.)
        return Some(match key.code {
            KeyCode::Char('s') if key.mods.ctrl => Action::TagEditSave, // ^S saves directly
            KeyCode::Esc | KeyCode::Enter => Action::TagEditStopEdit,
            KeyCode::Down => Action::TagEditMove(Motion::Down),
            KeyCode::Up => Action::TagEditMove(Motion::Up),
            KeyCode::Left => Action::TagEditCaret(Caret::Left),
            KeyCode::Right => Action::TagEditCaret(Caret::Right),
            KeyCode::Home => Action::TagEditCaret(Caret::Home),
            KeyCode::End => Action::TagEditCaret(Caret::End),
            KeyCode::Backspace => Action::TagEditBackspace,
            KeyCode::Delete => Action::TagEditDelete,
            KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => Action::TagEditInsert(c),
            _ => Action::Noop,
        });
    }
    // browsing fields: navigate, run actions, save (`s`), cancel (Esc/q).
    // Enter starts editing the focused field — it does not save.
    Some(match key.code {
        KeyCode::Esc | KeyCode::Char('q') => Action::TagEditCancel,
        KeyCode::Char('s') => Action::TagEditSave,
        KeyCode::Char('a') => Action::TagEditAlbumPrompt, // confirm, then apply album-wide
        KeyCode::Enter | KeyCode::Char('i') => Action::TagEditBeginEdit,
        KeyCode::Backspace => Action::TagEditClear,
        KeyCode::Char('j') | KeyCode::Down => Action::TagEditMove(Motion::Down),
        KeyCode::Char('k') | KeyCode::Up => Action::TagEditMove(Motion::Up),
        // size: plain `f` (ctrl-`f` below is Convert) or `=`/`+`
        KeyCode::Char('=') | KeyCode::Char('+') => Action::CycleOverlaySize,
        KeyCode::Char('f') if !key.mods.ctrl => Action::CycleOverlaySize,
        KeyCode::Char('t') if key.mods.ctrl => Action::TagEditCase(0),
        KeyCode::Char('u') if key.mods.ctrl => Action::TagEditCase(1),
        KeyCode::Char('l') if key.mods.ctrl => Action::TagEditCase(2),
        KeyCode::Char('n') if key.mods.ctrl => Action::TagEditAutoNumber,
        KeyCode::Char('f') if key.mods.ctrl => Action::TagConvertBegin,
        KeyCode::Char('r') if key.mods.ctrl => Action::TagRenameBegin,
        KeyCode::Char('e') if key.mods.ctrl => Action::TagReplaceBegin,
        KeyCode::Char('d') if key.mods.ctrl => Action::TagRemoveField,
        _ => Action::Noop,
    })
}

/// Naming input (new / rename playlist) captures all keys.
fn naming_input(app: &AppState, key: Key) -> Option<Action> {
    app.input.naming.as_ref()?;
    Some(match key.code {
        KeyCode::Esc => Action::Back,
        KeyCode::Enter => Action::Activate,
        KeyCode::Backspace => {
            let mut b = app.input.buffer.clone();
            b.pop();
            Action::NameInput(b)
        }
        KeyCode::Char(c) => Action::NameInput(format!("{}{}", app.input.buffer, c)),
        _ => Action::Noop,
    })
}

/// While a yes/no confirmation dialog (currently the playlist delete) is open it
/// owns the keyboard: `Enter`/`y` confirm, `Esc`/`n` cancel, everything else is
/// swallowed so no stray global command fires behind the dialog.
fn confirm_input(app: &AppState, key: Key) -> Option<Action> {
    app.input.confirm_delete?;
    Some(match key.code {
        KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => Action::Activate,
        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => Action::Back,
        _ => Action::Noop,
    })
}

/// A modal overlay/popup (per-view settings popup, full Settings, stats,
/// metadata) owns the screen: resolve straight to the global binding and let
/// through ONLY overlay-safe actions (navigation / value-adjust / confirm-cancel
/// plus quit & the overlay's own toggle that dismiss it). This stops global
/// one-key commands (`v`, `space`, …) and view switching (`1`–`7`) from
/// leaking in. Text-input modals are captured above and never reach here.
fn modal_overlay(app: &AppState, key: Key) -> Option<Action> {
    if !app.modal_overlay_open() {
        return None;
    }
    // the tabbed Settings overlay + per-view popup: Tab / Shift-Tab switch tabs,
    // `f` (or `=`/`+`) cycles the overlay size up, and h/l (←/→) step the selected
    // row's value (j/k move between rows via the global `move` binding below).
    if app.settings.overlay || app.settings.popup.is_some() {
        match key.code {
            KeyCode::Tab => return Some(Action::OverlayTab(1)),
            KeyCode::BackTab => return Some(Action::OverlayTab(-1)),
            KeyCode::Char('f') | KeyCode::Char('=') | KeyCode::Char('+') => {
                return Some(Action::CycleOverlaySize);
            }
            KeyCode::Char('h') | KeyCode::Left => return Some(Action::SettingsAdjust(-1)),
            KeyCode::Char('l') | KeyCode::Right => return Some(Action::SettingsAdjust(1)),
            _ => {}
        }
    }
    let action = match app.config.keymap.binding(&key_label(key)) {
        Some(a) => parse_action(a, app),
        None => Action::Noop,
    };
    Some(if action.allowed_in_overlay() {
        action
    } else {
        Action::Noop
    })
}

/// Standard text-input capture for a search / filter box, shared by every source
/// view's typing sub-mode (Spotify search, Radio search, Radio picker filter):
/// Esc/Tab leave it, Enter activates, arrows/page move the highlighted match, and
/// Backspace/typing edit the live `query`. `input` builds the per-source input
/// action from the new query (e.g. `Action::SpotifyInput`).
fn text_capture(
    key: Key,
    query: &str,
    input: fn(String) -> Action,
    enter: Action,
    leave: Action,
) -> Action {
    match key.code {
        KeyCode::Esc | KeyCode::Tab | KeyCode::BackTab => leave,
        KeyCode::Enter => enter,
        KeyCode::Up => Action::Move(Motion::Up),
        KeyCode::Down => Action::Move(Motion::Down),
        KeyCode::PageUp => Action::Move(Motion::PageUp),
        KeyCode::PageDown => Action::Move(Motion::PageDown),
        KeyCode::Backspace => {
            let mut q = query.to_string();
            q.pop();
            input(q)
        }
        KeyCode::Char(c) if !key.mods.ctrl && !key.mods.alt => input(format!("{query}{c}")),
        _ => Action::Noop,
    }
}

/// Spotify view: ⏎ logs in when disconnected, the search box captures typing, and
/// browse mode has its own single-key commands. List navigation + any unclaimed
/// key fall through to the global bindings (number keys switch views, `q` quits…).
fn spotify_view(app: &AppState, key: Key) -> Option<Action> {
    if app.layout != Layout::Spotify {
        return None;
    }
    let connected = matches!(
        app.spotify.conn,
        crate::spotify::ConnState::Connected { .. }
    );
    if !connected {
        match key.code {
            KeyCode::Enter => return Some(Action::SpotifyLogin),
            // set this account's own client id (e.g. after a "not registered"
            // error from switching accounts) without first being connected
            KeyCode::Char('c') => return Some(Action::SpotifyWriteConfig),
            _ => {}
        }
    } else if app.spotify.searching {
        // search box focused: shared text-input capture
        return Some(text_capture(
            key,
            &app.spotify.query,
            Action::SpotifyInput,
            Action::SpotifyActivate,
            Action::SpotifyCancel,
        ));
    } else {
        // browse mode: source-specific keys only. List navigation (j/k/g/G/arrows/
        // page) is left to the shared global table via the fall-through below.
        // When the cover grid is up AND the result list is focused, the directional
        // keys move 2-D instead (h/l one card, j/k a whole row) — `g`/`G`/`#`/⏎ still
        // fall through. Other focuses (sidebar / a pane) keep their own j/k. This
        // covers the section grid AND an artist page's album region (where the cursor
        // is on a release card; the POPULAR tracks above keep plain list nav).
        // on a "track list + carousels" page (artist / search), route h/l/j/k to the
        // grid nav only when the cursor is in the card region (the leading track list
        // keeps plain list nav).
        if app.grid_nav_active() {
            match key.code {
                KeyCode::Char('h') | KeyCode::Left => return Some(Action::GridMove(-1, 0)),
                KeyCode::Char('l') | KeyCode::Right => return Some(Action::GridMove(1, 0)),
                KeyCode::Char('j') | KeyCode::Down => return Some(Action::GridMove(0, 1)),
                KeyCode::Char('k') | KeyCode::Up => return Some(Action::GridMove(0, -1)),
                _ => {}
            }
        }
        // playlist management (a=add, and in the Playlists section n/e/r/d) takes
        // priority so contextual `n`=new-playlist wins over `n`=next-track there.
        if let Some(a) = spotify_playlist_keys(app, key) {
            return Some(a);
        }
        match key.code {
            KeyCode::Esc => return Some(Action::SpotifyCancel),
            KeyCode::Enter => return Some(Action::SpotifyActivate),
            KeyCode::Tab => return Some(Action::SpotifyCycleFocus(1)),
            KeyCode::BackTab => return Some(Action::SpotifyCycleFocus(-1)),
            KeyCode::Char('b') => return Some(Action::SpotifyToggleSidebar),
            KeyCode::Char('/') => return Some(Action::SpotifyFocusSearch),
            KeyCode::Char('n') => return Some(Action::SpotifyTrack(1)),
            KeyCode::Char('p') => return Some(Action::SpotifyTrack(-1)),
            KeyCode::Char('f') => return Some(Action::SpotifyLike),
            KeyCode::Char('F') => return Some(Action::SpotifyFollow),
            KeyCode::Char('c') => return Some(Action::SpotifyWriteConfig),
            _ => {}
        }
    }
    // list navigation + anything else → the global bindings (view switch, quit…)
    Some(global_binding(app, key))
}

/// The radio view's global fall-through: like [`global_binding`], but shuffle and
/// repeat are inert. A live stream has no queue to shuffle and no track to repeat,
/// so `s`/`r` (or whatever the user bound those to) would only disturb the frozen
/// local player behind the radio overlay — not something a radio browse should do.
/// Matched by resolved *action*, so it holds regardless of how the keys are bound.
fn radio_global(app: &AppState, key: Key) -> Action {
    match global_binding(app, key) {
        Action::ToggleShuffle | Action::CycleRepeat => Action::Noop,
        other => other,
    }
}

/// Radio view: the station list is focused by default with dedicated keys; the
/// search box and the country/genre pickers are explicit sub-modes that capture
/// typing while open. Unclaimed keys fall through to [`radio_global`] (so `q`
/// quits, space pauses, the number keys switch views — but shuffle/repeat stay
/// inert, having no meaning for a queue-less live stream).
fn radio_view(app: &AppState, key: Key) -> Option<Action> {
    if app.layout != Layout::Radio {
        return None;
    }
    // A playlist modal (name entry / add-to-playlist picker / delete confirm) is
    // open — it captures every key so no browse command fires beneath it.
    if app.radio.pl.modal_open() {
        if app.radio.pl.naming.is_some() {
            return Some(text_capture(
                key,
                &app.radio.pl.buffer,
                Action::RadioNameInput,
                Action::RadioModalConfirm,
                Action::RadioModalCancel,
            ));
        }
        if app.radio.pl.confirm_delete.is_some() {
            return Some(match key.code {
                KeyCode::Char('y') | KeyCode::Enter => Action::RadioModalConfirm,
                _ => Action::RadioModalCancel,
            });
        }
        // the add-to-playlist picker: j/k move, Enter adds (or opens New), Esc closes
        return Some(match key.code {
            KeyCode::Esc => Action::RadioModalCancel,
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => Action::RadioModalConfirm,
            KeyCode::Up | KeyCode::Char('k') => Action::Move(Motion::Up),
            KeyCode::Down | KeyCode::Char('j') => Action::Move(Motion::Down),
            _ => Action::Noop,
        });
    }
    // A country/genre picker is open. Like the station list, it defaults to
    // navigation (j/k/arrows move); '/' focuses its filter box for typing.
    if let Some(p) = &app.radio.picker {
        if p.editing {
            // filter box focused: shared text-input capture
            return Some(text_capture(
                key,
                &p.query,
                Action::RadioPickerInput,
                Action::RadioActivate,
                Action::RadioPickerEndSearch,
            ));
        }
        // picker nav (modal — captures every key so no global command fires while
        // it's open): j/k/arrows move, '/' filters, Enter applies, Esc closes
        return Some(match key.code {
            KeyCode::Esc => Action::RadioCancel,
            KeyCode::Enter => Action::RadioActivate,
            KeyCode::Up | KeyCode::Char('k') => Action::Move(Motion::Up),
            KeyCode::Down | KeyCode::Char('j') => Action::Move(Motion::Down),
            KeyCode::PageUp => Action::Move(Motion::PageUp),
            KeyCode::PageDown => Action::Move(Motion::PageDown),
            KeyCode::Char('/') => Action::RadioPickerStartSearch,
            KeyCode::Char('g') => Action::Move(Motion::Top),
            KeyCode::Char('G') => Action::Move(Motion::Bottom),
            KeyCode::Backspace if !p.query.is_empty() => Action::RadioPickerInput(String::new()),
            _ => Action::Noop,
        });
    }
    // Search box focused: shared text-input capture (Tab/Esc hand focus back).
    if app.radio.editing {
        return Some(text_capture(
            key,
            &app.radio.query,
            Action::RadioInput,
            Action::RadioActivate,
            Action::RadioCancel,
        ));
    }
    // Modifier combos are never radio single-key commands — let the global table
    // resolve them (ctrl-o = back, ctrl-q = queue …); ctrl-n/p are handled upstream
    // by `universal_nav`.
    if key.mods.ctrl || key.mods.alt {
        return Some(global_binding(app, key));
    }
    // Browse mode. Keys common to both panes first, then pane-specific keys; Tab
    // (cycle focus, sidebar ↔ list) and j/k (Move) fall through to the global table.
    match key.code {
        KeyCode::Esc => return Some(Action::RadioCancel),
        KeyCode::Char('/') => return Some(Action::RadioFocusSearch),
        // Enter plays a station on the list, or activates the section in the sidebar
        // (radio_activate branches on focus).
        KeyCode::Enter => return Some(Action::RadioActivate),
        _ => {}
    }
    // Section sidebar focused: l/→ enters the section (opens a picker / focuses the
    // list); h/← is a no-op; j/k + Tab fall through to the global bindings.
    if app.focus == Focus::Sidebar {
        return Some(match key.code {
            KeyCode::Char('l') | KeyCode::Right => Action::RadioActivate,
            KeyCode::Char('h') | KeyCode::Left => Action::Noop,
            _ => radio_global(app, key),
        });
    }
    // vim horizontal nav from the main pane: h/← jumps back to the section sidebar
    // (its counterpart l/→ enters the main pane from the sidebar, above).
    if matches!(key.code, KeyCode::Char('h') | KeyCode::Left) {
        return Some(Action::FocusPane(Focus::Sidebar));
    }
    // Playlists section: the flat list (create/rename/delete, ⏎ drills in) vs a
    // drilled-in playlist (its stations: d/x removes, a adds elsewhere, f stars).
    if app.radio.section == crate::app::RadioSection::Playlists {
        if app.radio.pl.open.is_none() {
            return Some(match key.code {
                KeyCode::Char('n') => Action::RadioNewPlaylist,
                KeyCode::Char('d') => Action::RadioDeletePlaylist,
                KeyCode::Char('r') | KeyCode::Char('e') => Action::RadioRenamePlaylist,
                KeyCode::Char('l') | KeyCode::Right => Action::RadioActivate, // drill in
                // no station under the cursor here — swallow the station operators so
                // they can't fall through to the local-music add-to-playlist / mark
                KeyCode::Char('a') | KeyCode::Char('x') => Action::Noop,
                _ => radio_global(app, key), // j/k move, Tab, q…
            });
        }
        match key.code {
            KeyCode::Char('d') | KeyCode::Char('x') => {
                return Some(Action::RadioRemoveFromPlaylist);
            }
            KeyCode::Char('a') => return Some(Action::RadioAddToPlaylist),
            KeyCode::Char('f') => return Some(Action::RadioStar),
            KeyCode::Char('n') => return Some(Action::RadioStation(1)),
            KeyCode::Char('p') => return Some(Action::RadioStation(-1)),
            _ => {}
        }
        return Some(radio_global(app, key));
    }
    // Station list focused: source-specific keys (`n`/`p` change station, not the
    // local queue; `f` stars, `a` adds the station to a playlist).
    match key.code {
        KeyCode::Char('c') => return Some(Action::RadioOpenCountry),
        KeyCode::Char('g') => return Some(Action::RadioOpenGenre),
        KeyCode::Char('f') => return Some(Action::RadioStar),
        KeyCode::Char('a') => return Some(Action::RadioAddToPlaylist),
        KeyCode::Char('o') => return Some(Action::RadioCycleSort),
        KeyCode::Char('R') => return Some(Action::RadioRefresh), // re-download directory
        KeyCode::Char('n') => return Some(Action::RadioStation(1)), // next channel
        KeyCode::Char('p') => return Some(Action::RadioStation(-1)), // prev channel
        _ => {}
    }
    // list navigation + anything else → the (shuffle/repeat-filtered) global bindings
    Some(radio_global(app, key))
}

/// Context keys: browsing the Playlists section (Dashboard, main pane) exposes
/// playlist-management actions. Only claims its own keys; everything else falls
/// through to the global bindings.
fn sidebar_playlists_context(app: &AppState, key: Key) -> Option<Action> {
    // Active while browsing the Playlists section, whether focus is on the section
    // sidebar or the list itself — so `n` (new) works the moment you pick the
    // section. rename/delete/add resolve the selected list item and no-op otherwise.
    if app.layout != Layout::Dashboard
        || !matches!(app.focus, Focus::Main | Focus::Sidebar)
        || app.local.section != LocalSection::Playlists
        || app.is_searching()
    {
        return None;
    }
    // Drilled *into* a (normal) playlist → the list shows its tracks, so `d`/`x`
    // removes the selected track from that playlist (the management keys below
    // only make sense on the playlists list itself).
    if app.current_local_playlist().is_some() {
        return match key.code {
            KeyCode::Char('d') | KeyCode::Char('x') => Some(Action::RemoveFromPlaylist),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Char('n') => Some(Action::BeginNewPlaylist),
        KeyCode::Char('S') => Some(Action::NewSmartPlaylist),
        KeyCode::Char('d') => Some(Action::DeletePlaylist),
        KeyCode::Char('e') | KeyCode::Char('r') => Some(Action::BeginRenamePlaylist),
        KeyCode::Char('a') => Some(Action::AddCurrentToPlaylist),
        _ => None,
    }
}

/// In the Albums/Artists cover-art grid the directional keys move 2-D: `h`/`l`
/// (and ←/→) step one card, `j`/`k` (and ↑/↓) move a whole row. Everything else
/// falls through — so `g`/`G` jump to the first/last card, `#` toggles back to a
/// list, `⏎` opens, etc. Active for the full-pane grid, OR the artist page's
/// trailing ALBUMS grid while the cursor is in it (the POPULAR tracks above keep
/// the plain list nav, so `k` off the grid's top row falls back into them).
fn grid_context(app: &AppState, key: Key) -> Option<Action> {
    // `grid_nav_active` already encodes Dashboard + Main + not-searching + (grid or
    // in the artist page's album region); `spotify_view` handles the Spotify layout
    // earlier in the chain, so this only fires for the local grid.
    if !app.grid_nav_active() {
        return None;
    }
    Some(match key.code {
        KeyCode::Char('h') | KeyCode::Left => Action::GridMove(-1, 0),
        KeyCode::Char('l') | KeyCode::Right => Action::GridMove(1, 0),
        KeyCode::Char('j') | KeyCode::Down => Action::GridMove(0, 1),
        KeyCode::Char('k') | KeyCode::Up => Action::GridMove(0, -1),
        _ => return None,
    })
}

/// Focus-scoped pane keys — a single place that gives the focused pane first crack
/// at a press, with anything it doesn't claim falling through to the global table.
/// Each pane's vocabulary lives in one `*_keys` helper, so "what does this pane do"
/// is answerable in one spot. (Layout-scoped remaps — the Library columns, the
/// cover grid, the Playlists section — stay in their own handlers below: they key
/// off the *main* focus + layout, not a focused side-pane.)
///
/// Ordering: the lyrics view/pane gets first refusal (so `F`/`,`/`.` work whether
/// the dedicated Lyrics view is up or the Lyrics side-pane is focused), then the
/// specifically-focused pane. This preserves the old two-handler behaviour — e.g. in
/// the Lyrics view with the Queue pane focused, `F` cycles the format AND `K`/`J`
/// reorder the queue.
fn pane_context(app: &AppState, key: Key) -> Option<Action> {
    // The lyrics view/pane's own keys apply first (`F`, and the `,`/`.` offset nudges),
    // in the dedicated Lyrics view or with the Lyrics side-pane focused.
    if (app.layout == Layout::LyricsFocus || app.focus == Focus::Pane(Panel::Lyrics))
        && let Some(a) = lyrics_keys(key)
    {
        return Some(a);
    }
    // A focused side-pane owns the keyboard: its own keys win, then every
    // non-universal key is *shadowed* (swallowed as a no-op) so a stray global — e.g.
    // `f` = favourite while the Lyrics pane is focused — can't leak in. Only universal
    // keys (navigation, playback transport, app chrome, and the focused pane's own
    // resize/move) pass through to the global table. This is what makes a focused
    // pane expose only its own options.
    if let Focus::Pane(panel) = app.focus {
        let claimed = match panel {
            Panel::Queue => queue_keys(key),
            Panel::Visualizer => viz_keys(key),
            // Lyrics handled above; Artist/Sidebar are navigation-only.
            _ => None,
        };
        if let Some(a) = claimed {
            return Some(a);
        }
        if !is_universal_key(key) {
            return Some(Action::Noop);
        }
    }
    None
}

/// Keys that stay live no matter what owns the focus: navigation, playback
/// transport, the core app chrome (quit / search / palette / help / switch view /
/// switch focus), and the focused pane's own geometry (resize / move / fit). Every
/// *other* key is a view-content action and is shadowed while a side-pane is focused
/// (see [`pane_context`]), so only that pane's own options are reachable. Matched by
/// canonical label, so a rebound key is classified by where it lands, not its glyph.
fn is_universal_key(key: Key) -> bool {
    const UNIVERSAL: &[&str] = &[
        // navigation (move within / between regions)
        "up", "down", "left", "right", "h", "l", "pageup", "pagedown", "home", "end", "enter",
        "esc", "tab", "backtab", "j", "k", "g", "G",
        // playback transport (seek / volume / speed / shuffle / repeat)
        "space", "n", "p", ",", ".", "+", "=", "-", "[", "]", "s", "r",
        // app chrome: quit / back / palette / help / search / copy-error / switch view
        "q", "Q", "ctrl-c", "ctrl-o", ":", "?", "/", "y", "1", "2", "3", "4", "5", "6", "7",
        // the focused pane's own geometry: resize (>< }{ ), move edge (m), fit/reset (zZ)
        ">", "<", "}", "{", "m", "z", "Z",
    ];
    UNIVERSAL.contains(&key_label(key).as_str())
}

/// Lyrics pane keys: `F` cycles the lyric format (plain → karaoke → teleprompter);
/// `,` / `.` nudge the synced-lyric offset earlier / later (the unshifted `<` / `>`,
/// so the direction reads naturally — and it dodges macOS reserving ctrl+arrows for
/// Spaces). This is the one place `,` / `.` don't seek: in the lyrics view/pane they
/// adjust the sync, everywhere else they're the global seek keys.
fn lyrics_keys(key: Key) -> Option<Action> {
    match key.code {
        KeyCode::Char('F') => Some(Action::CycleLyricsFormat),
        KeyCode::Char(',') => Some(Action::LyricsOffset(-50)), // earlier
        KeyCode::Char('.') => Some(Action::LyricsOffset(50)),  // later
        _ => None,
    }
}

/// Visualizer pane keys: `v` cycles the visualizer mode — the pane's own action,
/// kept reachable so the shadow rule doesn't swallow it while the pane is focused.
fn viz_keys(key: Key) -> Option<Action> {
    match key.code {
        KeyCode::Char('v') => Some(Action::CycleVisualizer),
        _ => None,
    }
}

/// Queue pane keys: reorder (`K` up / `J` down) and remove (`d` / `x` the selected
/// track, `D` clears everything upcoming). Only claims its own keys.
fn queue_keys(key: Key) -> Option<Action> {
    match key.code {
        KeyCode::Char('K') => Some(Action::QueueMove(Motion::Up)),
        KeyCode::Char('J') => Some(Action::QueueMove(Motion::Down)),
        KeyCode::Char('d') | KeyCode::Char('x') => Some(Action::QueueRemove),
        KeyCode::Char('D') => Some(Action::QueueClearUpcoming),
        _ => None,
    }
}

/// Vim-style visual selection on the tracklist (Shift+V). Only here, so the
/// Lyrics view keeps `V` for its visualizer toggle.
fn visual_select(app: &AppState, key: Key) -> Option<Action> {
    if key.code == KeyCode::Char('V') && app.focus == Focus::Main && app.layout == Layout::Dashboard
    {
        Some(Action::VisualSelect)
    } else {
        None
    }
}

/// The global keybinding table (the data-driven default + user overrides): the
/// fallback when no context handler claimed the key.
fn global_binding(app: &AppState, key: Key) -> Action {
    match app.config.keymap.binding(&key_label(key)) {
        Some(action) => parse_action(action, app),
        None => Action::Noop,
    }
}

/// Canonical label for a key, matching `keybindings.toml` keys.
pub fn key_label(key: Key) -> String {
    let base = match key.code {
        KeyCode::Char(' ') => "space".to_string(),
        // Defensive twin of `normalize_shift`: bindings are stored uppercase, so a
        // `Shift+f` that reached here un-normalised (any caller outside `map`) still
        // resolves to `"F"`. Idempotent — an already-uppercase char skips this arm.
        KeyCode::Char(c) if key.mods.shift && c.is_ascii_lowercase() => {
            c.to_ascii_uppercase().to_string()
        }
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Esc => "esc".into(),
        KeyCode::Tab => "tab".into(),
        KeyCode::BackTab => "backtab".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Delete => "delete".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::PageUp => "pageup".into(),
        KeyCode::PageDown => "pagedown".into(),
        KeyCode::F(n) => format!("f{n}"),
    };
    let mut s = String::new();
    if key.mods.ctrl {
        s.push_str("ctrl-");
    }
    if key.mods.alt {
        s.push_str("alt-");
    }
    s.push_str(&base);
    s
}

/// Parse an action string (e.g. `"seek:+5"`, `"layout:dashboard"`).
fn parse_action(s: &str, app: &AppState) -> Action {
    use Action::*;
    if let Some((verb, arg)) = s.split_once(':') {
        return match verb {
            "move" => Move(match arg {
                "up" => Motion::Up,
                "down" => Motion::Down,
                "left" => Motion::Left,
                "right" => Motion::Right,
                "top" => Motion::Top,
                "bottom" => Motion::Bottom,
                "pageup" => Motion::PageUp,
                "pagedown" => Motion::PageDown,
                _ => return Noop,
            }),
            "seek" => arg.parse::<i64>().map(Seek).unwrap_or(Noop),
            "focus" => match arg {
                "left" => FocusDir(-1),
                "right" => FocusDir(1),
                _ => Noop,
            },
            "resize_pane" => arg.parse::<i32>().map(ResizeFocusedPane).unwrap_or(Noop),
            "resize_pane_h" => arg.parse::<i32>().map(ResizePaneHeight).unwrap_or(Noop),
            "volume" => arg.parse::<i8>().map(VolumeDelta).unwrap_or(Noop),
            "speed" => arg
                .parse::<f32>()
                .map(|d| {
                    // step along the 0.25× grid (0.25..=2.0), snapping any
                    // off-grid value (e.g. a restored older speed) first
                    let snapped = (app.player.speed / 0.25).round() * 0.25;
                    SetSpeed((snapped + d).clamp(0.25, 2.0))
                })
                .unwrap_or(Noop),
            "rate" => match (app.player.current, arg.parse::<i32>()) {
                (Some(id), Ok(delta)) => {
                    let cur = app.library.track(id).map(|t| t.rating as i32).unwrap_or(0);
                    Rate(id, (cur + delta).clamp(0, 5) as u8)
                }
                _ => Noop,
            },
            "layout" => SwitchLayout(match arg {
                "dashboard" => Layout::Dashboard,
                "library_focus" => Layout::LibraryFocus,
                "full_player" => Layout::FullPlayer,
                "lyrics_focus" => Layout::LyricsFocus,
                "concert" => Layout::Concert,
                "radio" => Layout::Radio,
                "spotify" => Layout::Spotify,
                _ => return Noop,
            }),
            _ => Noop,
        };
    }
    match s {
        "quit" => Quit,
        "quit_or_back" => QuitOrBack,
        "copy_error" => CopyError,
        "toggle_error_log" => ToggleErrorLog,
        "toggle_help" => ToggleHelp,
        "toggle_stats" => ToggleStats,
        "command_palette" => OpenPalette,
        "cycle_pane" => CyclePane,
        "cycle_pane_rev" => CyclePaneRev,
        "begin_search" => BeginSearch,
        "cycle_theme" => CycleTheme,
        "toggle_play" => TogglePlay,
        "go_live" => GoLive,
        "go_stream_start" => GoStreamStart,
        "next" => Next,
        "previous" => Previous,
        "toggle_shuffle" => ToggleShuffle,
        "cycle_repeat" => CycleRepeat,
        "activate" => Activate,
        "back" => Back,
        "toggle_lyrics" => ToggleLyrics,
        "toggle_artist_info" => ToggleArtistInfo,
        "toggle_queue" => ToggleQueue,
        "toggle_queue_side" => ToggleQueueSide,
        "move_pane" => MoveFocusedPane,
        "move_artist_panel" => MoveArtistPanel,
        "move_lyrics_viz" => MoveLyricsViz,
        "move_sidebar" => MoveSidebar,
        "fit_layout" => FitLayout,
        "toggle_grid" => ToggleGridView,
        "settings_remove" => SettingsRemove,
        "restore_keybinds" => RestoreKeybinds,
        "open_settings" => OpenSettings,
        "open_equalizer" => OpenEqualizer,
        "toggle_sidebar" => ToggleSidebar,
        "toggle_lyrics_viz" => ToggleLyricsViz,
        "cycle_lyrics_format" => CycleLyricsFormat,
        "cycle_visualizer" => CycleVisualizer,
        "open_view_settings" => OpenViewSettings,
        "reset_layout" => ResetLayout,
        "toggle_favorite" => ToggleFavoriteSel,
        "edit_metadata" => BeginTagEdit,
        "open_radio" => OpenRadio,
        "open_spotify" => OpenSpotify,
        "play_current_album" => PlayCurrentAlbum,
        "play_current_artist" => PlayCurrentArtist,
        "random_album" => RandomAlbum,
        "bookmark_search" => BookmarkSearch,
        "cycle_sleep_timer" => CycleSleepTimer,
        "ab_loop" => AbLoopCycle,
        "clear_queue" => ClearQueue,
        "toggle_mark" => ToggleMark,
        "add_to_playlist" => AddToPlaylistPrompt,
        _ => Noop,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(c: char, shift: bool) -> Key {
        Key {
            code: KeyCode::Char(c),
            mods: Mods {
                shift,
                ..Mods::default()
            },
        }
    }

    #[test]
    fn normalize_shift_uppercases_shifted_letters() {
        // kitty-protocol delivery: base char + SHIFT → the legacy uppercase form,
        // and the redundant shift bit is cleared.
        let k = normalize_shift(ch('f', true));
        assert_eq!(k.code, KeyCode::Char('F'));
        assert!(!k.mods.shift);
    }

    #[test]
    fn normalize_shift_is_idempotent_on_uppercase() {
        // legacy delivery already folds Shift into the case; don't double-transform.
        let k = normalize_shift(ch('F', true));
        assert_eq!(k.code, KeyCode::Char('F'));
    }

    #[test]
    fn normalize_shift_leaves_plain_and_symbol_keys() {
        assert_eq!(normalize_shift(ch('f', false)).code, KeyCode::Char('f'));
        // symbols/digits keep their shifted glyph as delivered (layout-dependent).
        let sym = normalize_shift(ch('=', true));
        assert_eq!(sym.code, KeyCode::Char('='));
    }

    #[test]
    fn key_label_folds_shift_and_keeps_modifiers() {
        // `F` and a shifted `f` must produce the same binding label…
        assert_eq!(key_label(ch('F', false)), "F");
        assert_eq!(key_label(ch('f', true)), "F");
        assert_eq!(key_label(ch('f', false)), "f");
        // …and ctrl is still prefixed.
        let ctrl_c = Key {
            code: KeyCode::Char('c'),
            mods: Mods {
                ctrl: true,
                ..Mods::default()
            },
        };
        assert_eq!(key_label(ctrl_c), "ctrl-c");
    }
}
