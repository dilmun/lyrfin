//! Tracklist table (extracted from ui/components). The queue + artist panes moved
//! to the shared `components::queue` / `components::artist`.

use super::*;
use crate::app::{AppState, Focus, LocalItem, LocalSection, MouseTarget, ScrollBox};
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph};

// ---- tracklist table -----------------------------------------------------
#[derive(Clone, Copy)]
pub(crate) enum Col {
    Index,
    Title,
    Artist,
    AlbumArtist,
    Album,
    Year,
    Genre,
    Composer,
    Format,
    Bitrate,
    Rating,
    Time,
    Plays,
    Comment,
}

/// Active tracklist columns in display order (Title is always shown).
pub(crate) fn tracklist_cols(c: &crate::config::Columns) -> Vec<Col> {
    let mut v = Vec::new();
    if c.index {
        v.push(Col::Index);
    }
    v.push(Col::Title);
    if c.artist {
        v.push(Col::Artist);
    }
    if c.album_artist {
        v.push(Col::AlbumArtist);
    }
    if c.album {
        v.push(Col::Album);
    }
    if c.year {
        v.push(Col::Year);
    }
    if c.genre {
        v.push(Col::Genre);
    }
    if c.composer {
        v.push(Col::Composer);
    }
    if c.format {
        v.push(Col::Format);
    }
    if c.bitrate {
        v.push(Col::Bitrate);
    }
    if c.rating {
        v.push(Col::Rating);
    }
    if c.time {
        v.push(Col::Time);
    }
    if c.plays {
        v.push(Col::Plays);
    }
    if c.comment {
        v.push(Col::Comment);
    }
    v
}

pub(crate) fn col_header(c: Col) -> &'static str {
    match c {
        Col::Index => "#",
        Col::Title => "TITLE",
        Col::Artist => "ARTIST",
        Col::AlbumArtist => "ALBART",
        Col::Album => "ALBUM",
        Col::Year => "YEAR",
        Col::Genre => "GENRE",
        Col::Composer => "COMPOSER",
        Col::Format => "TYPE",
        Col::Bitrate => "KBPS",
        Col::Rating => "RATE", // ≤ the 5-wide stars so the column has no trailing pad
        Col::Time => "TIME",
        Col::Plays => "PLAYS",
        Col::Comment => "COMMENT",
    }
}

/// Display width of a column's cell for `t` — used to size each column to its
/// widest *visible* content (Index/Title handled by the caller). Capped at
/// [`col_max_w`] so a long value sizes the column no wider than its cap (the cell
/// text is clipped to match via [`col_text`]); keeps in step with the Spotify /
/// POPULAR tables, which clip the same way.
pub(crate) fn col_cell_len(c: Col, t: &crate::core::model::Track) -> usize {
    let raw = match c {
        Col::Title => t.title.chars().count(),
        Col::Artist => t.artist.chars().count(),
        Col::AlbumArtist => t.album_artist.chars().count(),
        Col::Album => t.album.chars().count(),
        Col::Year => t.year.map(|_| 4).unwrap_or(0),
        Col::Genre => t.genre.as_deref().map(|g| g.chars().count()).unwrap_or(0),
        Col::Composer => t.composer.chars().count(),
        Col::Format => t.audio.map(|a| a.codec.name().len()).unwrap_or(0),
        Col::Bitrate => t
            .audio
            .filter(|a| a.bitrate_kbps > 0)
            .map(|a| a.bitrate_kbps.to_string().len())
            .unwrap_or(0),
        Col::Rating => 5,
        Col::Time => mmss(t.duration()).chars().count(),
        Col::Plays => t.play_count.to_string().len(),
        Col::Comment => t.comment.chars().count(),
        Col::Index => 0,
    };
    match col_max_w(c) {
        0 => raw,
        max => raw.min(max),
    }
}

/// Minimum width reserved for the (flexible) Title column in the fit decision.
pub(crate) const TITLE_MIN: usize = 12;

/// Drop priority when the table is too narrow (lower = dropped first). Title is
/// never dropped.
pub(crate) fn drop_rank(c: Col) -> u8 {
    match c {
        Col::Comment => 0,
        Col::Composer => 1,
        Col::Bitrate => 2,
        Col::Format => 3,
        Col::AlbumArtist => 4,
        Col::Plays => 5,
        Col::Genre => 6,
        Col::Year => 7,
        Col::Album => 8,
        Col::Index => 9,
        Col::Rating => 10,
        Col::Artist => 11,
        Col::Time => 12,
        Col::Title => 255,
    }
}

/// Max display width for a free-text column's cell (0 = uncapped). Caps the
/// wide, variable columns (artist/album/…) so one outlier value — a 40-char
/// "feat." list or a `… (From "Movie")` album — truncates with an ellipsis
/// instead of sizing the whole column to its width. Without this, [`fit`] would
/// rather *drop the entire column* than show it that wide, so a single long row
/// could evict every other enabled column down to Title + Time. Bounded columns
/// (Year/Time/Index/Rating/Format/Bitrate/Plays) are naturally short → uncapped.
pub(crate) fn col_max_w(c: Col) -> usize {
    match c {
        Col::Title => 0, // the flexible column; sized/truncated by the layout
        Col::Artist | Col::AlbumArtist => 26,
        Col::Album => 28,
        Col::Composer => 22,
        Col::Genre => 16,
        Col::Comment => 28,
        _ => 0,
    }
}

/// A data cell's text clipped to its column's [`col_max_w`] (ellipsis), so a long
/// value truncates rather than forcing its column to drop out of a narrow table.
/// Shared by every columnar tracklist (local, Spotify, POPULAR) so they clip
/// identically. Returns the text unchanged for uncapped columns.
pub(crate) fn col_text(c: Col, s: &str) -> String {
    match col_max_w(c) {
        0 => s.to_string(),
        max => super::clip(s, max),
    }
}

/// The local view's standardized main-pane title: `MUSIC · {context} · {n}`. While
/// searching, the query lives in the inline [`search_bar`] row drawn over the list
/// (see [`local_main_body`]), so the title keeps showing the browse context.
pub fn tracklist_title(app: &AppState) -> String {
    // the drill-in breadcrumb when inside a container, else the section name
    let ctx = app
        .local
        .crumb
        .as_deref()
        .map(|c| c.to_uppercase())
        .unwrap_or_else(|| app.local.section.label().to_uppercase());
    let n = app.local.items.len();
    if app.sort.is_empty() {
        format!("MUSIC  ·  {ctx}  ·  {n}")
    } else {
        format!("MUSIC  ·  {ctx}  ·  {n}  ⇅ {}", app.sort_describe())
    }
}

/// The local library's drill-in main pane: renders `app.local.items` — a flat
/// track list as the columnar table, a list of containers (or the grouped artist
/// page) as a name + subtitle list. The local analogue of `spotify_main_body`.
pub fn local_main_body(f: &mut Frame, inner: Rect, app: &AppState) {
    let th = &app.theme;
    let focused = app.focus == Focus::Main;
    // an active `/` filter shows the local search results (flat tracks) under the
    // shared inline search row — the unified search box every source uses.
    if app.search.active || !app.search.query.is_empty() {
        let ids = app.search_results();
        let [bar, body] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
        let info = format!("{} results", ids.len());
        search_bar(
            f,
            bar,
            th,
            &SearchBar {
                query: &app.search.query,
                caret: app.search.query.chars().count(),
                focused: app.focus == Focus::Search,
                loading: false,
                tick: app.tick,
                placeholder: "search your library…",
                scope: "Library",
                info: &info,
            },
        );
        app.register_click(body, MouseTarget::Scroll(ScrollBox::Tracklist));
        local_tracklist(f, body, app, &ids, app.selection, true);
        return;
    }
    let items = &app.local.items;
    if items.is_empty() {
        local_empty_state(f, inner, app);
        return;
    }
    app.register_click(inner, MouseTarget::Scroll(ScrollBox::Tracklist));
    // a flat track list (a section of tracks, or a drilled album/playlist/genre) →
    // the columnar table; containers / the grouped artist page → a name list.
    if items.iter().all(|i| i.is_track()) {
        let ids: Vec<crate::core::model::TrackId> = items
            .iter()
            .filter_map(|i| match i {
                LocalItem::Track(t) => Some(*t),
                _ => None,
            })
            .collect();
        local_tracklist(f, inner, app, &ids, app.local.sel, focused);
        return;
    }
    // Albums/Artists can render as a cover-art grid (the `#` toggle).
    if app.local_grid_active() {
        super::local_grid(f, inner, app, focused);
        return;
    }
    // The grouped artist page mixes a POPULAR track list with grouped album grids.
    if let Some(from) = app.artist_releases_from() {
        render_artist_page(f, inner, app, from, focused);
        return;
    }
    // other container lists → a plain name list
    local_item_list(f, inner, app, focused);
}

/// What to draw when the current local list has no items. A genuinely empty
/// library (fresh install) shows the full onboarding panel; otherwise this
/// particular section just happens to be empty (no favorites yet, nothing played)
/// → a short, centered contextual hint. Mirrors `radio_empty_state`.
fn local_empty_state(f: &mut Frame, inner: Rect, app: &AppState) {
    if app.library.track_count() == 0 {
        super::welcome(f, inner, app);
        return;
    }
    let th = &app.theme;
    // Only special-case the leaf smart-lists (which can't be drilled into, so the
    // message is always accurate); containers fall back to a neutral line.
    let (head, hint): (&str, &str) = match app.local.section {
        LocalSection::Favorites => ("No favorites yet", "Press  f  on a track to favorite it"),
        LocalSection::RecentlyPlayed => ("Nothing played yet", "Tracks you play show up here"),
        LocalSection::MostPlayed => ("No plays yet", "Your most-played tracks appear here"),
        _ => ("Nothing here", "This list is empty"),
    };
    let lines = vec![
        Line::from(Span::styled(
            head,
            Style::default()
                .fg(col(th.text_dim))
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(hint, Style::default().fg(col(th.text_faint)))),
    ];
    let h = (lines.len() as u16).min(inner.height);
    let y = (inner.y + inner.height / 3).min(inner.y + inner.height.saturating_sub(h));
    f.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        Rect::new(inner.x, y, inner.width, h),
    );
}

/// Render a flat local track list in the configured layout: the column table
/// ([`track_table`]) or the compact one-line rows ([`track_rows`]), per the shared
/// `config.track_columns` toggle.
fn local_tracklist(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    ids: &[crate::core::model::TrackId],
    sel: usize,
    focused: bool,
) {
    if app.config.track_columns {
        track_table(f, area, app, ids, sel, focused);
    } else {
        track_rows(f, area, app, ids, sel, focused);
    }
}

/// Render a list of browse items (containers + group headers) as `icon · name ·
/// subtitle` rows with the cursor highlighted — section lists and the grouped
/// artist page.
fn local_item_list(f: &mut Frame, inner: Rect, app: &AppState, focused: bool) {
    let n = app.local.items.len();
    let body_h = inner.height as usize;
    let sel = app.local.sel.min(n.saturating_sub(1));
    // sticky (not recentring) so clicking a visible row doesn't make the list jump
    let off = sticky_off(&app.scroll.items, sel, n, body_h);
    for (vis, i) in (off..(off + body_h).min(n)).enumerate() {
        let row = Rect::new(inner.x, inner.y + vis as u16, inner.width, 1);
        render_item_row(f, row, app, i, sel, focused);
    }
}

/// Render one browse-list row at `row`: a styled group header, or a clickable
/// `icon · name · subtitle` row highlighted when it's the (focused) selection.
/// The unified popular-track meta line: `artist · album · year` (skips empties).
/// One place, so the local and Spotify POPULAR lists read identically.
pub(crate) fn track_meta(artist: &str, album: &str, year: Option<u16>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !artist.is_empty() {
        parts.push(artist.to_string());
    }
    if !album.is_empty() {
        parts.push(album.to_string());
    }
    if let Some(y) = year {
        parts.push(y.to_string());
    }
    parts.join("  ·  ")
}

/// The compact rows-layout meta — `artist · album · year`, including only the
/// fields whose column toggle is on, so the rows layout honors the same show/hide
/// choices as the column table. Delegates to [`track_meta`] for the join.
pub(crate) fn row_meta(
    c: &crate::config::Columns,
    artist: &str,
    album: &str,
    year: Option<u16>,
) -> String {
    track_meta(
        if c.artist { artist } else { "" },
        if c.album { album } else { "" },
        if c.year { year } else { None },
    )
}

/// `m:ss` for a track duration in ms (empty when unknown).
pub(crate) fn fmt_duration(duration_ms: u32) -> String {
    if duration_ms > 0 {
        mmss(std::time::Duration::from_millis(duration_ms as u64))
    } else {
        String::new()
    }
}

/// One unified POPULAR-track row — the track name, a faint `artist · album · year`
/// meta, and the duration pinned to the right (no columns). Shared by the local +
/// Spotify artist pages so the POPULAR list is identical for both (the caller
/// registers the click target).
fn render_popular_row(
    f: &mut Frame,
    row: Rect,
    app: &AppState,
    r: &PopularTrack,
    selected: bool,
    focused: bool,
) {
    let th = &app.theme;
    let shape = app.config.arabic_shaping;
    let cols = &app.config.columns;
    // calm title tier by default; the now-playing row takes the accent + ▶ lead
    // (like the main tracklist), and the cursor selection is the row background.
    let name_style = if r.now_playing {
        Style::default()
            .fg(col(th.now_playing_color()))
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(col(th.title_text()))
    };
    let meta_style = Style::default().fg(col(th.meta_text()));
    let lead = if r.now_playing {
        Span::styled("▶", Style::default().fg(col(th.now_playing_color())))
    } else {
        Span::raw(" ")
    };
    let meta = crate::arabic::shaped(&row_meta(cols, &r.artist, &r.album, r.year), shape);
    let name = crate::arabic::shaped(&r.name, shape);
    let time = if cols.time {
        fmt_duration(r.duration_ms)
    } else {
        String::new()
    };
    let line = compact_track_line(
        (row.width as usize).saturating_sub(2),
        lead,
        &name,
        name_style,
        &meta,
        meta_style,
        &time,
    );
    let line = pill_line(
        app,
        row.width as usize,
        line,
        sel_fill(th, selected, focused),
    );
    f.render_widget(Paragraph::new(line), row);
}

/// Lay out one compact track line into `width` cols: a 1-wide `lead` glyph, the
/// `name` (truncated first so meta/time always fit), a faint `meta` string, and
/// `time` pinned to the right edge. The shared geometry behind every rows-layout
/// list (local [`track_rows`], [`spotify_tracks`]' rows form, and the artist
/// POPULAR rows) so they align identically. `name`/`meta` are pre-shaped.
pub(crate) fn compact_track_line(
    width: usize,
    lead: Span<'static>,
    name: &str,
    name_style: Style,
    meta: &str,
    meta_style: Style,
    time: &str,
) -> Line<'static> {
    use unicode_width::UnicodeWidthStr;
    let time_w = if time.is_empty() {
        0
    } else {
        time.width() + 1 // a trailing space off the right edge
    };
    // Budget for the "name  meta" body: full width minus the lead glyph (1), the
    // two-space gap before the meta (2), the time column, and — when a time is
    // shown — one guaranteed space so the meta can never touch it.
    let gap = if time_w > 0 { 1 } else { 0 };
    let body = width.saturating_sub(3 + gap + time_w);
    // The title identifies the track, so it takes priority: shown in full when it
    // fits, capped only enough to leave the meta (artist · album) a small slice —
    // never more than half the row, so a narrow pane still favors the title. The
    // meta then fills whatever the title leaves and is clipped there, so it's
    // trimmed before the time column instead of overrunning it and a long artist
    // list can never reduce the title to a bare ellipsis.
    let meta_reserve = 12.min(body / 2);
    let name = clip(name, body.saturating_sub(meta_reserve).max(1));
    let meta = clip(meta, body.saturating_sub(name.width()));
    let used = 1 + name.width() + 2 + meta.width();
    let mut spans = vec![
        lead,
        Span::styled(name, name_style),
        Span::styled(format!("  {meta}"), meta_style),
    ];
    if time_w > 0 {
        spans.push(Span::raw(" ".repeat(width.saturating_sub(used + time_w))));
        spans.push(Span::styled(format!("{time} "), meta_style));
    }
    Line::from(spans)
}

/// One artist-page POPULAR track, source-agnostic — the fields both the row and the
/// columnar layout need. `idx` is the flat item index (selection + click target).
pub(crate) struct PopularTrack {
    pub idx: usize,
    pub name: String,
    pub artist: String,
    pub album: String,
    pub year: Option<u16>,
    pub duration_ms: u32,
    /// This is the currently-playing track (local `player.current` or Spotify
    /// `now_spotify`) — drives the ▶ marker + accent, like the main tracklist.
    pub now_playing: bool,
}

/// The artist-page POPULAR list as a columnar TITLE / ARTIST / ALBUM / YEAR / TIME
/// table (the `config.track_columns` layout) — reuses the shared `columns_table`
/// so it looks like the main tracklist. Shared by both artist pages.
pub(crate) fn render_popular_columns(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    rows: &[PopularTrack],
    sel: usize,
    focused: bool,
    click: &dyn Fn(usize) -> MouseTarget,
) {
    let th = &app.theme;
    let shape = app.config.arabic_shaping;
    let meta = Style::default().fg(col(th.meta_text()));
    let specs = vec![
        TableColumn::flexible(col_header(Col::Title), TITLE_MIN as u16),
        TableColumn::fixed(col_header(Col::Artist), drop_rank(Col::Artist)),
        TableColumn::fixed(col_header(Col::Album), drop_rank(Col::Album)),
        TableColumn::fixed(col_header(Col::Year), drop_rank(Col::Year)),
        TableColumn::fixed(col_header(Col::Time), drop_rank(Col::Time)),
    ];
    let cell = |s: String, style: Style| {
        let w = s.chars().count();
        TableCell::new(Cell::from(s).style(style), w)
    };
    let trows: Vec<TableRow> = rows
        .iter()
        .map(|r| {
            let selected = r.idx == sel;
            // no index column here to hold a ▶, so the now-playing row is marked by
            // the accent + bold title alone (as the main tracklist does when # is off).
            let title_style = if r.now_playing {
                Style::default()
                    .fg(col(th.now_playing_color()))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(col(th.title_text()))
            };
            let bg = if selected && focused {
                Some(col(th.selection))
            } else if selected {
                Some(col(th.panel.mix(th.text_faint, 0.25)))
            } else {
                None
            };
            TableRow {
                cells: vec![
                    cell(crate::arabic::shaped(&r.name, shape), title_style),
                    cell(
                        crate::arabic::shaped(&col_text(Col::Artist, &r.artist), shape),
                        meta,
                    ),
                    cell(
                        crate::arabic::shaped(&col_text(Col::Album, &r.album), shape),
                        meta,
                    ),
                    cell(r.year.map(|y| y.to_string()).unwrap_or_default(), meta),
                    cell(fmt_duration(r.duration_ms), meta),
                ],
                bg,
                click: Some(click(r.idx)),
            }
        })
        .collect();
    columns_table(f, area, app, &specs, trows, 2);
}

/// Render the artist-page POPULAR region into the TOP of `area`: a " POPULAR"
/// header, then the track list in the configured layout — the column table when
/// `config.track_columns`, else the compact rows. Returns the number of rows it
/// used so the caller can place the release grids below. Shared by both pages.
// One extra arg (the header label) over clippy's 7-arg soft limit — the leading
// track list is shared by the artist page ("POPULAR") and search ("SONGS").
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_popular_region(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    header: &str,
    rows: &[PopularTrack],
    sel: usize,
    focused: bool,
    click: &dyn Fn(usize) -> MouseTarget,
) -> u16 {
    if rows.is_empty() || area.height == 0 {
        return 0;
    }
    let th = &app.theme;
    f.render_widget(
        Paragraph::new(Span::styled(
            format!(" {header}"),
            Style::default()
                .fg(col(th.accent[0]))
                .add_modifier(Modifier::BOLD),
        )),
        Rect::new(area.x, area.y, area.width, 1),
    );
    let body = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );
    if body.height == 0 {
        return 1;
    }
    if app.config.track_columns {
        // the column table draws its own TITLE/ARTIST/… header + the track rows
        let h = (rows.len() as u16 + 1).min(body.height);
        render_popular_columns(
            f,
            Rect::new(body.x, body.y, body.width, h),
            app,
            rows,
            sel,
            focused,
            click,
        );
        1 + h
    } else {
        let n = (rows.len() as u16).min(body.height);
        for (k, r) in rows.iter().take(n as usize).enumerate() {
            let row = Rect::new(body.x, body.y + k as u16, body.width, 1);
            app.register_click(row, click(r.idx));
            render_popular_row(f, row, app, r, r.idx == sel, focused);
        }
        1 + n
    }
}

/// Shared by the plain item list and the artist page's POPULAR region.
fn render_item_row(f: &mut Frame, row: Rect, app: &AppState, i: usize, sel: usize, focused: bool) {
    let th = &app.theme;
    let item = &app.local.items[i];
    if let LocalItem::Header(h) = item {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {h}"),
                Style::default()
                    .fg(col(th.accent[0]))
                    .add_modifier(Modifier::BOLD),
            ))),
            row,
        );
        return;
    }
    app.register_click(row, MouseTarget::Track(i));
    let on = focused && i == sel;
    let (icon, name, subtitle) = local_item_meta(app, item);
    let mut name = crate::arabic::shaped(&name, app.config.arabic_shaping);
    // -2 leaves a col on each side for the selected row's rounded end-caps.
    let w = (row.width as usize).saturating_sub(2);
    let sub_w = subtitle.chars().count();
    let name_max = w.saturating_sub(3 + sub_w + 2);
    if name.chars().count() > name_max {
        name = name
            .chars()
            .take(name_max.saturating_sub(1))
            .collect::<String>()
            + "…";
    }
    let line = Line::from(vec![
        Span::styled(
            format!(" {icon} "),
            Style::default().fg(col(if on { th.accent[0] } else { th.text_dim })),
        ),
        Span::styled(
            name,
            if on {
                Style::default()
                    .fg(col(th.text))
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(col(th.title_text()))
            },
        ),
        Span::styled(
            format!("  {subtitle}"),
            Style::default().fg(col(th.meta_text())),
        ),
    ]);
    let line = pill_line(app, row.width as usize, line, sel_fill(th, on, focused));
    f.render_widget(Paragraph::new(line), row);
}

/// The grouped artist page as a mixed layout: the POPULAR tracks as a list pinned
/// at the top (`items[0..from]`), then the artist's releases as grouped cover
/// grids below (`items[from..]`, via `artist_release_grid`). One flat `local.sel`
/// spans both; the release grids scroll within their region while the list stays.
fn render_artist_page(f: &mut Frame, inner: Rect, app: &AppState, from: usize, focused: bool) {
    // collect the POPULAR tracks (items[0..from]; item 0 is the "POPULAR" header)
    let popular: Vec<PopularTrack> = (0..from)
        .filter_map(|i| match &app.local.items[i] {
            LocalItem::Track(id) => app.library.track(*id).map(|t| PopularTrack {
                idx: i,
                name: t.title.clone(),
                artist: t.artist.to_string(),
                album: t.album.to_string(),
                year: t.year,
                duration_ms: t.duration_ms,
                now_playing: app.player.current == Some(*id),
            }),
            _ => None,
        })
        .collect();
    // pin the POPULAR region at the top, always leaving the release grids ≥6 rows
    let list_h = render_popular_region(
        f,
        Rect::new(
            inner.x,
            inner.y,
            inner.width,
            inner.height.saturating_sub(6),
        ),
        app,
        crate::app::release::POPULAR_HEADER,
        &popular,
        app.local.sel,
        focused,
        &MouseTarget::Track,
    );
    let region = Rect::new(
        inner.x,
        inner.y + list_h,
        inner.width,
        inner.height.saturating_sub(list_h),
    );
    super::release_grid(
        f,
        region,
        app,
        focused && app.local.sel >= from,
        super::ReleaseGridData {
            sel: app.local.sel,
            cols: &app.local.cols,
            car_off: &app.local.car_off,
            car_key: &app.local.car_key,
            rows_at: &|| app.artist_release_rows(),
            card_at: &|idx| {
                let item = &app.local.items[idx];
                let (_, name, _) = local_item_meta(app, item);
                super::GridCard {
                    name,
                    // grid cards stay clean: the artist/owner only (shared policy)
                    subtitle: local_grid_subtitle(app, item),
                    year: local_grid_year(app, item),
                    art: app.item_art(item).map(|(k, s, _)| (k, s)),
                    followed: false,
                    tint: None,
                }
            },
            click: &|idx| MouseTarget::Track(idx),
        },
    );
}

/// `(icon, name, subtitle)` for a browse item, from the in-memory library.
pub(crate) fn local_item_meta(app: &AppState, item: &LocalItem) -> (&'static str, String, String) {
    match item {
        LocalItem::Track(id) => {
            let t = app.library.track(*id);
            (
                "♪",
                t.map(|t| t.title.clone()).unwrap_or_default(),
                t.map(|t| format!("{}  ·  {}", t.artist, t.album))
                    .unwrap_or_default(),
            )
        }
        LocalItem::Album(id) => {
            let a = app.library.albums.get(id);
            let n = app.library.tracks_of(*id).len();
            (
                "◉",
                a.map(|a| a.title.clone()).unwrap_or_default(),
                a.map(|a| {
                    let year = a.year.map(|y| format!("{y}  ·  ")).unwrap_or_default();
                    format!("{}  ·  {year}{n} tracks", a.artist)
                })
                .unwrap_or_default(),
            )
        }
        LocalItem::Artist(id) => {
            let a = app.library.artists.get(id);
            let n = app.library.albums_of(*id).len();
            (
                "☻",
                a.map(|a| a.name.clone()).unwrap_or_default(),
                format!("{n} albums"),
            )
        }
        LocalItem::Playlist(id) => {
            let p = app.library.playlists.get(id);
            let n = app.library.playlist_tracks(*id).len();
            (
                "≡",
                p.map(|p| p.name.clone()).unwrap_or_default(),
                format!("{n} tracks"),
            )
        }
        LocalItem::Genre(g) => {
            let n = app.library.genre_counts.get(g).copied().unwrap_or(0);
            ("⊞", g.clone(), format!("{n} tracks"))
        }
        LocalItem::Header(_) => ("", String::new(), String::new()),
    }
}

/// The cover-grid card subtitle for a local browse item: the "who" only — the artist
/// (albums/tracks); an artist/genre card IS the subject and local playlists have no
/// owner, so those are blank. Shared by every local grid (top-level Albums/Artists +
/// the artist-page carousels) so they read identically and match Spotify; list rows
/// keep the richer `local_item_meta`. See `grid_card_subtitle`.
pub(crate) fn local_grid_subtitle(app: &AppState, item: &LocalItem) -> String {
    let who = match item {
        LocalItem::Album(id) => app.library.albums.get(id).map(|a| a.artist.to_string()),
        LocalItem::Track(id) => app.library.track(*id).map(|t| t.artist.to_string()),
        LocalItem::Artist(_)
        | LocalItem::Genre(_)
        | LocalItem::Playlist(_)
        | LocalItem::Header(_) => None,
    };
    super::grid_card_subtitle(&who.unwrap_or_default())
}

/// Release year for a local grid card — albums only (shown right-aligned on the
/// card title; everything else has no year).
pub(crate) fn local_grid_year(app: &AppState, item: &LocalItem) -> Option<u16> {
    match item {
        LocalItem::Album(id) => app.library.albums.get(id).and_then(|a| a.year),
        _ => None,
    }
}

/// The [`TableColumn`] spec for a tracklist column: Title flexes, Index sizes to
/// the row-count's digit width, the rest are fixed at their content width and
/// drop (lowest-priority first) when the pane is too narrow.
fn col_spec(c: Col, index_w: usize) -> TableColumn {
    match c {
        Col::Title => TableColumn::flexible(col_header(c), TITLE_MIN as u16),
        Col::Index => TableColumn::fixed(col_header(c), drop_rank(c)).seed(index_w),
        _ => TableColumn::fixed(col_header(c), drop_rank(c)),
    }
}

/// Render a track table for `ids` into `inner` using the configured columns
/// (responsive full-or-hide via the shared [`columns_table`]).
pub fn track_table(
    f: &mut Frame,
    inner: Rect,
    app: &AppState,
    ids: &[crate::core::model::TrackId],
    sel_raw: usize,
    focused: bool,
) {
    let th = &app.theme;
    let sel = sel_raw.min(ids.len().saturating_sub(1));
    // live visual range over THIS list's cursor (`sel`), computed once — the app
    // side resolves marks/bulk-ops against the same anchor+cursor, so what's shaded
    // is exactly what an operator would act on.
    let vis = app.marks.anchor.map(|a| (a.min(sel), a.max(sel)));

    let active: Vec<Col> = tracklist_cols(&app.config.columns).into_iter().collect();

    // window the rows so the selection stays visible in large libraries. Sticky
    // (not recentring) so clicking a visible row doesn't make the list jump.
    let body_h = inner.height.saturating_sub(1) as usize; // header row
    let total = ids.len();
    let offset = sticky_off(&app.scroll.list, sel, total, body_h);
    // index cells show row numbers — size the column to the row-count's digits.
    let index_w = total.to_string().len().max(1);

    let cols: Vec<TableColumn> = active.iter().map(|c| col_spec(*c, index_w)).collect();

    // build the visible rows; the shared table measures, responsively drops the
    // low-priority columns, and registers the per-row click target.
    let mut rows: Vec<TableRow> = Vec::new();
    for (i, id) in ids.iter().enumerate().skip(offset).take(body_h.max(1)) {
        let Some(tk) = app.library.track(*id) else {
            continue;
        };
        let target = MouseTarget::Track(i);
        let is_now = app.player.current == Some(*id);
        let is_sel = i == sel;
        // metadata reads a step below the title. The now-playing row is marked by an
        // accent + bold title and a ▶ (below) — matching the Spotify list — with no
        // full-row background, so it never competes with the cursor selection.
        let title_style = if is_now {
            Style::default()
                .fg(col(th.now_playing_color()))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(col(th.title_text()))
        };
        let meta = Style::default().fg(col(th.meta_text()));

        // marked set + the live visual range (both against this list's cursor)
        let marked = app.marks.ids.contains(&crate::app::MarkKey::Track(*id))
            || vis.is_some_and(|(lo, hi)| i >= lo && i <= hi);
        let cells: Vec<TableCell> = active
            .iter()
            .map(|c| {
                let w = col_cell_len(*c, tk);
                let cell = match c {
                    Col::Index if marked => {
                        Cell::from("✓").style(Style::default().fg(col(th.marked_color())))
                    }
                    Col::Index if is_now => {
                        Cell::from("▶").style(Style::default().fg(col(th.now_playing_color())))
                    }
                    Col::Index => {
                        // width must match the column (index_w) — a hardcoded {:>2}
                        // gets truncated to a leading space in the 1-wide column of a
                        // short (≤9-track) album, showing nothing.
                        Cell::from(format!("{:>index_w$}", i + 1)).style(meta)
                    }
                    Col::Title => Cell::from(tk.title.clone()).style(title_style),
                    Col::Artist => Cell::from(col_text(Col::Artist, &tk.artist)).style(meta),
                    Col::AlbumArtist => {
                        Cell::from(col_text(Col::AlbumArtist, &tk.album_artist)).style(meta)
                    }
                    Col::Album => Cell::from(col_text(Col::Album, &tk.album)).style(meta),
                    Col::Year => {
                        Cell::from(tk.year.map(|y| y.to_string()).unwrap_or_default()).style(meta)
                    }
                    Col::Genre => Cell::from(col_text(
                        Col::Genre,
                        tk.genre.as_deref().unwrap_or_default(),
                    ))
                    .style(meta),
                    Col::Composer => Cell::from(col_text(Col::Composer, &tk.composer)).style(meta),
                    Col::Format => {
                        Cell::from(tk.audio.map(|a| a.codec.name()).unwrap_or("")).style(meta)
                    }
                    Col::Bitrate => Cell::from(
                        tk.audio
                            .filter(|a| a.bitrate_kbps > 0)
                            .map(|a| a.bitrate_kbps.to_string())
                            .unwrap_or_default(),
                    )
                    .style(meta),
                    Col::Rating => {
                        Cell::from(stars(tk.rating)).style(Style::default().fg(col(th.warning)))
                    }
                    Col::Time => Cell::from(mmss(tk.duration())).style(meta),
                    Col::Plays => Cell::from(tk.play_count.to_string()).style(meta),
                    Col::Comment => Cell::from(col_text(Col::Comment, &tk.comment)).style(meta),
                };
                TableCell::new(cell, w)
            })
            .collect();
        // the cursor owns the row background (bright focused / dim unfocused, via
        // the shared `sel_fill`); marked rows tint when not selected. The now-playing
        // row carries no background — only its accent title + ▶.
        let bg = if is_sel {
            sel_fill(th, true, focused).map(col)
        } else if marked {
            Some(col(th.marked_color().mix(th.bg, 0.82)))
        } else {
            None
        };
        rows.push(TableRow {
            cells,
            bg,
            click: Some(target),
        });
    }

    columns_table(f, inner, app, &cols, rows, 2);
}

/// The local main tracklist as compact one-line rows — the **rows** layout (the
/// local analogue of [`track_table`], chosen when `config.track_columns` is off).
/// Mirrors the table's row windowing, now-playing / marked / visual-range
/// highlight precedence and click targets, so flipping layout only reshapes each
/// row. Each row honors the artist/album/year/time column toggles (via
/// [`row_meta`]) so both layouts respect the same show/hide choices.
pub fn track_rows(
    f: &mut Frame,
    inner: Rect,
    app: &AppState,
    ids: &[crate::core::model::TrackId],
    sel_raw: usize,
    focused: bool,
) {
    let th = &app.theme;
    let shape = app.config.arabic_shaping;
    let cols = &app.config.columns;
    let total = ids.len();
    if total == 0 || inner.height == 0 || inner.width == 0 {
        return;
    }
    let sel = sel_raw.min(total - 1);
    let vis = app.marks.anchor.map(|a| (a.min(sel), a.max(sel)));
    let body_h = inner.height as usize;
    let off = sticky_off(&app.scroll.list, sel, total, body_h);
    let meta_style = Style::default().fg(col(th.meta_text()));
    for (i, id) in ids.iter().enumerate().skip(off).take(body_h) {
        let Some(tk) = app.library.track(*id) else {
            continue;
        };
        let row = Rect::new(inner.x, inner.y + (i - off) as u16, inner.width, 1);
        app.register_click(row, MouseTarget::Track(i));
        let is_now = app.player.current == Some(*id);
        let is_sel = i == sel;
        let marked = app.marks.ids.contains(&crate::app::MarkKey::Track(*id))
            || vis.is_some_and(|(lo, hi)| i >= lo && i <= hi);
        // leading 1-wide status glyph: ▶ playing · ✓ marked · else blank — the
        // rows-layout stand-in for the table's index-column markers.
        let lead = if is_now {
            Span::styled("▶", Style::default().fg(col(th.now_playing_color())))
        } else if marked {
            Span::styled("✓", Style::default().fg(col(th.marked_color())))
        } else {
            Span::raw(" ")
        };
        // the now-playing row takes an accent + bold title (matching the Spotify
        // list); every other title reads as a calm tier just above the faint meta.
        let name_style = if is_now {
            Style::default()
                .fg(col(th.now_playing_color()))
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(col(th.title_text()))
        };
        let meta = crate::arabic::shaped(&row_meta(cols, &tk.artist, &tk.album, tk.year), shape);
        let name = crate::arabic::shaped(&tk.title, shape);
        let time = if cols.time {
            mmss(tk.duration())
        } else {
            String::new()
        };
        let line = compact_track_line(
            (inner.width as usize).saturating_sub(2),
            lead,
            &name,
            name_style,
            &meta,
            meta_style,
            &time,
        );
        // match track_table: the cursor owns the background (bright focused / dim
        // unfocused, via `sel_fill`), marked rows tint when not selected, and the
        // now-playing row carries no background — only its accent title + ▶ lead.
        let bg = if is_sel {
            sel_fill(th, true, focused)
        } else if marked {
            Some(th.marked_color().mix(th.bg, 0.82))
        } else {
            None
        };
        let line = pill_line(app, inner.width as usize, line, bg);
        f.render_widget(Paragraph::new(line), row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flatten a `Line` back to the plain text it renders (spans concatenated).
    fn flat(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// A meta far longer than the row must be trimmed *before* the time column,
    /// never overrun it: the line fits the width and the time keeps a leading gap.
    #[test]
    fn compact_line_trims_long_meta_before_the_time() {
        let width = 60;
        let time = "3:56";
        let long_meta =
            "Tanishk Bagchi, Sanju Rathod, G-SPXRK, Rashmi Virag  ·  Saree (From \"Dhamaal 4\")";
        let line = compact_track_line(
            width,
            Span::raw(" "),
            "Saree (From \"Dhamaal 4\")",
            Style::default(),
            long_meta,
            Style::default(),
            time,
        );
        let s = flat(&line);
        assert!(
            s.chars().count() <= width,
            "line overruns width: {} > {width} — {s:?}",
            s.chars().count()
        );
        let idx = s.rfind(time).expect("time is still rendered");
        assert!(
            s[..idx].ends_with(' '),
            "time is glued to the meta (no gap): {s:?}"
        );
    }

    /// A title takes priority over the meta: even an 8-artist meta longer than the
    /// row must not collapse the title to a bare ellipsis — the title stays whole
    /// when it fits, and the meta is the part that yields.
    #[test]
    fn compact_line_keeps_the_title_when_meta_is_huge() {
        let width = 80;
        let title = "Ishq Kameena 2.0 (From \"Baby Do Die Do\")";
        let huge_meta = "Alka Yagnik, Lijo George, Anu Malik, Dee MC, Sonu Nigam, AKASA, \
             Sameer Anjaan, Mohsin Shaikh  ·  Ishq Kameena 2.0 (From \"Baby Do Die Do\")";
        let line = compact_track_line(
            width,
            Span::raw(" "),
            title,
            Style::default(),
            huge_meta,
            Style::default(),
            "3:08",
        );
        let s = flat(&line);
        assert!(s.chars().count() <= width, "line overruns width: {s:?}");
        assert!(
            s.contains(title),
            "title was clipped away by the meta: {s:?}"
        );
        let idx = s.rfind("3:08").expect("time is still rendered");
        assert!(s[..idx].ends_with(' '), "time is glued to the meta: {s:?}");
    }

    /// A long CJK title is budgeted by display *columns*, not chars: each wide
    /// glyph is two columns, so the rendered line must still fit the width and the
    /// duration must keep its leading gap (the reported "time overlaps the title"
    /// bug, where a char-count budget let a Chinese title overrun the row).
    #[test]
    fn compact_line_fits_a_wide_cjk_title_without_overlapping_time() {
        use unicode_width::UnicodeWidthStr;
        let width = 60;
        let time = "3:26";
        // 25 CJK chars → 50 display columns, well over a char-count budget's guess.
        let line = compact_track_line(
            width,
            Span::raw(" "),
            "花落无痕（《白月梵星》影视剧片头曲）左手指月電視劇",
            Style::default(),
            "Sa Dingding · 花落无痕（《白月梵星》影视剧片头曲）",
            Style::default(),
            time,
        );
        let s = flat(&line);
        assert!(
            s.width() <= width,
            "line overruns width: {} cols > {width} — {s:?}",
            s.width()
        );
        let idx = s.rfind(time).expect("time is still rendered");
        assert!(
            s[..idx].ends_with(' '),
            "time is glued to / overlapping the title: {s:?}"
        );
    }

    /// The common case: short name + meta leaves the time right-aligned with plenty
    /// of gap, and nothing is clipped.
    #[test]
    fn compact_line_right_aligns_time_when_it_fits() {
        let width = 60;
        let line = compact_track_line(
            width,
            Span::raw(" "),
            "Song",
            Style::default(),
            "Artist  ·  Album",
            Style::default(),
            "3:02",
        );
        let s = flat(&line);
        assert!(s.chars().count() <= width);
        assert!(
            s.contains("Song") && s.contains("Album"),
            "no clipping: {s:?}"
        );
        assert!(s.trim_end().ends_with("3:02"));
    }
}
