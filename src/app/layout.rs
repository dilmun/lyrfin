//! Panels, docks, panes, layout switching methods on `AppState` (extracted from app/mod.rs).

use super::*;

/// Per-`Layout` view state, grouped out of `AppState`: `cursors` is each view's
/// remembered tracklist+queue selection (restored on switch), `viz_modes` the
/// big-visualizer mode per view, and `panels` the per-view panel show/dock config
/// (missing entries fall back to `Layout::default_panel`).
#[derive(Default)]
pub struct ViewState {
    pub cursors: std::collections::HashMap<Layout, (usize, usize)>,
    pub viz_modes: std::collections::HashMap<Layout, u8>,
    pub panels: std::collections::HashMap<(Layout, Panel), PanelCfg>,
    /// Per-section grid/list override (the `#` toggle); absent → the section's
    /// default (`LocalSection::default_grid`). Persisted across restarts.
    pub grid: std::collections::HashMap<LocalSection, bool>,
    /// Spotify's per-section grid/list override (Albums/Artists); absent → on for
    /// those container sections. Persisted across restarts.
    pub spotify_grid: std::collections::HashMap<crate::spotify::api::Section, bool>,
}

impl AppState {
    // ---- per-view panels --------------------------------------------------
    /// Panel state for `panel` in the *current* view (defaults if untouched).
    pub fn panel(&self, panel: Panel) -> PanelCfg {
        self.panel_in(self.layout, panel)
    }

    /// Panel state for `panel` in a specific view.
    pub fn panel_in(&self, layout: Layout, panel: Panel) -> PanelCfg {
        self.views
            .panels
            .get(&(layout, panel))
            .copied()
            .unwrap_or_else(|| layout.default_panel(panel))
    }

    pub(crate) fn panel_mut(&mut self, panel: Panel) -> &mut PanelCfg {
        let d = self.layout.default_panel(panel);
        self.views.panels.entry((self.layout, panel)).or_insert(d)
    }

    /// Toggle a panel's visibility in the current view.
    pub fn toggle_panel(&mut self, panel: Panel) {
        let shown = {
            let p = self.panel_mut(panel);
            p.shown = !p.shown;
            p.shown
        };
        // don't strand focus on a queue that was just hidden
        if panel == Panel::Queue && !shown && self.focus == Focus::Pane(Panel::Queue) {
            self.focus = Focus::Main;
        }
        // same for the Spotify view's movable panes (Queue/Artist/Lyrics): hiding
        // the focused one drops focus back to the result list, not a hidden pane
        if !shown && self.focus == Focus::Pane(panel) {
            self.focus = Focus::Main;
        }
    }

    /// Cycle a panel's dock edge in the current view (with a toast).
    pub fn move_panel(&mut self, panel: Panel) {
        let p = self.panel_mut(panel);
        p.dock = p.dock.cycle();
        let edge = p.dock.label();
        self.notify(format!("{}: {edge}", panel.label()));
    }

    /// Grow/shrink a panel in the current view (`dir` = +1 / -1) — by 5 percentage
    /// points of the window, clamped to a usable 10–60 % band.
    pub fn resize_panel(&mut self, panel: Panel, dir: i32) {
        let p = self.panel_mut(panel);
        p.size = (p.size as i32 + dir * 5).clamp(10, 60) as u16;
    }

    /// Set the band on `dock` to `band_pct` (a percentage of the window along the
    /// dock axis) — the absolute counterpart to [`resize_panel`], used by mouse
    /// edge-dragging. Every shown pane on that edge is updated so the band matches:
    /// stacked panes share one width (the band is their *max*), so each takes
    /// `band_pct`; side-by-side panes sum to the band, so the target is split
    /// across the cluster (their individual sizes only feed the sum — the inner
    /// split is by `len`). Clamped to the same 10–60 % band as the keyboard resize.
    pub fn set_edge_size(&mut self, dock: Dock, band_pct: u16) {
        let panes: Vec<Panel> = self
            .layout
            .panels()
            .iter()
            .copied()
            .filter(|&p| self.panel(p).shown && self.panel(p).dock == dock)
            .collect();
        if panes.is_empty() {
            return;
        }
        let side_by_side = matches!(dock, Dock::Left | Dock::Right)
            && self.config.panes_horizontal
            && panes.len() > 1;
        let each = if side_by_side {
            band_pct / panes.len() as u16
        } else {
            band_pct
        };
        let each = each.clamp(10, 60);
        for p in panes {
            self.panel_mut(p).size = each;
        }
    }

    /// Move the divider between two panes stacked on one edge to `frac` (0..1) of
    /// their shared span — the absolute, mouse counterpart to [`resize_pane_height`].
    /// Only the two panes' cross-axis `len` weights change (their sum is preserved,
    /// so column-mates keep their share and the split just shifts between `a` and
    /// `b`). Clamped so neither drops below the same floor the keyboard uses.
    pub fn set_divider(&mut self, a: Panel, b: Panel, frac: f32) {
        let combined = self.panel(a).len as i32 + self.panel(b).len as i32;
        // keep both within the keyboard's 20–80 band → clamp `a` so `b = sum − a`
        // also lands in range.
        let lo = 20.max(combined - 80);
        let hi = 80.min(combined - 20);
        if lo > hi {
            return; // no room to move (both pinned at a bound)
        }
        let na = ((combined as f32 * frac).round() as i32).clamp(lo, hi);
        self.panel_mut(a).len = na as u16;
        self.panel_mut(b).len = (combined - na) as u16;
    }

    /// The dock pane that currently holds keyboard focus, if any — any focused
    /// movable pane (Queue / Artist / Lyrics), or the sidebar. Unified across
    /// every view: each one's focus ring already decides which panes it exposes,
    /// so the focused region maps straight to its panel. The main list/tracklist
    /// isn't a dock pane, so returns `None`.
    pub(crate) fn focused_panel(&self) -> Option<Panel> {
        match self.focus {
            Focus::Pane(p) => Some(p),
            Focus::Sidebar => Some(Panel::Sidebar),
            _ => None,
        }
    }

    /// Resize the focused dock pane (`<`/`>`), with a toast so the change is visible
    /// even when the pane is off-screen-narrow. Hints (rather than silently doing
    /// nothing) when the focus isn't on a resizable pane.
    pub fn resize_focused_pane(&mut self, dir: i32) {
        let Some(panel) = self.focused_panel() else {
            self.notify("Focus a pane (Tab) to resize it".into());
            return;
        };
        let edge = self.panel(panel).dock;
        // `<`/`>` move the pane↔main boundary in the key's on-screen direction: for
        // a left/top pane `>` grows it, but a right/bottom pane is anchored to the
        // far edge and grows back toward the centre, so flip the sign there — `<`
        // grows it, `>` shrinks it.
        let dir = if matches!(edge, Dock::Right | Dock::Bottom) {
            -dir
        } else {
            dir
        };
        // Panes stacked on the same edge share one column width (the band is the
        // *largest* of them), so resize every shown pane on that edge together —
        // otherwise growing a pane that isn't the largest does nothing visible
        // (e.g. the queue stacked under a wider lyrics pane).
        let peers: Vec<Panel> = self
            .layout
            .panels()
            .iter()
            .copied()
            .filter(|&p| self.panel(p).dock == edge && (self.panel(p).shown || p == panel))
            .collect();
        for p in peers {
            self.resize_panel(p, dir);
        }
        let size = self.panel(panel).size;
        self.notify(format!("{}: {size}%", panel.label()));
    }

    /// Grow/shrink the focused pane's *cross-axis* share (`{`/`}`) relative to the
    /// other panes stacked on its edge — its height when docked left/right, its
    /// width when docked top/bottom. Adjusts only this pane's `len` weight (the
    /// column-mates keep theirs, so the split shifts). Hints when the pane is alone
    /// on its edge (nothing to share the band with).
    pub fn resize_pane_height(&mut self, dir: i32) {
        let Some(panel) = self.focused_panel() else {
            self.notify("Focus a pane (Tab) to resize it".into());
            return;
        };
        let edge = self.panel(panel).dock;
        let stacked = self
            .layout
            .panels()
            .iter()
            .filter(|&&p| self.panel(p).shown && self.panel(p).dock == edge)
            .count()
            >= 2;
        if !stacked {
            self.notify("Stack another pane on this edge to adjust its height".into());
            return;
        }
        let p = self.panel_mut(panel);
        p.len = (p.len as i32 + dir * 8).clamp(20, 80) as u16;
        self.notify(format!("{} height adjusted", panel.label()));
    }

    /// Move the focused dock pane to the next edge (`m`): cycles left → top →
    /// right → bottom, so e.g. the Artist/Lyrics pane can sit *under* the library
    /// instead of beside it. Only the view's movable panes cycle — the sidebar is
    /// the shell's fixed column. Hints when focus isn't on a movable pane.
    pub fn move_focused_pane(&mut self) {
        let Some(panel) = self.focused_panel() else {
            self.notify("Focus a pane (Tab) to move it".into());
            return;
        };
        if !self.layout.panels().contains(&panel) {
            self.notify("This pane is fixed in this view".into());
            return;
        }
        self.move_panel(panel); // cycles the edge + toasts the new position
    }

    /// Restore the current view's layout to its defaults: drop every saved panel
    /// override (visibility / dock / width-height) and its visualizer mode, so
    /// `panel()` falls back to [`Layout::default_panel`].
    pub fn reset_layout(&mut self) {
        let layout = self.layout;
        self.views.panels.retain(|(l, _), _| *l != layout);
        self.views.viz_modes.remove(&layout);
        // a panel that defaults to hidden must not keep keyboard focus
        if self.focus == Focus::Pane(Panel::Queue) && !self.panel(Panel::Queue).shown {
            self.focus = Focus::Main;
        }
        self.notify(format!("{} layout reset to defaults", layout.title()));
    }

    /// Re-fit the current view's panes to the window: reset every pane's *size*
    /// back to its default percentage, while keeping which panes are shown and
    /// where they're docked. Lighter than [`reset_layout`] — undoes drifted
    /// `<`/`>` sizes (and any stale persisted ones) without throwing away your
    /// pane arrangement, and since sizes are percentages it always fits.
    pub fn fit_layout(&mut self) {
        let layout = self.layout;
        for &panel in layout.panels() {
            let def = layout.default_panel(panel).size;
            let cur = self.panel(panel);
            if cur.size != def || cur.len != 50 {
                self.views.panels.insert(
                    (layout, panel),
                    PanelCfg {
                        size: def,
                        len: 50,
                        ..cur
                    },
                );
            }
        }
        self.notify("Layout fitted to the window".into());
    }

    /// Change the active view, remembering each view's tracklist/queue cursor so
    /// switching back restores where you were. First visit to a view inherits the
    /// current cursor; afterwards each view keeps its own.
    pub fn set_layout(&mut self, target: Layout) {
        if target == self.layout {
            return;
        }
        self.views
            .cursors
            .insert(self.layout, (self.selection, self.queue_sel));
        self.layout = target;
        if let Some(&(sel, qsel)) = self.views.cursors.get(&target) {
            self.selection = sel;
            self.queue_sel = qsel;
        }
        self.clamp_focus();
        // Re-target the (single, shared) artist-info slot at the now-active source's
        // artist. Both artist panes read `meta.artist_info`, so without this the
        // previous source's bio bleeds into the other pane on a view switch (Spotify
        // ↔ local). Runs after `self.layout = target`, so `request_artist_info`
        // resolves the right source; it dedups when the artist is unchanged and
        // clears the slot (→ "loading…") when it differs, so no stale frame shows.
        self.request_artist_info();
        // Same story for the single shared `meta.lyrics` slot: re-target it at the
        // now-active source so a local track's lyrics can't linger in the Spotify
        // pane after a switch (and vice-versa).
        self.request_lyrics();
    }

    /// Keep `focus` on a region the current view actually exposes. Focus is now a
    /// single shared field across every view, so a value carried over from another
    /// view (e.g. an Artist pane this layout doesn't focus) is reset to the view's
    /// first focusable region (`Main` when it has none).
    pub(crate) fn clamp_focus(&mut self) {
        let ring = self.focus_order();
        if !ring.contains(&self.focus) {
            self.focus = ring.first().copied().unwrap_or(Focus::Main);
        }
    }

    pub(crate) fn cycle_pane(&mut self) {
        self.cycle_focus(1);
    }

    pub(crate) fn cycle_pane_rev(&mut self) {
        self.cycle_focus(-1);
    }

    /// Move keyboard focus to `f`. Entering the Up-Next/Queue pane parks its cursor
    /// on the now-playing track so the list doesn't scroll away from where it sat
    /// (the queue shows the now-playing row whether focused or not). Shared by every
    /// source view — the Spotify queue uses its own cursor + index.
    pub(crate) fn set_focus(&mut self, f: Focus) {
        if f == Focus::Pane(Panel::Queue) && self.focus != f {
            if self.layout == Layout::Spotify {
                self.spotify.queue_sel = self.spov.sp_idx;
            } else {
                self.queue_sel = self.player.queue.position;
            }
        }
        self.focus = f;
    }

    /// The focusable regions of the current view, in Tab order — one ring for every
    /// source view, so Tab only ever stops on something on screen. Sidebar + Main
    /// plus the view's shown movable panes (the Spotify view exposes all of them;
    /// the Dashboard keeps Artist/Lyrics display-only). The queue, when present,
    /// leads in the player views so the first Tab lands on the navigable list.
    pub(crate) fn focus_order(&self) -> Vec<Focus> {
        let queue = self.panel(Panel::Queue).shown;
        match self.layout {
            // sidebar (fixed shell column) + main + each shown movable pane
            // (Queue/Artist/Lyrics) — Tab reaches every visible pane, the same way
            // on the Dashboard and the Spotify view.
            Layout::Dashboard | Layout::Spotify => {
                let mut p = vec![Focus::Sidebar, Focus::Main];
                for &panel in self.layout.panels() {
                    if panel != Panel::Sidebar && self.panel(panel).shown {
                        p.push(Focus::Pane(panel));
                    }
                }
                p
            }
            // the 3-column Miller browser is the Main content (browser.col sub-selects
            // a column); the optional Queue/Artist/Lyrics panes follow when shown
            Layout::LibraryFocus => {
                let mut p = vec![Focus::Main];
                for &panel in self.layout.panels() {
                    if self.panel(panel).shown {
                        p.push(Focus::Pane(panel));
                    }
                }
                p
            }
            // now-playing card / lyrics + (optional) leading queue
            Layout::FullPlayer | Layout::LyricsFocus => {
                let mut p = Vec::new();
                if queue {
                    p.push(Focus::Pane(Panel::Queue));
                }
                p.push(Focus::Main);
                p
            }
            // Radio: the section Sidebar + the station-list Main (its search box is
            // a text-input sub-mode reached with '/', not a ring stop).
            Layout::Radio => vec![Focus::Sidebar, Focus::Main],
            // Concert is chrome-less — no focusable panes.
            Layout::Concert => Vec::new(),
        }
    }

    /// Shift focus one region left (`dir < 0`) or right (`dir > 0`) through the
    /// view's focus ring — the horizontal counterpart to Tab, driven by `h`/`l`
    /// (and ←/→). Clamped at the ends (unlike Tab's wrap) so the direction reads
    /// literally: `h` walks toward the sidebar, `l` toward the panes, and neither
    /// wraps past the edge. Views whose main content is 2-D (the Library columns,
    /// the cover grid, the Radio sidebar) intercept `h`/`l` earlier in the keymap,
    /// so this only runs where horizontal movement means "change focused region".
    pub(crate) fn focus_dir(&mut self, dir: i32) {
        let ring = self.focus_order();
        if ring.is_empty() {
            return;
        }
        let i = ring.iter().position(|&f| f == self.focus).unwrap_or(0) as i32;
        let j = (i + dir).clamp(0, ring.len() as i32 - 1) as usize;
        self.set_focus(ring[j]);
    }

    /// Move focus by `dir` (+1 next / -1 prev) through the view's focus ring
    /// (Tab / BackTab). One engine for every source view.
    pub(crate) fn cycle_focus(&mut self, dir: i32) {
        let ring = self.focus_order();
        if ring.is_empty() {
            return;
        }
        let next = match ring.iter().position(|&f| f == self.focus) {
            Some(i) => ring[(i as i32 + dir).rem_euclid(ring.len() as i32) as usize],
            None => ring[0], // focus isn't on screen → land on the first region
        };
        self.set_focus(next);
    }
}
