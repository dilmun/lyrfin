//! Columnar Spotify track list renderer, extracted from `tracklist`. It reuses the
//! local column model (`Col` / `col_header` / `drop_rank` / the shared
//! `columns_table`) so the Spotify list looks/behaves identically to the local one
//! over `api::Item` rows. The QUEUE + artist pane moved to the shared
//! `components::queue` / `components::artist`.

use super::*;
use crate::app::AppState;
use crate::app::MouseTarget;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Cell, Paragraph};

/// Whether `it` is the track Spotify currently has loaded — matched by `uri`
/// against `spov.now_spotify`. The Spotify analogue of `player.current == id` in
/// the local tracklist; drives the ▶ now-playing marker + accent on the row.
pub(crate) fn is_now_playing(app: &AppState, it: &crate::spotify::api::Item) -> bool {
    !it.uri.is_empty()
        && app
            .spov
            .now_spotify
            .as_ref()
            .is_some_and(|t| t.uri == it.uri)
}

/// Which Spotify columns are active (config-gated), in display order. Spotify
/// only carries data for #, Title, Artist, Album, Year, Time. Title is the flex
/// column; the rest reuse the local [`drop_rank`] so a narrow Spotify pane hides
/// the same low-priority columns in the same order as the local tracklist.
fn spotify_col_specs(c: &crate::config::Columns, index_w: usize) -> (Vec<TableColumn>, Vec<Col>) {
    let mut specs = Vec::new();
    let mut kinds = Vec::new();
    if c.index {
        specs.push(TableColumn::fixed(col_header(Col::Index), drop_rank(Col::Index)).seed(index_w));
        kinds.push(Col::Index);
    }
    specs.push(TableColumn::flexible(
        col_header(Col::Title),
        TITLE_MIN as u16,
    ));
    kinds.push(Col::Title);
    if c.artist {
        specs.push(TableColumn::fixed(
            col_header(Col::Artist),
            drop_rank(Col::Artist),
        ));
        kinds.push(Col::Artist);
    }
    if c.album {
        specs.push(TableColumn::fixed(
            col_header(Col::Album),
            drop_rank(Col::Album),
        ));
        kinds.push(Col::Album);
    }
    if c.year {
        specs.push(TableColumn::fixed(
            col_header(Col::Year),
            drop_rank(Col::Year),
        ));
        kinds.push(Col::Year);
    }
    if c.time {
        specs.push(TableColumn::fixed(
            col_header(Col::Time),
            drop_rank(Col::Time),
        ));
        kinds.push(Col::Time);
    }
    (specs, kinds)
}

/// Columnar tracklist for Spotify rows (`api::Item`), rendered through the shared
/// [`columns_table`] so it looks/behaves identically to the local tracklist and
/// the radio list (content-sized columns, responsive hide-when-narrow). Honors
/// the local `config.columns` show/hide toggles for the columns Spotify has data
/// for (#, Artist, Album, Year, Time); Title is the flexible column.
pub(crate) fn spotify_tracks(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    items: &[crate::spotify::api::Item],
    sel: usize,
    focused: bool,
) {
    let th = &app.theme;
    let shape = app.config.arabic_shaping; // shape Arabic fields (RTL) for display
    if area.height == 0 || area.width == 0 {
        return;
    }
    let total = items.len();
    let sel = sel.min(total.saturating_sub(1));
    let body_h = area.height.saturating_sub(1) as usize; // header row
    // sticky (not recentring) so clicking a visible row doesn't make the list jump
    let off = sticky_off(&app.spotify.list_off, sel, total, body_h);
    let index_w = total.to_string().len().max(1);

    let (cols, kinds) = spotify_col_specs(&app.config.columns, index_w);
    let meta = Style::default().fg(col(th.meta_text()));

    let mut rows: Vec<TableRow> = Vec::new();
    for (i, it) in items.iter().enumerate().skip(off).take(body_h.max(1)) {
        let selected = i == sel;
        let is_now = is_now_playing(app, it);
        // the now-playing row takes the accent + bold title (like the QUEUE pane)
        // so the current track stands out; the cursor selection keeps its own
        // background highlight (below), so the two never fight over one colour.
        let title = if is_now {
            Style::default()
                .fg(col(th.now_playing_color()))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(col(th.title_text()))
        };
        let cells: Vec<TableCell> = kinds
            .iter()
            .map(|k| match k {
                // ▶ replaces the row number on the now-playing row (its accent)
                Col::Index if is_now => TableCell::new(
                    Cell::from("▶").style(Style::default().fg(col(th.now_playing_color()))),
                    index_w,
                ),
                Col::Index => {
                    let s = format!("{:>index_w$}", i + 1);
                    TableCell::new(Cell::from(s).style(meta), index_w)
                }
                Col::Title => {
                    let s = crate::arabic::shaped(&it.name, shape);
                    let w = s.chars().count();
                    TableCell::new(Cell::from(s).style(title), w)
                }
                Col::Artist => {
                    let s = crate::arabic::shaped(&col_text(Col::Artist, &it.subtitle), shape);
                    let w = s.chars().count();
                    TableCell::new(Cell::from(s).style(meta), w)
                }
                Col::Album => {
                    let s = crate::arabic::shaped(&col_text(Col::Album, &it.album), shape);
                    let w = s.chars().count();
                    TableCell::new(Cell::from(s).style(meta), w)
                }
                Col::Year => {
                    let s = it.year.map(|y| y.to_string()).unwrap_or_default();
                    let w = s.chars().count();
                    TableCell::new(Cell::from(s).style(meta), w)
                }
                Col::Time => {
                    let s = if it.duration_ms > 0 {
                        mmss(std::time::Duration::from_millis(it.duration_ms as u64))
                    } else {
                        String::new()
                    };
                    let w = s.chars().count();
                    TableCell::new(Cell::from(s).style(meta), w)
                }
                // spotify_col_specs only emits the kinds matched above
                _ => TableCell::new(Cell::from(""), 0),
            })
            .collect();
        // selected row keeps a (dim) highlight even when the list isn't focused,
        // so you can see where the cursor is from the sidebar / a pane
        let bg = if selected && focused {
            Some(col(th.selection))
        } else if selected {
            Some(col(th.panel.mix(th.text_faint, 0.25)))
        } else {
            None
        };
        rows.push(TableRow {
            cells,
            bg,
            click: Some(MouseTarget::SpotifyItem(i)), // click selects, dbl-click plays
        });
    }

    columns_table(f, area, app, &cols, rows, 2);
}

/// Spotify's main track list as compact one-line rows — the **rows** layout
/// (chosen when `config.track_columns` is off), the Spotify analogue of
/// [`spotify_tracks`]. Shares [`compact_track_line`] / [`row_meta`] with the local
/// rows so both sources render identically, and honors the artist/album/year/time
/// column toggles Spotify can populate.
pub(crate) fn spotify_track_rows(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    items: &[crate::spotify::api::Item],
    sel: usize,
    focused: bool,
) {
    let th = &app.theme;
    let shape = app.config.arabic_shaping;
    let cols = &app.config.columns;
    if area.height == 0 || area.width == 0 {
        return;
    }
    let total = items.len();
    let sel = sel.min(total.saturating_sub(1));
    let body_h = area.height as usize;
    // sticky (not recentring) so clicking a visible row doesn't make the list jump
    let off = sticky_off(&app.spotify.list_off, sel, total, body_h);
    let meta_style = Style::default().fg(col(th.meta_text()));
    for (i, it) in items.iter().enumerate().skip(off).take(body_h.max(1)) {
        let row = Rect::new(area.x, area.y + (i - off) as u16, area.width, 1);
        app.register_click(row, MouseTarget::SpotifyItem(i)); // click selects, dbl plays
        let selected = i == sel;
        let is_now = is_now_playing(app, it);
        // ▶ + accent (bold) marks the now-playing track, mirroring the QUEUE pane;
        // the cursor selection keeps its own pill background (`sel_fill`), so both
        // can show at once and the playing row is visible even off the cursor.
        let name_style = if is_now {
            Style::default()
                .fg(col(th.now_playing_color()))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(col(th.title_text()))
        };
        let lead = if is_now {
            Span::styled("▶", Style::default().fg(col(th.now_playing_color())))
        } else {
            Span::raw(" ")
        };
        let meta = crate::arabic::shaped(&row_meta(cols, &it.subtitle, &it.album, it.year), shape);
        let name = crate::arabic::shaped(&it.name, shape);
        let time = if cols.time && it.duration_ms > 0 {
            mmss(std::time::Duration::from_millis(it.duration_ms as u64))
        } else {
            String::new()
        };
        let line = compact_track_line(
            (area.width as usize).saturating_sub(2),
            lead,
            &name,
            name_style,
            &meta,
            meta_style,
            &time,
        );
        // selected row keeps a (dim) highlight even when unfocused — matches
        // `spotify_tracks` so the cursor stays visible from the sidebar / a pane.
        let line = pill_line(
            app,
            area.width as usize,
            line,
            sel_fill(th, selected, focused),
        );
        f.render_widget(Paragraph::new(line), row);
    }
}
