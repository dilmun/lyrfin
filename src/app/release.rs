//! Shared **sectioned-carousels** model — ONE place for how a view's card content
//! is grouped into labelled horizontal carousels ([`ReleaseRow`]s), rendered
//! (`grid::release_grid`), and navigated ([`release_grid_step`]). Used by the
//! artist pages (local + Spotify, grouped by release type) and by the Spotify
//! Home feed (grouped by shelf title) — any multi-section card view renders the
//! same way, so navigation is consistent everywhere.
//!
//! *Classifying* releases into a section is per-source (the local library guesses
//! from the title + track count; Spotify reads its API [`Group`]) and the artist
//! *taxonomy* (which sections exist, their order, labels) lives here so the two
//! can't drift. Views with free-form section titles (Home) carry the label on the
//! [`ReleaseRow::Header`] directly instead of mapping to [`ReleaseSection`].

use std::borrow::Cow;

use crate::spotify::api::Group;

/// Canonical artist-page release sections, in display order. Both the local
/// classifier (`local_browse::release_kind`) and Spotify's [`Group`] map into
/// these, so the section set, order, and header labels are defined exactly once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReleaseSection {
    Albums,
    SinglesEps,
    Remixes,
    Compilations,
}

impl ReleaseSection {
    /// The sections in the order they appear down an artist page.
    pub(crate) const ORDER: [ReleaseSection; 4] = [
        ReleaseSection::Albums,
        ReleaseSection::SinglesEps,
        ReleaseSection::Remixes,
        ReleaseSection::Compilations,
    ];

    /// The uppercase header drawn above this section's carousel — the single source
    /// of truth for the labels. Change "SINGLES & EPs" (etc.) here and both the
    /// local and Spotify artist pages follow.
    pub(crate) fn label(self) -> &'static str {
        match self {
            ReleaseSection::Albums => "ALBUMS",
            ReleaseSection::SinglesEps => "SINGLES & EPs",
            ReleaseSection::Remixes => "REMIXES",
            ReleaseSection::Compilations => "COMPILATIONS",
        }
    }

    /// The section a Spotify release [`Group`] maps to. `None` for the non-release
    /// groups (`Popular`/`None`) — those aren't carousels (Popular is the leading
    /// track list, handled separately).
    pub(crate) fn from_group(g: Group) -> Option<ReleaseSection> {
        match g {
            Group::Albums => Some(ReleaseSection::Albums),
            Group::Singles => Some(ReleaseSection::SinglesEps),
            Group::Compilations => Some(ReleaseSection::Compilations),
            Group::Popular | Group::None => None,
        }
    }
}

/// Header shown above the leading popular-tracks list — not a carousel section, but
/// shared so local and Spotify spell it identically.
pub(crate) const POPULAR_HEADER: &str = "POPULAR";

/// One visual row of an artist page's release region: a section header (its label),
/// or a run of album-card item indices — a horizontal carousel (the render windows
/// it + scrolls left/right). Source-agnostic — the header carries its label
/// directly, so a source without header *items* (Spotify, grouped by [`Group`]) can
/// build rows too. Shared by the render (`grid::release_grid`) and the 2-D nav.
pub(crate) enum ReleaseRow {
    /// A section header. `Cow` so fixed taxonomies pass a `&'static str` label with
    /// no allocation, while free-form sections (Home shelves) pass an owned title.
    Header(Cow<'static, str>),
    Cards(Vec<usize>),
    /// A full-width clickable **button** for a single item (e.g. "Browse all
    /// categories") — its own row, drawn as a rounded bar, not a cover card. Holds the
    /// item index so it selects/opens like any other; navigates as a 1-item row.
    Banner(usize),
}

impl ReleaseRow {
    /// The selectable item indices the grid stepper walks: a `Cards` group, or the
    /// single item of a `Banner` (so a button is reachable by j/k like a 1-item row).
    pub(crate) fn cards(&self) -> Option<&[usize]> {
        match self {
            ReleaseRow::Cards(c) => Some(c),
            ReleaseRow::Banner(i) => Some(std::slice::from_ref(i)),
            ReleaseRow::Header(_) => None,
        }
    }
}

/// Move within an artist page's grouped-release carousels (the [`ReleaseRow`] rows).
/// Each `Cards` row is one group's horizontal carousel: `h`/`l` (`dx`) step ±1
/// WITHIN the current carousel (clamped — Netflix-style, no group crossing); `j`/`k`
/// (`dy`) move between carousels, keeping the column. Returns the new selected item
/// index, or `None` when a `k`/up move goes above the first carousel (the caller
/// then drops into the track list above). Shared by the local + Spotify artist pages.
pub(crate) fn release_grid_step(
    rows: &[ReleaseRow],
    sel: usize,
    dx: i32,
    dy: i32,
) -> Option<usize> {
    let Some((ri, pos)) = rows.iter().enumerate().find_map(|(ri, row)| {
        row.cards()
            .and_then(|c| c.iter().position(|&x| x == sel).map(|pos| (ri, pos)))
    }) else {
        return Some(sel); // selection isn't in a carousel → no move
    };
    if dx != 0 {
        // scroll/select within this carousel only (clamped to its ends)
        let cur = rows[ri].cards().unwrap_or(&[]);
        let np = (pos as i32 + dx).clamp(0, cur.len() as i32 - 1) as usize;
        cur.get(np).copied()
    } else if dy > 0 {
        // the next carousel down, keeping the column (clamped)
        Some(
            rows[ri + 1..]
                .iter()
                .find_map(ReleaseRow::cards)
                .map(|row| row[pos.min(row.len() - 1)])
                .unwrap_or(sel),
        )
    } else if dy < 0 {
        // the previous carousel; `None` → the caller drops into the track list above
        rows[..ri]
            .iter()
            .rev()
            .find_map(ReleaseRow::cards)
            .map(|row| row[pos.min(row.len() - 1)])
    } else {
        Some(sel)
    }
}
