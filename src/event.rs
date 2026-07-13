//! `Event` — raw inputs arriving from the outside world.
//!
//! The event loop (M1, in `tui.rs`) merges three sources onto one channel:
//!   * terminal input (crossterm key/mouse/resize)
//!   * a periodic tick (drives animations + progress)
//!   * worker messages (audio engine, library scanner)
//!
//! Events are mapped to [`crate::action::Action`]s by the keymap before they
//! reach `AppState::update`.

/// Backend-agnostic key input (crossterm's `KeyEvent` maps onto this in M1, so
/// the keymap and core never depend on the terminal backend directly).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub code: KeyCode,
    pub mods: Mods,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCode {
    Char(char),
    Enter,
    Esc,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    F(u8),
}

/// Modifier bitflags (kept tiny; a real bitflags crate can replace this in M1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}
