//! Mouse-driven pane resizing. The render layer registers draggable handles for
//! each docked pane, and a left-drag on one adjusts the layout live. Two kinds of
//! handle (see [`ResizeKind`]):
//!
//! - a **band edge** — the pane↔main border; dragging it changes the pane's `size`
//!   percentage (its width when docked left/right, its height when top/bottom);
//! - a **divider** — the border between two panes stacked on one edge; dragging it
//!   shifts their cross-axis `len` share (e.g. the Queue-over-Artist height ratio).
//!
//! Kept separate from the click hit-map (`mouse.rs`) on purpose: a resize needs
//! both the grab strip *and* the reference frame the drag is measured against
//! (captured at grab time, so the drag keeps tracking once the pointer leaves the
//! 2-cell strip), and it must win over a content click that lands on the border.

use super::*;

impl ResizeKind {
    /// Whether dragging this handle moves the boundary vertically — a band on a
    /// top/bottom edge, or a divider between vertically-stacked panes. Drives the
    /// `ns-resize` vs `ew-resize` mouse-pointer shape.
    fn vertical(self) -> bool {
        match self {
            ResizeKind::Band { dock } => matches!(dock, Dock::Top | Dock::Bottom),
            ResizeKind::Divider { axis_y, .. } => axis_y,
        }
    }

    /// The CSS mouse-pointer shape (OSC 22) for a drag on this handle.
    pub fn pointer_shape(self) -> &'static str {
        if self.vertical() {
            "ns-resize"
        } else {
            "ew-resize"
        }
    }
}

impl AppState {
    /// Register a band-edge resize handle for a docked pane (the pane↔main border).
    /// `area` is the frame the pane docks within (the reference for the size
    /// percentage), `slot` its drawn rect, `dock` its edge. The handle is the
    /// 2-cell border strip between the pane and the main region — a comfortable
    /// grab target. Skipped when mouse support is off or the pane is fixed in this
    /// view (e.g. the Now-Playing visualizer is the main content).
    pub fn register_pane_edge(&self, area: Rect, dock: Dock, slot: Rect, panel: Panel) {
        if !self.config.mouse || !self.layout.panel_movable(panel) {
            return;
        }
        let strip = edge_strip(slot, dock).intersection(area);
        if strip.width == 0 || strip.height == 0 {
            return; // pane flush against the frame edge — nothing to grab
        }
        self.resize_edges.borrow_mut().push(ResizeEdge {
            strip,
            area,
            kind: ResizeKind::Band { dock },
        });
    }

    /// Register divider handles between panes sharing an edge, so the split of a
    /// stacked column (or a side-by-side row) can be dragged. `slots` is the final
    /// per-pane layout in edge/axis order (as produced by `dock_panels`), so any
    /// two *consecutive* same-edge entries are neighbours with a shared border.
    pub fn register_pane_dividers(&self, area: Rect, slots: &[(Panel, Rect)]) {
        if !self.config.mouse {
            return;
        }
        for pair in slots.windows(2) {
            let (a, ra) = pair[0];
            let (b, rb) = pair[1];
            let dock = self.panel(a).dock;
            // consecutive entries on different edges aren't neighbours
            if dock != self.panel(b).dock {
                continue;
            }
            if !self.layout.panel_movable(a) || !self.layout.panel_movable(b) {
                continue;
            }
            // stacked (vertical) only for left/right edges that aren't side-by-side;
            // side-by-side and top/bottom edges lay panes out horizontally.
            let axis_y = matches!(dock, Dock::Left | Dock::Right) && !self.config.panes_horizontal;
            let (strip, region) = divider_geometry(ra, rb, axis_y);
            let strip = strip.intersection(area);
            if strip.width == 0 || strip.height == 0 {
                continue;
            }
            self.resize_edges.borrow_mut().push(ResizeEdge {
                strip,
                area: region,
                kind: ResizeKind::Divider { a, b, axis_y },
            });
        }
    }

    /// Try to start a pane-resize drag at `(x, y)`. Returns `true` if the pointer
    /// was on a resize handle, so the caller skips the normal click. No-op while a
    /// modal overlay owns input.
    pub fn begin_pane_resize(&mut self, x: u16, y: u16) -> bool {
        if self.modal_open() {
            return false;
        }
        // last-registered wins, matching the click hit-map's topmost-first rule
        let grabbed = self
            .resize_edges
            .borrow()
            .iter()
            .rev()
            .find(|e| contains(e.strip, x, y))
            .copied();
        match grabbed {
            Some(e) => {
                self.resize_drag = Some(ResizeDrag {
                    area: e.area,
                    kind: e.kind,
                });
                true
            }
            None => false,
        }
    }

    /// Continue an active pane-resize drag: move the grabbed boundary to follow the
    /// pointer, resizing live. Returns `true` while a drag is active, so the caller
    /// skips seek/volume scrubbing.
    pub fn drag_pane_resize(&mut self, x: u16, y: u16) -> bool {
        let Some(drag) = self.resize_drag else {
            return false;
        };
        match drag.kind {
            ResizeKind::Band { dock } => {
                let pct = band_pct_at(drag.area, dock, x, y);
                self.set_edge_size(dock, pct);
            }
            ResizeKind::Divider { a, b, axis_y } => {
                let frac = frac_along(drag.area, axis_y, x, y);
                self.set_divider(a, b, frac);
            }
        }
        self.mark_dirty(); // continuous resize → repaint (mouse events don't auto-dirty)
        true
    }

    /// End any active pane-resize drag (mouse button released).
    pub fn end_pane_resize(&mut self) {
        self.resize_drag = None;
    }

    /// The mouse-pointer shape (OSC 22 CSS name) for the active drag, if any.
    pub fn resize_pointer_shape(&self) -> Option<&'static str> {
        self.resize_drag.map(|d| d.kind.pointer_shape())
    }
}

/// The 2-cell border strip between a docked pane and the main region — the grab
/// target for a band resize. Left/right docks yield the pane's outer border column
/// plus the main's border column (full pane height); top/bottom the mirror in rows.
fn edge_strip(slot: Rect, dock: Dock) -> Rect {
    match dock {
        Dock::Left => Rect::new(
            slot.x + slot.width.saturating_sub(1),
            slot.y,
            2,
            slot.height,
        ),
        Dock::Right => Rect::new(slot.x.saturating_sub(1), slot.y, 2, slot.height),
        Dock::Top => Rect::new(
            slot.x,
            slot.y + slot.height.saturating_sub(1),
            slot.width,
            2,
        ),
        Dock::Bottom => Rect::new(slot.x, slot.y.saturating_sub(1), slot.width, 2),
    }
}

/// The grab strip and combined region for a divider between two adjacent panes
/// `a` (first along the axis) and `b`. `axis_y` = they're stacked vertically, so
/// the divider is a horizontal strip straddling their shared border and the region
/// spans both heights; otherwise it's a vertical strip and the region spans both
/// widths. The region is the reference `frac_along` measures the drag against.
fn divider_geometry(a: Rect, b: Rect, axis_y: bool) -> (Rect, Rect) {
    if axis_y {
        let boundary = a.y + a.height; // == b.y for contiguous slots
        let strip = Rect::new(a.x, boundary.saturating_sub(1), a.width, 2);
        let region = Rect::new(a.x, a.y, a.width, (b.y + b.height).saturating_sub(a.y));
        (strip, region)
    } else {
        let boundary = a.x + a.width;
        let strip = Rect::new(boundary.saturating_sub(1), a.y, 2, a.height);
        let region = Rect::new(a.x, a.y, (b.x + b.width).saturating_sub(a.x), a.height);
        (strip, region)
    }
}

/// Band percentage of `area` implied by the pointer at `(x, y)` for a `dock` edge:
/// the fraction of the axis between the frame's anchored side and the pointer. The
/// inverse of `ui::components::pane_span` — left/top bands measure from the near
/// edge, right/bottom from the far edge (those grow inward from the side they're
/// anchored to).
fn band_pct_at(area: Rect, dock: Dock, x: u16, y: u16) -> u16 {
    let (span, dim) = match dock {
        Dock::Left => (
            u32::from(x.saturating_sub(area.x)) + 1,
            u32::from(area.width),
        ),
        Dock::Right => (
            u32::from((area.x + area.width).saturating_sub(x)),
            u32::from(area.width),
        ),
        Dock::Top => (
            u32::from(y.saturating_sub(area.y)) + 1,
            u32::from(area.height),
        ),
        Dock::Bottom => (
            u32::from((area.y + area.height).saturating_sub(y)),
            u32::from(area.height),
        ),
    };
    if dim == 0 {
        return 0;
    }
    ((span * 100) / dim).min(100) as u16
}

/// Where the pointer at `(x, y)` falls within `area` as a 0..1 fraction along the
/// drag axis (`axis_y` → vertical). Used to place a divider inside the two panes'
/// combined region.
fn frac_along(area: Rect, axis_y: bool, x: u16, y: u16) -> f32 {
    let (pos, origin, dim) = if axis_y {
        (y, area.y, area.height)
    } else {
        (x, area.x, area.width)
    };
    if dim == 0 {
        return 0.5;
    }
    (f32::from(pos.saturating_sub(origin)) / f32::from(dim)).clamp(0.0, 1.0)
}

/// Whether `(x, y)` falls within `r`.
fn contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

#[cfg(test)]
mod tests {
    use super::{band_pct_at, contains, divider_geometry, edge_strip, frac_along};
    use crate::app::Dock;
    use ratatui::layout::Rect;

    #[test]
    fn edge_strip_is_the_two_border_cells() {
        // a left sidebar 20 wide in a 100-wide frame: grab cols 19..=20
        let slot = Rect::new(0, 0, 20, 40);
        let s = edge_strip(slot, Dock::Left);
        assert_eq!((s.x, s.width, s.y, s.height), (19, 2, 0, 40));
        // a right pane starting at col 70: grab cols 69..=70
        let slot = Rect::new(70, 0, 30, 40);
        let s = edge_strip(slot, Dock::Right);
        assert_eq!((s.x, s.width), (69, 2));
        // top pane 10 tall: grab rows 9..=10
        let slot = Rect::new(0, 0, 100, 10);
        let s = edge_strip(slot, Dock::Top);
        assert_eq!((s.y, s.height, s.x, s.width), (9, 2, 0, 100));
    }

    #[test]
    fn band_pct_is_the_inverse_of_pane_span() {
        let area = Rect::new(0, 0, 200, 50);
        // left/top measure from the near edge
        assert_eq!(band_pct_at(area, Dock::Left, 49, 0), 25); // 50/200
        assert_eq!(band_pct_at(area, Dock::Top, 0, 9), 20); // 10/50
        // right/bottom measure from the far edge (anchored to the far side)
        assert_eq!(band_pct_at(area, Dock::Right, 140, 0), 30); // (200-140)/200
        assert_eq!(band_pct_at(area, Dock::Bottom, 0, 30), 40); // (50-30)/50
    }

    #[test]
    fn band_pct_scales_with_a_translated_frame() {
        // an offset frame still measures relative to its own origin
        let area = Rect::new(10, 5, 100, 40);
        assert_eq!(band_pct_at(area, Dock::Left, 34, 5), 25); // (34-10+1)=25 → 25%
        assert_eq!(band_pct_at(area, Dock::Right, 60, 5), 50); // (110-60)/100
    }

    #[test]
    fn band_pct_is_bounded_and_safe_at_the_edges() {
        let area = Rect::new(0, 0, 100, 40);
        assert_eq!(band_pct_at(area, Dock::Left, 0, 0), 1); // near edge → tiny, never 0-div
        assert_eq!(band_pct_at(area, Dock::Right, 0, 0), 100); // far past → clamped
        assert_eq!(band_pct_at(Rect::new(0, 0, 0, 0), Dock::Left, 5, 5), 0); // no window
    }

    #[test]
    fn contains_is_half_open() {
        let r = Rect::new(2, 3, 2, 4); // x∈{2,3}, y∈{3,4,5,6}
        assert!(contains(r, 2, 3));
        assert!(contains(r, 3, 6));
        assert!(!contains(r, 4, 3)); // right edge exclusive
        assert!(!contains(r, 2, 7)); // bottom edge exclusive
    }

    #[test]
    fn divider_between_stacked_panes_is_a_horizontal_strip() {
        // two right-edge panes stacked: a on top (rows 0..20), b below (20..40)
        let a = Rect::new(70, 0, 30, 20);
        let b = Rect::new(70, 20, 30, 20);
        let (strip, region) = divider_geometry(a, b, true);
        assert_eq!((strip.x, strip.width), (70, 30)); // spans the panes' width
        assert_eq!((strip.y, strip.height), (19, 2)); // straddles the shared border
        assert_eq!((region.y, region.height), (0, 40)); // both panes' full height
    }

    #[test]
    fn divider_between_side_by_side_panes_is_a_vertical_strip() {
        // two top-edge panes laid out horizontally: a (cols 0..50), b (50..100)
        let a = Rect::new(0, 0, 50, 10);
        let b = Rect::new(50, 0, 50, 10);
        let (strip, region) = divider_geometry(a, b, false);
        assert_eq!((strip.y, strip.height), (0, 10)); // spans the panes' height
        assert_eq!((strip.x, strip.width), (49, 2)); // straddles the shared border
        assert_eq!((region.x, region.width), (0, 100)); // both panes' full width
    }

    #[test]
    fn frac_along_maps_the_pointer_into_the_region() {
        let region = Rect::new(10, 4, 100, 40);
        assert_eq!(frac_along(region, false, 60, 0), 0.5); // halfway across
        assert_eq!(frac_along(region, true, 0, 24), 0.5); // halfway down
        assert_eq!(frac_along(region, true, 0, 0), 0.0); // before the top → clamped
        assert_eq!(frac_along(region, true, 0, 200), 1.0); // past the bottom → clamped
    }
}
