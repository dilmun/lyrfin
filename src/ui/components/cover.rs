//! Big album art, now-playing meta, and the lyrics panel.

use super::*;
use crate::app::{AppState, MouseTarget};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

// ---- big album art (Now Playing / Lyrics views) --------------------------

/// A persistent inline-image cover: the resize protocol plus the source image it
/// was built from. Keeping the image lets [`Cover::rebuild`] mint a *fresh*
/// protocol (new Kitty image id) on demand.
///
/// Why the image is retained: ratatui-image v11's Kitty renderer reuses the image
/// id on `resize_encode`, and Ghostty won't repaint occluded unicode-placeholder
/// cells when the `(id, row, col)` is unchanged — so after a modal overlay partly
/// covers a cover, a same-id re-transmit leaves stale glyphs on the covered edge.
/// A *new* id (which a new track's cover naturally gets) is the only thing that
/// makes Ghostty re-place the whole image. `rebuild` forces that new id by
/// re-building the protocol from the kept image — synchronously, so the swap is
/// instant with no blank frame. Called on the modal-close edge (see
/// [`AppState::rebuild_persistent_covers`]).
pub struct Cover {
    pub proto: std::cell::RefCell<ratatui_image::protocol::StatefulProtocol>,
    img: std::sync::Arc<image::DynamicImage>,
}

impl Cover {
    /// Build a cover from a decoded image, retaining it for later [`Self::rebuild`].
    pub fn new(picker: &ratatui_image::picker::Picker, img: image::DynamicImage) -> Self {
        let img = std::sync::Arc::new(img);
        Self {
            proto: std::cell::RefCell::new(picker.new_resize_protocol((*img).clone())),
            img,
        }
    }

    /// Replace the protocol with a freshly-built one (new image id) from the kept
    /// image, so the next render re-places the whole image over any stale cells.
    pub fn rebuild(&self, picker: &ratatui_image::picker::Picker) {
        *self.proto.borrow_mut() = picker.new_resize_protocol((*self.img).clone());
    }
}

pub type CoverState = Option<Cover>;

/// The one place tmux's image limitation lives: the largest inline-image edge
/// (px) that passes through tmux's escape-passthrough. There's no API to query
/// it, so it's a single tuned constant — applied *only* inside tmux. Outside
/// tmux images are sized naturally (by layout + the source's own resolution).
pub(crate) const TMUX_IMG_PX: u32 = 384;

/// The terminal's font cell size (px), for sizing image rects. Falls back to a
/// reasonable 1:2 cell if the picker isn't available.
pub(crate) fn image_font(app: &AppState) -> (u16, u16) {
    app.art
        .picker
        .as_ref()
        .map(|p| {
            let f = p.font_size();
            (f.width, f.height)
        })
        .unwrap_or((10, 20))
}

/// A centred, square (cover-aspect) image rect inside `area`, capped to `max_px`
/// on each edge — so we never transmit a larger image than needed (or than tmux
/// can handle) and never upscale beyond the source.
pub(crate) fn square_image_rect(area: Rect, font: (u16, u16), max_px: u32) -> Rect {
    let (cw, ch) = (font.0.max(1) as u32, font.1.max(1) as u32);
    let native_h = (max_px / ch).max(1); // cells tall at 1:1
    let h_by_w = (area.width as u32 * cw / ch).max(1); // keep width ≤ area
    let side_h = (area.height as u32).min(native_h).min(h_by_w).max(1) as u16;
    let pw = ((side_h as u32 * ch / cw) as u16).min(area.width);
    Rect::new(
        area.x + (area.width.saturating_sub(pw)) / 2,
        area.y + (area.height.saturating_sub(side_h)) / 2,
        pw,
        side_h,
    )
}

/// Render the real embedded cover via an inline image protocol (true image in
/// iTerm2/kitty/sixel/tmux-passthrough, colored half-blocks elsewhere), falling
/// back to the gradient placeholder when there's no cover.
pub(crate) fn render_cover(f: &mut Frame, area: Rect, cover: &CoverState, app: &AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if let Some(c) = cover
        && let Ok(mut proto) = c.proto.try_borrow_mut()
    {
        f.render_stateful_widget(ratatui_image::StatefulImage::default(), area, &mut *proto);
        return;
    }
    f.render_widget(
        Paragraph::new(art_lines(
            &app.theme,
            area.width as usize,
            area.height as usize,
        )),
        area,
    );
}

/// Like [`render_cover`] but *upscales* the cover to fill `area` (`Resize::Scale`),
/// so small covers don't render tiny. tmux can't forward an upscaled image, so it
/// falls back to a centred, tmux-safe square. Pass an aspect-matched rect to get
/// the result centred without letterboxing.
pub(crate) fn render_cover_filled(f: &mut Frame, area: Rect, cover: &CoverState, app: &AppState) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    if let Some(c) = cover
        && let Ok(mut proto) = c.proto.try_borrow_mut()
    {
        render_proto_filled(f, area, &mut proto, app);
        return;
    }
    f.render_widget(
        Paragraph::new(art_lines(
            &app.theme,
            area.width as usize,
            area.height as usize,
        )),
        area,
    );
}

/// Render an already-built image protocol so it FILLS `area` (`Resize::Scale`
/// upscales, so a small source doesn't render tiny in a top-left corner). tmux
/// can't forward an upscaled image, so there it draws a centred, tmux-safe square
/// instead. Pass an aspect-matched `area` to avoid letterboxing. Shared by the big
/// cover, the Spotify pane, and the local Artist pane photo.
pub(crate) fn render_proto_filled(
    f: &mut Frame,
    area: Rect,
    proto: &mut ratatui_image::protocol::StatefulProtocol,
    app: &AppState,
) {
    if app.in_tmux {
        let rect = square_image_rect(area, image_font(app), TMUX_IMG_PX);
        f.render_stateful_widget(ratatui_image::StatefulImage::default(), rect, proto);
    } else {
        f.render_stateful_widget(
            ratatui_image::StatefulImage::default().resize(ratatui_image::Resize::Scale(None)),
            area,
            proto,
        );
    }
}

/// Render a pre-cropped carousel "peek" slice filling `rect`. The worker already
/// sized it to ~`rect` (card width × the visible slice height), so this is a
/// near-1:1 scale — no tmux square-centring (a peek is a wide-short strip, not a
/// square cover).
pub(crate) fn render_proto_peek(
    f: &mut Frame,
    rect: Rect,
    proto: &mut ratatui_image::protocol::StatefulProtocol,
) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    f.render_stateful_widget(
        ratatui_image::StatefulImage::default().resize(ratatui_image::Resize::Scale(None)),
        rect,
        proto,
    );
}

pub fn album_art(f: &mut Frame, area: Rect, app: &AppState) {
    // suppress the background cover while the art-search popup is open, so its
    // (persistent, kitty/iTerm) image doesn't fight the popup's preview image.
    let none: CoverState = None;
    let cover = if app.tags.cover.is_some() {
        &none
    } else {
        &app.art.full
    };
    render_cover(f, area, cover, app);
}

/// Like [`album_art`] but *upscales* the cover to fill `area` (`Resize::Scale`),
/// so small covers don't render tiny. The caller is responsible for passing an
/// aspect-matched rect if it wants the result centred without letterboxing.
pub fn album_art_filled(f: &mut Frame, area: Rect, app: &AppState) {
    // suppress the background cover while the art-search popup is open, so its
    // (persistent) preview image doesn't fight this one.
    let none: CoverState = None;
    let cover = if app.tags.cover.is_some() {
        &none
    } else {
        &app.art.full
    };
    render_cover_filled(f, area, cover, app);
}

// ---- synced lyrics panel -------------------------------------------------
/// Render the lyrics pane. `focused` highlights the border when this pane holds
/// keyboard focus (the caller decides — `Focus::Main` in local views, the
/// Spotify view's `Focus::Pane(Panel::Lyrics)`).
///
/// `pane` declares which source this instance shows. Local and Spotify views share
/// this one component and the single `meta.lyrics` slot, so the pane is gated to
/// its own source (`lyrics_for_pane`): a local track's lyrics never render in the
/// Spotify pane, nor vice-versa.
pub fn lyrics_panel(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    focused: bool,
    pane: crate::app::LyricsPane,
) {
    let th = &app.theme;
    // whole-pane hit target so a click/scroll anywhere in the box focuses it
    app.register_click(area, MouseTarget::Scroll(crate::app::ScrollBox::Lyrics));

    // resolve the pane's lyrics (source-gated) + the active line (synced → by time,
    // plain → by progress); no lyrics → a status message.
    let (lyric_lines, trans, cur, frac, synced) = match app.lyrics_for_pane(pane) {
        Some(l) if !l.lines.is_empty() => {
            let n = l.lines.len();
            // (active line, fraction through it) — fraction drives the karaoke
            // wipe; `playback_*` follow the Spotify clock in the Spotify view
            let (cur, frac) = if l.synced {
                l.active_progress(app.playback_elapsed())
                    .unwrap_or((0, 0.0))
            } else {
                // plain lyrics: line by progress, offset-adjusted so `,`/`.` nudge
                // unsynced lyrics too (`lyrics_progress` folds in `lyrics_offset_ms`)
                (
                    ((app.lyrics_progress() * n as f32) as usize).min(n - 1),
                    0.0,
                )
            };
            (
                l.lines.iter().map(|(_, s)| s.clone()).collect::<Vec<_>>(),
                l.trans.clone(),
                cur,
                frac,
                l.synced,
            )
        }
        _ => {
            let block = rounded(th, "LYRICS", focused);
            let inner = block.inner(area);
            f.render_widget(block, area);
            let msg = if app.lyrics_pane_loading(pane) {
                "searching for lyrics online…"
            } else if app.lyrics_pane_has_track(pane) {
                "no lyrics found"
            } else {
                "—"
            };
            f.render_widget(
                Paragraph::new(vec![
                    Line::raw(""),
                    Line::from(Span::styled(msg, Style::default().fg(col(th.text_faint)))),
                ])
                .alignment(Alignment::Center),
                inner,
            );
            return;
        }
    };

    let base = if synced {
        "LYRICS · synced"
    } else {
        "LYRICS · text"
    };
    // surface the manual sync nudge so it's not an invisible setting
    let title = match app.config.lyrics_offset_ms {
        0 => base.to_string(),
        o => format!("{base} · {}{o}ms", if o > 0 { "+" } else { "" }),
    };
    let block = rounded(th, &title, focused);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let w = inner.width as usize;
    let h = inner.height as usize;
    let n = lyric_lines.len();
    // Pre-shape Arabic only when enabled; off → pass raw text so a shaping-capable
    // terminal (Ghostty/Kitty) joins it natively without per-cell cracks.
    let arabic = app.config.arabic_shaping
        && lyric_lines
            .iter()
            .any(|l| crate::arabic::contains_arabic(l));

    // show translations (bilingual .lrc) when present and enabled
    let dual = app.config.lyrics_dual && trans.iter().any(|t| t.is_some());

    let shape = |s: &str| {
        if arabic && crate::arabic::contains_arabic(s) {
            crate::arabic::shape_line(s, true)
        } else {
            s.to_string()
        }
    };
    let pad_for = |lw: usize| match app.config.lyrics_align {
        1 => 0,
        2 => w.saturating_sub(lw),
        _ => w.saturating_sub(lw) / 2,
    };
    let palette = [
        th.accent[0],
        th.accent[1],
        th.accent[2],
        th.warning,
        th.text,
    ];
    let solid = palette[app.config.lyrics_color.min(4) as usize];
    let tr_line = |i: usize| -> Option<Line<'static>> {
        let tr = trans
            .get(i)
            .and_then(|t| t.as_deref())
            .filter(|s| !s.is_empty())?;
        let s = shape(tr);
        let p = pad_for(s.chars().count());
        Some(Line::from(Span::styled(
            format!("{}{}", " ".repeat(p), s),
            Style::default()
                .fg(col(th.text_dim))
                .add_modifier(Modifier::ITALIC),
        )))
    };

    // teleprompter: only the current line (karaoke) + the next line, faint,
    // vertically centred. A focused, karaoke-screen presentation.
    if app.config.lyrics_teleprompter {
        let mut body: Vec<Line> = Vec::new();
        let cur_s = shape(&lyric_lines[cur]);
        let lw = cur_s.chars().count();
        let cut = if synced && app.config.lyrics_karaoke {
            (frac * lw as f32).round() as usize
        } else {
            lw
        };
        body.push(Line::from(karaoke_spans(
            th,
            &cur_s,
            pad_for(lw),
            cut,
            app.config.lyrics_gradient,
            solid,
        )));
        if dual && let Some(t) = tr_line(cur) {
            body.push(t);
        }
        body.push(Line::raw(""));
        if cur + 1 < n {
            let nx = shape(&lyric_lines[cur + 1]);
            body.push(Line::from(Span::styled(
                format!("{}{}", " ".repeat(pad_for(nx.chars().count())), nx),
                Style::default().fg(col(th.text_faint)),
            )));
            if dual && let Some(t) = tr_line(cur + 1) {
                body.push(t);
            }
        }
        let top = h.saturating_sub(body.len().min(h)) / 2;
        let mut full: Vec<Line> = (0..top).map(|_| Line::raw("")).collect();
        full.extend(body);
        f.render_widget(Paragraph::new(full), inner);
        return;
    }

    // line spacing (blank rows between lines) + how many lines fit per screen.
    // when dual, reserve a row per line for the translation.
    let gap = app.config.lyrics_gap as usize;
    let stride = 1 + gap + usize::from(dual);
    let per_screen = (h / stride).max(1);
    // window so the active line stays vertically centred (auto-scroll), unless the
    // user has scrolled by hand — then honour their offset until the track changes
    let max_off = n.saturating_sub(per_screen);
    app.scroll.lyrics_max.set(max_off);
    let off = if app.scroll.lyrics_manual.get() {
        app.scroll.lyrics.get().min(max_off)
    } else if n > per_screen {
        cur.saturating_sub(per_screen / 2).min(max_off)
    } else {
        0
    };
    let visible: Vec<usize> = (off..(off + per_screen).min(n)).collect();

    // vertical-centre the block when it doesn't fill the box
    let used = visible.len() * stride;
    let top_pad = h.saturating_sub(used.min(h)) / 2;
    let mut out: Vec<Line> = (0..top_pad).map(|_| Line::raw("")).collect();

    for &i in &visible {
        let shaped = if arabic {
            crate::arabic::shape_line(&lyric_lines[i], true)
        } else {
            lyric_lines[i].clone()
        };
        let lw = shaped.chars().count();
        let pad = match app.config.lyrics_align {
            1 => 0,                        // left
            2 => w.saturating_sub(lw),     // right
            _ => w.saturating_sub(lw) / 2, // center
        };

        if i == cur {
            // active line: karaoke "wipe" (cut == full width when karaoke off)
            let cut = if synced && app.config.lyrics_karaoke {
                (frac * lw as f32).round() as usize
            } else {
                lw
            };
            let palette = [
                th.accent[0],
                th.accent[1],
                th.accent[2],
                th.warning,
                th.text,
            ];
            let solid = palette[app.config.lyrics_color.min(4) as usize];
            out.push(Line::from(karaoke_spans(
                th,
                &shaped,
                pad,
                cut,
                app.config.lyrics_gradient,
                solid,
            )));
        } else {
            let d = i.abs_diff(cur);
            let c = if d == 1 {
                th.text
            } else if d <= 3 {
                th.text_dim
            } else {
                th.text_faint
            };
            out.push(Line::from(Span::styled(
                format!("{}{}", " ".repeat(pad), shaped),
                Style::default().fg(col(c)),
            )));
        }
        // translation row (bilingual .lrc), dim, under its line
        if dual {
            let tr = trans.get(i).and_then(|t| t.as_deref()).unwrap_or("");
            let tr_shaped = if arabic && crate::arabic::contains_arabic(tr) {
                crate::arabic::shape_line(tr, true)
            } else {
                tr.to_string()
            };
            let tlen = tr_shaped.chars().count();
            let tpad = match app.config.lyrics_align {
                1 => 0,
                2 => w.saturating_sub(tlen),
                _ => w.saturating_sub(tlen) / 2,
            };
            let c = if i == cur { th.text_dim } else { th.text_faint };
            out.push(Line::from(Span::styled(
                format!("{}{}", " ".repeat(tpad), tr_shaped),
                Style::default().fg(col(c)).add_modifier(Modifier::ITALIC),
            )));
        }
        for _ in 0..gap {
            out.push(Line::raw(""));
        }
    }
    f.render_widget(Paragraph::new(out), inner);
}
