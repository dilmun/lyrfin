//! now_bar rendering (extracted from ui/components).

use super::*;
use crate::app::{AppState, Layout as AppLayout, MouseTarget, TransportButton};
use crate::core::player::{Repeat, Status};
use crate::ui::theme::Theme;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use std::time::Duration;

/// Playback state a [`transport_row`] reflects — sourced from the local player
/// ([`TransportState::local`]) or the Spotify overlay ([`TransportState::spotify`])
/// so one row renderer serves both bars.
pub(crate) struct TransportState {
    playing: bool,
    shuffle: bool,
    repeat: Repeat,
}

impl TransportState {
    pub(crate) fn local(app: &AppState) -> Self {
        Self {
            playing: app.player.status == Status::Playing,
            shuffle: app.player.shuffle,
            repeat: app.player.repeat,
        }
    }
    pub(crate) fn spotify(app: &AppState) -> Self {
        Self {
            playing: !app.spov.spotify_paused,
            shuffle: app.spov.sp_shuffle,
            repeat: app.spov.sp_repeat,
        }
    }
}

/// A centred transport row + its clickable button rects, laid out at (`x`,`y`)
/// across `width`. Order: shuffle · −10s · ( play/pause ) · +10s · repeat. Only
/// the play button is framed; active shuffle/repeat are bright accent (flat).
/// `st` carries the play/shuffle/repeat state (local or Spotify).
pub(crate) fn transport_row(
    app: &AppState,
    th: &Theme,
    x: u16,
    width: u16,
    y: u16,
    st: TransportState,
) -> (Line<'static>, Vec<(Rect, TransportButton)>) {
    let bold = Modifier::BOLD;
    let ic = &app.icons;
    let pp = if st.playing { &ic.pause } else { &ic.play };
    let rep = if st.repeat == Repeat::One {
        &ic.repeat_one
    } else {
        &ic.repeat
    };
    // active toggles use the toggle-on role; inactive the toggle-off role.
    let off = Style::default().fg(col(th.toggle_off()));
    let active = |on: bool| {
        if on {
            Style::default().fg(col(th.toggle_on())).add_modifier(bold)
        } else {
            off
        }
    };
    let strong = Style::default().fg(col(th.text)).add_modifier(bold);
    let buttons: [(String, Style, TransportButton); 5] = [
        (
            ic.shuffle.clone(),
            active(st.shuffle),
            TransportButton::Shuffle,
        ),
        (ic.prev.clone(), strong, TransportButton::Prev),
        (
            pp.clone(),
            Style::default().fg(col(th.accent[0])).add_modifier(bold),
            TransportButton::PlayPause,
        ),
        (ic.next.clone(), strong, TransportButton::Next),
        (
            rep.clone(),
            active(st.repeat != Repeat::Off),
            TransportButton::Repeat,
        ),
    ];
    let gap = 4usize;
    let widths: Vec<usize> = buttons.iter().map(|(s, _, _)| s.chars().count()).collect();
    let total = widths.iter().sum::<usize>() + gap * (buttons.len() - 1);
    let left = (width as usize).saturating_sub(total) / 2;
    let mut spans: Vec<Span> = vec![Span::raw(" ".repeat(left))];
    let mut bx = x + left as u16;
    let mut clicks = Vec::new();
    for (i, (label, style, btn)) in buttons.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" ".repeat(gap)));
            bx += gap as u16;
        }
        let w = widths[i] as u16;
        clicks.push((Rect::new(bx, y, w, 1), *btn));
        spans.push(Span::styled(label.clone(), *style));
        bx += w;
    }
    (Line::from(spans), clicks)
}

/// A source-agnostic snapshot of "what's playing", populated by the local player
/// ([`NowPlaying::local`]) or the Spotify overlay ([`NowPlaying::spotify`]) and
/// drawn by the one shared [`playback_bar`] renderer — the same local/spotify
/// adapter split [`TransportState`] uses, extended to the whole bar.
pub(crate) struct NowPlaying<'a> {
    pub cover: &'a CoverState,
    pub title: String,
    pub artist: String,
    /// Album name, shown as a fainter trailing tier after the artist. "" hides it.
    pub album: String,
    pub favorite: bool,
    pub elapsed: Duration,
    pub duration: Duration,
    pub frac: f32,
    pub transport: TransportState,
    /// the time label under the bar's left end (shown only when there's a
    /// dedicated row): elapsed/total + a speed badge, or "buffering…".
    pub under_bar: Line<'static>,
}

/// The playback-rate badge (`1.25×`), or `None` at normal speed so an untouched
/// rate never adds clutter. Trailing zeros trimmed: 0.5× not 0.50×.
pub(crate) fn speed_badge(app: &AppState) -> Option<String> {
    let speed = app.player.speed;
    if (speed - 1.0).abs() <= 0.01 {
        return None;
    }
    let s = format!("{speed:.2}");
    Some(format!(
        "{}×",
        s.trim_end_matches('0').trim_end_matches('.')
    ))
}

/// The base "elapsed / total" spans under the progress bar. The local bar appends
/// a speed badge; the Spotify bar swaps in "buffering…" while the stream fills.
pub(crate) fn time_label(th: &Theme, elapsed: Duration, duration: Duration) -> Vec<Span<'static>> {
    vec![
        Span::styled(mmss(elapsed), Style::default().fg(col(th.text_dim))),
        Span::styled(
            format!("/{}", mmss(duration)),
            Style::default().fg(col(th.text_faint)),
        ),
    ]
}

/// Draw the shared playback bar: responsive album art + a centre column stacking
/// title/artist, a blended visualizer, the progress bar (with a seek hit-box), the
/// transport row, and a thin volume meter. Source-specific data arrives via
/// [`NowPlaying`]; the layout/geometry is identical for every source.
pub(crate) fn playback_bar(f: &mut Frame, area: Rect, app: &AppState, np: NowPlaying) {
    let th = &app.theme;
    // mini layout: too few rows for the bordered bar — one shared compact form
    if area.height <= MINI_NOW_VIZ_H + 1 {
        compact_bar(
            f,
            area,
            app,
            CompactNow {
                playing: np.transport.playing,
                title: &np.title,
                subtitle: &np.artist,
                progress: Some((np.frac, np.elapsed, np.duration)),
            },
        );
        return;
    }
    let block = rounded(th, "", false);
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Responsive: drop the album-art thumbnail when narrow so the centre column
    // (details / viz / progress / transport + time/volume) keeps room; below this
    // the art crowds the controls.
    let w = inner.width;
    let show_art = app.config.album_art && w >= 56;
    let mut cons: Vec<Constraint> = Vec::new();
    let art_i = if show_art {
        cons.push(Constraint::Length(12)); // album art
        cons.push(Constraint::Length(2)); // breathing room
        Some(0usize)
    } else {
        cons.push(Constraint::Length(1)); // tiny left pad
        None
    };
    let center_i = cons.len();
    cons.push(Constraint::Min(16)); // details + viz + progress + transport
    let rects = Layout::horizontal(cons).split(inner);
    if let Some(i) = art_i {
        render_cover(f, rects[i], np.cover, app);
    }
    let center = rects[center_i];

    // gradient title; a favorite/liked track gets a trailing ♥ (placed after the
    // title so its start never shifts when the flag toggles)
    let tlen = np.title.chars().count().max(2);
    let mut title_spans: Vec<Span> = np
        .title
        .chars()
        .enumerate()
        .map(|(i, ch)| {
            let g = th.accent_at(i as f32 / tlen as f32);
            Span::styled(
                ch.to_string(),
                Style::default().fg(col(g)).add_modifier(Modifier::BOLD),
            )
        })
        .collect();
    if np.favorite {
        title_spans.push(Span::styled(
            " ♥",
            Style::default()
                .fg(col(th.warning))
                .add_modifier(Modifier::BOLD),
        ));
    }

    // centre column, stacked: title+artist · (optional) blended visualizer ·
    // progress bar · transport. The elapsed/total time sits under the bar when
    // there's a dedicated row, else it falls back to inline times flanking the bar.
    // The under-bar time needs a *dedicated* row AND enough width: it is drawn in
    // the margin beside the centred transport, so on a narrow bar it silently does
    // not fit and the times disappear entirely. Below that width, flank the bar
    // with them instead — visible times matter more than where they sit.
    let with_time = center.height >= 5 && center.width >= 56;
    let details_h = 2; // title + artist
    let show_viz = app.config.player_viz && center.height >= details_h + 3;
    let (details, viz_opt, prog_row, trans_row) = if show_viz {
        let [d, v, p, t] = Layout::vertical([
            Constraint::Length(details_h), // title · artist
            Constraint::Min(1),            // visualizer (blends into the bar background)
            Constraint::Length(1),         // progress
            Constraint::Length(1),         // transport + volume
        ])
        .areas(center);
        (d, Some(v), p, t)
    } else {
        let [d, p, t, _pad] = Layout::vertical([
            Constraint::Length(details_h),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0), // absorb any spare height below the transport
        ])
        .areas(center);
        (d, None, p, t)
    };

    // details: the gradient title carries the emphasis (the now-playing showcase),
    // and the line below is the shared metadata tier — artist · album read exactly
    // like the metadata in every list, column, and grid card.
    let meta_style = Style::default().fg(col(th.meta_text()));
    let mut meta: Vec<Span> = vec![Span::styled(np.artist, meta_style)];
    if !np.album.is_empty() {
        meta.push(Span::styled(" · ", meta_style));
        meta.push(Span::styled(np.album, meta_style));
    }
    f.render_widget(
        Paragraph::new(vec![Line::from(title_spans), Line::from(meta)]),
        details,
    );

    // progress geometry: full-width bar when the time sits under it, else flanked
    let time_prefix = if with_time {
        String::new()
    } else {
        format!("{} ", mmss(np.elapsed))
    };
    let plen = time_prefix.chars().count() as u16;
    // the flanked form reserves room for "<elapsed> …bar… <remaining>", plus the
    // speed badge when one is shown — without that the badge is drawn past the
    // row's end and truncated to a stub ("1." instead of "1.25×")
    let badge_w = speed_badge(app)
        .filter(|_| !with_time)
        .map_or(0, |b| b.chars().count() as u16 + 2);
    let prog_w = if with_time {
        prog_row.width
    } else {
        prog_row.width.saturating_sub(14 + badge_w)
    };
    let bar_x = prog_row.x + plen;

    // blended visualizer: spans exactly the progress bar (its own mode, independent
    // of the view's big visualizer)
    if let Some(viz) = viz_opt
        && viz.height > 0
        && prog_w > 0
    {
        spectrum_bare(
            f,
            Rect::new(bar_x, viz.y, prog_w, viz.height),
            app,
            app.config.player_viz_mode,
        );
    }

    // progress bar + clickable/draggable seek hit-box over the bar
    let mut prog: Vec<Span> = Vec::new();
    if !time_prefix.is_empty() {
        prog.push(Span::styled(
            time_prefix,
            Style::default().fg(col(th.text_dim)),
        ));
    }
    prog.extend(progress_spans(th, np.frac, prog_w as usize));
    if !with_time {
        let remaining = np.duration.saturating_sub(np.elapsed);
        prog.push(Span::styled(
            format!(" {}", mmss(remaining)),
            Style::default().fg(col(th.text_faint)),
        ));
        // Without a dedicated time row the speed badge has nowhere to live, so a
        // non-1× rate would be invisible on every short bar — including the whole
        // mini layout. Park it after the remaining time, still only when off 1×.
        if let Some(badge) = speed_badge(app) {
            prog.push(Span::styled(
                format!("  {badge}"),
                Style::default()
                    .fg(col(th.accent[0]))
                    .add_modifier(Modifier::BOLD),
            ));
        }
    }
    app.register_click(Rect::new(bar_x, prog_row.y, prog_w, 1), MouseTarget::Seek);
    f.render_widget(Paragraph::new(Line::from(prog)), prog_row);

    // transport — centred under the bar
    let (transport, transport_clicks) = transport_row(
        app,
        th,
        trans_row.x,
        trans_row.width,
        trans_row.y,
        np.transport,
    );
    f.render_widget(Paragraph::new(transport), trans_row);
    // the centred transport's occupied span: on a narrow bar the flanking time
    // (left) and volume (right) would overdraw the buttons, so gate each on the
    // free margin beside the transport.
    let t_left = transport_clicks.first().map_or(trans_row.x, |(r, _)| r.x);
    let t_right = transport_clicks
        .last()
        .map_or(trans_row.right(), |(r, _)| r.x + r.width);
    let left_room = t_left.saturating_sub(trans_row.x);
    for (rect, btn) in transport_clicks {
        app.register_click(rect, MouseTarget::Transport(btn));
    }

    // time label under the bar's left end — only when it fits in the left margin
    // (else it'd overdraw the leftmost transport buttons)
    if with_time {
        let tw = (np.under_bar.width() as u16).min(prog_w);
        if tw <= left_room {
            f.render_widget(
                Paragraph::new(np.under_bar),
                Rect::new(bar_x, trans_row.y, tw, 1),
            );
        }
    }

    // thin horizontal volume under the bar's right end — dropped when it would
    // reach past the transport's right edge (`t_right`).
    volume_meter(f, app, th, bar_x, prog_w, trans_row, t_right);
}

// ---- now-playing transport bar ------------------------------------------
/// Rows the mini layout's playback bar occupies without the visualizer: one
/// naming what's playing, one for progress. Borderless — at this size a rounded
/// frame would cost more rows than the content it wraps.
pub const MINI_NOW_H: u16 = 2;

/// Rows the mini bar occupies *with* the visualizer: a spectrum strip above the
/// progress line, mirroring where the full-size bar puts it. One row, so the
/// motion is there without eating the list it sits under.
pub const MINI_NOW_VIZ_H: u16 = 3;

/// The mini bar's height for the current visualizer setting. A short frame keeps
/// the 2-row form regardless: below this the strip would take a row the list
/// cannot spare.
fn mini_now_h(player_viz: bool, frame_h: u16) -> u16 {
    if player_viz && frame_h >= 14 {
        MINI_NOW_VIZ_H
    } else {
        MINI_NOW_H
    }
}

/// Height of the playback bar, kept identical across every view (#1–#4): the
/// compact 2/3-row bar on a mini-sized frame, else the tall layout (with the bar
/// visualizer) when there's room, else the standard one. `frame_w`/`frame_h` are
/// the full frame dimensions so all views agree on the thresholds.
pub fn now_bar_height(player_viz: bool, frame_w: u16, frame_h: u16) -> u16 {
    if crate::ui::breakpoint::is_mini(Rect::new(0, 0, frame_w, frame_h)) {
        // The mini layout keeps the STANDARD bar, so shuffle, repeat, the transport
        // row and the speed badge stay visible — it is the same playback everywhere,
        // just in a narrower frame, and the bar already drops its art and volume
        // meter on its own when there isn't room. Only a genuinely short window
        // falls back to the stripped 2/3-row form, where the full bar would leave
        // almost nothing for the card above it.
        if frame_h < 18 {
            mini_now_h(player_viz, frame_h)
        } else if player_viz && frame_h >= 20 {
            // one more row than the standard bar: the centre column needs 5 rows
            // (title+meta, spectrum, progress, transport) before it will draw the
            // blended visualizer at all, so at 6 the narrow layout would silently
            // lose it
            7
        } else {
            6
        }
    } else if player_viz && frame_h >= 30 {
        9
    } else {
        6
    }
}

/// What the compact bar shows, independent of source. `progress` is `None` for a
/// live stream, which has a position but no total to measure it against.
pub(crate) struct CompactNow<'a> {
    pub playing: bool,
    pub title: &'a str,
    pub subtitle: &'a str,
    pub progress: Option<(f32, Duration, Duration)>,
}

/// The mini layout's 2-row playback bar. Shared by every source — the local and
/// Spotify bars reach it through [`playback_bar`], radio through
/// [`radio_now_bar`] — so the narrow layout stays consistent across views.
pub(crate) fn compact_bar(f: &mut Frame, area: Rect, app: &AppState, now: CompactNow<'_>) {
    let th = &app.theme;
    if area.width < 4 || area.height == 0 {
        return;
    }
    // title row · (optional) spectrum strip · progress row. The strip sits above
    // the progress line, the same place the full-size bar puts it.
    let show_viz = app.config.player_viz && area.height >= MINI_NOW_VIZ_H;
    let [top, viz, bot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(if show_viz { 1 } else { 0 }),
        Constraint::Min(0),
    ])
    .areas(area);

    // ── line 1: status glyph, then title · subtitle, clipped to the row
    let ic = if now.playing {
        &app.icons.play
    } else {
        &app.icons.pause
    };
    let lead = format!("{ic} ");
    let mut room = (area.width as usize).saturating_sub(lead.chars().count());
    let mut spans = vec![Span::styled(
        lead,
        Style::default()
            .fg(col(th.accent[0]))
            .add_modifier(Modifier::BOLD),
    )];
    // the title is the point of the row; the subtitle only gets the leftovers, and
    // only if enough survive to be worth a separator
    let title = super::clip(now.title, room);
    room = room.saturating_sub(title.chars().count());
    spans.push(Span::styled(
        title,
        Style::default()
            .fg(col(th.title_text()))
            .add_modifier(Modifier::BOLD),
    ));
    if !now.subtitle.is_empty() && room > 4 {
        let sub = super::clip(now.subtitle, room - 3);
        spans.push(Span::styled(
            format!(" · {sub}"),
            Style::default().fg(col(th.meta_text())),
        ));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), top);

    if show_viz && viz.height > 0 && viz.width > 1 {
        spectrum_bare(f, viz, app, app.config.player_viz_mode);
    }
    if bot.height == 0 {
        return;
    }
    // ── line 2: elapsed ── bar ── total, with the same seek hit-box as the full bar
    let Some((frac, elapsed, duration)) = now.progress else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  ● LIVE",
                Style::default().fg(col(th.accent[0])),
            ))),
            bot,
        );
        return;
    };
    let (l, r) = (mmss(elapsed), mmss(duration));
    let times = l.chars().count() + r.chars().count() + 2;
    let bar_w = (bot.width as usize).saturating_sub(times);
    let dimmed = Style::default().fg(col(th.text_faint));
    let mut prog = vec![Span::styled(format!("{l} "), dimmed)];
    prog.extend(progress_spans(th, frac, bar_w));
    prog.push(Span::styled(format!(" {r}"), dimmed));
    if bar_w > 0 {
        app.register_click(
            Rect::new(bot.x + l.chars().count() as u16 + 1, bot.y, bar_w as u16, 1),
            MouseTarget::Seek,
        );
    }
    f.render_widget(Paragraph::new(Line::from(prog)), bot);
}

/// A friendly idle now-bar: a rounded frame with a bold headline and a faint
/// hint, for when nothing is loaded — laid out like the active bars (a 1-col
/// gutter then the text).
fn idle_bar(f: &mut Frame, area: Rect, th: &Theme, head: &str, hint: &str) {
    // mini layout: the rounded frame would consume both rows it wraps
    if area.height <= MINI_NOW_VIZ_H + 1 {
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    head.to_string(),
                    Style::default()
                        .fg(col(th.text_dim))
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    hint.to_string(),
                    Style::default().fg(col(th.text_faint)),
                )),
            ]),
            area,
        );
        return;
    }
    let block = rounded(th, "", false);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let [_, center] = Layout::horizontal([Constraint::Length(1), Constraint::Min(16)]).areas(inner);
    f.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                head.to_string(),
                Style::default()
                    .fg(col(th.text_dim))
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(
                hint.to_string(),
                Style::default().fg(col(th.text_faint)),
            )),
        ]),
        center,
    );
}

pub fn now_bar(f: &mut Frame, area: Rect, app: &AppState) {
    let th = &app.theme;
    // A *source* view's bar belongs to that source: the Radio view always shows the
    // radio context (idle when nothing's tuned) and never the local track, and the
    // Spotify view likewise. Those two never leak into each other.
    if app.layout == AppLayout::Radio {
        radio_now_bar(f, area, app);
        return;
    }
    if app.layout == AppLayout::Spotify {
        spotify_now_bar(f, area, app);
        return;
    }
    // The player views (Now Playing / Lyrics / Concert) belong to no single
    // source, so they follow whatever is playing — the same resolver the OS "Now
    // Playing" bridge uses. Before this they showed the local player
    // unconditionally and sat idle while Spotify or radio played. The local
    // browser (Home / Library) is a *source* view and keeps showing local.
    match app
        .layout
        .is_player_view()
        .then(|| app.now_playing_source())
        .flatten()
    {
        Some(crate::app::NpSource::Radio) => {
            radio_now_bar(f, area, app);
            return;
        }
        Some(crate::app::NpSource::Spotify) => {
            spotify_now_bar(f, area, app);
            return;
        }
        _ => {}
    }
    let tk = app.current_track();
    // Nothing loaded (e.g. a fresh library, before anything is played) → a
    // friendly idle bar instead of a bare "—" and an empty seek line.
    if tk.is_none() {
        idle_bar(
            f,
            area,
            th,
            "♫  lyrfin",
            "Nothing playing — press ⏎ on a track to start",
        );
        return;
    }
    // album art: the bar thumbnail, unless a tag-edit cover preview is active
    let none: CoverState = None;
    let cover = if app.tags.cover.is_some() {
        &none
    } else {
        &app.art.bar
    };
    // under-bar time: elapsed/total + a speed badge when off 1.0× (so normal play
    // stays uncluttered). 2 decimals trimmed → 0.25× / 0.5× / 1.25× / 2×.
    let mut tspans = time_label(th, app.player.elapsed, app.player.duration);
    if let Some(badge) = speed_badge(app) {
        tspans.push(Span::styled(
            format!("  {badge}"),
            Style::default()
                .fg(col(th.accent[0]))
                .add_modifier(Modifier::BOLD),
        ));
    }
    playback_bar(
        f,
        area,
        app,
        NowPlaying {
            cover,
            title: tk.map(|t| t.title.clone()).unwrap_or_else(|| "—".into()),
            artist: tk.map(|t| t.artist.to_string()).unwrap_or_default(),
            album: tk.map(|t| t.album.to_string()).unwrap_or_default(),
            favorite: tk.is_some_and(|t| t.favorite),
            elapsed: app.player.elapsed,
            duration: app.player.duration,
            frac: app.player.progress(),
            transport: TransportState::local(app),
            under_bar: Line::from(tspans),
        },
    );
}

use crate::app::LIVE_EDGE_SECS;

/// The radio status row's scrub/window bar (and the spectrum above it) are inset by
/// a left rewind-depth label and a right play/LIVE badge, so the spectrum spans
/// exactly that bar — like the local playback bar — not the full playback width.
const DVR_BAR_L: u16 = 8;
const DVR_BAR_R: u16 = 12;

/// The regional-indicator flag for an ISO-3166-1 alpha-2 country code
/// (e.g. "US" → 🇺🇸); empty when the code isn't exactly two ASCII letters. Used on
/// the single-line radio now-bar only — flags are kept out of the columnar station
/// table, where their width-2-vs-computed-4 mismatch would misalign the columns.
fn flag_emoji(cc: &str) -> String {
    let bytes = cc.trim().as_bytes();
    if bytes.len() != 2 || !bytes.iter().all(|b| b.is_ascii_alphabetic()) {
        return String::new();
    }
    bytes
        .iter()
        .map(|b| {
            let cp = 0x1F1E6u32 + (b.to_ascii_uppercase() - b'A') as u32;
            char::from_u32(cp).unwrap_or('?')
        })
        .collect()
}

/// Draw the station's country in the now-bar's art slot — the small flag emoji atop
/// the ISO code in large 3-row block letters (accent-tinted), centred. Emoji can't
/// be scaled in a terminal, so the big code is what fills the album-cover-sized box.
fn render_flag_art(f: &mut Frame, area: Rect, th: &Theme, flag: &str, cc: &str) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let code = big_code(cc); // 3 rows of block letters
    // flag row (when there's vertical room) + the 3 code rows, block centred
    let with_flag = area.height >= 5;
    let block_h = if with_flag { 4 } else { 3 };
    let mut y = area.y + area.height.saturating_sub(block_h) / 2;
    let bottom = area.y + area.height;
    if with_flag && y < bottom {
        f.render_widget(
            Paragraph::new(Line::from(Span::raw(flag.to_string()))).alignment(Alignment::Center),
            Rect::new(area.x, y, area.width, 1),
        );
        y += 1;
    }
    let style = Style::default()
        .fg(col(th.accent[0]))
        .add_modifier(Modifier::BOLD);
    for row in code {
        if y >= bottom {
            break;
        }
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(row, style))).alignment(Alignment::Center),
            Rect::new(area.x, y, area.width, 1),
        );
        y += 1;
    }
}

/// The (2-letter) ISO code as 3 rows of block letters, glyphs separated by a column.
fn big_code(cc: &str) -> [String; 3] {
    let mut rows = [String::new(), String::new(), String::new()];
    for (i, ch) in cc.to_uppercase().chars().take(3).enumerate() {
        if i > 0 {
            rows.iter_mut().for_each(|r| r.push(' '));
        }
        let g = block_glyph(ch);
        for (r, gr) in rows.iter_mut().zip(g) {
            r.push_str(gr);
        }
    }
    rows
}

/// A 3-row × 3-column block glyph for `A`–`Z` (ISO country codes are letters). The
/// country name is shown in the now-bar sub-line, so the few close pairs (M/N,
/// H/W) read fine in context.
fn block_glyph(c: char) -> [&'static str; 3] {
    match c {
        'A' => ["▄▀▄", "█▀█", "█ █"],
        'B' => ["█▀▄", "█▀▄", "█▄▀"],
        'C' => ["▄▀▀", "█  ", "▀▄▄"],
        'D' => ["█▀▄", "█ █", "█▄▀"],
        'E' => ["█▀▀", "█▀ ", "█▄▄"],
        'F' => ["█▀▀", "█▀ ", "█  "],
        'G' => ["▄▀▀", "█ ▄", "▀▄▀"],
        'H' => ["█ █", "█▀█", "█ █"],
        'I' => ["▀█▀", " █ ", "▄█▄"],
        'J' => ["▀▀█", "  █", "█▄▀"],
        'K' => ["█ ▄", "█▀ ", "█ ▀"],
        'L' => ["█  ", "█  ", "█▄▄"],
        'M' => ["█▄█", "█▀█", "█ █"],
        'N' => ["█▄█", "█ █", "█ █"],
        'O' => ["▄▀▄", "█ █", "▀▄▀"],
        'P' => ["█▀▄", "█▀▀", "█  "],
        'Q' => ["▄▀▄", "█ █", "▀▄█"],
        'R' => ["█▀▄", "█▀▄", "█ ▀"],
        'S' => ["▄▀▀", "▀▀▄", "▀▄▀"],
        'T' => ["▀█▀", " █ ", " █ "],
        'U' => ["█ █", "█ █", "▀▄▀"],
        'V' => ["█ █", "█ █", " ▀ "],
        'W' => ["█ █", "█▄█", "█▀█"],
        'X' => ["▀ ▀", " █ ", "▄ ▄"],
        'Y' => ["█ █", "▀▄▀", " █ "],
        'Z' => ["▀▀█", "▄▀ ", "█▄▄"],
        _ => ["   ", "   ", "   "],
    }
}

/// Format a DVR duration as `H:MM:SS` past an hour, else `M:SS`. Session/window
/// times run to hours, which plain `mmss` would show as `83:45`.
fn dvr_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

/// A DVR seekbar: the played portion is the bright accent gradient; the buffered-
/// ahead portion (still seekable, up to the live edge) is a *dim accent* so the
/// whole window reads as buffered — distinct from an empty/unbuffered track.
fn dvr_bar_spans(th: &Theme, frac: f32, width: usize) -> Vec<Span<'static>> {
    let width = width.max(2);
    let filled = ((frac.clamp(0.0, 1.0) * width as f32).round() as usize).min(width - 1);
    let buffered = col(th.accent[0].mix(th.panel, 0.62)); // dim accent = "buffered"
    let mut spans = Vec::with_capacity(width);
    for i in 0..width {
        if i < filled {
            let g = th.accent_at(i as f32 / (width - 1) as f32);
            spans.push(Span::styled("━", Style::default().fg(col(g))));
        } else if i == filled {
            let knob = th.accent_at(frac.clamp(0.0, 1.0)).mix(th.text, 0.3);
            spans.push(Span::styled("●", Style::default().fg(col(knob))));
        } else {
            spans.push(Span::styled("━", Style::default().fg(buffered)));
        }
    }
    spans
}

/// The now-playing bar for a tuned radio station (Radio view only). A DVR-buffered
/// stream gets a seekable window bar (played + dim buffered-ahead) with a LIVE /
/// "−m:ss behind" marker; a forward-only stream shows a LIVE / paused indicator.
/// Also shows the live ICY song, the station, and a spectrum.
pub(crate) fn radio_now_bar(f: &mut Frame, area: Rect, app: &AppState) {
    let th = &app.theme;
    // mini layout: the shared compact bar, so radio reads like every other source
    if area.height <= MINI_NOW_VIZ_H + 1 {
        let Some(st) = &app.rnow.now_station else {
            idle_bar(f, area, th, "Radio", "No station — pick one and press ⏎");
            return;
        };
        // headline = live ICY song if present, else the station name (as below)
        let song = app
            .rnow
            .now_station_title
            .as_deref()
            .filter(|t| !t.is_empty());
        compact_bar(
            f,
            area,
            app,
            CompactNow {
                playing: !app.rnow.radio_paused,
                title: song.unwrap_or(&st.name),
                // once a song is showing, the station becomes the subtitle
                subtitle: if song.is_some() { &st.name } else { "" },
                progress: None, // a live stream has no total to measure against
            },
        );
        return;
    }
    let block = rounded(th, "", false);
    let inner = block.inner(area);
    f.render_widget(block, area);

    // nothing tuned → an idle radio bar (never the local track)
    let Some(st) = &app.rnow.now_station else {
        let [_, center] =
            Layout::horizontal([Constraint::Length(1), Constraint::Min(16)]).areas(inner);
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "Radio",
                    Style::default()
                        .fg(col(th.text_dim))
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    "No station — pick one and press ⏎",
                    Style::default().fg(col(th.text_faint)),
                )),
            ]),
            center,
        );
        return;
    };

    // The station's country flag stands in for the album cover the local/Spotify
    // bars show — drawn in the art slot on the left, dropped when the bar is too
    // narrow so the controls keep room.
    let flag = flag_emoji(&st.countrycode);
    let center = if !flag.is_empty() && inner.width >= 56 {
        let [art, _gap, center] = Layout::horizontal([
            Constraint::Length(12),
            Constraint::Length(2),
            Constraint::Min(16),
        ])
        .areas(inner);
        render_flag_art(f, art, th, &flag, &st.countrycode);
        center
    } else {
        let [_, center] =
            Layout::horizontal([Constraint::Length(1), Constraint::Min(16)]).areas(inner);
        center
    };

    // headline = live ICY song if present, else the station name
    let headline = app
        .rnow
        .now_station_title
        .as_deref()
        .filter(|t| !t.is_empty())
        .unwrap_or(&st.name)
        .to_string();
    // sub-line: station · genre · codec · bitrate (the flag is the art now, no 📻);
    // when the headline is the ICY song, lead with the station name, else the
    // headline already is the name so just show the details. For a DVR stream,
    // append the tuned-in session time and, when rewound, how far behind live.
    let mut sub = if headline == st.name {
        st.subtitle()
    } else {
        format!("{} · {}", st.name, st.subtitle())
    };
    if let Some(d) = app.rnow.dvr {
        sub.push_str(&format!("  ·  ⏱ {}", dvr_time(d.live)));
        let behind = d.behind_live();
        if behind >= LIVE_EDGE_SECS {
            sub.push_str(&format!("  ·  ⏴ -{} behind", dvr_time(behind)));
        }
    }

    let show_viz = app.config.player_viz && center.height >= 5;
    let (details, viz_opt, status_row) = if show_viz {
        let [d, v, s] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(center);
        (d, Some(v), s)
    } else {
        let [d, s, _] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(center);
        (d, None, s)
    };

    let title_spans: Vec<Span> = headline
        .chars()
        .enumerate()
        .map(|(i, ch)| {
            let g = th.accent_at(i as f32 / headline.chars().count().max(2) as f32);
            Span::styled(
                ch.to_string(),
                Style::default().fg(col(g)).add_modifier(Modifier::BOLD),
            )
        })
        .collect();
    f.render_widget(
        Paragraph::new(vec![
            Line::from(title_spans),
            Line::from(Span::styled(sub, Style::default().fg(col(th.text_dim)))),
        ]),
        details,
    );

    if let Some(v) = viz_opt
        && v.height > 0
    {
        // span exactly the scrub bar below (inset by the label + LIVE badge), like
        // the local playback bar; fall back to full width only when too narrow to inset
        let (vx, vw) = if v.width > DVR_BAR_L + DVR_BAR_R + 4 {
            (v.x + DVR_BAR_L, v.width - DVR_BAR_L - DVR_BAR_R)
        } else {
            (v.x, v.width)
        };
        spectrum_bare(
            f,
            Rect::new(vx, v.y, vw, v.height),
            app,
            app.config.player_viz_mode,
        );
    }

    // Timeshift (DVR) window bar when the stream is buffered — a "window timeline":
    // left = how far back you can rewind now (the window depth); the bar is that
    // window with the knob at the play-head (played bright · buffered-ahead dim);
    // right = a persistent LIVE badge (bright at the edge, dim when rewound/paused)
    // that jumps to live. Session + behind-live sit in the sub-line above. Falls
    // back to a plain LIVE / paused indicator for a forward-only (un-buffered) stream.
    if let Some(dvr) = app.rnow.dvr.filter(|d| d.live - d.start > 0.5) {
        let span = (dvr.live - dvr.start).max(0.001);
        let behind = dvr.behind_live();
        // At/near the live edge, pin the play-head to the right end: the byte-
        // estimated live edge advances in bursts, so computing the fraction from it
        // there makes the knob jitter. Only once rewound do we show the true spot.
        let at_live = behind < LIVE_EDGE_SECS;
        let frac = if at_live {
            1.0
        } else {
            ((dvr.pos - dvr.start) / span).clamp(0.0, 1.0) as f32
        };
        let [lab, bar, right] = Layout::horizontal([
            Constraint::Length(DVR_BAR_L), // −window: how far back you can rewind now
            Constraint::Min(4),            // the rewindable window
            Constraint::Length(DVR_BAR_R), // play/pause + persistent LIVE badge
        ])
        .areas(status_row);

        // left: the rewind depth (window), dim — a reference edge, not the position
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("-{}", dvr_time(span)),
                Style::default().fg(col(th.text_dim)),
            ))),
            lab,
        );
        f.render_widget(
            Paragraph::new(Line::from(dvr_bar_spans(th, frac, bar.width as usize))),
            bar,
        );
        app.register_click(bar, MouseTarget::Seek); // scrub to seek within the window

        // right: play/pause glyph + a persistent LIVE badge — bright ◉ at the edge,
        // dim ○ when rewound or paused; click it to jump to live.
        let playing = !app.rnow.radio_paused;
        let pp = if playing { "⏸" } else { "▶" };
        let (dot, live_fg) = if at_live && playing {
            ("◉", th.accent[0])
        } else {
            ("○", th.text_dim)
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!(" {pp}  "),
                    Style::default()
                        .fg(col(th.accent[0]))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{dot} LIVE"),
                    Style::default()
                        .fg(col(live_fg))
                        .add_modifier(Modifier::BOLD),
                ),
            ])),
            right,
        );
        app.register_click(
            Rect::new(right.x + 1, right.y, 1, 1),
            MouseTarget::Transport(TransportButton::PlayPause),
        );
        app.register_click(
            Rect::new(right.x + 4, right.y, right.width.saturating_sub(4), 1),
            MouseTarget::RadioGoLive,
        );
    } else {
        // forward-only live stream: LIVE / paused indicator — clicking toggles it.
        let (icon, label, fg) = if app.rnow.radio_paused {
            ("▶", "paused", th.text_dim)
        } else {
            ("◉", "LIVE", th.accent[0])
        };
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!(" {icon} "),
                    Style::default()
                        .fg(col(th.accent[0]))
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    label,
                    Style::default().fg(col(fg)).add_modifier(Modifier::BOLD),
                ),
            ])),
            status_row,
        );
        app.register_click(
            Rect::new(status_row.x, status_row.y, 8, 1),
            MouseTarget::Transport(TransportButton::PlayPause),
        );
    }
}

/// Now-playing bar for the Spotify view: the streaming track + a real progress
/// bar (librespot is seekable) + a spectrum + a playing/paused indicator.
/// The Spotify playback bar — laid out to match the local `now_bar` exactly:
/// album art on the left; then a centre column of title (gradient + ♥ when
/// liked) · artist · a blended visualizer · the progress bar · the transport
/// row, with the elapsed/total time tucked under the bar's left and a horizontal
/// volume meter under its right.
pub(crate) fn spotify_now_bar(f: &mut Frame, area: Rect, app: &AppState) {
    let th = &app.theme;
    let Some(tr) = &app.spov.now_spotify else {
        // nothing playing: draw the empty bar with a hint
        let block = rounded(th, "", false);
        let inner = block.inner(area);
        f.render_widget(block, area);
        let [_, c] = Layout::horizontal([Constraint::Length(1), Constraint::Min(16)]).areas(inner);
        f.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "Spotify",
                    Style::default()
                        .fg(col(th.text_dim))
                        .add_modifier(Modifier::BOLD),
                )),
                Line::from(Span::styled(
                    "Pick a track and press ⏎",
                    Style::default().fg(col(th.text_faint)),
                )),
            ]),
            c,
        );
        return;
    };

    // librespot reports the position only on play/pause, so the clock is ticked
    // locally (see app::tick); sp_dur == 0 until the first metadata arrives.
    let sh = app.config.arabic_shaping;
    let elapsed = Duration::from_secs_f64(app.spov.sp_pos.max(0.0));
    let duration = Duration::from_secs_f64(app.spov.sp_dur.max(0.0));
    let frac = if app.spov.sp_dur > 0.0 {
        (app.spov.sp_pos / app.spov.sp_dur).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    // under-bar: elapsed/total, or a buffering indicator while the stream fills
    let under_bar = if app.sp_buffering() {
        Line::from(Span::styled(
            "◌ buffering…",
            Style::default().fg(col(th.text_dim)),
        ))
    } else {
        Line::from(time_label(th, elapsed, duration))
    };
    playback_bar(
        f,
        area,
        app,
        NowPlaying {
            cover: &app.spov.sp_cover,
            title: crate::arabic::shaped(&tr.name, sh),
            artist: crate::arabic::shaped(&tr.subtitle, sh),
            album: crate::arabic::shaped(&tr.album, sh),
            favorite: app.spov.sp_saved,
            elapsed,
            duration,
            frac,
            transport: TransportState::spotify(app),
            under_bar,
        },
    );
}

/// The thin horizontal volume meter drawn under the right end of a playback bar
/// (`bar_x`..`bar_x + prog_w`), with a click/drag hit-box. Shared by the local
/// and Spotify bars — both route audio through the same engine volume. No-op when
/// the bar is too narrow (`prog_w <= 20`) or when it would reach left of `guard_x`
/// (the transport's right edge) and overdraw the buttons.
fn volume_meter(
    f: &mut Frame,
    app: &AppState,
    th: &Theme,
    bar_x: u16,
    prog_w: u16,
    trans_row: Rect,
    guard_x: u16,
) {
    if prog_w <= 20 {
        return;
    }
    let bar_right = bar_x + prog_w;
    let mw = 8u16; // thin meter cells
    // muted-speaker glyph at 0%, normal speaker otherwise
    let vglyph = if app.player.volume == 0 {
        &app.icons.volume_mute
    } else {
        &app.icons.volume
    };
    let label = format!("{vglyph} ");
    let llen = label.chars().count() as u16;
    // fixed-width percent (right-aligned 3 digits) so the widget doesn't shift
    // when it grows to "100%"
    let pct = format!(" {:>3}%", app.player.volume);
    let widget_w = llen + mw + pct.chars().count() as u16;
    let vol_x = bar_right.saturating_sub(widget_w).max(trans_row.x);
    if vol_x < guard_x {
        return; // would overdraw the transport buttons — drop the meter
    }
    let filled = (app.player.volume as f32 / 100.0 * mw as f32).round() as u16;
    let mut spans: Vec<Span> = vec![Span::styled(
        label,
        Style::default()
            .fg(col(th.text_dim))
            .add_modifier(Modifier::BOLD),
    )];
    for i in 0..mw {
        let (ch, c) = if i < filled {
            ("━", th.slider_color())
        } else {
            ("─", th.text_faint)
        };
        spans.push(Span::styled(ch, Style::default().fg(col(c))));
    }
    spans.push(Span::styled(pct, Style::default().fg(col(th.text_dim))));
    f.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(vol_x, trans_row.y, widget_w.min(trans_row.width), 1),
    );
    // the meter portion is the clickable/draggable volume control
    app.register_click(
        Rect::new(vol_x + llen, trans_row.y, mw, 1),
        MouseTarget::Volume,
    );
}
