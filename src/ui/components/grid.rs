//! The album/artist cover-art grid for the local library's container sections.
//! Each card shows the real cover/photo (from the `artwork` worker via
//! `app.grid_art`) inside a shape — a rounded square or, with the `grid_circle`
//! setting, a round avatar — falling back to a name-tinted placeholder (same
//! shape) while loading or on terminals without inline images. Pure render + a
//! layout helper (`grid_cells`) shared with mouse hit-testing.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use super::{clip, col, local_grid_subtitle, local_grid_year, local_item_meta};
use crate::app::{AppState, MouseTarget, ReleaseRow};

/// Smallest card width before a column is dropped.
const MIN_CARD_W: u16 = 16;
/// Side padding on each edge of a card (so covers don't touch).
const CARD_PAD: u16 = 1;
/// Rows reserved below the cover: a title row, a subtitle row, and a one-row gap.
const CARD_LABEL_ROWS: u16 = 3;
/// A large px cap so `square_image_rect` is bounded only by the card area, never by
/// the image size — the centred cover square is as big as the cell allows.
const GRID_ART_PX: u32 = 100_000;

/// Card height for a width: the cover kept ~square in PIXELS for THIS terminal's
/// actual cell aspect (`font` = cell w/h px), plus the label rows. Using the real
/// aspect + the side padding — instead of assuming 2:1 cells over the full width —
/// reserves exactly the cover's height, so the grid doesn't waste a near-row of
/// vertical space (which could otherwise cost a whole row).
fn card_h(card_w: u16, font: (u16, u16)) -> u16 {
    let (cw, ch) = (font.0.max(1) as u32, font.1.max(1) as u32);
    let cover_w_px = card_w.saturating_sub(CARD_PAD * 2) as u32 * cw;
    let cover_h = ((cover_w_px / ch) as u16).max(4);
    cover_h + CARD_LABEL_ROWS
}

/// The visible grid layout: `(item index, card rect)` for each card on screen,
/// plus the column count (the row stride that drives 2-D navigation). Shared by
/// the renderer and mouse hit-testing.
///
/// `row_off` is the persisted top-row scroll offset: the viewport is **sticky**
/// (it only scrolls when the selection leaves the visible rows), NOT re-centred on
/// the selection every frame. Re-centring made mouse use imprecise — a click moved
/// the selection, the grid then re-centred under the cursor, so the highlight
/// appeared stuck in the middle and a double-click's second click landed on a
/// different (scrolled-in) card. Sticky keeps clicked cards where they are.
pub fn grid_cells(
    inner: Rect,
    n: usize,
    sel: usize,
    card_w: u16,
    font: (u16, u16),
    row_off: &std::cell::Cell<usize>,
) -> (Vec<(usize, Rect)>, usize) {
    if inner.width < MIN_CARD_W || inner.height < 6 || n == 0 {
        row_off.set(0);
        return (Vec::new(), 1);
    }
    // FIXED card width (the size setting), clamped to fit at least one column. It
    // does NOT stretch to fill the row, so the cover size stays constant as a side
    // pane resizes — only the column count changes, discretely. The leftover is
    // absorbed by the centring below.
    let card_w = card_w.clamp(MIN_CARD_W, inner.width);
    let cols = (inner.width / card_w).max(1) as usize;
    // Card height from the real cell aspect (so the cover is square in pixels and we
    // reserve no excess vertical space) — tied to the width so the round photo
    // always covers the tinted placeholder disc behind it.
    let ch = card_h(card_w, font);
    let rows_visible = (inner.height / ch).max(1) as usize;
    let rows_total = n.div_ceil(cols);
    let sel_row = sel / cols;
    // Sticky vertical scroll (like the flat list's `sticky_off`): hold the offset and
    // only move it when the selected row scrolls off the top/bottom, so clicking a
    // visible card selects it in place instead of re-centring the grid under the cursor.
    let off_row = super::sticky_off(row_off, sel_row, rows_total, rows_visible);
    // Centre the card block in the pane on both axes (margins derived from the live
    // pane size — nothing hardcoded): the columns rarely tile the width exactly and
    // the rows rarely the height, so split the leftover evenly into equal margins
    // instead of hugging the top-left and trailing a gap.
    let last_row = (off_row + rows_visible).min(rows_total);
    let rows_shown = (last_row - off_row) as u16;
    let grid_w = cols as u16 * card_w;
    let grid_h = rows_shown * ch;
    let x0 = inner.x + inner.width.saturating_sub(grid_w) / 2;
    let y0 = inner.y + inner.height.saturating_sub(grid_h) / 2;
    let mut cells = Vec::new();
    for r in off_row..last_row {
        for c in 0..cols {
            let i = r * cols + c;
            if i >= n {
                break;
            }
            let rect = Rect::new(
                x0 + c as u16 * card_w,
                y0 + (r - off_row) as u16 * ch,
                card_w,
                ch,
            );
            cells.push((i, rect));
        }
    }
    (cells, cols)
}

/// A source-agnostic grid card: its label, a faint subtitle, and the artwork to
/// fetch/render (`None` → placeholder only). Built on demand for visible cards
/// only, so a huge library never materialises every card every frame.
pub struct GridCard {
    pub name: String,
    pub subtitle: String,
    /// Release year, shown right-aligned on the title line (albums only) — the title
    /// truncates to make room so the year is never lost. `None` → no year shown.
    pub year: Option<u16>,
    pub art: Option<(crate::artwork::ArtKey, crate::artwork::ArtSource)>,
    /// A ♥ badge in the cover's top-right — a podcast show the user already follows.
    /// `false` for every non-podcast card.
    pub followed: bool,
    /// Packed `0xRRGGBB` tile colour used for the placeholder when there's no cover
    /// (a colour-only category tile); `None` falls back to the name-hashed tint.
    pub tint: Option<u32>,
}

/// The faint subtitle under ANY cover-art grid card: the "who" only — the artist
/// (albums/tracks) or owner (playlists), never a year, track count, or album count.
/// Grids stay scannable (cover + name + who) and read identically across local and
/// Spotify. This is the single place the grid-card subtitle policy lives — change it
/// here and every grid (top-level Albums/Artists + the artist-page carousels, both
/// sources) follows. (List rows keep the richer `item_meta`/`local_item_meta`.)
pub fn grid_card_subtitle(who: &str) -> String {
    who.trim().to_string()
}

/// The data backing a grid render: the item count, the cursor, the column-count
/// cell to write (the 2-D nav stride), a per-index card supplier (called only for
/// on-screen cards), and the click-target mapper (local tracklist row vs Spotify
/// item). Bundled into one parameter so each source builds it inline.
pub struct GridData<'a> {
    pub n: usize,
    pub sel: usize,
    pub cols: &'a std::cell::Cell<usize>,
    /// Persisted top-row scroll offset (sticky viewport — see [`grid_cells`]).
    pub row_off: &'a std::cell::Cell<usize>,
    pub card_at: &'a dyn Fn(usize) -> GridCard,
    /// Maps a card index to its mouse target. A closure (not a fn pointer) so a
    /// sub-range grid (the artist page's albums) can offset into the flat list.
    pub click: &'a dyn Fn(usize) -> MouseTarget,
}

/// Render the local Albums/Artists cover-art grid (album covers / artist photos).
pub fn local_grid(f: &mut Frame, inner: Rect, app: &AppState, focused: bool) {
    render_grid(
        f,
        inner,
        app,
        focused,
        GridData {
            n: app.local.items.len(),
            sel: app.local.sel,
            cols: &app.local.cols,
            row_off: &app.local.row_off,
            card_at: &|i| {
                let item = &app.local.items[i];
                let (_, name, _) = local_item_meta(app, item);
                GridCard {
                    name,
                    // grid cards stay clean: the artist/owner only (shared policy)
                    subtitle: local_grid_subtitle(app, item),
                    year: local_grid_year(app, item),
                    art: app.item_art(item).map(|(k, s, _)| (k, s)),
                    followed: false,
                    tint: None,
                }
            },
            click: &MouseTarget::Track,
        },
    );
}

/// Render a cover-art grid from a source-agnostic [`GridData`]: lay out the visible
/// cells, record the column count, register a per-card click target, and draw each
/// card in the global circle/rounded shape. Shared by the local + Spotify grids.
pub fn render_grid(f: &mut Frame, inner: Rect, app: &AppState, focused: bool, data: GridData) {
    let sel = data.sel.min(data.n.saturating_sub(1));
    let card_w = app.config.grid_card_size.card_width();
    let (cells, ncols) = grid_cells(
        inner,
        data.n,
        sel,
        card_w,
        super::image_font(app),
        data.row_off,
    );
    data.cols.set(ncols);
    let circle = app.config.grid_circle;
    let last_visible = cells.iter().map(|(i, _)| *i).max();
    for (i, rect) in cells {
        app.register_click(rect, (data.click)(i));
        render_card(
            f,
            rect,
            app,
            &(data.card_at)(i),
            circle,
            focused && i == sel,
        );
    }
    // Prefetch-ahead: warm the covers of the next couple of rows so they're already
    // decoded by the time the user scrolls to them (smooth scroll, no pop-in). Art
    // requests are coalesced + LRU-bounded, so this is cheap and idempotent.
    if let Some(last) = last_visible {
        const PREFETCH_ROWS: usize = 2;
        let end = (last + 1 + ncols * PREFETCH_ROWS).min(data.n);
        for i in (last + 1)..end {
            if let Some((key, source)) = (data.card_at)(i).art {
                app.request_art(key, source, circle);
            }
        }
    }
}

/// The data backing a grouped-release region (the artist page): the selected source
/// item index, the column-count cell, a `visual rows` builder, and the per-index
/// card + click mappers. Source-agnostic — local builds it from `LocalItem`s,
/// Spotify from `api::Item`s — so both share `release_grid`.
pub struct ReleaseGridData<'a> {
    pub sel: usize,
    pub cols: &'a std::cell::Cell<usize>,
    /// Persisted horizontal scroll offset of the *selected* carousel (sticky — it
    /// only scrolls when the selection leaves the visible cards, so clicking a
    /// visible card doesn't re-centre the row under the cursor).
    pub car_off: &'a std::cell::Cell<usize>,
    /// Identity of the carousel `car_off` belongs to (its first item index). When the
    /// selection moves to a *different* carousel this changes, so `car_off` resets
    /// instead of carrying the old carousel's scroll into the new one (which would
    /// jump a clicked card in the newly-selected carousel).
    pub car_key: &'a std::cell::Cell<usize>,
    pub rows_at: &'a dyn Fn() -> Vec<ReleaseRow>,
    pub card_at: &'a dyn Fn(usize) -> GridCard,
    pub click: &'a dyn Fn(usize) -> MouseTarget,
}

/// First visible row so the carousels pack from the top while the selected one
/// stays on screen: advance the top row only until the selected carousel's bottom
/// fits, never past its own header (`sel_row - 1`), so a selected shelf is shown
/// with its title, and the viewport fills top-down rather than centring one row in
/// empty space. `row_tops[i]` is row `i`'s top y; `sel_bottom` is the selected
/// carousel's bottom y. Pure → unit-tested.
pub(crate) fn pack_top_row(
    row_tops: &[u16],
    sel_row: usize,
    sel_bottom: u16,
    area_h: u16,
) -> usize {
    let header_row = sel_row.saturating_sub(1);
    let mut top = 0;
    while top < header_row && sel_bottom.saturating_sub(row_tops[top]) > area_h {
        top += 1;
    }
    top
}

/// Render a grouped-release region (the artist page's ALBUMS / SINGLES / … sections)
/// as Netflix-style horizontal carousels: each section is a header + ONE row of
/// covers that scrolls left/right (`h`/`l`). Sections stack vertically and the
/// region scrolls vertically to keep the selected carousel in view; a blank row
/// precedes each header. Shared by the local + Spotify artist drill-ins.
pub fn release_grid(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    focused: bool,
    data: ReleaseGridData,
) {
    if area.width < MIN_CARD_W || area.height < 4 {
        return;
    }
    let card_w = app
        .config
        .grid_card_size
        .card_width()
        .clamp(MIN_CARD_W, area.width);
    let cols = (area.width / card_w).max(1) as usize;
    data.cols.set(cols);
    let ch = card_h(card_w, super::image_font(app));
    let rows = (data.rows_at)();
    const HEADER_H: u16 = 2; // a blank gap + the label row
    const BANNER_H: u16 = 3; // a rounded button: top border + label + bottom border

    let row_h = |r: &ReleaseRow| match r {
        ReleaseRow::Banner(_) => BANNER_H,
        _ if r.cards().is_some() => ch,
        _ => HEADER_H,
    };
    // cumulative content-space y (top) of each row
    let ys: Vec<u16> = rows
        .iter()
        .scan(0u16, |acc, r| {
            let y = *acc;
            *acc = acc.saturating_add(row_h(r));
            Some(y)
        })
        .collect();
    // Vertical scroll: pack carousels from the top and only scroll enough to keep
    // the selected one (with its header) visible — so the region fills top-down
    // instead of parking one row in empty space and dropping the others. Row-aligned
    // (no half-clipped carousels): inline-image covers scale to their cell, so a
    // partially-clipped row would squash the covers rather than crop them.
    let sel = data.sel;
    let top_row = rows
        .iter()
        .position(|r| r.cards().is_some_and(|c| c.contains(&sel)))
        .map(|sr| pack_top_row(&ys, sr, ys[sr] + row_h(&rows[sr]), area.height))
        .unwrap_or(0);
    let scroll = ys[top_row];

    // Sticky horizontal scroll for the SELECTED carousel (computed once per frame):
    // hold its offset and only move it when the selected card leaves the visible
    // window, so clicking a visible card selects it in place instead of re-centring
    // the row under the cursor. Non-selected carousels always render from the start.
    let sel_h_off = carousel_off(&rows, sel, cols, data.car_off, data.car_key);
    let h_off_of = |indices: &[usize]| {
        if indices.contains(&sel) { sel_h_off } else { 0 }
    };

    // Don't show a carousel sliver thinner than this (skip the peek / header below it).
    const MIN_PEEK: u16 = 3;
    let bottom = area.y + area.height;
    let cover_full = ch.saturating_sub(CARD_LABEL_ROWS); // the cover, minus the label rows

    for (r, row) in rows.iter().enumerate().skip(top_row) {
        let h = row_h(row);
        let y = area.y + ys[r] - scroll;
        if y >= bottom {
            break;
        }
        match row {
            // a full-width rounded button (e.g. "Browse all categories") — its own
            // row, selectable/clickable like a card but drawn as a bar with a chevron.
            ReleaseRow::Banner(idx) => {
                let idx = *idx;
                if y + BANNER_H > bottom {
                    break; // no room for the whole button — don't draw a half one
                }
                let btn = Rect::new(area.x, y, area.width, BANNER_H);
                app.register_click(btn, (data.click)(idx));
                let selected = focused && idx == sel;
                let accent = app.theme.accent[0];
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(col(if selected {
                        accent
                    } else {
                        app.theme.border
                    })));
                let inner = block.inner(btn);
                f.render_widget(block, btn);
                let style = if selected {
                    Style::default()
                        .fg(col(accent))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(col(app.theme.text))
                };
                let name = (data.card_at)(idx).name;
                f.render_widget(
                    Paragraph::new(Line::from(Span::styled(format!("  ▦  {name}"), style))),
                    inner,
                );
                if inner.width > 2 {
                    f.render_widget(
                        Paragraph::new(Span::styled("›", style)),
                        Rect::new(inner.x + inner.width - 2, inner.y, 1, 1),
                    );
                }
            }
            ReleaseRow::Header(label) => {
                if y + HEADER_H > bottom {
                    break; // no room for the header row itself
                }
                // don't strand a header without its carousel: the row below must fit
                // fully, or show at least a MIN_PEEK-tall top slice.
                if let Some(next) = rows.get(r + 1).filter(|n| n.cards().is_some()) {
                    let cards_y = y + HEADER_H;
                    let fits_full = cards_y + row_h(next) <= bottom;
                    let peek = bottom.saturating_sub(cards_y).min(cover_full);
                    if !fits_full && peek < MIN_PEEK {
                        break;
                    }
                }
                f.render_widget(
                    Paragraph::new(Span::styled(
                        format!(" {label}"),
                        Style::default()
                            .fg(col(app.theme.accent[0]))
                            .add_modifier(Modifier::BOLD),
                    )),
                    Rect::new(area.x, y + 1, area.width, 1), // row 0 is the gap
                );
            }
            // one group's horizontal carousel: window `cols` cards, scrolled so the
            // selected card (when it's in this carousel) stays visible.
            ReleaseRow::Cards(indices) => {
                let len = indices.len();
                let h_off = h_off_of(indices);
                let visible = cols.min(len - h_off);
                // A carousel that spills past the bottom renders as a "peek": each
                // cover's top slice (worker-cropped to card width), no labels/frame.
                if y + h > bottom {
                    let peek_h = bottom.saturating_sub(y).min(cover_full);
                    if peek_h < MIN_PEEK {
                        break;
                    }
                    let font = super::image_font(app);
                    for (w, &idx) in indices.iter().enumerate().skip(h_off).take(cols) {
                        let cx = area.x + (w - h_off) as u16 * card_w;
                        // pass the FULL cover geometry so the peek reuses the same
                        // centred art square as the rows above, clipped to `peek_h`.
                        let full_cover = Rect::new(
                            cx + CARD_PAD,
                            y,
                            card_w.saturating_sub(CARD_PAD * 2),
                            cover_full,
                        );
                        app.register_click(Rect::new(cx, y, card_w, peek_h), (data.click)(idx));
                        let card = (data.card_at)(idx);
                        render_peek_cover(f, app, &card, full_cover, peek_h, font);
                    }
                    break;
                }
                for (w, &idx) in indices.iter().enumerate().skip(h_off).take(cols) {
                    let cell = Rect::new(area.x + (w - h_off) as u16 * card_w, y, card_w, ch);
                    app.register_click(cell, (data.click)(idx));
                    let card = (data.card_at)(idx);
                    render_card(
                        f,
                        cell,
                        app,
                        &card,
                        app.config.grid_circle,
                        focused && idx == sel,
                    );
                }
                // prefetch a few covers just beyond the window (both directions) so
                // they're decoded before you scroll to them — no load flicker. The
                // request is coalesced + cached, so the already-shown ones (and
                // re-renders) are cheap no-ops; recency keeps the prefetched set warm.
                const PREFETCH: usize = 4;
                let lo = h_off.saturating_sub(PREFETCH);
                let hi = (h_off + visible + PREFETCH).min(len);
                for &idx in &indices[lo..hi] {
                    if let Some((k, source)) = (data.card_at)(idx).art {
                        app.request_art(k, source, app.config.grid_circle);
                    }
                }
                // ❮/❯ scroll cues when there are more covers off either edge: a bold
                // accent glyph (no background bar) at the cover's vertical middle, over
                // the outermost card's padding column (the cover is inset, so that
                // column is background). It's themed via the accent foreground, so it
                // recolors with the theme. Clickable (`MouseTarget::GridScroll`) over a
                // small band around the glyph so it's easy to hit — a click selects the
                // next hidden card, sliding the row one step (scroll-only, never plays).
                let mid = y + cover_full / 2;
                let cue = Style::default()
                    .fg(col(app.theme.accent[0]))
                    .add_modifier(Modifier::BOLD);
                let band = cover_full.clamp(1, 3); // clickable rows around the glyph
                let band_y = mid.saturating_sub(band / 2);
                let mut arrow = |ax: u16, glyph: &str, target: usize| {
                    f.render_widget(
                        Paragraph::new(Span::styled(glyph, cue)),
                        Rect::new(ax, mid, 1, 1),
                    );
                    app.register_click(
                        Rect::new(ax, band_y, 1, band),
                        MouseTarget::GridScroll(target),
                    );
                };
                if h_off > 0 {
                    arrow(area.x, "❮", indices[h_off - 1]);
                }
                if h_off + visible < len {
                    arrow(
                        area.x + visible as u16 * card_w - 1,
                        "❯",
                        indices[h_off + visible],
                    );
                }
            }
        }
    }

    // Warm covers a little beyond the fold so scrolling down (and the first open of a
    // long feed) doesn't stall on decode: request the windowed covers of the next few
    // carousels from the top of the view, bounded so a 100-item feed doesn't flood the
    // worker (request_art is a cheap no-op for already-cached / in-flight ones).
    let ahead = cols.saturating_mul(6);
    let mut warmed = 0;
    for row in rows.iter().skip(top_row) {
        if warmed >= ahead {
            break;
        }
        if let ReleaseRow::Cards(indices) = row {
            let h_off = h_off_of(indices);
            for &idx in indices.iter().skip(h_off).take(cols) {
                if let Some((k, source)) = (data.card_at)(idx).art {
                    app.request_art(k, source, app.config.grid_circle);
                }
                warmed += 1;
            }
        }
    }

    // Sticky section header: keep the current shelf's title pinned at the top while
    // its cards scroll under it, until the next shelf's header rises and pushes it
    // off. Shared by every grouped/carousel view (Home, the podcast hub, Browse
    // categories, artist pages), so the behaviour is standard app-wide.
    let scroll = scroll as i32;
    let top = area.y as i32;
    let active = rows
        .iter()
        .enumerate()
        .take_while(|(r, _)| ys[*r] as i32 <= scroll)
        .filter(|(_, row)| matches!(row, ReleaseRow::Header(_)))
        .last();
    if let Some((h, ReleaseRow::Header(label))) = active {
        // where the NEXT header currently sits on screen (far below when there's none)
        let next_y = rows
            .iter()
            .enumerate()
            .skip(h + 1)
            .find(|(_, row)| matches!(row, ReleaseRow::Header(_)))
            .map(|(r, _)| top + ys[r] as i32 - scroll)
            .unwrap_or(i32::MAX);
        // pinned at the top, unless the next header is close enough to push it up + off
        let pin_y = top.min(next_y - HEADER_H as i32);
        let label_y = pin_y + 1; // row 0 of a header block is a blank gap
        if label_y >= top {
            // clear the header block (gap + label) so cards don't bleed through, then
            // draw the label — same style as an inline header (grid.rs header arm)
            let strip_top = pin_y.max(top) as u16;
            let strip_h = ((pin_y + HEADER_H as i32) - strip_top as i32).clamp(0, HEADER_H as i32);
            if strip_h > 0 {
                f.render_widget(
                    Block::default().style(Style::default().bg(col(app.theme.panel))),
                    Rect::new(area.x, strip_top, area.width, strip_h as u16),
                );
            }
            f.render_widget(
                Paragraph::new(Span::styled(
                    format!(" {label}"),
                    Style::default()
                        .fg(col(app.theme.accent[0]))
                        .add_modifier(Modifier::BOLD),
                )),
                Rect::new(area.x, label_y as u16, area.width, 1),
            );
        }
    }
}

/// The selected carousel's sticky horizontal scroll offset. `car_off` is held across
/// frames and only moved when the selected card leaves the visible `cols` window
/// (via [`super::sticky_off`]), so a clicked card stays put instead of the carousel
/// re-centring under the cursor. `car_key` records which carousel `car_off` belongs
/// to (its first item index); when the selection moves to a *different* carousel the
/// key changes and the offset resets to 0, so the new carousel doesn't inherit the
/// old one's scroll (which would jump a clicked card). Returns 0 when the selection
/// isn't in any carousel (e.g. the POPULAR track list). Pure → unit-tested.
pub(crate) fn carousel_off(
    rows: &[ReleaseRow],
    sel: usize,
    cols: usize,
    car_off: &std::cell::Cell<usize>,
    car_key: &std::cell::Cell<usize>,
) -> usize {
    match rows
        .iter()
        .find_map(|r| r.cards().filter(|c| c.contains(&sel)))
    {
        Some(indices) => {
            let key = indices.first().copied().unwrap_or(usize::MAX);
            if car_key.get() != key {
                car_key.set(key);
                car_off.set(0);
            }
            let pos = indices.iter().position(|&x| x == sel).unwrap_or(0);
            super::sticky_off(car_off, pos, indices.len(), cols)
        }
        None => 0,
    }
}

/// Draw a partially-visible carousel cover as the TOP SLICE of the exact same
/// centred art square a full card uses — so peeked covers match the covers above in
/// size + position, not a wider edge-to-edge block. `full_cover` is the cover area as
/// if the card were fully shown; `peek_h` is how many rows are visible from its top.
/// Shows the worker-cropped top slice, or the same name-tinted disc/block placeholder
/// (clipped) while it loads.
fn render_peek_cover(
    f: &mut Frame,
    app: &AppState,
    card: &GridCard,
    full_cover: Rect,
    peek_h: u16,
    font: (u16, u16),
) {
    // the identical square the full card centres its art in (see render_thumb)
    let inset = Rect::new(
        full_cover.x + 1,
        full_cover.y + 1,
        full_cover.width.saturating_sub(2),
        full_cover.height.saturating_sub(2),
    );
    let art = super::square_image_rect(inset, font, GRID_ART_PX);
    let band_bottom = full_cover.y + peek_h;
    if art.width == 0 || art.height == 0 || art.y >= band_bottom {
        return;
    }
    let vis_h = (band_bottom - art.y).min(art.height);
    if vis_h == 0 {
        return;
    }
    let slice = Rect::new(art.x, art.y, art.width, vis_h);
    let th = &app.theme;
    if let Some((base, source)) = &card.art {
        let key = app.request_peek(
            *base,
            source.clone(),
            art.width as u32 * font.0 as u32,
            vis_h as u32 * font.1 as u32,
            vis_h,
        );
        if let Some((_, crate::app::ArtThumb::Ready(proto))) = app.grid_art.borrow().get(&key)
            && let Ok(mut p) = proto.try_borrow_mut()
        {
            f.render_widget(
                Block::default().style(Style::default().bg(col(th.panel))),
                slice,
            );
            super::render_proto_peek(f, slice, &mut p);
            return;
        }
    }
    // loading / no art → the same placeholder shape as full cards, clipped to the peek
    let tint = match card.tint {
        Some(rgb) => ratatui::style::Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8),
        None => col(th.accent[name_hash(&card.name) % th.accent.len()]),
    };
    if app.config.grid_circle {
        fill_disc(f, art, tint, vis_h);
    } else {
        f.render_widget(Block::default().style(Style::default().bg(tint)), slice);
    }
}

/// Compose a grid card's title line when it carries a year (albums): the title and
/// release year render as one centred unit `"<title> <year>"`. The year always
/// survives — when the title is too long, the *title* end-truncates (…), never the
/// year. Returns `(title, " <year>")` for the caller to render as two styled spans
/// and centre together, or `None` (centre the bare title) when there's no year or no
/// room for one. Pure → unit-tested.
pub(crate) fn card_title_parts(
    title: &str,
    year: Option<u16>,
    avail: usize,
) -> Option<(String, String)> {
    let year_str = year.map(|y| y.to_string()).unwrap_or_default();
    let yw = year_str.chars().count();
    if yw == 0 || avail < yw + 2 {
        return None;
    }
    // reserve the year + one separating space; the title takes the rest and truncates
    let t = clip(title, avail - yw - 1);
    Some((t, format!(" {year_str}")))
}

/// Draw one grid card: the cover/photo (or a name-tinted placeholder) in the
/// given shape with a selection cue, then the title + faint subtitle.
fn render_card(
    f: &mut Frame,
    rect: Rect,
    app: &AppState,
    card: &GridCard,
    circle: bool,
    selected: bool,
) {
    let th = &app.theme;
    // side padding; the label rows sit below the cover (see CARD_LABEL_ROWS)
    let pad = CARD_PAD;
    let cw = rect.width.saturating_sub(pad * 2);
    if cw == 0 || rect.height < 4 {
        return;
    }
    let x = rect.x + pad;
    let cover_h = rect.height - CARD_LABEL_ROWS;
    let cover = Rect::new(x, rect.y, cw, cover_h);

    // Art: every card follows the global circle/rounded shape, and the *placeholder*
    // matches it, not just the image. The request is coalesced + cached in `grid_art`.
    let name = &card.name;
    let key = card.art.as_ref().map(|(k, _)| *k);
    if let Some((k, source)) = &card.art {
        app.request_art(*k, source.clone(), circle);
    }
    // Both shapes: the artwork is a centred square-px region inside the cover, inset
    // a cell so the frame/ring sits in the margin (never over the opaque image).
    // `render_thumb` fills + returns that square; the frame then hugs it (one cell
    // out): a circle shows the ring only when selected, a square always shows the
    // rounded border (accent when selected).
    let inset = Rect::new(
        cover.x + 1,
        cover.y + 1,
        cover.width.saturating_sub(2),
        cover.height.saturating_sub(2),
    );
    let art = render_thumb(f, inset, app, key, name, circle, card.tint);
    // The box the user sees is the frame hugging the square (one cell out). Centre the
    // labels on THIS box, not the cell — so they line up under the artwork even when
    // the square's integer centring leaves the box a half-cell off the cell centre.
    let box_x = art.x.saturating_sub(1);
    let box_w = art.width + 2;
    if (!circle || selected) && art.width > 0 && art.height > 0 {
        let fg = if selected { th.accent[0] } else { th.border };
        f.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(col(fg))),
            Rect::new(box_x, art.y.saturating_sub(1), box_w, art.height + 2),
        );
    }
    // ♥ badge over the cover's top-right for an already-followed podcast show
    if card.followed && art.width > 0 && art.height > 0 {
        f.render_widget(
            Paragraph::new(Span::styled("♥", Style::default().fg(col(th.accent[0])))),
            Rect::new(box_x + box_w.saturating_sub(2), art.y, 1, 1),
        );
    }

    // title + subtitle — centred on the box above them. The card title/subtitle
    // read through the same title/metadata tiers as every list and the table.
    let title_style = if selected {
        Style::default()
            .fg(col(th.accent[0]))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(col(th.title_text()))
    };
    let shaped = crate::arabic::shaped(name, app.config.arabic_shaping);
    let avail = box_w as usize;
    let title_row = Rect::new(box_x, rect.y + cover_h, box_w, 1);
    match card_title_parts(&shaped, card.year, avail) {
        // album card: title + year render as one centred unit under the cover. The
        // title truncates (…) when long so the year always survives, and the pair
        // stays centred — the year is never pushed to the far right.
        Some((title, year_suffix)) => f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(title, title_style),
                Span::styled(year_suffix, Style::default().fg(col(th.meta_text()))),
            ]))
            .alignment(Alignment::Center),
            title_row,
        ),
        // no year (artists/genres/…): the title stays centred under the cover
        None => f.render_widget(
            Paragraph::new(Span::styled(clip(&shaped, avail), title_style))
                .alignment(Alignment::Center),
            title_row,
        ),
    }
    if !card.subtitle.is_empty() {
        f.render_widget(
            Paragraph::new(Span::styled(
                clip(&card.subtitle, box_w as usize),
                Style::default().fg(col(th.meta_text())),
            ))
            .alignment(Alignment::Center),
            Rect::new(box_x, rect.y + cover_h + 1, box_w, 1),
        );
    }
}

/// Draw a thumbnail inside `area` and RETURN the square rect it used (so the caller
/// can frame it). The disc/cover placeholder AND the photo share ONE largest centred
/// square-in-pixels rect, and the photo *fills* it exactly (`render_proto_filled`,
/// the same path the Spotify pane uses) — so the image always exactly covers the
/// placeholder, centred, with no tint sliver / wings / off-centre gap, for circles
/// AND rounded squares at any card aspect. A name-tinted disc (circle) or block
/// (square) shows while loading; a centred initial when nothing could be loaded.
/// No selection chrome — each caller adds its own around the returned rect.
pub(crate) fn render_thumb(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    key: Option<crate::artwork::ArtKey>,
    name: &str,
    circle: bool,
    tint: Option<u32>,
) -> Rect {
    let art = super::square_image_rect(area, super::image_font(app), GRID_ART_PX);
    fill_thumb(f, art, app, key, name, circle, tint);
    art
}

/// Fill an already-square rect with the grid-cached image, or a name-tinted
/// placeholder while it loads. Unlike [`render_thumb`], this does NOT re-square the
/// rect — the caller has already sized a clean square-in-pixels slot and wants the
/// image to fill it *exactly*. That's what lets the local ARTIST pane match the
/// Spotify pane: both fill the shared `photo_layout` square (Spotify via
/// `render_cover_filled`, local via this), so the two draw the same size with the
/// same gap below. `render_thumb` (grid cards) re-squares first because its `area`
/// isn't guaranteed square; this one trusts the caller.
pub(crate) fn fill_thumb(
    f: &mut Frame,
    art: Rect,
    app: &AppState,
    key: Option<crate::artwork::ArtKey>,
    name: &str,
    circle: bool,
    tint: Option<u32>,
) {
    if art.width == 0 || art.height == 0 {
        return;
    }
    let th = &app.theme;
    // Real cover / photo: clear the square to the PANEL bg first — NOT the tint —
    // then fill it with the image. `Scale` preserves aspect, so when the square
    // rounds to whole cells (cells aren't exactly 2:1) the image can land a fraction
    // short; clearing to the panel bg makes that letterbox edge blend into the UI
    // instead of showing the placeholder tint as a strip.
    if let Some(k) = key
        && let Some((_, crate::app::ArtThumb::Ready(proto))) = app.grid_art.borrow().get(&k)
        && let Ok(mut p) = proto.try_borrow_mut()
    {
        f.render_widget(
            Block::default().style(Style::default().bg(col(th.panel))),
            art,
        );
        super::render_proto_filled(f, art, &mut p);
        return;
    }
    // still loading / missing → a clean solid-colour placeholder + the centred
    // initial. Colour: the tile's own brand colour when it has one (colour-only
    // category tiles), else a name-hashed theme accent so cards stay distinct.
    // Shape: a generated + rounded IMAGE (the same path as the real covers), so its
    // edge is crisp instead of the blocky cell disc — which only flashes for a frame
    // while the worker generates the circle.
    let color = tint.unwrap_or_else(|| {
        let c = th.accent[name_hash(name) % th.accent.len()];
        ((c.0 as u32) << 16) | ((c.1 as u32) << 8) | c.2 as u32
    });
    // A circle needs the crisp rounded IMAGE; a rounded square is just a filled block
    // (the caller draws the rounded border around it), so it doesn't. The solid image
    // is built synchronously (`ensure_solid_art`) so every cover-less circle is crisp
    // and consistent — no more blocky cell disc stuck behind the worker's queue.
    let drew = if circle
        && let Some(solid) = app.ensure_solid_art(color, true)
        && let Some((_, crate::app::ArtThumb::Ready(proto))) = app.grid_art.borrow().get(&solid)
        && let Ok(mut p) = proto.try_borrow_mut()
    {
        f.render_widget(
            Block::default().style(Style::default().bg(col(th.panel))),
            art,
        );
        super::render_proto_filled(f, art, &mut p);
        true
    } else {
        false
    };
    if !drew {
        let fill = ratatui::style::Color::Rgb((color >> 16) as u8, (color >> 8) as u8, color as u8);
        if circle {
            fill_disc(f, art, fill, art.height);
        } else {
            f.render_widget(Block::default().style(Style::default().bg(fill)), art);
        }
    }
    let init = name
        .chars()
        .next()
        .unwrap_or('♪')
        .to_uppercase()
        .to_string();
    f.render_widget(
        Paragraph::new(Span::styled(
            init,
            Style::default().fg(col(th.bg)).add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Center),
        Rect::new(art.x, art.y + art.height / 2, art.width, 1),
    );
}

/// Fill the inscribed disc of `area`'s cells with `bg`, leaving the corners as the
/// panel background — so a placeholder reads as a circle. A card cover is ~2:1
/// cells (≈ square in pixels), so the unit ellipse touching all edges looks round.
/// Only the top `rows` cell-rows are drawn (the disc geometry still uses the full
/// `area`), so a partially-visible peek shows the top arc of the circle.
fn fill_disc(f: &mut Frame, area: Rect, bg: ratatui::style::Color, rows: u16) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let rows = rows.min(area.height);
    let buf = f.buffer_mut();
    let bounds = buf.area;
    let (cx, cy) = (area.width as f32 / 2.0, area.height as f32 / 2.0);
    for yy in 0..rows {
        for xx in 0..area.width {
            let dx = (xx as f32 + 0.5 - cx) / cx;
            let dy = (yy as f32 + 0.5 - cy) / cy;
            let (px, py) = (area.x + xx, area.y + yy);
            if dx * dx + dy * dy <= 1.0 && px < bounds.right() && py < bounds.bottom() {
                buf[(px, py)].set_bg(bg);
            }
        }
    }
}

/// A small deterministic hash of a name → a card tint index (stable per item).
fn name_hash(s: &str) -> usize {
    s.bytes()
        .fold(0usize, |h, b| h.wrapping_mul(31).wrapping_add(b as usize))
}
