//! Generic columnar table — the shared layout engine behind every source's track
//! list (local tracklist, radio stations, …). It owns the parts those renderers
//! used to hand-roll identically: measuring each column to its widest *visible*
//! content, responsively dropping the lowest-priority columns when the pane is
//! too narrow (one flexible column always survives and fills the rest), drawing
//! the header + rows via ratatui's `Table`, and registering a per-row click
//! target. Sources keep full control of cell content/colour: they supply column
//! specs and pre-windowed, pre-styled rows; this places them.

use super::col;
use crate::app::{AppState, MouseTarget};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Cell, Row, Table};

/// `drop_rank` for a column that must never be dropped but isn't the flexible
/// one (e.g. radio's play/star marker). It still occupies fixed width.
pub const PIN: u8 = u8::MAX;

/// One column of the table. `drop_rank` orders responsive hiding: **lower drops
/// first**; the flexible column (`flex`) and [`PIN`]-ranked columns never drop.
pub struct TableColumn {
    pub header: &'static str,
    /// Header (and, by convention, cell) alignment within the column.
    pub align: Alignment,
    /// This column flexes to fill the leftover width and is never dropped.
    pub flex: bool,
    /// Minimum width reserved for the flexible column in the fit decision.
    pub flex_min: u16,
    /// Lower = dropped first when space is tight ([`PIN`] = never).
    pub drop_rank: u8,
    /// Extra minimum content width beyond the header (e.g. the index column sizes
    /// to the row-count's digit width even when the visible rows are 1-digit).
    pub seed_w: usize,
}

impl TableColumn {
    /// A fixed-width data column.
    pub fn fixed(header: &'static str, drop_rank: u8) -> Self {
        Self {
            header,
            align: Alignment::Left,
            flex: false,
            flex_min: 0,
            drop_rank,
            seed_w: 0,
        }
    }
    /// The single flexible column that fills the remaining width.
    pub fn flexible(header: &'static str, min: u16) -> Self {
        Self {
            header,
            align: Alignment::Left,
            flex: true,
            flex_min: min,
            drop_rank: PIN,
            seed_w: 0,
        }
    }
    pub fn right(mut self) -> Self {
        self.align = Alignment::Right;
        self
    }
    pub fn seed(mut self, w: usize) -> Self {
        self.seed_w = w;
        self
    }
}

/// One cell: a fully-styled ratatui [`Cell`] plus its display width (so the table
/// can size the column without re-measuring the source's text).
pub struct TableCell {
    pub cell: Cell<'static>,
    pub w: usize,
}

impl TableCell {
    pub fn new(cell: Cell<'static>, w: usize) -> Self {
        Self { cell, w }
    }
}

/// One visible row: cells parallel to the column specs, an optional background,
/// and an optional click target registered at the row's rendered position.
pub struct TableRow {
    pub cells: Vec<TableCell>,
    pub bg: Option<Color>,
    pub click: Option<MouseTarget>,
}

/// Render a responsive columnar table into `area` (header row at the top, data
/// rows below). `rows` must already be windowed to the visible slice and styled.
/// `spacing` is the inter-column gap. Each row is registered as a click target.
pub fn columns_table(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    cols: &[TableColumn],
    rows: Vec<TableRow>,
    spacing: u16,
) {
    if cols.is_empty() || area.width == 0 || area.height == 0 {
        return;
    }
    let th = &app.theme;
    // Inset 1 col on each side: the table (header + rows) lays out in this
    // interior, leaving the extreme columns free for a selected row's rounded
    // end-caps. Clicks and caps still address the full-width `area`.
    let inner = Rect::new(
        area.x.saturating_add(1),
        area.y,
        area.width.saturating_sub(2),
        area.height,
    );

    // measure: each column to its widest visible content, seeded with the header
    // width (so headers never trim) and the column's `seed_w`.
    let mut cw: Vec<usize> = cols
        .iter()
        .map(|c| c.header.chars().count().max(c.seed_w))
        .collect();
    for r in &rows {
        for (k, cell) in r.cells.iter().enumerate() {
            if k < cw.len() {
                cw[k] = cw[k].max(cell.w);
            }
        }
    }

    // responsive: drop the lowest-priority columns (by `drop_rank`) until the rest
    // fit at full content width; the flexible column survives and flexes to fill.
    let keep = fit(cols, &cw, inner.width, spacing);

    let header_style = Style::default()
        .fg(col(th.text_faint))
        .add_modifier(Modifier::BOLD);
    let header = Row::new(
        keep.iter()
            .map(|&k| {
                let c = &cols[k];
                let cell = if matches!(c.align, Alignment::Right) {
                    Cell::from(Line::from(c.header).alignment(Alignment::Right))
                } else {
                    Cell::from(c.header)
                };
                cell.style(header_style)
            })
            .collect::<Vec<_>>(),
    )
    .height(1);

    let mut out: Vec<Row> = Vec::with_capacity(rows.len());
    // Parallel to `out`: the fill colour of each highlighted row, so its rounded
    // end-caps can be overlaid after the table paints.
    let mut caps: Vec<Option<Color>> = Vec::with_capacity(rows.len());
    for (vis, r) in rows.into_iter().enumerate() {
        if let Some(target) = r.click {
            let row_y = area.y + 1 + vis as u16;
            app.register_click(Rect::new(area.x, row_y, area.width, 1), target);
        }
        let cells: Vec<Cell> = r
            .cells
            .into_iter()
            .enumerate()
            .filter(|(k, _)| keep.contains(k))
            .map(|(_, c)| c.cell)
            .collect();
        let mut row = Row::new(cells);
        if let Some(bg) = r.bg {
            row = row.style(Style::default().bg(bg));
        }
        caps.push(r.bg);
        out.push(row);
    }

    let widths: Vec<Constraint> = keep
        .iter()
        .map(|&k| {
            if cols[k].flex {
                Constraint::Min(cols[k].flex_min)
            } else {
                Constraint::Length(cw[k] as u16)
            }
        })
        .collect();
    let table = Table::new(out, widths)
        .header(header)
        .column_spacing(spacing);
    f.render_widget(table, inner);

    // Round every highlighted row: overlay end-caps in the row's fill colour over
    // the freed margin columns. Header sits at `inner.y`, data rows below; skip
    // any row the table clipped past the bottom so caps never bleed downward.
    let bottom = area.y.saturating_add(area.height);
    for (vis, bg) in caps.into_iter().enumerate() {
        let Some(c) = bg else { continue };
        let row_y = inner.y + 1 + vis as u16;
        if row_y < bottom {
            super::cap_row(f, app, Rect::new(area.x, row_y, area.width, 1), c);
        }
    }
}

/// Indices of `cols` that fit `avail` cells at full content width `cw`. The
/// lowest-`drop_rank` column is dropped first until the rest fit; the flexible
/// column is never dropped (it flexes to `flex_min`).
fn fit(cols: &[TableColumn], cw: &[usize], avail: u16, spacing: u16) -> Vec<usize> {
    let mut keep = vec![true; cols.len()];
    loop {
        let kept: Vec<usize> = (0..cols.len()).filter(|k| keep[*k]).collect();
        if kept.is_empty() {
            break;
        }
        let sum: usize = kept
            .iter()
            .map(|&k| {
                if cols[k].flex {
                    cols[k].flex_min as usize
                } else {
                    cw[k]
                }
            })
            .sum();
        let total = sum + spacing as usize * kept.len().saturating_sub(1);
        if total <= avail as usize {
            break;
        }
        let victim = kept
            .iter()
            .copied()
            .filter(|&k| !cols[k].flex && cols[k].drop_rank != PIN)
            .min_by_key(|&k| cols[k].drop_rank);
        match victim {
            Some(k) => keep[k] = false,
            None => break, // only the flexible / pinned columns remain
        }
    }
    (0..cols.len()).filter(|k| keep[*k]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_drops_low_priority_when_narrow() {
        // a flexible Title plus fixed columns at varying drop priority + content
        let cols = [
            TableColumn::fixed("#", 9).seed(2),
            TableColumn::flexible("TITLE", 12),
            TableColumn::fixed("ARTIST", 11),
            TableColumn::fixed("ALBUM", 8),
            TableColumn::fixed("YEAR", 7),
            TableColumn::fixed("RATE", 10),
            TableColumn::fixed("TIME", 12),
        ];
        let cw = [2usize, 20, 14, 18, 4, 5, 5];
        // wide: keep every column
        assert_eq!(fit(&cols, &cw, 200, 2).len(), cols.len());
        // narrow: the flexible Title survives; low-priority Album drops first
        let keep = fit(&cols, &cw, 40, 2);
        assert!(keep.iter().any(|&k| cols[k].flex), "flexible column kept");
        assert!(
            !keep.iter().any(|&k| cols[k].header == "ALBUM"),
            "low-priority ALBUM dropped"
        );
        // very narrow: only the flexible column remains
        let keep = fit(&cols, &cw, 8, 2);
        assert!(keep.iter().all(|&k| cols[k].flex));
    }

    #[test]
    fn fit_never_drops_pinned_columns() {
        // a pinned marker + flexible name, both survive even at width 1
        let cols = [
            TableColumn::fixed("", PIN).seed(2),
            TableColumn::flexible("NAME", 16),
            TableColumn::fixed("VOTES", 1),
        ];
        let cw = [2usize, 30, 5];
        let keep = fit(&cols, &cw, 1, 2);
        assert!(keep.contains(&0), "pinned marker never drops");
        assert!(keep.contains(&1), "flexible column never drops");
        assert!(!keep.contains(&2), "droppable VOTES drops");
    }
}
