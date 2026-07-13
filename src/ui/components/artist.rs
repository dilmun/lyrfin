//! The shared "ARTIST" info pane. One renderer (`artist_pane`) draws the photo +
//! name · stat · meta · now-playing · top tracks · bio for every source view; thin
//! adapters gather its content from the local library (`artist_panel`) or Spotify
//! (`spotify_artist_panel`). Standardized chrome — the bordered, scrollable text
//! region with ▴/▾ overflow arrows (`artist_scroll_region`), the photo geometry
//! (`photo_layout`), and the bio wrapping (`bio_lines`) are all shared; the sources
//! differ only in the `ArtistContent` they build. Both read the MusicBrainz/Wikipedia
//! bio through `app.current_artist_info()` — a source gate that returns the shared
//! `meta.artist_info` slot only when it belongs to the artist *this* pane is showing,
//! so a bio fetched for the other source can't bleed across. Only one artist pane is
//! visible at a time, so they share the scroll offset.

use super::*;
use crate::app::{AppState, MouseTarget, ScrollBox};
use crate::artwork::{ArtKey, ArtSource};
use crate::ui::theme::Theme;
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Choose the pane photo for a grid-cached image, but only once it's decoded: a
/// `Grid` photo while `key` is ready, else `None` (text-only). Keeps a Spotify pane
/// with no available online photo from reserving an empty placeholder box.
fn grid_photo_when_ready<'a>(
    app: &AppState,
    key: ArtKey,
    name: &'a str,
    circle: bool,
) -> ArtistPhoto<'a> {
    if app.art_ready(key) {
        ArtistPhoto::Grid { key, name, circle }
    } else {
        ArtistPhoto::None
    }
}

/// Wrap an artist bio into styled display lines: newlines flattened, truncated to
/// a sane cap, then shaped + right-aligned for Arabic. Shared by both adapters.
fn bio_lines(th: &Theme, bio: &str, width: usize, shape: bool) -> Vec<Line<'static>> {
    let bio = trunc(&bio.replace(['\n', '\r'], " "), 1600);
    // justify + hyphenate the prose bio; the Arabic/RTL path ignores justify and
    // keeps its right-alignment.
    crate::arabic::display_lines(&bio, width, shape, true)
        .into_iter()
        .map(|dl| Line::from(Span::styled(dl, Style::default().fg(col(th.text)))))
        .collect()
}

/// Render a built `lines` list into `text` (the content region — the whole inner
/// pane, or below a fixed photo), sharing the artist scroll offset, drawing the
/// ▴/▾ overflow arrows on `area`'s right border, and registering the wheel target.
fn artist_scroll_region(f: &mut Frame, area: Rect, app: &AppState, text: Rect, lines: Vec<Line>) {
    let th = &app.theme;
    let visible = text.height as usize;
    let max = lines.len().saturating_sub(visible);
    app.scroll.artist_max.set(max);
    let off = app.scroll.artist.get().min(max);
    app.scroll.artist.set(off);
    app.register_click(area, MouseTarget::Scroll(ScrollBox::Artist));
    let shown: Vec<Line> = lines.into_iter().skip(off).take(visible).collect();
    f.render_widget(Paragraph::new(shown), text);

    // overflow arrows on the right of the top/bottom border (don't cover text)
    let bx = area.x + area.width.saturating_sub(2);
    if off > 0 {
        f.render_widget(
            Paragraph::new(Span::styled("▴", Style::default().fg(col(th.text_faint)))),
            Rect::new(bx, area.y, 1, 1),
        );
    }
    if off < max {
        f.render_widget(
            Paragraph::new(Span::styled("▾", Style::default().fg(col(th.accent[0])))),
            Rect::new(bx, area.y + area.height.saturating_sub(1), 1, 1),
        );
    }
}

/// How many of an artist's tracks the "TOP TRACKS" list shows.
const TOP_TRACKS: usize = 8;

/// Layout for an artist pane with a photo fixed at the top: a centred square photo
/// (driven by the pane width, capped so a few text rows always remain) and the
/// scrollable text region below it. Returns `(None, inner)` when the pane is too
/// short for a photo. `font` is the terminal cell size (`image_font`). Shared by
/// the local and Spotify artist panes.
fn photo_layout(font: (u16, u16), inner: Rect) -> (Option<Rect>, Rect) {
    const MIN_TEXT_ROWS: u16 = 6; // name + meta + now-playing + a little body
    if inner.height < 8 {
        return (None, inner);
    }
    let (cw, ch) = (font.0.max(1) as u32, font.1.max(1) as u32);
    let pad = 1u16; // breathing room on each side
    let avail_w = inner.width.saturating_sub(pad * 2).max(1);
    // square height that would fill the available width…
    let w_driven_h = (avail_w as u32 * cw / ch).max(1) as u16;
    // …capped only by the height left after reserving room for the text below
    let max_photo_h = inner.height.saturating_sub(MIN_TEXT_ROWS).max(3);
    let photo_h = w_driven_h.min(max_photo_h);
    // keep it square: derive the width back from the (possibly capped) height
    let photo_w = ((photo_h as u32 * ch / cw) as u16).min(avail_w).max(1);
    let px = inner.x + inner.width.saturating_sub(photo_w) / 2;
    let photo = Rect::new(px, inner.y, photo_w, photo_h);
    let text = Rect::new(
        inner.x,
        inner.y + photo_h + 1,
        inner.width,
        inner.height.saturating_sub(photo_h + 1),
    );
    (Some(photo), text)
}

/// One "TOP TRACKS" row: the display title (already Arabic-shaped) and an optional
/// play count — `Some(n)` appends "· n" (the local pane), `None` omits it (Spotify
/// has no per-artist counts).
pub struct ArtistTop {
    pub title: String,
    pub plays: Option<u32>,
}

/// How the pane draws its top photo. The two real variants mirror the two photo
/// sources without the renderer needing to know which view it is (the same shape
/// as `MouseTarget`'s source-specific variants).
pub enum ArtistPhoto<'a> {
    /// No photo — the pane is all text.
    None,
    /// Local: a grid-artwork-cache key drawn via `grid::fill_thumb` (`name` is the
    /// text fallback while the image loads, `circle` masks it into a disc).
    Grid {
        key: ArtKey,
        name: &'a str,
        circle: bool,
    },
    /// Spotify: an already-decoded cover image drawn via `render_cover_filled`.
    Cover(&'a CoverState),
}

/// Source-agnostic content for the ARTIST pane. Single-label strings arrive already
/// Arabic-shaped (`arabic::shaped`); only the width-wrapped bio is shaped by the
/// renderer (it needs the pane width, via `app.config.arabic_shaping`). Build one
/// with the `artist_panel` (local) or `spotify_artist_panel` (Spotify) adapter.
pub struct ArtistContent<'a> {
    pub photo: ArtistPhoto<'a>,
    /// Artist name (accent, bold).
    pub name: String,
    /// Headline stat on its own line — Spotify followers/popularity; local has none.
    pub stat: Option<String>,
    /// genre/genres · formed · country, joined with "  ·  ".
    pub meta: Vec<String>,
    /// The full "♪ …" now-playing title line.
    pub now_track: String,
    /// The full "◇ …" now-playing album line; `None`/empty drops the line.
    pub now_album: Option<String>,
    /// The local pane bolds the album line; Spotify doesn't.
    pub now_album_bold: bool,
    pub top: Vec<ArtistTop>,
    /// Raw bio text (wrapped + shaped by the renderer using the pane width).
    pub bio: Option<String>,
    /// Shown when `bio` is absent but a fetch is in flight ("loading artist info…"
    /// vs "loading bio…").
    pub loading_label: &'a str,
}

/// Render the ARTIST pane from source-agnostic `content` (or "nothing playing" when
/// `None`): a bordered panel, an optional top photo, then the scrollable text region
/// (name · stat · meta · now-playing · top tracks · bio). Shared by every source
/// view — they differ only in the `ArtistContent` they build.
pub fn artist_pane(
    f: &mut Frame,
    area: Rect,
    app: &AppState,
    focused: bool,
    content: Option<ArtistContent>,
) {
    let th = &app.theme;
    let block = rounded(th, "ARTIST", focused);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let Some(c) = content else {
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "nothing playing",
                Style::default().fg(col(th.text_faint)),
            )))
            .alignment(Alignment::Center),
            inner,
        );
        return;
    };

    // photo fixed at the top; the text region below it scrolls (shared geometry — a
    // centred square that scales with the pane width, capped to leave text rows).
    let mut text = inner;
    if let (Some(rect), below) = photo_layout(image_font(app), inner) {
        let drawn = match c.photo {
            ArtistPhoto::None => false,
            ArtistPhoto::Grid { key, name, circle } => {
                // fill the shared square exactly (no re-square) — the same size the
                // Spotify branch fills below, so both panes match and the text sits
                // flush one row under the photo.
                super::grid::fill_thumb(f, rect, app, Some(key), name, circle, None);
                true
            }
            ArtistPhoto::Cover(cover) => {
                render_cover_filled(f, rect, cover, app);
                true
            }
        };
        if drawn {
            text = below;
        }
    }
    if text.height == 0 {
        return;
    }

    let faint_bold = Style::default()
        .fg(col(th.text_faint))
        .add_modifier(Modifier::BOLD);
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        c.name,
        Style::default()
            .fg(col(th.accent[0]))
            .add_modifier(Modifier::BOLD),
    ))];

    // headline stat (Spotify) on its own line so a long genre list can't clip it
    if let Some(stat) = c.stat {
        lines.push(Line::from(Span::styled(
            stat,
            Style::default()
                .fg(col(th.accent[2]))
                .add_modifier(Modifier::BOLD),
        )));
    }
    // genre/genres · formed · country
    if !c.meta.is_empty() {
        lines.push(Line::from(Span::styled(
            c.meta.join("  ·  "),
            Style::default().fg(col(th.text_dim)),
        )));
    }

    // now-playing context: the track + its album
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        c.now_track,
        Style::default().fg(col(th.text)),
    )));
    if let Some(album) = c.now_album.filter(|a| !a.is_empty()) {
        let mut style = Style::default().fg(col(th.accent[2]));
        if c.now_album_bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        lines.push(Line::from(Span::styled(album, style)));
    }

    // top tracks
    if !c.top.is_empty() {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled("TOP TRACKS", faint_bold)));
        for (i, t) in c.top.into_iter().enumerate() {
            let mut spans = vec![
                Span::styled(
                    format!("{:>2} ", i + 1),
                    Style::default().fg(col(th.text_faint)),
                ),
                Span::styled(t.title, Style::default().fg(col(th.text))),
            ];
            if let Some(n) = t.plays {
                spans.push(Span::styled(
                    format!("  · {n}"),
                    Style::default().fg(col(th.text_faint)),
                ));
            }
            lines.push(Line::from(spans));
        }
    }

    // bio (Spotify's own first, then the MusicBrainz/Wikipedia bio — resolved by the
    // adapter); else a loading line while the info worker is still fetching
    if let Some(bio) = c.bio {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled("ABOUT", faint_bold)));
        lines.extend(bio_lines(
            th,
            &bio,
            inner.width as usize,
            app.config.arabic_shaping,
        ));
    } else if app.artist_info_loading() {
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            c.loading_label,
            Style::default().fg(col(th.text_faint)),
        )));
    }

    artist_scroll_region(f, area, app, text, lines);
}

/// Local artist pane: the now-playing artist's photo (from the grid artwork cache),
/// name · genre/formed/country, the now-playing track + album, the artist's
/// most-played local tracks, and the MusicBrainz/Wikipedia bio. Builds the content
/// for the shared `artist_pane`.
pub fn artist_panel(f: &mut Frame, area: Rect, app: &AppState, focused: bool) {
    let Some(track) = app.current_track() else {
        return artist_pane(f, area, app, focused, None);
    };
    let sh = app.config.arabic_shaping;
    let name = track.album_artist.to_string();

    // Photo from the grid artwork cache (same `ArtKey::Artist` + global shape, so
    // decodes are shared with the Albums/Artists grid and the request is coalesced).
    // The request fires whenever art is available; the pane only shows it if there's
    // room (decided in `artist_pane`).
    let mut photo = ArtistPhoto::None;
    if let Some(id) = track.artist_id
        && let Some((key, source, circle)) = app.artist_pane_art(id)
    {
        app.request_art(key, source, circle);
        photo = ArtistPhoto::Grid {
            key,
            name: &name,
            circle,
        };
    }

    // genre · formed · country (source-gated to this artist — see `current_artist_info`)
    let info = app.current_artist_info();
    let meta = info
        .map(|i| {
            [i.genre.clone(), i.formed.clone(), i.country.clone()]
                .into_iter()
                .flatten()
                .collect()
        })
        .unwrap_or_default();

    let album_head = match track.year {
        Some(y) => format!("◇ {} · {y}", track.album),
        None => format!("◇ {}", track.album),
    };

    // the artist's most-played local tracks (by play count, then title)
    let mut top = Vec::new();
    if let Some(id) = track.artist_id {
        let mut tops: Vec<&crate::core::model::Track> = app
            .library
            .tracks_of_artist(id)
            .into_iter()
            .filter_map(|tid| app.library.track(tid))
            .collect();
        tops.sort_by(|a, b| {
            b.play_count
                .cmp(&a.play_count)
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        tops.truncate(TOP_TRACKS);
        top = tops
            .iter()
            .map(|t| ArtistTop {
                title: crate::arabic::shaped(&t.title, sh),
                plays: (t.play_count > 0).then_some(t.play_count),
            })
            .collect();
    }

    let bio = info.filter(|i| !i.bio.is_empty()).map(|i| i.bio.clone());

    artist_pane(
        f,
        area,
        app,
        focused,
        Some(ArtistContent {
            photo,
            name: crate::arabic::shaped(&name, sh),
            stat: None,
            meta,
            now_track: format!("♪ {}", crate::arabic::shaped(&track.title, sh)),
            now_album: Some(crate::arabic::shaped(&album_head, sh)),
            now_album_bold: true,
            top,
            bio,
            loading_label: "loading artist info…",
        }),
    );
}

/// Spotify artist pane: photo + name, genres · followers · formed · country, the
/// now-playing track, top tracks, and a bio. Spotify has no bio API, so bio/formed/
/// country come from the MusicBrainz+Wikipedia info worker (by artist name);
/// genres/followers/top-tracks from Spotify. Builds the content for `artist_pane`.
pub fn spotify_artist_panel(f: &mut Frame, area: Rect, app: &AppState, focused: bool) {
    let Some(tr) = &app.spov.now_spotify else {
        return artist_pane(f, area, app, focused, None);
    };
    let sh = app.config.arabic_shaping;
    let art = app.spov.sp_artist.as_ref();
    // source-gated wiki info — only this Spotify artist's, never the local pane's
    let info = app.current_artist_info();

    // a podcast episode has no artist: the SHOW stands in for it (name = show,
    // stat = publisher, "about" = show description), gated to the current show so a
    // stale fetch never leaks into the next episode.
    let is_episode = AppState::is_episode_uri(&tr.uri);
    let show = app
        .spov
        .sp_show_meta
        .as_ref()
        .filter(|m| Some(m.uri.as_str()) == tr.show_uri.as_deref());

    let name = if is_episode {
        Some(tr.album.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| tr.subtitle.clone())
    } else {
        art.map(|a| a.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| tr.subtitle.clone())
    };

    // Photo priority: the Web API artist image (music artists with a token) → for a
    // podcast episode there is no artist, so show the show's cover → otherwise a
    // keyless Deezer-by-name photo, so a token-less session still shows a face (parity
    // with the local pane). The last two go through the grid artwork cache (their own
    // `StatefulProtocol`), never reusing the now-bar cover's protocol, so nothing
    // thrashes a shared resize cache.
    // Each fallback fires its art request every frame (coalesced) but only claims the
    // photo box once the image is actually decoded (`art_ready`) — so an artist/show
    // with no online photo stays cleanly text-only instead of holding a placeholder
    // box open forever.
    let circle = app.config.grid_circle;
    let photo = if app.spov.sp_artist_cover.is_some() {
        ArtistPhoto::Cover(&app.spov.sp_artist_cover)
    } else if AppState::is_episode_uri(&tr.uri) {
        match tr.image.as_deref() {
            Some(url) => {
                let key = ArtKey::remote(url);
                // same circle/rounded shape as every other cover (config.grid_circle),
                // and the same shape the grid requests this URL with, so they share the
                // ArtKey::remote cache entry cleanly
                app.request_art(key, ArtSource::Url(url.to_string()), circle);
                grid_photo_when_ready(app, key, &name, circle)
            }
            None => ArtistPhoto::None,
        }
    } else if !name.is_empty() {
        let key = ArtKey::artist_name(&name);
        app.request_art(
            key,
            ArtSource::Artist {
                name: name.clone(),
                fallback: None,
            },
            circle,
        );
        grid_photo_when_ready(app, key, &name, circle)
    } else {
        ArtistPhoto::None
    };

    // podcast: the show's publisher as the headline; music: the real follower count
    // when the Web API provides it, else librespot's 0–100 popularity (the shared
    // dev-mode client id strips follower counts)
    let stat = if is_episode {
        show.map(|m| m.publisher.clone()).filter(|p| !p.is_empty())
    } else {
        art.and_then(|a| {
            if a.followers > 0 {
                Some(format!(
                    "{} followers",
                    crate::spotify::api::fmt_count(a.followers)
                ))
            } else if a.popularity > 0 {
                Some(format!("Popularity {}/100", a.popularity))
            } else {
                None
            }
        })
    };

    // podcast: release date (when `subtitle` is really a date, not the show name
    // librespot puts there) + episode length; music: genres · formed · country
    let mut meta: Vec<String> = Vec::new();
    if is_episode {
        if !tr.subtitle.is_empty() && tr.subtitle != tr.album {
            meta.push(tr.subtitle.clone());
        }
        let mins = tr.duration_ms / 60_000;
        if mins > 0 {
            meta.push(if mins >= 60 {
                format!("{}h {}m", mins / 60, mins % 60)
            } else {
                format!("{mins} min")
            });
        }
    } else {
        if let Some(a) = art.filter(|a| !a.genres.is_empty()) {
            meta.push(a.genres.clone());
        }
        if let Some(i) = info {
            if let Some(formed) = &i.formed {
                meta.push(formed.clone());
            }
            if let Some(country) = &i.country {
                meta.push(country.clone());
            }
        }
    }

    // for an episode the show name is already the pane title, so drop the redundant
    // ◇ album line; music keeps it
    let now_album = (!is_episode && !tr.album.is_empty())
        .then(|| format!("◇ {}", crate::arabic::shaped(&tr.album, sh)));

    let top = app
        .spov
        .sp_artist_top
        .iter()
        .map(|t| ArtistTop {
            title: crate::arabic::shaped(&t.name, sh),
            plays: None,
        })
        .collect();

    // podcast: the show's own "about" description; music: Spotify's official bio
    // (right-language) first, then the Wikipedia bio
    let bio = if is_episode {
        show.map(|m| m.description.clone())
            .filter(|d| !d.is_empty())
    } else {
        let spotify_bio = art.map(|a| a.bio.as_str()).filter(|b| !b.is_empty());
        let wiki_bio = info.map(|i| i.bio.as_str()).filter(|b| !b.is_empty());
        spotify_bio.or(wiki_bio).map(|b| b.to_string())
    };

    artist_pane(
        f,
        area,
        app,
        focused,
        Some(ArtistContent {
            photo,
            name: crate::arabic::shaped(&name, sh),
            stat,
            meta,
            now_track: format!("♪ {}", crate::arabic::shaped(&tr.name, sh)),
            now_album,
            now_album_bold: false,
            top,
            bio,
            loading_label: if is_episode {
                "loading show info…"
            } else {
                "loading bio…"
            },
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::photo_layout;
    use ratatui::layout::Rect;

    // a typical 1:2 (w:h px) terminal cell, like `image_font`'s fallback
    const FONT: (u16, u16) = (10, 20);

    #[test]
    fn photo_layout_skips_photo_when_pane_too_short() {
        let inner = Rect::new(0, 0, 30, 7);
        let (photo, text) = photo_layout(FONT, inner);
        assert!(photo.is_none(), "no photo below the 8-row floor");
        assert_eq!(text, inner, "the whole pane is text");
    }

    #[test]
    fn photo_layout_reserves_text_rows_and_fits_inside_the_pane() {
        let inner = Rect::new(2, 1, 40, 24);
        let (photo, text) = photo_layout(FONT, inner);
        let photo = photo.expect("a tall enough pane gets a photo");
        // photo stays inside the pane, leaves a 1-row gap, then the text region fills
        // the rest (≥ the reserved rows, less the gap)
        assert!(photo.x >= inner.x && photo.right() <= inner.right());
        assert_eq!(photo.y, inner.y);
        assert_eq!(text.y, photo.y + photo.height + 1);
        assert_eq!(text.height, inner.height - photo.height - 1);
        assert!(text.height >= 5, "the reserved text rows always remain");
        // square in pixels: width ≈ height × (cell_h / cell_w)
        assert_eq!(photo.width, photo.height * 2);
    }

    #[test]
    fn photo_layout_caps_a_wide_pane_so_text_survives() {
        // a very wide, shallow pane: the width-driven square would swamp the text, so
        // the height cap keeps the reserved rows (less the 1-row gap)
        let inner = Rect::new(0, 0, 120, 12);
        let (photo, text) = photo_layout(FONT, inner);
        let photo = photo.expect("photo present");
        assert!(text.height >= 5);
        assert!(photo.height <= inner.height - 6);
    }
}
