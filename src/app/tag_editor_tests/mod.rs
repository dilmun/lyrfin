//! Tag-editor + app behaviour tests, extracted from app/mod.rs (a ~1000-line
//! `#[cfg(test)]` block that was nearly half the file), then split by theme into
//! editing / library / playlists / playback sub-modules. Same module path
//! (`crate::app::tag_editor_tests`), so the tests keep full access to AppState's
//! private fields/methods; each sub-module reaches the shared `app()` fixture via
//! `use super::*`.

use super::*;
// re-exported to the sub-modules via their `use super::*` (the action types come
// through crate::app; the event types don't, so surface them here)
use crate::event::{Key, KeyCode, Mods};

mod editing;
mod library;
mod playback;
mod playlists;

fn app() -> AppState {
    // a throwaway dir so tag-edit/theme/settings saves never touch the user's real
    // ~/.config/lyrfin (these tests run commands that call config.save()/save user data)
    let cfg = Config {
        dir: std::env::temp_dir().join("lyrfin-tag-editor-test"),
        ..Config::default()
    };
    let mut a = AppState::new(cfg);
    a.seed_demo();
    a
}
