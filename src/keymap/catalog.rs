//! The action catalog: the built-in default key bindings plus the action
//! metadata (which actions are user-configurable, and their human-readable
//! descriptions) consumed by the Keys settings UI and `config::Keymap`. Pure
//! data + string mapping — no app or event dependencies; the keypress→Action
//! dispatch that uses these lives in the parent module.

/// Built-in defaults (label, action). `keybindings.toml` overrides these.
pub const DEFAULT_BINDINGS: &[(&str, &str)] = &[
    ("q", "quit"), // q always quits; back / up-one-level is esc + ctrl-o (vim)
    ("Q", "quit"), // shift-q quits too (also ctrl-c)
    ("ctrl-c", "quit"),
    ("u", "toggle_queue"), // u toggles the QUEUE panel
    ("?", "toggle_help"),
    (":", "command_palette"),
    // pane focus: Tab → next active pane, Shift-Tab → previous. (Item navigation
    // within a pane/list/menu is ctrl-n/ctrl-p, handled globally in `map`.)
    ("tab", "cycle_pane"),
    ("backtab", "cycle_pane_rev"),
    ("/", "begin_search"),
    ("t", "cycle_theme"),
    ("space", "toggle_play"),
    ("n", "next"),
    ("p", "previous"),
    ("s", "toggle_shuffle"),
    ("r", "cycle_repeat"),
    // h/l (and ←/→) are navigation: shift focus one region left/right through the
    // view's pane ring (views with 2-D content — the Library columns, the cover
    // grid, the Radio sidebar — remap them to column/card moves in the keymap).
    // Seek lives on `,`/`.` (below).
    ("right", "focus:right"),
    ("left", "focus:left"),
    ("h", "focus:left"),
    ("l", "focus:right"),
    ("$", "go_live"),
    ("0", "go_stream_start"),
    ("+", "volume:+5"),
    ("=", "volume:+5"),
    ("-", "volume:-5"),
    ("]", "speed:+0.25"),
    ("[", "speed:-0.25"),
    ("j", "move:down"),
    ("k", "move:up"),
    ("down", "move:down"),
    ("up", "move:up"),
    ("g", "move:top"),
    ("G", "move:bottom"),
    ("pageup", "move:pageup"),
    ("pagedown", "move:pagedown"),
    ("enter", "activate"),
    ("esc", "back"),               // back / up one level / close
    ("ctrl-o", "back"),            // vim-style jump back / up one level (q is quit-only)
    ("y", "copy_error"),           // yank the last error message to the clipboard
    ("1", "layout:dashboard"),     // Home
    ("2", "layout:library_focus"), // Library (Artists ▸ Albums ▸ Tracks)
    ("3", "layout:full_player"),   // Now Playing
    ("4", "layout:lyrics_focus"),  // Lyrics
    ("5", "layout:concert"),       // Concert
    ("6", "open_radio"),           // Radio (internet stations)
    ("7", "open_spotify"),         // Spotify (librespot)
    ("f", "toggle_favorite"),
    ("a", "add_to_playlist"),
    // seek: the `<`/`>` glyphs read as rewind / fast-forward. Universal transport —
    // works in every view (the Lyrics view/pane remaps them to sync-offset nudges).
    (".", "seek:+5"),
    (",", "seek:-5"),
    // rate the current track (0..=5 stars); relocated off `,`/`.` when seek took them.
    (")", "rate:+1"),
    ("(", "rate:-1"),
    ("C", "clear_queue"),
    ("x", "toggle_mark"),
    ("L", "toggle_lyrics"),
    ("i", "toggle_artist_info"), // i toggles the artist panel
    ("ctrl-q", "toggle_queue_side"),
    ("b", "toggle_sidebar"), // b toggles the library sidebar
    ("V", "toggle_lyrics_viz"),
    // `F` (cycle lyrics format) is no longer a global binding — it's a Lyrics-pane
    // key now (see `keymap::pane_context`), so it only fires with the lyrics view /
    // pane focused and can never shadow `f` = favourite elsewhere.
    ("v", "cycle_visualizer"),
    (">", "resize_pane:+1"), // widen the focused dock pane (left/top; flips on right/bottom)
    ("<", "resize_pane:-1"), // narrow the focused dock pane
    ("}", "resize_pane_h:+1"), // taller: grow the focused pane's height share (stacked panes)
    ("{", "resize_pane_h:-1"), // shorter: shrink the focused pane's height share
    ("m", "move_pane"),      // cycle the focused pane's edge (l→t→r→b)
    (";", "open_view_settings"),
    ("e", "open_equalizer"), // 10-band graphic equalizer overlay
    ("z", "fit_layout"),     // re-fit pane sizes to the window (keep shown/dock)
    ("Z", "reset_layout"),   // restore this view's panels to defaults
    ("#", "toggle_grid"),    // Albums/Artists: cover-art grid vs list
    // tags/metadata: one palette command "Tag Edit" (Edit · Auto Tag · Cover tabs)
    ("A", "play_current_album"), // play the current/selected track's album
    ("R", "random_album"),
    ("B", "bookmark_search"), // shift-B: bookmark the search
    ("T", "cycle_sleep_timer"),
    ("o", "ab_loop"),
    ("I", "toggle_stats"),          // shift-I: stats overlay
    ("delete", "settings_remove"),  // settings: delete the selected row (ctrl-d is now half-page)
    ("ctrl-r", "restore_keybinds"), // Keys settings: reset to defaults
];

/// `(key, action)` pairs that used to be built-in defaults but have since moved
/// to a different key or been retired. Older lyrfin versions persisted the *full*
/// effective keymap to `keybindings.toml`, so an upgraded user's file can still
/// pin a key to a stale default — shadowing the new one forever. On load, a file
/// entry matching one of these is dropped and the key reverts to its current
/// default (see `crate::config::Keymap::load`). Append a row whenever a default
/// binding's action changes.
pub const RETIRED_BINDINGS: &[(&str, &str)] = &[
    // `toggle_queue` moved from `q` to `u`; `q` was `quit_or_back`, now `quit`.
    ("q", "toggle_queue"),
    // `q` is now quit-only — back / up-one-level moved to esc + ctrl-o.
    ("q", "quit_or_back"),
    // the speed step changed from 0.1 to the 0.25 grid.
    ("]", "speed:+0.1"),
    ("[", "speed:-0.1"),
    // z/Z swapped: `z` is now fit_layout, `Z` is reset_layout.
    ("z", "reset_layout"),
    ("Z", "fit_layout"),
    // `2` was a stopgap `layout:split` while the Miller browser was gone; it's now
    // the restored 3-column `layout:library_focus`.
    ("2", "layout:split"),
    // `F` moved from a global binding to a Lyrics-pane key; drop a stale full-dump
    // pin so it doesn't resurrect the global binding and shadow the pane scope.
    ("F", "cycle_lyrics_format"),
    // seek moved off h/l/←/→ (now focus navigation) onto `,`/`.`; rate moved off
    // `,`/`.` onto `(`/`)`. Drop the stale full-dump pins so the keys pick up their
    // new roles instead of staying frozen on the old ones.
    ("h", "seek:-5"),
    ("l", "seek:+5"),
    ("left", "seek:-5"),
    ("right", "seek:+5"),
    (",", "rate:-1"),
    (".", "rate:+1"),
];

/// Distinct, user-configurable actions (one row per action in the Keys settings),
/// in the order they first appear in [`DEFAULT_BINDINGS`].
pub fn configurable_actions() -> Vec<&'static str> {
    let mut seen = std::collections::HashSet::new();
    let mut v = Vec::new();
    for (_, action) in DEFAULT_BINDINGS {
        if *action != "noop" && seen.insert(*action) {
            v.push(*action);
        }
    }
    v
}

/// A friendly description of an action string for the Keys settings list.
pub fn keybind_desc(action: &str) -> String {
    let fixed = match action {
        "toggle_queue" => "Toggle queue panel",
        "quit" => "Quit",
        "quit_or_back" => "Up one level / quit",
        "toggle_help" => "Keybindings help",
        "command_palette" => "Command palette",
        "cycle_pane" => "Focus next pane",
        "cycle_pane_rev" => "Focus previous pane",
        "focus:left" => "Focus left (pane / column)",
        "focus:right" => "Focus right (pane / column)",
        "begin_search" => "Search",
        "cycle_theme" => "Cycle theme",
        "toggle_play" => "Play / pause",
        "go_live" => "Radio: jump to live edge",
        "go_stream_start" => "Radio: jump to buffer start",
        "next" => "Next track",
        "previous" => "Previous track",
        "toggle_shuffle" => "Toggle shuffle",
        "cycle_repeat" => "Cycle repeat",
        "activate" => "Activate / play selection",
        "back" => "Back / up one level / cancel",
        "copy_error" => "Copy last error message",
        "toggle_favorite" => "Toggle favorite",
        "add_to_playlist" => "Add to playlist",
        "clear_queue" => "Clear queue",
        "toggle_mark" => "Mark track",
        "toggle_lyrics" => "Toggle lyrics",
        "toggle_artist_info" => "Toggle artist panel",
        "toggle_queue_side" => "Move queue (cycle edge)",
        "toggle_sidebar" => "Toggle sidebar",
        "toggle_lyrics_viz" => "Toggle this view's visualizer",
        "cycle_lyrics_format" => "Cycle lyric format",
        "cycle_visualizer" => "Cycle visualizer mode",
        "resize_pane:+1" => "Widen focused pane",
        "resize_pane:-1" => "Narrow focused pane",
        "resize_pane_h:+1" => "Taller (focused pane's height share)",
        "resize_pane_h:-1" => "Shorter (focused pane's height share)",
        "move_pane" => "Move focused pane (cycle edge)",
        "open_view_settings" => "Open this view's settings",
        "reset_layout" => "Reset this view's layout",
        "fit_layout" => "Fit panes to the window",
        "toggle_grid" => "Albums/Artists: grid ↔ list",
        "edit_metadata" => "Edit tags",
        "open_radio" => "Internet radio",
        "open_spotify" => "Spotify",
        "play_current_album" => "Play the current track's album",
        "play_current_artist" => "Play the current track's artist",
        "random_album" => "Play a random album",
        "bookmark_search" => "Bookmark search",
        "cycle_sleep_timer" => "Cycle sleep timer",
        "ab_loop" => "A-B loop",
        "toggle_stats" => "Stats overlay",
        "settings_remove" => "Settings: delete row",
        "move:down" => "Move down",
        "move:up" => "Move up",
        "move:top" => "Jump to top",
        "move:bottom" => "Jump to bottom",
        "move:pageup" => "Page up",
        "move:pagedown" => "Page down",
        "seek:+5" => "Seek forward",
        "seek:-5" => "Seek back",
        "volume:+5" => "Volume up",
        "volume:-5" => "Volume down",
        "speed:+0.25" => "Speed up",
        "speed:-0.25" => "Speed down",
        "rate:+1" => "Rate up",
        "rate:-1" => "Rate down",
        _ => "",
    };
    if !fixed.is_empty() {
        return fixed.to_string();
    }
    if let Some(view) = action.strip_prefix("layout:") {
        return format!("Go to {}", view.replace('_', " "));
    }
    action.replace(['_', ':'], " ")
}
