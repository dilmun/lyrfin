//! A generic drill-in history shared by every browsable source view (the local
//! library and the Spotify view). Each source owns its *current* browse list (the
//! items + cursor + breadcrumb it's displaying); this owns the **history** either
//! side of it — the parent lists behind (Back) and the lists stepped out of
//! (Forward) — restored verbatim, no refetch. The per-frame context `C` carries
//! each source's extra restore state (Spotify's search / open-item context; the
//! local library's grid-vs-list mode).
//!
//! Semantics are the browser ones: Back and Forward walk the two stacks with the
//! current location passing between them, and a fresh drill-in truncates the
//! forward branch.

use super::types::Focus;

/// Which way a history step moves. Shared by the local and Spotify browse views so
/// both drive one step function instead of a near-duplicate pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavDir {
    Back,
    Forward,
}

/// One saved browse location: its items + cursor + breadcrumb + focus, plus the
/// source-specific context to restore when navigating onto it.
pub struct Frame<T, C = ()> {
    pub items: Vec<T>,
    pub sel: usize,
    pub crumb: Option<String>,
    /// Which region was focused at this location. Restored on Back so drilling in
    /// from a dock pane (e.g. ⏎ on the Artist pane) returns you to that pane
    /// rather than dumping you on Main.
    pub focus: Focus,
    pub ctx: C,
}

/// The drill-in history. `depth() == 0` means the top (section / search) level;
/// `can_forward()` reports whether a Back has been taken that Forward can undo.
pub struct NavStack<T, C = ()> {
    /// Locations behind the current one, nearest parent last.
    parents: Vec<Frame<T, C>>,
    /// Locations ahead of the current one, nearest child last. Populated only by
    /// [`Self::back`] and truncated by [`Self::push`].
    fwd: Vec<Frame<T, C>>,
}

// A manual Default (derive would needlessly require `T: Default`, `C: Default`).
impl<T, C> Default for NavStack<T, C> {
    fn default() -> Self {
        Self {
            parents: Vec::new(),
            fwd: Vec::new(),
        }
    }
}

impl<T, C> NavStack<T, C> {
    /// Save the parent list before drilling into a child. The caller then resets
    /// its own current `items` / `sel` / `crumb` to the child level.
    ///
    /// This is a *new* navigation, so it discards the forward branch — same as a
    /// browser: going back and then somewhere else drops what you'd stepped out of.
    pub fn push(&mut self, items: Vec<T>, sel: usize, crumb: Option<String>, focus: Focus, ctx: C) {
        self.fwd.clear();
        self.parents.push(Frame {
            items,
            sel,
            crumb,
            focus,
            ctx,
        });
    }

    /// Step back one level, returning the parent to restore (`None` at the top).
    ///
    /// `current` — the location being left — is built **lazily and only on
    /// success**, then moved onto the forward stack so [`Self::forward`] can return
    /// to it. That laziness is load-bearing, not a style choice: building a frame
    /// means `mem::take`-ing the caller's live `items`, so an eager argument would
    /// gut the visible list on the `None` path (Back at the top level).
    pub fn back(&mut self, current: impl FnOnce() -> Frame<T, C>) -> Option<Frame<T, C>> {
        if self.parents.is_empty() {
            return None;
        }
        self.fwd.push(current());
        self.parents.pop()
    }

    /// Step forward one level, undoing a [`Self::back`]. `None` when no Back has
    /// been taken (or a new drill-in truncated the branch). `current` is built
    /// lazily on success and moved back onto the parent stack — see [`Self::back`]
    /// for why it must not be eager.
    pub fn forward(&mut self, current: impl FnOnce() -> Frame<T, C>) -> Option<Frame<T, C>> {
        if self.fwd.is_empty() {
            return None;
        }
        self.parents.push(current());
        self.fwd.pop()
    }

    /// Clear the whole history — back to the top level (e.g. on a section change).
    pub fn clear(&mut self) {
        self.parents.clear();
        self.fwd.clear();
    }

    /// Current drill-in depth (0 at the top level).
    pub fn depth(&self) -> usize {
        self.parents.len()
    }

    /// Whether a Forward step is available (a Back was taken and not truncated).
    pub fn can_forward(&self) -> bool {
        !self.fwd.is_empty()
    }

    /// The container opened at each level (the cursor item of each parent frame),
    /// top → bottom — i.e. the drill path. Used to persist + restore a drill-in.
    pub fn opened(&self) -> impl Iterator<Item = &T> + '_ {
        self.parents.iter().filter_map(|f| f.items.get(f.sel))
    }
}
