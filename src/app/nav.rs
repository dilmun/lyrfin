//! A generic drill-in back-stack shared by every browsable source view (the local
//! library and the Spotify view). Each source owns its *current* browse list (the
//! items + cursor + breadcrumb it's displaying); this owns the **back-stack** of
//! parent lists, pushed on drill-in and popped on Back to restore the parent
//! verbatim — no refetch. The per-frame context `C` carries each source's extra
//! restore state (Spotify's search / open-item context; `()` for the local
//! library, which has nothing extra to restore).

/// One saved parent browse list: its items + cursor + breadcrumb, plus the
/// source-specific context to restore on Back.
pub struct Frame<T, C = ()> {
    pub items: Vec<T>,
    pub sel: usize,
    pub crumb: Option<String>,
    pub ctx: C,
}

/// The drill-in back-stack. Push the current list when drilling into a container;
/// pop it on Back. `depth() == 0` means the top (section / search) level.
pub struct NavStack<T, C = ()> {
    frames: Vec<Frame<T, C>>,
}

// A manual Default (derive would needlessly require `T: Default`, `C: Default`).
impl<T, C> Default for NavStack<T, C> {
    fn default() -> Self {
        Self { frames: Vec::new() }
    }
}

impl<T, C> NavStack<T, C> {
    /// Save the parent list before drilling into a child. The caller then resets
    /// its own current `items` / `sel` / `crumb` to the child level.
    pub fn push(&mut self, items: Vec<T>, sel: usize, crumb: Option<String>, ctx: C) {
        self.frames.push(Frame {
            items,
            sel,
            crumb,
            ctx,
        });
    }

    /// Pop the parent list on Back; the caller restores its current state from the
    /// returned frame (items / sel / crumb + the source context). `None` at the top.
    pub fn pop(&mut self) -> Option<Frame<T, C>> {
        self.frames.pop()
    }

    /// Clear the stack — back to the top level (e.g. on a section change).
    pub fn clear(&mut self) {
        self.frames.clear();
    }

    /// Current drill-in depth (0 at the top level). Part of the generic engine's
    /// public API; currently only exercised by the nav-stack unit tests.
    #[allow(dead_code)]
    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    /// The container opened at each level (the cursor item of each parent frame),
    /// top → bottom — i.e. the drill path. Used to persist + restore a drill-in.
    pub fn opened(&self) -> impl Iterator<Item = &T> + '_ {
        self.frames.iter().filter_map(|f| f.items.get(f.sel))
    }
}
