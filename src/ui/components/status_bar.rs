//! The bottom status bar: a four-zone chrome line drawn under every view.
//! Left = context-sensitive navigation / prompt for the active window or modal;
//! centre = the queue-derived "▶ Next:" hint; right = transient toggle state (sleep
//! timer, A-B loop, ReplayGain); far right = the current view name.
//! Pure rendering — reads `&AppState`, draws into the `Frame`. The left prompt
//! and right toggles are built by the `*_prompt` / `*_spans` helpers below so
//! `status_bar` itself stays a thin layout/dispatch function.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use super::{clip, col};
use crate::app::{AppState, Layout as AppLayout};

pub fn status_bar(f: &mut Frame, area: Rect, app: &AppState) {
    let th = &app.theme;
    f.render_widget(
        Block::default().style(Style::default().bg(col(th.bg))),
        area,
    );
    // four zones: nav/help (left) · next-track hint (centre) · toggles · the
    // current view name (far right). The centre + view name only appear when the
    // terminal is wide enough to spare the room.
    // the "Next:" hint follows the active source (local queue or the Spotify
    // up-next); radio streams have no queue, so it's hidden then — the source
    // selection lives in `status_next_title`, keeping this a pure display layer.
    // gated by the "Next:" hint toggle (config.next_hint); the title is compacted
    // — parentheticals + clutter suffixes dropped — so the centre stays short.
    let next = app
        .config
        .next_hint
        .then(|| app.status_next_title())
        .flatten()
        .map(|t| compact_title(&t).to_string());
    let next_w = match &next {
        // budget by DISPLAY width, not char count — a CJK title is 2 cols/char, so
        // char count under-sizes the slot and the title gets clipped ("不染" → "不").
        Some(t) if area.width >= 90 => {
            (unicode_width::UnicodeWidthStr::width(t.as_str()).min(22) as u16) + 12
        }
        _ => 0,
    };
    // fixed-width view slot so switching views never reflows the bar
    let view = app.layout.title();
    let view_w = if area.width >= 70 {
        crate::app::VIEW_NAME_W + 2
    } else {
        0
    };
    // When a modal/popup owns the screen, its shortcuts take the whole bar (the
    // global toggles / view name / next-track are hidden — the popup has no footer).
    let modal_open = app.modal_open();
    let zero = Rect::new(area.x, area.y, 0, 0);
    let [left, center, right, view_zone] = if modal_open {
        [area, zero, zero, zero]
    } else {
        Layout::horizontal([
            Constraint::Min(16),
            Constraint::Length(next_w),
            Constraint::Length(34),
            Constraint::Length(view_w),
        ])
        .areas(area)
    };

    // FAR RIGHT: the current view name (replaces the old top bar).
    if view_w > 0 {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("{view} "),
                Style::default()
                    .fg(col(th.toggle_on()))
                    .add_modifier(Modifier::BOLD),
            )))
            .alignment(Alignment::Right),
            view_zone,
        );
    }

    // CENTRE: "▶ Next: <title>" — informational, queue-derived (leading gap keeps
    // it clear of the nav text on its left)
    if let Some(t) = &next
        && next_w > 0
    {
        // width-aware clip (CJK-safe) so the title fills the slot without overrunning
        let title = clip(t, next_w.saturating_sub(11) as usize);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled("   ▶ Next: ", Style::default().fg(col(th.meta_text()))),
                Span::styled(title, Style::default().fg(col(th.title_text()))),
            ])),
            center,
        );
    }

    // LEFT: navigation / help for the active window or popup.
    f.render_widget(Paragraph::new(left_prompt(app)), left);

    // RIGHT: status of toggleable elements — toggle-on / toggle-off roles.
    f.render_widget(
        Paragraph::new(Line::from(toggle_spans(app))).alignment(Alignment::Right),
        right,
    );
}

/// Shorten an up-next title for the compact status-bar hint: drop a parenthetical
/// or bracketed suffix and anything after the first clutter delimiter (` - `, `;`,
/// `,`, `..`), so `"Song (feat. X) - Live 2019"` → `"Song"`. Falls back to the
/// full (trimmed) title if stripping would leave nothing (e.g. a title that opens
/// with a parenthesis, like "(I Can't Get No) Satisfaction").
fn compact_title(title: &str) -> &str {
    let mut cut = title.len();
    for pat in [" (", " [", " - ", ";", ",", ".."] {
        if let Some(i) = title.find(pat) {
            cut = cut.min(i);
        }
    }
    let short = title[..cut].trim();
    if short.is_empty() {
        title.trim()
    } else {
        short
    }
}

/// The left zone: the context-sensitive prompt for whatever owns input right now
/// — a confirm prompt, a directory-load bar, a search/naming field, a
/// notification, the Tag Edit modal ([`tag_prompt`]), a visual selection, else
/// the per-view navigation hint ([`nav_hint`]).
fn left_prompt(app: &AppState) -> Line<'static> {
    let th = &app.theme;
    let faint = Style::default().fg(col(th.text_faint));
    if app.settings.confirm_logout {
        // Spotify log-out / cached-login reset awaiting confirmation
        let connected = matches!(
            app.spotify.conn,
            crate::spotify::ConnState::Connected { .. }
        );
        let q = if connected {
            "Log out of Spotify?"
        } else {
            "Reset cached Spotify login?"
        };
        Line::from(vec![
            Span::styled(
                format!("  {q}  "),
                Style::default()
                    .fg(col(th.accent[0]))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("⏎/y confirm · esc cancel", faint),
        ])
    } else if app.radio.directory_loading {
        // a directory download (or weekly refresh) is in progress — show a bar
        let est = crate::radio::DIRECTORY_EST_BYTES.max(1);
        let read = app.radio.directory_progress;
        let frac = if read == 0 {
            0.0
        } else {
            (read as f64 / est as f64).min(0.99)
        };
        let cells = 12usize;
        let filled = ((frac * cells as f64).round() as usize).min(cells);
        let bar = format!("{}{}", "█".repeat(filled), "░".repeat(cells - filled));
        let txt = if read == 0 {
            " ⟳ Loading station directory…".to_string()
        } else {
            format!(
                " ⟳ Updating directory [{bar}] {:.0}%  {:.1} MB",
                frac * 100.0,
                read as f64 / (1024.0 * 1024.0)
            )
        };
        Line::from(Span::styled(
            txt,
            Style::default()
                .fg(col(th.accent[0]))
                .add_modifier(Modifier::BOLD),
        ))
    } else if app.search.active && app.layout != AppLayout::Dashboard {
        // The Dashboard draws the query in its inline `search_bar` row, so only the
        // other layouts need the status-bar fallback indicator.
        Line::from(vec![
            Span::styled(
                " Search: ",
                Style::default()
                    .fg(col(th.accent[0]))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(app.search.query.clone(), Style::default().fg(col(th.text))),
            Span::styled("▌", Style::default().fg(col(th.accent[0]))),
        ])
    } else if app.input.confirm_delete.is_some() {
        // the named confirm prompt lives in the centered dialog; mirror the mode
        Line::from(vec![
            Span::styled(
                "  Delete playlist?  ",
                Style::default()
                    .fg(col(th.accent[0]))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("⏎/y delete · esc cancel", faint),
        ])
    } else if app.input.naming.is_some() {
        // the editable field lives in the centered dialog (`name_overlay`); the
        // bar just reflects the mode so it doesn't read as the active view
        Line::from(vec![
            Span::styled(
                "  ✎ Type in the dialog  ",
                Style::default()
                    .fg(col(th.accent[0]))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("⏎ save · esc cancel", faint),
        ])
    } else if let Some(n) = &app.notification {
        Line::from(Span::styled(
            format!(" {}", n.text),
            Style::default()
                .fg(col(th.good))
                .add_modifier(Modifier::BOLD),
        ))
    } else if let Some(line) = tag_prompt(app) {
        line
    } else if app.marks.anchor.is_some() {
        let n = app
            .marks
            .anchor
            .map(|a| a.max(app.selection) - a.min(app.selection) + 1)
            .unwrap_or(0);
        Line::from(Span::styled(
            format!(" visual · {n} lines · j/k extend · x mark · esc"),
            faint,
        ))
    } else {
        nav_hint(app)
    }
}

/// The Tag Edit modal's per-tab prompt (Edit / Auto Tag / Cover), or `None` when
/// the modal isn't on a tab with a live prompt. Folded out of [`left_prompt`].
fn tag_prompt(app: &AppState) -> Option<Line<'static>> {
    let th = &app.theme;
    let faint = Style::default().fg(col(th.text_faint));
    if app.tags.tab == 0
        && let Some(te) = &app.tags.edit
    {
        // EDIT tab: the filename/find-replace prompts surface in the status bar
        Some(if te.confirm_album {
            // how many album tracks the write will touch
            let n = te
                .targets
                .first()
                .and_then(|id| app.library.track(*id))
                .and_then(|t| t.album_id)
                .map(|a| app.library.tracks_of(a).len())
                .unwrap_or(te.targets.len());
            Line::from(vec![
                Span::styled(
                    format!("  Apply changes to all {n} album track(s)?  "),
                    Style::default()
                        .fg(col(th.accent[0]))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled("⏎/y confirm · esc/n cancel", faint),
            ])
        } else if let Some((to_filename, buf)) = &te.convert {
            let (label, hint) = if *to_filename {
                (
                    "Tags→Filename: ",
                    "   #:track T:title A:album AA:albumartist AR:artist Y:year · Enter rename · Esc",
                )
            } else {
                ("Filename→Tags: ", "   Enter apply · Esc")
            };
            Line::from(vec![
                Span::styled(
                    format!("  {label}{buf}▌"),
                    Style::default().fg(col(th.text)),
                ),
                Span::styled(hint, faint),
            ])
        } else if let Some((find, repl, on_repl)) = &te.replace {
            let field = crate::tags::FIELDS.get(te.cursor).copied().unwrap_or("");
            let txt = Style::default().fg(col(th.text));
            let (find_s, repl_s) = if *on_repl { ("", "▌") } else { ("▌", "") };
            Line::from(vec![
                Span::styled(format!("  Replace in {field}: "), faint),
                Span::styled(format!("{find}{find_s}"), txt),
                Span::styled(" → ", faint),
                Span::styled(format!("{repl}{repl_s}"), txt),
                Span::styled("   Tab switch · Enter apply · Esc", faint),
            ])
        } else if te.editing {
            Line::from(vec![Span::styled(
                "  type · ←/→ · ⌫/Del · Home/End · Tab field · Enter/Esc done · ^S save · ⇥ tabs",
                faint,
            )])
        } else {
            Line::from(vec![Span::styled(
                "  j/k · ⏎ edit · ⌫ clear · ^d remove · ^t/^u/^l case · ^n # · ^f from-file · s save · a album · ⇥ tabs · esc",
                faint,
            )])
        })
    } else if app.tags.tab == 1
        && let Some(ts) = &app.tags.search
    {
        // AUTO TAG tab shortcuts (confirm prompt shows what will be written)
        let accent = Style::default()
            .fg(col(th.accent[0]))
            .add_modifier(Modifier::BOLD);
        Some(if let Some(p) = ts.pending {
            use crate::app::PendingApply::*;
            let what = match p {
                Song => "Write tags to this track".to_string(),
                AlbumBasic => format!("Write album fields to {} tracks", ts.album.len()),
                AlbumFull => {
                    let local: Vec<(u16, String)> = ts
                        .album_tracks
                        .iter()
                        .map(|(_, t)| (t.track_no.parse::<u16>().unwrap_or(0), t.title.clone()))
                        .collect();
                    let n = ts
                        .albums
                        .get(ts.album_sel)
                        .map(|s| {
                            crate::tagsearch::match_album(&local, &s.tracks)
                                .iter()
                                .filter(|a| a.is_some())
                                .count()
                        })
                        .unwrap_or(0);
                    format!("Write album tags to {n} matched track(s)")
                }
            };
            Line::from(Span::styled(
                format!("  {what}?  ⏎/y confirm · esc cancel"),
                accent,
            ))
        } else {
            let h = if ts.editing {
                "  type · ←/→ · ⌫/Del · Home/End · enter search · esc"
            } else if ts.album_mode {
                "  j/k track · ←→ source · ⏎ apply album · s single · / edit · ⇥ tabs · esc"
            } else {
                "  j/k select · ⏎ apply song · a apply album · s album · / edit · ⇥ tabs · esc"
            };
            Line::from(Span::styled(h, faint))
        })
    } else if app.tags.tab == 2
        && let Some(cs) = &app.tags.cover
    {
        // COVER tab shortcuts (scope = whole album vs just this song)
        Some(if cs.confirm {
            let scope = if cs.album_wide {
                format!("the album ({} tracks)", cs.paths.len())
            } else {
                "this song".to_string()
            };
            Line::from(Span::styled(
                format!("  Embed this cover into {scope}?  ⏎/y confirm · esc cancel"),
                Style::default()
                    .fg(col(th.accent[0]))
                    .add_modifier(Modifier::BOLD),
            ))
        } else if cs.editing {
            Line::from(Span::styled(
                "  type · ←/→ · ⌫/Del · Home/End · enter search · esc",
                faint,
            ))
        } else {
            let scope = if cs.album_wide {
                format!("album:{}", cs.paths.len())
            } else {
                "this song".to_string()
            };
            Line::from(Span::styled(
                format!("  j/k · [ ] select · ⏎ embed · s scope[{scope}] · / edit · ⇥ tabs · esc"),
                faint,
            ))
        })
    } else {
        None
    }
}

/// What the Main-list selection resolves to — decides the primary action the
/// Main hint advertises. Every browser view drills into containers and plays
/// tracks; only the per-source key labels differ.
#[derive(Clone, Copy)]
enum MainSel {
    /// the selection plays (⏎)
    Track,
    /// the selection drills in (⏎)
    Container,
}

/// A browser view's per-region status-bar hints. The dispatch *skeleton* — which
/// focused region maps to which line, the search short-circuit, and the Main
/// line adapting to its selection (and gaining "esc back" when drilled) — lives
/// once in [`browse_hint`]. Dashboard and Spotify are two *instances* of that
/// one model: each fills these source-specific strings, so there is no second
/// copy of the dispatch logic to keep in sync. A future source is a third
/// constructor, nothing more.
struct BrowseHints {
    /// the search box is focused (also the `Focus::Search` ring slot)
    search: &'static str,
    /// the section sidebar is focused
    sidebar: &'static str,
    /// the Queue pane is focused
    queue: &'static str,
    /// the Artist pane is focused
    artist: &'static str,
    /// the Lyrics pane is focused
    lyrics: &'static str,
    /// any other movable pane is focused
    pane: &'static str,
    /// Main focused, selection drills in (album / artist / playlist / …)
    open: &'static str,
    /// Main focused, selection plays (a track)
    play: &'static str,
    /// when set, replaces the Main line wholesale — for a section with bespoke
    /// actions (e.g. the local Playlists section: new / rename / delete)
    main_override: Option<&'static str>,
}

/// The single, source-agnostic assembler: map the focused region to its hint,
/// adapting the Main line to the selection and the drill depth. The only shared
/// Main tail (`/ search · ? keys`, prefixed with `esc back` when drilled) is
/// applied here so both sources stay identical in shape.
fn browse_hint(
    focus: crate::app::Focus,
    searching: bool,
    drilled: bool,
    sel: MainSel,
    h: &BrowseHints,
) -> String {
    use crate::app::{Focus, Panel};
    if searching {
        return h.search.into();
    }
    match focus {
        Focus::Sidebar => h.sidebar.into(),
        Focus::Pane(Panel::Queue) => h.queue.into(),
        Focus::Pane(Panel::Artist) => h.artist.into(),
        Focus::Pane(Panel::Lyrics) => h.lyrics.into(),
        Focus::Pane(_) => h.pane.into(),
        Focus::Search => h.search.into(),
        Focus::Main => {
            if let Some(o) = h.main_override {
                return o.into();
            }
            let head = match sel {
                MainSel::Track => h.play,
                MainSel::Container => h.open,
            };
            let back = if drilled { "esc back · " } else { "" };
            format!("{head} · {back}/ search · tab panes · ? keys")
        }
    }
}

/// Hint shown when the Lyrics pane holds focus (`Focus::Pane(Panel::Lyrics)`), in
/// any view. `F` cycles format; `,`/`.` nudge the synced-lyric offset earlier /
/// later (see `keymap::lyrics_offset`). One definition so every surface matches.
const LYRICS_PANE_HINT: &str = "F format · , . sync · tab panes · ? keys";

/// The per-region hints shared by EVERY browser view. One keymap → the keys read
/// identically in the local library and Spotify (they differ only in plumbing and
/// Spotify's logged-out prompt). Each region shows just the few keys a first-time
/// user needs — how to move within it, its primary `⏎` action — and ends with the
/// same two universal anchors, `tab panes` (move between regions) and `? keys`
/// (everything else). Power-user keys (resize `‹›`, move `m`, favourite `f`, queue
/// reorder, …) live in `?`, so every region is the same shape and length-class.
fn browse_hints() -> BrowseHints {
    BrowseHints {
        search: "type to search · ↑↓ pick · ⏎ · esc",
        sidebar: "j/k section · ⏎ load · tab panes · ? keys",
        queue: "j/k move · ⏎ jump · tab panes · ? keys",
        artist: "j/k scroll · tab panes · ? keys",
        lyrics: LYRICS_PANE_HINT,
        pane: "tab panes · ? keys",
        open: "j/k move · ⏎ open",
        play: "j/k move · ⏎ play",
        main_override: None,
    }
}

/// The Dashboard (local library) instance of [`browse_hint`].
fn dashboard_hint(app: &AppState) -> String {
    use crate::app::LocalSection;
    let sel = if app
        .local
        .items
        .get(app.local.sel)
        .is_some_and(|i| i.is_track())
    {
        MainSel::Track
    } else {
        MainSel::Container
    };
    let mut hints = browse_hints();
    // Section-specific Main lines: the cover grid navigates 2-D (and toggles back
    // to a list); the Albums/Artists list offers the `#` grid toggle; the Playlists
    // section offers create/rename/delete (or play/remove inside a playlist).
    hints.main_override = if app.local_grid_active() {
        Some("hjkl move · ⏎ open · # list · tab panes · ? keys")
    } else if app.local.section == LocalSection::Playlists {
        if app.current_local_playlist().is_some() {
            Some("⏎ play · d remove · esc back · tab panes · ? keys")
        } else {
            Some("n new · e rename · ⇧D delete · a add · ⏎ open · tab panes · ? keys")
        }
    } else if app.local.crumb.is_none()
        && matches!(
            app.local.section,
            LocalSection::Albums | LocalSection::Artists
        )
    {
        Some("j/k move · ⏎ open · # grid · tab panes · ? keys")
    } else {
        None
    };
    browse_hint(
        app.focus,
        app.is_searching(),
        app.local.crumb.is_some(),
        sel,
        &hints,
    )
}

/// The Spotify instance of [`browse_hint`] — same shared hints, only a different
/// logged-out prompt and a different state source for search/drill/selection.
fn spotify_hint(app: &AppState) -> String {
    use crate::spotify::ConnState;
    use crate::spotify::api::{Kind, Section};
    if !matches!(app.spotify.conn, ConnState::Connected { .. }) {
        return "⏎ log in · ; settings · ? keys".into();
    }
    let sel = if app
        .spotify
        .items
        .get(app.spotify.sel)
        .is_some_and(|it| it.kind == Kind::Track)
    {
        MainSel::Track
    } else {
        MainSel::Container
    };
    let mut hints = browse_hints();
    // mirror the local hint: the cover grid navigates 2-D (and toggles back to a
    // list), while the Albums/Artists list offers the `#` grid toggle.
    let in_playlist = app
        .spotify
        .open_item
        .as_ref()
        .is_some_and(|it| it.kind == Kind::Playlist);
    hints.main_override = if app.spotify_grid_active() {
        Some("hjkl move · ⏎ open · # list · tab panes · ? keys")
    } else if app.spotify_podcast_grid_active() {
        // browsing podcast shows/categories as a grid: F follows the selected show
        Some("hjkl move · ⏎ open · F follow · # list · ? keys")
    } else if in_playlist {
        // drilled into a playlist: play a track, add it elsewhere, or remove it
        Some("⏎ play · a add · d remove · esc back · ? keys")
    } else if !app.spotify.in_search
        && app.spotify.crumb.is_none()
        && app.spotify.section == Section::Playlists
    {
        // the Playlists list: create / rename / delete, or add the now-playing track
        Some("n new · e rename · ⇧D delete · a add · ⏎ open · ? keys")
    } else if !app.spotify.in_search
        && app.spotify.crumb.is_none()
        && app.spotify.section == Section::Podcasts
    {
        // Your Shows: follow/unfollow, drill in, or toggle the grid
        Some("⏎ open · F follow · # grid · tab panes · ? keys")
    } else if !app.spotify.in_search
        && app.spotify.crumb.is_none()
        && matches!(app.spotify.section, Section::Albums | Section::Artists)
    {
        Some("j/k move · ⏎ open · # grid · tab panes · ? keys")
    } else {
        None
    };
    browse_hint(
        app.focus,
        app.spotify.searching,
        app.spotify.crumb.is_some(),
        sel,
        &hints,
    )
}

/// The default left zone: the minimal per-view navigation hint (or the active
/// popup's nav), prefixed with a "N marked" badge when tracks are marked.
fn nav_hint(app: &AppState) -> Line<'static> {
    let th = &app.theme;
    let faint = Style::default().fg(col(th.text_faint));
    // navigation / help for the active popup, else the current view, with the
    // shortcuts that view's edit/settings live behind.
    let nav: String = if app.settings.rebinding.is_some() {
        "press a key to bind it · esc to cancel".into()
    } else if app.settings.popup.is_some() {
        "j/k move · enter toggle · h/l adjust · del remove · esc close".into()
    } else if app.palette.is_some() {
        "↑↓ / ^j^k select · enter run · esc close".into()
    } else if let Some(info) = &app.info {
        use crate::app::InfoTab;
        match info.tab {
            InfoTab::Keys => "tab switch · type to filter · esc close".into(),
            InfoTab::Health => "tab switch · j/k scroll · y copy latest · esc close".into(),
            _ => "tab switch · j/k scroll · esc close".into(),
        }
    } else if !app.input.add_targets.is_empty() {
        "↑↓ select · enter add · esc close".into()
    } else if app.settings.overlay {
        if app.settings_active_group() == Some("Keys") {
            "tab switch · j/k move · enter rebind · ctrl-r defaults · esc close".into()
        } else {
            "tab switch · j/k move · enter toggle · h/l adjust · esc close".into()
        }
    } else if app.layout == AppLayout::LibraryFocus
        && app.focus == crate::app::Focus::Pane(crate::app::Panel::Lyrics)
    {
        // The Library view's per-view hint (the `_` arm below) isn't focus-aware,
        // so surface the lyrics keys when its docked Lyrics pane holds focus —
        // Dashboard/Spotify already get this through `browse_hints`.
        LYRICS_PANE_HINT.into()
    } else {
        // Minimal per-view bar: only the important navigation, then the two
        // gateways — `; settings` and `? keys`. Everything else (open/play,
        // like, seek, channel, sort, …) lives in the `?` shortcut overlay.
        // Transient input sub-modes (search boxes, pickers) keep their own
        // functional prompt since `?` isn't reachable while typing.
        match app.layout {
            AppLayout::FullPlayer => "space ⏯ · , . seek · tab panes · ; settings · ? keys".into(),
            AppLayout::LyricsFocus => "space ⏯ · F format · , . sync · V viz · ? keys".into(),
            AppLayout::Concert => "space ⏯ · , . seek · ; settings · ? keys".into(),
            AppLayout::Radio => {
                if let Some(p) = &app.radio.picker {
                    if p.editing {
                        "type to filter · ↑↓ pick · esc".into()
                    } else {
                        "↑↓ navigate · / filter · esc".into()
                    }
                } else if app.radio.editing {
                    "type to filter · ↑↓ pick · esc".into()
                } else if app.rnow.dvr.is_some() {
                    // timeshifted live stream — seekable within the DVR window
                    "space ⏯ · , . seek · 0 start · $ live · / search · ? keys".into()
                } else {
                    // forward-only live stream — no seek
                    "space ⏯ · / search · j/k move · ; settings · ? keys".into()
                }
            }
            AppLayout::Spotify => spotify_hint(app),
            AppLayout::Dashboard => dashboard_hint(app),
            _ => {
                "space ⏯ · , . seek · / search · j/k move · tab panes · ; settings · ? keys".into()
            }
        }
    };
    let mut spans: Vec<Span> = Vec::new();
    if !app.marks.ids.is_empty() {
        spans.push(Span::styled(
            format!(" {} marked · : edit tags · esc clear ", app.marks.ids.len()),
            Style::default()
                .fg(col(th.accent[2]))
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled("· ", faint));
    }
    spans.push(Span::styled(format!(" {nav}"), faint));
    Line::from(spans)
}

/// The right zone: transient toggle-state badges — sleep timer, A-B loop, ReplayGain
/// (shuffle / repeat are intentionally excluded; see below).
fn toggle_spans(app: &AppState) -> Vec<Span<'static>> {
    let th = &app.theme;
    let on = Style::default()
        .fg(col(th.toggle_on()))
        .add_modifier(Modifier::BOLD);
    let mut rs: Vec<Span> = Vec::new();
    if let Some(secs) = app.sleep_remaining_secs() {
        rs.push(Span::styled(
            format!("⏲ {}:{:02}", secs / 60, secs % 60),
            on,
        ));
        rs.push(Span::raw("   "));
    }
    if app.fx.ab_loop.is_some() || app.fx.ab_a.is_some() {
        let t = if app.fx.ab_loop.is_some() {
            "⟲ A-B"
        } else {
            "⟲ A·"
        };
        rs.push(Span::styled(t, on));
        rs.push(Span::raw("   "));
    }
    if let Some(rg) = &app.fx.rg_status {
        rs.push(Span::styled(format!("RG {rg}"), on));
        rs.push(Span::raw("   "));
    }
    // shuffle / repeat are deliberately NOT shown here — they're toggled with s/r and
    // surfaced by a toast, not kept permanently on the status bar (matching the
    // Spotify/Radio views, which never showed them).
    rs
}

#[cfg(test)]
mod tests {
    use super::compact_title;

    #[test]
    fn compact_title_strips_clutter() {
        assert_eq!(compact_title("Song (feat. X) - Live 2019"), "Song");
        assert_eq!(
            compact_title("Bohemian Rhapsody - Remastered 2011"),
            "Bohemian Rhapsody"
        );
        assert_eq!(
            compact_title("Empire State of Mind, Pt. 2"),
            "Empire State of Mind"
        );
        assert_eq!(compact_title("Intro [Bonus Track]"), "Intro");
        assert_eq!(compact_title("Plain Title"), "Plain Title");
    }

    #[test]
    fn compact_title_keeps_titles_that_open_with_a_paren() {
        // stripping would leave nothing → keep the full title
        assert_eq!(
            compact_title("(I Can't Get No) Satisfaction"),
            "(I Can't Get No) Satisfaction"
        );
    }
}
