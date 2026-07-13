//! The unified read-only **Info** overlay state — `Keys` / `Stats` / `Health` /
//! `Track` tabs (replacing the separate help, stats, error-log, and metadata
//! overlays). `AppState::info` is `Some` while it's open. Rendering lives in
//! `ui::components::info`.

use std::cell::Cell;

use super::*;
use crate::action::Motion;

/// The Info overlay's tabs, in display order.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum InfoTab {
    #[default]
    Keys,
    Stats,
    Health,
    Track,
}

impl InfoTab {
    pub const ALL: [InfoTab; 4] = [
        InfoTab::Keys,
        InfoTab::Stats,
        InfoTab::Health,
        InfoTab::Track,
    ];

    /// Tab-bar label.
    pub fn label(self) -> &'static str {
        match self {
            InfoTab::Keys => "Keys",
            InfoTab::Stats => "Stats",
            InfoTab::Health => "Health",
            InfoTab::Track => "Track",
        }
    }

    /// All labels in display order (for the shared `tab_bar`).
    pub fn labels() -> [&'static str; 4] {
        [
            InfoTab::Keys.label(),
            InfoTab::Stats.label(),
            InfoTab::Health.label(),
            InfoTab::Track.label(),
        ]
    }

    /// Position in `ALL` (the active-tab index for the tab bar).
    pub fn index(self) -> usize {
        InfoTab::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

/// State for the Info overlay. Each tab keeps its own scroll offset + measured
/// content height (`*_max`, set by the render layer, read by `info_scroll` to
/// clamp), plus the Keys tab's live search filter.
#[derive(Default)]
pub struct Info {
    pub tab: InfoTab,
    pub keys_query: String,
    pub keys_scroll: usize,
    pub keys_max: Cell<usize>,
    pub stats_scroll: usize,
    pub stats_max: Cell<usize>,
    pub errors_scroll: usize,
    pub errors_max: Cell<usize>,
    pub track_scroll: usize,
    pub track_max: Cell<usize>,
}

impl AppState {
    /// Open the Info overlay at `tab`. If it's already open: same tab → close,
    /// a different tab → switch (preserving each tab's scroll). The single entry
    /// point for `?` (Keys), `I` (Stats), the palette's Health row, and Track.
    pub fn toggle_info(&mut self, tab: InfoTab) {
        match self.info.as_ref().map(|i| i.tab) {
            Some(cur) if cur == tab => self.info = None,
            Some(_) => {
                if let Some(i) = &mut self.info {
                    i.tab = tab;
                }
            }
            None => {
                self.info = Some(Info {
                    tab,
                    ..Default::default()
                })
            }
        }
    }

    /// Close the Info overlay (Esc).
    pub fn close_info(&mut self) {
        self.info = None;
    }

    /// Tab / Shift-Tab: step the active tab (wraps both directions).
    pub fn info_tab_step(&mut self, dir: i32) {
        if let Some(i) = &mut self.info {
            let n = InfoTab::ALL.len() as i32;
            let cur = i.tab.index() as i32;
            i.tab = InfoTab::ALL[(((cur + dir) % n + n) % n) as usize];
        }
    }

    /// j/k / arrows / page / g / G: scroll the active tab, clamped to its content.
    pub fn info_scroll(&mut self, m: Motion) {
        let d: i32 = match m {
            Motion::Up => -1,
            Motion::Down => 1,
            Motion::PageUp => -10,
            Motion::PageDown => 10,
            Motion::Top => i32::MIN / 2,
            Motion::Bottom => i32::MAX / 2,
            _ => 0,
        };
        if let Some(i) = &mut self.info {
            let (scroll, max) = match i.tab {
                InfoTab::Keys => (&mut i.keys_scroll, i.keys_max.get()),
                InfoTab::Stats => (&mut i.stats_scroll, i.stats_max.get()),
                InfoTab::Health => (&mut i.errors_scroll, i.errors_max.get()),
                InfoTab::Track => (&mut i.track_scroll, i.track_max.get()),
            };
            *scroll = (*scroll as i32 + d).clamp(0, max as i32) as usize;
        }
    }
}
