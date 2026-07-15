//! The Radio source view: the RADIO section sidebar (drill-in [`RadioSection`]s),
//! the station list (a responsive columnar table), the country/genre filter
//! picker, and the empty state. Split out of `views` to keep the radio rendering
//! self-contained; the shared `clip` helper lives in the parent module, the
//! sidebar reuses `components::section_list`, and the search row is the shared
//! `components::search_bar`.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph};

use super::clip;
use crate::app::{AppState, Focus, MouseTarget, Panel, RadioSection, ScrollBox};
use crate::ui::components;

pub fn radio(f: &mut Frame, area: Rect, app: &AppState) {
    let r = &app.radio;

    // A filter picker owns the whole panel (it has its own search header), so the
    // station search box is never shown beneath it — no second search on screen.
    if let Some(p) = &r.picker {
        let title = match p.kind {
            crate::app::PickerKind::Country => "RADIO  ·  COUNTRY".to_string(),
            // genres are scoped to the selected country, so name it in the title
            crate::app::PickerKind::Genre => match &r.country {
                Some((name, _)) => format!("RADIO  ·  GENRE IN {}", name.to_uppercase()),
                None => "RADIO  ·  GENRE".to_string(),
            },
        };
        let inner = components::panel(f, area, app, &title, true);
        if inner.height >= 2 {
            radio_picker(f, inner, app);
        }
        return;
    }

    // Standardized shell: a docked RADIO section sidebar + a titled main pane (the
    // active section's name, or a drilled-in playlist's name), body drawn by
    // `radio_body` — same chrome as the Dashboard / Spotify views.
    let title = radio_main_title(r);
    components::browser_shell(
        f,
        area,
        app,
        &[Panel::Sidebar],
        &|f, slot, app, panel| {
            if panel == Panel::Sidebar {
                let inner = components::panel(f, slot, app, "RADIO", app.focus == Focus::Sidebar);
                radio_sidebar(f, inner, app);
            }
        },
        components::ShellPane {
            title: &title,
            title_right: None,
            focused: app.focus == Focus::Main || r.editing,
            render: &|f, inner, app| radio_body(f, inner, app),
        },
    );

    // playlist modals (name entry / add picker / delete confirm) draw on top
    if app.radio.pl.modal_open() {
        radio_playlist_modal(f, area, app);
    }
}

/// The main-pane title: the active section, or `PLAYLISTS · <name>` when drilled in.
fn radio_main_title(r: &crate::app::Radio) -> String {
    if r.section == RadioSection::Playlists
        && let Some(id) = r.pl.open
        && let Some(p) = r.playlists.iter().find(|p| p.id == id)
    {
        return format!("PLAYLISTS · {}", p.name.to_uppercase());
    }
    r.section.label().to_uppercase()
}

/// The RADIO section sidebar: a flat drill-in list of [`RadioSection`]s, rendered
/// with the shared `section_list` widget so it matches the library/Spotify sidebars.
fn radio_sidebar(f: &mut Frame, inner: Rect, app: &AppState) {
    if inner.height == 0 {
        return;
    }
    app.register_click(inner, MouseTarget::Scroll(ScrollBox::Tree));
    let rows: Vec<(&str, &str)> = RadioSection::ALL
        .iter()
        .map(|s| (s.icon(), s.label()))
        .collect();
    let selected = RadioSection::ALL
        .iter()
        .position(|&s| s == app.radio.section)
        .unwrap_or(0);
    // per-section click targets (click selects, double-click activates)
    for i in 0..rows.len().min(inner.height as usize) {
        app.register_click(
            Rect::new(inner.x, inner.y + i as u16, inner.width, 1),
            MouseTarget::RadioSectionRow(i),
        );
    }
    components::section_list(f, inner, app, &rows, selected, app.focus == Focus::Sidebar);
}

/// The flat list of playlists (Playlists section, not drilled in): `≡ name · N`
/// rows with a rounded selection pill (bright when the list is focused). Empty →
/// the shared centred empty state ("press n to create one").
fn radio_playlist_list(f: &mut Frame, inner: Rect, app: &AppState) {
    let th = &app.theme;
    app.register_click(inner, MouseTarget::Scroll(ScrollBox::Radio));
    let playlists = app.radio_playlists_sorted();
    let n = playlists.len();
    if n == 0 || inner.height == 0 {
        radio_empty_state(f, inner, app);
        return;
    }
    let body_h = inner.height as usize;
    let sel = app.radio.pl.sel.min(n - 1);
    let off = components::sticky_off(&app.radio.pl.list_off, sel, n, body_h);
    let focused = app.focus == Focus::Main;
    let mut lines: Vec<Line> = Vec::new();
    for (i, p) in playlists.iter().enumerate().skip(off).take(body_h) {
        let on = i == sel;
        let row_y = inner.y + (i - off) as u16;
        app.register_click(
            Rect::new(inner.x, row_y, inner.width, 1),
            MouseTarget::RadioPlaylistRow(i),
        );
        let mut name_style = Style::default().fg(th.title_text().into());
        if on {
            name_style = name_style.add_modifier(Modifier::BOLD);
        }
        let content = Line::from(vec![
            Span::styled("≡ ", Style::default().fg(th.accent[0].into())),
            Span::styled(p.name.clone(), name_style),
            Span::styled(
                format!("  · {}", p.stations.len()),
                Style::default().fg(th.meta_text().into()),
            ),
        ]);
        let bg = if on && focused {
            Some(th.selection)
        } else {
            None
        };
        lines.push(components::pill_line(
            app,
            inner.width as usize,
            content,
            bg,
        ));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

/// Whichever radio-playlist modal is open (name entry / add-to-playlist picker /
/// delete confirm), drawn as a centred rounded dialog over `area`.
fn radio_playlist_modal(f: &mut Frame, area: Rect, app: &AppState) {
    use crate::app::RadioNameTarget;
    let th = &app.theme;
    let pl = &app.radio.pl;

    // name entry (create / rename) — priority, since it can open over the add picker
    if let Some(target) = pl.naming {
        let title = match target {
            RadioNameTarget::New => "NEW PLAYLIST",
            RadioNameTarget::Rename(_) => "RENAME PLAYLIST",
        };
        let body = components::overlay_frame(
            f,
            area,
            app,
            46,
            5,
            &components::FrameSpec::dialog(title, &[("⏎", "save"), ("esc", "cancel")]),
        );
        if body.height > 0 {
            let text = Line::from(vec![
                Span::styled(pl.buffer.clone(), Style::default().fg(th.text.into())),
                Span::styled("▌", Style::default().fg(th.accent[0].into())),
            ]);
            f.render_widget(Paragraph::new(text), body);
        }
        return;
    }

    // delete confirm
    if let Some(id) = pl.confirm_delete {
        let name = app
            .radio
            .playlists
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.name.clone())
            .unwrap_or_default();
        let body = components::overlay_frame(
            f,
            area,
            app,
            46,
            5,
            &components::FrameSpec::dialog("DELETE PLAYLIST", &[("y", "delete"), ("n", "cancel")]),
        );
        if body.height > 0 {
            let text = Line::from(vec![
                Span::styled("Delete ", Style::default().fg(th.text_dim.into())),
                Span::styled(
                    format!("\"{name}\""),
                    Style::default()
                        .fg(th.title_text().into())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("?", Style::default().fg(th.text_dim.into())),
            ]);
            f.render_widget(Paragraph::new(text), body);
        }
        return;
    }

    // add-to-playlist picker (a trailing "New playlist" row)
    if !pl.adding.is_empty() {
        let playlists = app.radio_playlists_sorted();
        let rows = playlists.len() + 1;
        let h = (rows as u16 + 4).clamp(6, area.height);
        let title = if pl.adding.len() == 1 {
            "ADD TO PLAYLIST".to_string()
        } else {
            format!("ADD {} STATIONS TO PLAYLIST", pl.adding.len())
        };
        let body = components::overlay_frame(
            f,
            area,
            app,
            48,
            h,
            &components::FrameSpec::dialog(
                &title,
                &[("⏎", "add"), ("↑↓", "move"), ("esc", "cancel")],
            ),
        );
        if body.height == 0 {
            return;
        }
        let sel = pl.add_sel.min(rows - 1);
        let mut lines: Vec<Line> = Vec::new();
        for (i, p) in playlists.iter().enumerate().take(body.height as usize) {
            let on = i == sel;
            let content = Line::from(vec![
                Span::styled("≡ ", Style::default().fg(th.accent[0].into())),
                Span::styled(p.name.clone(), Style::default().fg(th.title_text().into())),
                Span::styled(
                    format!("  · {}", p.stations.len()),
                    Style::default().fg(th.meta_text().into()),
                ),
            ]);
            let bg = if on { Some(th.selection) } else { None };
            lines.push(components::pill_line(app, body.width as usize, content, bg));
        }
        if (lines.len() as u16) < body.height {
            let on_new = sel == playlists.len();
            let content = Line::from(Span::styled(
                "＋ New playlist…",
                Style::default()
                    .fg(th.accent[0].into())
                    .add_modifier(Modifier::BOLD),
            ));
            let bg = if on_new { Some(th.selection) } else { None };
            lines.push(components::pill_line(app, body.width as usize, content, bg));
        }
        f.render_widget(Paragraph::new(lines), body);
    }
}

/// The Radio view body (search box + filter chips + station list), drawn into the
/// shell's inner rect.
fn radio_body(f: &mut Frame, inner: Rect, app: &AppState) {
    let th = &app.theme;
    let r = &app.radio;
    if inner.height < 3 {
        return;
    }
    // Playlists section has no search box / filter chips: the flat list of playlists,
    // or (drilled in) that playlist's stations as the full-height station table.
    if r.section == RadioSection::Playlists {
        if r.pl.open.is_none() {
            radio_playlist_list(f, inner, app);
        } else {
            radio_station_list(f, inner, app);
        }
        return;
    }
    // a blank gutter row separates the options pane from the list
    let [search, filters, _gap, list] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(inner);

    // --- search box (focused via '/') ---
    let info = if !r.note.is_empty() {
        r.note.clone()
    } else {
        let n = app.radio_view_list().len();
        let noun = match r.section {
            RadioSection::Favorites => "favorites",
            RadioSection::Recent => "recent",
            RadioSection::MostPlayed => "most played",
            RadioSection::Playlists => "playlists",
            _ => "stations",
        };
        format!("{n} {noun}")
    };
    let caret = r.query.chars().count();
    components::search_bar(
        f,
        search,
        th,
        &components::SearchBar {
            query: &r.query,
            caret,
            focused: r.editing,
            loading: false,
            tick: app.tick,
            placeholder: "search stations…",
            scope: "",
            info: &info,
        },
    );
    app.register_click(search, MouseTarget::RadioChip(0)); // click to focus search

    // --- filter chips: Country · Genre · Sort (clickable pills) ---
    let country = r
        .country
        .as_ref()
        .map(|(n, _)| n.clone())
        .unwrap_or_else(|| "All".into());
    let genre = r.tag.clone().unwrap_or_else(|| "Any".into());
    let defs: [(u8, &str, String, bool); 3] = [
        (1, "Country", country, r.country.is_some()),
        (2, "Genre", genre, r.tag.is_some()),
        (3, "Sort", r.sort.label().to_string(), false),
    ];
    let mut chips: Vec<Span> = vec![Span::raw(" ")];
    let mut cx = filters.x + 1; // leading space
    let right = filters.x + filters.width;
    for (i, (target, label, val, active)) in defs.iter().enumerate() {
        if i > 0 {
            chips.push(Span::styled(
                "  ·  ",
                Style::default().fg(th.text_faint.into()),
            ));
            cx += 5; // "  ·  "
        }
        let text = format!("{label}: {val}");
        let wlen = text.chars().count() as u16;
        if cx < right {
            app.register_click(
                Rect::new(cx, filters.y, wlen.min(right - cx), 1),
                MouseTarget::RadioChip(*target),
            );
        }
        let val_style = if *active {
            Style::default()
                .fg(th.accent[0].into())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(th.text_faint.into())
        };
        chips.push(Span::styled(
            format!("{label}: "),
            Style::default().fg(th.text_dim.into()),
        ));
        chips.push(Span::styled(val.clone(), val_style));
        cx += wlen;
    }
    f.render_widget(Paragraph::new(Line::from(chips)), filters);

    // --- the station list (or favorites) ---
    radio_station_list(f, list, app);
}

/// A station-table column. `Name` flexes to fill; the rest are content-width and
/// drop (lowest priority first) when the panel is too narrow.
#[derive(Clone, Copy)]
enum SCol {
    Mark,
    Name,
    Country,
    Genre,
    Codec,
    Bitrate,
    Plays,
    Votes,
}

impl SCol {
    fn header(self) -> &'static str {
        match self {
            SCol::Mark => "",
            SCol::Name => "STATION",
            SCol::Country => "COUNTRY",
            SCol::Genre => "GENRE",
            SCol::Codec => "CODEC",
            SCol::Bitrate => "KBPS",
            SCol::Plays => "PLAYS",
            SCol::Votes => "VOTES",
        }
    }
    fn max_w(self) -> usize {
        match self {
            SCol::Country => 20,
            SCol::Genre => 16,
            SCol::Codec => 8,
            _ => usize::MAX,
        }
    }
    fn cell(self, st: &crate::radio::Station) -> String {
        match self {
            SCol::Mark => String::new(),
            SCol::Name => st.name.clone(),
            SCol::Country => {
                if !st.country.is_empty() {
                    st.country.clone()
                } else {
                    st.countrycode.clone()
                }
            }
            SCol::Genre => st.genre().to_string(),
            SCol::Codec => st.codec.to_uppercase(),
            SCol::Bitrate => {
                if st.bitrate > 0 {
                    st.bitrate.to_string()
                } else {
                    String::new()
                }
            }
            SCol::Plays => fmt_count(st.clickcount),
            SCol::Votes => fmt_count(st.votes),
        }
    }
}

/// Compact a large count: 1820 → "1.8k", 254000 → "254k", 1_200_000 → "1.2M".
fn fmt_count(n: u32) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 100_000 {
        format!("{}k", n / 1_000)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else if n > 0 {
        n.to_string()
    } else {
        String::new()
    }
}

/// A centered, friendly message when the station list is empty, so an
/// over-narrow filter combo (e.g. China + a Latin genre) explains itself
/// instead of just going blank.
fn radio_empty_state(f: &mut Frame, list: Rect, app: &AppState) {
    let th = &app.theme;
    let r = &app.radio;
    if list.height == 0 {
        return;
    }
    let (line1, line2): (String, String) = if r.loading {
        ("Searching…".into(), String::new())
    } else if r.section == RadioSection::Favorites {
        (
            "No favorites yet".into(),
            "Press f on a station to star it".into(),
        )
    } else if r.section == RadioSection::Recent {
        (
            "No recent stations".into(),
            "Stations you play show up here".into(),
        )
    } else if r.section == RadioSection::MostPlayed {
        (
            "No plays yet".into(),
            "Your most-played stations show up here".into(),
        )
    } else if r.section == RadioSection::Playlists {
        if r.pl.open.is_some() {
            (
                "No stations in this playlist".into(),
                "Press a on a station to add it".into(),
            )
        } else {
            ("No playlists yet".into(), "Press n to create one".into())
        }
    } else if r.country.is_some() || r.tag.is_some() {
        let mut active = Vec::new();
        if let Some((name, _)) = &r.country {
            active.push(name.clone());
        }
        if let Some(t) = &r.tag {
            active.push(t.clone());
        }
        (
            format!("No stations for {}", active.join(" + ")),
            "Press c / g to change filters".into(),
        )
    } else {
        ("No stations found".into(), "Try a different search".into())
    };
    let mut lines = vec![Line::from(Span::styled(
        line1,
        Style::default()
            .fg(th.text_dim.into())
            .add_modifier(Modifier::BOLD),
    ))];
    if !line2.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            line2,
            Style::default().fg(th.text_faint.into()),
        )));
    }
    let mid = list.y + list.height / 3;
    let area = Rect::new(
        list.x,
        mid.min(list.y + list.height - 1),
        list.width,
        list.height.saturating_sub(mid - list.y),
    );
    f.render_widget(Paragraph::new(lines).alignment(Alignment::Center), area);
}

/// The station list (search results or favorites) as a columned table.
fn radio_station_list(f: &mut Frame, list: Rect, app: &AppState) {
    let th = &app.theme;
    let r = &app.radio;
    app.register_click(list, MouseTarget::Scroll(ScrollBox::Radio)); // wheel anywhere
    let stations = app.radio_view_list();
    let body_h = list.height.saturating_sub(1) as usize; // header row
    let n = stations.len();
    if n == 0 {
        radio_empty_state(f, list, app);
        return;
    }
    if body_h == 0 {
        return;
    }
    let sel = r.sel.min(n - 1);
    // sticky (not recentring) so clicking a visible row doesn't make the list jump
    let off = components::sticky_off(&r.list_off, sel, n, body_h);

    // column specs (Name flexes; the Mark pin never drops; the rest drop the
    // lowest-priority first when the pane is narrow). `drop_rank` is *lower =
    // dropped first*, so Votes goes before Plays before Genre…
    use components::{TableCell, TableColumn, TableRow};
    let cols = vec![
        TableColumn::fixed(SCol::Mark.header(), components::PIN).seed(2),
        TableColumn::flexible(SCol::Name.header(), 16),
        TableColumn::fixed(SCol::Country.header(), 5),
        TableColumn::fixed(SCol::Genre.header(), 4),
        TableColumn::fixed(SCol::Bitrate.header(), 6).right(),
        TableColumn::fixed(SCol::Codec.header(), 3),
        TableColumn::fixed(SCol::Plays.header(), 2).right(),
        TableColumn::fixed(SCol::Votes.header(), 1).right(),
    ];
    let order = [
        SCol::Mark,
        SCol::Name,
        SCol::Country,
        SCol::Genre,
        SCol::Bitrate,
        SCol::Codec,
        SCol::Plays,
        SCol::Votes,
    ];

    // multi-select: the live visual range (over the passed cursor) + the marked
    // station keys, gathered once so the row loop stays allocation-free.
    let vis = app.marks.anchor.map(|a| (a.min(sel), a.max(sel)));
    let marked_keys: std::collections::HashSet<&str> = app
        .marks
        .ids
        .iter()
        .filter_map(|k| match k {
            crate::app::MarkKey::Station(s) => Some(s.as_str()),
            _ => None,
        })
        .collect();

    let mut rows: Vec<TableRow> = Vec::new();
    for (i, st) in stations.iter().enumerate().skip(off).take(body_h) {
        let on = i == sel;
        let marked = marked_keys.contains(crate::app::station_key(st))
            || vis.is_some_and(|(lo, hi)| i >= lo && i <= hi);
        let playing = app
            .rnow
            .now_station
            .as_ref()
            .is_some_and(|p| !p.uuid.is_empty() && p.uuid == st.uuid);
        let fav = app.radio_is_fav(st);
        let name_fg: Color = if playing {
            th.now_playing_color().into()
        } else {
            th.title_text().into()
        };
        let cells: Vec<TableCell> = order
            .iter()
            .map(|c| match c {
                SCol::Mark => {
                    // ✓ (selected) takes the play slot over ▶; the star keeps its slot.
                    let (lead, lead_fg) = if marked {
                        ("✓", th.marked_color())
                    } else if playing {
                        ("▶", th.accent[0])
                    } else {
                        (" ", th.accent[0])
                    };
                    let star = if fav { "★" } else { " " };
                    let cell = Cell::from(Line::from(vec![
                        Span::styled(lead, Style::default().fg(lead_fg.into())),
                        Span::styled(star, Style::default().fg(th.accent[1].into())),
                    ]));
                    TableCell::new(cell, 2)
                }
                // Name flexes to fill — the shared table clips it to the live width
                SCol::Name => {
                    let cell = Cell::from(st.name.clone())
                        .style(Style::default().fg(name_fg).add_modifier(Modifier::BOLD));
                    TableCell::new(cell, st.name.chars().count())
                }
                SCol::Plays | SCol::Bitrate | SCol::Votes => {
                    let s = clip(&c.cell(st), c.max_w());
                    let w = s.chars().count();
                    let cell = Cell::from(Line::from(s).alignment(Alignment::Right))
                        .style(Style::default().fg(th.meta_text().into()));
                    TableCell::new(cell, w)
                }
                _ => {
                    let s = clip(&c.cell(st), c.max_w());
                    let w = s.chars().count();
                    let cell = Cell::from(s).style(Style::default().fg(th.meta_text().into()));
                    TableCell::new(cell, w)
                }
            })
            .collect();
        let bg: Option<Color> = if on {
            Some(th.selection.into())
        } else if marked {
            Some(th.marked_color().mix(th.bg, 0.82).into())
        } else if playing {
            Some(th.panel.mix(th.selection, 0.4).into())
        } else {
            None
        };
        rows.push(TableRow {
            cells,
            bg,
            click: Some(MouseTarget::RadioRow(i)),
        });
    }

    components::columns_table(f, list, app, &cols, rows, 2);
}

/// The country / genre filter picker (a live-filtered list that owns the whole
/// Radio panel while open; the panel title shows which filter it is).
fn radio_picker(f: &mut Frame, area: Rect, app: &AppState) {
    let th = &app.theme;
    let Some(p) = &app.radio.picker else { return };
    let [head, _gap, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    let opts = app.radio_picker_options();
    let count_note = if p.loading {
        "loading…".to_string()
    } else {
        format!("{} matches", opts.len().saturating_sub(1))
    };
    // filter box: focused via '/'; otherwise a dim prompt and j/k navigate below
    let caret = p.query.chars().count();
    components::search_bar(
        f,
        head,
        th,
        &components::SearchBar {
            query: &p.query,
            caret,
            focused: p.editing,
            loading: false,
            tick: app.tick,
            placeholder: "/ to filter",
            scope: "",
            info: &count_note,
        },
    );

    app.register_click(body, MouseTarget::Scroll(ScrollBox::Radio)); // wheel anywhere
    let rows = body.height as usize;
    let n = opts.len();
    if n == 0 || rows == 0 {
        return;
    }
    let sel = p.sel.min(n - 1);
    // sticky (not recentring) so clicking a visible row doesn't make the list jump
    let off = components::sticky_off(&p.off, sel, n, rows);
    let mut lines: Vec<Line> = Vec::new();
    for (i, (label, choice)) in opts.iter().enumerate().skip(off).take(rows) {
        let on = i == sel;
        let row_y = body.y + (i - off) as u16;
        app.register_click(
            Rect::new(body.x, row_y, body.width, 1),
            MouseTarget::RadioPick(i),
        );
        let marker = if on { "▸ " } else { "  " };
        // the leading "clear filter" row is faint; selected row is bold accent
        let fg = if on {
            th.accent[0]
        } else if choice.is_none() {
            th.text_faint
        } else {
            th.text_dim
        };
        let mut style = Style::default().fg(fg.into());
        if on {
            style = style.add_modifier(Modifier::BOLD);
        }
        let content = Line::from(vec![
            Span::styled(marker, Style::default().fg(th.accent[0].into())),
            Span::styled(label.clone(), style),
        ]);
        let bg = if on { Some(th.selection) } else { None };
        lines.push(components::pill_line(app, body.width as usize, content, bg));
    }
    f.render_widget(Paragraph::new(lines), body);
}
