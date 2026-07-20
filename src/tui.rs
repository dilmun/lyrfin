//! Terminal runtime: init/restore (with panic-safe cleanup via `ratatui::init`)
//! and the draw → input → tick loop. Worker channels (audio/library) are merged
//! in here as their milestones land.

use std::time::{Duration, Instant};

use anyhow::Result;
use crossbeam_channel::{Receiver, unbounded};
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyCode as CKeyCode,
    KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use ratatui::crossterm::execute;

use crate::action::{Action, Motion};
use crate::app::AppState;
use crate::audio::engine::CpalEngine;
use crate::event::{Key, KeyCode, Mods};
use crate::library::store::UserData;
use crate::library::{LibraryEvent, scanner};
use crate::{keymap, ui};

/// Set the terminal's *default* background (OSC 11) to the theme colour, so any
/// window padding the terminal draws around the cell grid (e.g. Ghostty) matches
/// the UI instead of showing the original terminal background.
fn set_terminal_bg(bg: crate::ui::theme::Rgb) {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]11;#{:02x}{:02x}{:02x}\x1b\\", bg.0, bg.1, bg.2);
    let _ = out.flush();
}

/// Restore the terminal's default background (OSC 111) on exit.
fn reset_terminal_bg() {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = write!(out, "\x1b]111\x1b\\");
    let _ = out.flush();
}

/// Write an OSC 22 "set mouse pointer shape" sequence with `payload` (a CSS cursor
/// name like `ew-resize`, or empty to reset to the default). When running inside
/// tmux — which otherwise swallows escapes it doesn't recognise — the sequence is
/// wrapped in tmux's passthrough DCS (every inner `ESC` doubled) so it reaches the
/// outer terminal. Requires tmux `allow-passthrough on` (the default since 3.3a).
/// Terminals without OSC 22 (Terminal.app, older iTerm2) ignore it harmlessly.
fn write_osc22(payload: &str) {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = out.write_all(osc22_seq(payload, std::env::var_os("TMUX").is_some()).as_bytes());
    let _ = out.flush();
}

/// Build the OSC 22 byte sequence for `payload`, wrapping it in tmux's passthrough
/// DCS (with every inner `ESC` doubled) when `in_tmux`. Pure so the tmux escaping
/// can be unit-tested.
fn osc22_seq(payload: &str, in_tmux: bool) -> String {
    let osc = format!("\x1b]22;{payload}\x1b\\");
    if in_tmux {
        // tmux passthrough: ESC P tmux ; <osc, ESCs doubled> ESC \
        format!("\x1bPtmux;{}\x1b\\", osc.replace('\x1b', "\x1b\x1b"))
    } else {
        osc
    }
}

/// Swap the mouse pointer to a named CSS resize shape (`ew-resize` / `ns-resize`)
/// while a pane-resize drag is held — a live affordance, restored on release.
fn set_pointer_shape(shape: &str) {
    write_osc22(shape);
}

/// Restore the normal mouse pointer after a drag. Sends the explicit `default`
/// shape rather than an empty payload: Ghostty's OSC 22 ignores the empty-means-
/// reset convention (the resize arrow would otherwise stick after release).
fn reset_pointer_shape() {
    write_osc22("default");
}

pub fn run(mut app: AppState) -> Result<()> {
    // Start the system light/dark watcher (Linux D-Bus; no-op elsewhere) before the
    // palette query below, so its off-thread read has that window to publish the
    // current appearance — the follow-system startup apply then lands on the right
    // theme with no flash.
    crate::appearance::start_watcher();
    // The `auto` theme matches the terminal's own colours: ask the terminal for its
    // palette (OSC query, before we take over the screen) and build the theme from
    // the reply. Falls back to the default if the terminal can't answer in time.
    // Detect the terminal's own colours ONCE, before we take over the screen — so
    // the `auto` theme works whether it's the configured theme OR the user cycles to
    // it with `t` later. The running event loop owns stdin, so the OSC handshake
    // can't safely re-run mid-session; caching it here in `auto_theme` is what makes
    // a later switch to `auto` apply real colours instead of the fallback. A
    // responsive terminal answers in a few ms (300ms is only a busy-terminal cap).
    // The auto-palette query is needed when `auto` is the active single theme OR when
    // following the system and either the light or dark slot is `auto`.
    let want_auto = app.config.theme == "auto"
        || (app.config.theme_follows_system
            && (app.config.light_theme == "auto" || app.config.dark_theme == "auto"));
    let dir = app.config.dir.clone();
    // pass the config dir only when a probe is wanted, so a failed probe writes a
    // diagnostic for auto users but stays silent for everyone else
    let log_dir = want_auto.then_some(dir.as_path());
    app.auto_theme = crate::termquery::query(Duration::from_millis(300), log_dir).to_theme();
    if app.config.theme_follows_system {
        // Follow-system: apply the light/dark theme matching the current OS appearance
        // now, before the first draw (overrides the single `theme` resolved at init).
        app.applied_sys_theme = None;
        app.poll_system_appearance();
    } else if app.config.theme == "auto" {
        app.set_theme("auto");
    }

    // Query the terminal for image-protocol support BEFORE taking over the
    // screen (handles tmux + iTerm2/kitty/sixel; falls back to halfblocks). The
    // picker is always created so the album-art toggle can take effect live; the
    // `album_art` setting governs whether covers are actually loaded/rendered.
    {
        app.set_picker(build_picker());
        // tmux can't reliably forward large inline-image transmissions, so the UI
        // caps the biggest covers to a tmux-safe size — but only inside tmux.
        app.in_tmux = std::env::var_os("TMUX").is_some();
    }

    // Attach a real audio engine if a device is available (else NullEngine +
    // demo clock keep the UI working).
    match CpalEngine::new(app.config.volume) {
        Ok(engine) => app.attach_engine(Box::new(engine)),
        Err(e) => app.update(Action::Notify(format!("Audio disabled: {e}"))),
    }

    // Online artist/album info worker.
    let (info_tx, info_rx) = crate::artistinfo::spawn();
    app.set_info_sender(info_tx);

    // Online lyrics worker.
    let (lyr_tx, lyr_rx) = crate::lyricsfetch::spawn();
    app.set_lyrics_sender(lyr_tx);

    // Lyric translation worker (machine translation to the configured language).
    let (tr_tx, tr_rx) = crate::translate::spawn();
    app.set_translate_sender(tr_tx);

    // Podcast resolver (Spotify episode → public RSS MP3).
    let (pod_tx, pod_rx) = crate::podcastfetch::spawn();
    app.set_podcast_sender(pod_tx);

    // Album-art search/embed worker.
    let (cov_tx, cov_rx) = crate::coversearch::spawn();
    app.set_cover_sender(cov_tx);

    // Online tag/metadata search worker.
    let (tag_tx, tag_rx) = crate::tagsearch::spawn();
    app.set_tag_sender(tag_tx);

    // Internet-radio (Radio Browser) search worker.
    let (radio_tx, radio_rx) = crate::radio::spawn(app.config.dir.clone());
    app.set_radio_sender(radio_tx);

    // Spotify Web API worker (metadata / library / search).
    let (sp_tx, sp_rx) = crate::spotify::api::spawn();
    app.set_spotify_sender(sp_tx);

    let (spart_tx, spart_rx) = crate::spotify::artwork::spawn();
    app.set_spotify_art_sender(spart_tx);

    let (art_tx, art_rx) = crate::artwork::spawn();
    app.set_art_sender(art_tx);

    // Kick off a background (incremental) sync of the music dirs. The scanner
    // reuses unchanged files from the cached catalogue, so an unchanged library
    // syncs almost instantly; new/changed files are parsed, removed ones dropped.
    let (tx, rx) = unbounded::<LibraryEvent>();
    if !app.config.music_dirs.is_empty() {
        scanner::spawn(app.config.music_dirs.clone(), app.cache_map(), tx.clone());
    }

    let mut terminal = ratatui::init();
    if app.config.mouse {
        let _ = execute!(std::io::stdout(), EnableMouseCapture);
    }
    set_terminal_bg(app.theme.bg); // match terminal padding to the theme
    let result = event_loop(
        &mut terminal,
        &mut app,
        rx,
        tx,
        info_rx,
        lyr_rx,
        tr_rx,
        pod_rx,
        cov_rx,
        tag_rx,
        radio_rx,
        sp_rx,
        spart_rx,
        art_rx,
    );
    reset_terminal_bg();
    if app.config.mouse {
        let _ = execute!(std::io::stdout(), DisableMouseCapture);
    }
    ratatui::restore();

    // Persist user data (ratings/favorites/plays) + playlists + session + the
    // debounced listening history on exit.
    UserData::capture(&app.library).save(&app.config.dir);
    crate::library::store::PlaylistStore::from_library(&app.library).save(&app.config.dir);
    app.save_session();
    app.spotify_save_view_cache(); // instant Spotify view on next launch
    app.flush_history();
    result
}

#[allow(clippy::too_many_arguments)] // worker receiver channels
fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut AppState,
    lib_rx: Receiver<LibraryEvent>,
    lib_tx: crossbeam_channel::Sender<LibraryEvent>,
    info_rx: Receiver<crate::artistinfo::InfoResult>,
    lyr_rx: Receiver<crate::lyricsfetch::LyricsResult>,
    tr_rx: Receiver<crate::translate::TranslateResult>,
    pod_rx: Receiver<crate::podcastfetch::PodcastResult>,
    cov_rx: Receiver<crate::coversearch::CoverResult>,
    tag_rx: Receiver<crate::tagsearch::TagResult>,
    radio_rx: Receiver<crate::radio::RadioResult>,
    sp_rx: Receiver<crate::spotify::api::SpResult>,
    spart_rx: Receiver<crate::spotify::artwork::ArtResult>,
    art_rx: Receiver<crate::artwork::ArtResult>,
) -> Result<()> {
    // Two cadences: `frame` (the configured fps) drives ticks + redraws while
    // something is animating; `idle` is a slow heartbeat when nothing changes, so
    // a paused/static screen costs ~4 wakeups/sec instead of 60. Input wakes
    // `event::poll` immediately regardless of the timeout, so responsiveness is
    // unaffected.
    let frame = Duration::from_millis((1000 / app.config.fps.max(1) as u64).max(8));
    let idle = Duration::from_millis(250);
    let mut last = Instant::now();
    let mut last_click: Option<(u16, u16, Instant)> = None;
    // true while a pane-resize drag has swapped the mouse pointer shape (OSC 22),
    // so the release restores it exactly once (balanced push/pop).
    let mut resizing = false;
    let mut term_bg = app.theme.bg;
    // capture was enabled at startup to match `config.mouse`; track it so a runtime
    // toggle of the setting can be applied live (below).
    let mut mouse_on = app.config.mouse;
    // Tracks whether a modal overlay was open last iteration, to catch the
    // modal-close edge below.
    let mut modal_prev = false;
    // OS "Now Playing" bridge (macOS Control Center + media keys / AirPods, Linux
    // MPRIS). Owned here — not on `AppState` — so the platform handle (which is
    // `!Send` on macOS) stays out of the core state; it's driven each iteration
    // below. Inert when `os_media_controls` is off or on a platform with no backend.
    let mut media = crate::media::MediaBridge::new("lyrfin", app.config.os_media_controls);
    // Track the setting so a runtime toggle rebuilds the bridge (dropping the old
    // one tears down the OS entry), like the mouse-capture toggle below.
    let mut media_on = app.config.os_media_controls;
    while app.running {
        // keep the terminal's default background in sync with the theme
        if app.theme.bg != term_bg {
            set_terminal_bg(app.theme.bg);
            term_bg = app.theme.bg;
        }
        // Settings ▸ Mouse toggled at runtime → enable/disable capture immediately so
        // it takes effect without a restart (off releases the mouse for the
        // terminal's own text selection).
        if app.config.mouse != mouse_on {
            let _ = if app.config.mouse {
                execute!(std::io::stdout(), EnableMouseCapture)
            } else {
                execute!(std::io::stdout(), DisableMouseCapture)
            };
            mouse_on = app.config.mouse;
        }
        // Settings ▸ OS media controls toggled at runtime → rebuild the bridge so it
        // attaches/detaches immediately (dropping the old bridge removes the OS entry).
        if app.config.os_media_controls != media_on {
            media = crate::media::MediaBridge::new("lyrfin", app.config.os_media_controls);
            media_on = app.config.os_media_controls;
        }
        // Modal-close edge: a modal overlay that partly covered a persistent inline
        // cover leaves stale glyphs on the covered edge (ratatui-image v11 reuses the
        // Kitty image id, and Ghostty won't repaint occluded placeholder cells for an
        // unchanged id). Rebuild those covers with a fresh id so they re-place cleanly
        // — done before the draw so the repaint lands this frame.
        let modal_now = app.modal_open();
        if modal_prev && !modal_now {
            app.rebuild_persistent_covers();
        }
        modal_prev = modal_now;

        let animating = app.is_animating();
        // Only repaint when state changed or something is animating.
        if app.take_dirty() || animating {
            terminal.draw(|f| ui::render(f, app))?;
        }

        let period = if animating { frame } else { idle };
        let timeout = period.checked_sub(last.elapsed()).unwrap_or(Duration::ZERO);
        if event::poll(timeout)? {
            match event::read()? {
                CEvent::Key(k) if k.kind == KeyEventKind::Press => {
                    let action = keymap::map(app, convert_key(k));
                    app.update(action);
                }
                CEvent::Mouse(m) => match m.kind {
                    MouseEventKind::ScrollDown => app.handle_scroll(m.column, m.row, Motion::Down),
                    MouseEventKind::ScrollUp => app.handle_scroll(m.column, m.row, Motion::Up),
                    // two-finger horizontal touchpad scroll → left/right (terminals
                    // that report it; others simply never send these)
                    MouseEventKind::ScrollLeft => app.handle_scroll(m.column, m.row, Motion::Left),
                    MouseEventKind::ScrollRight => {
                        app.handle_scroll(m.column, m.row, Motion::Right)
                    }
                    MouseEventKind::Down(MouseButton::Left) => {
                        let now = Instant::now();
                        let double = last_click
                            .map(|(lx, ly, t)| {
                                lx == m.column
                                    && ly == m.row
                                    && now.duration_since(t) < Duration::from_millis(400)
                            })
                            .unwrap_or(false);
                        // grabbing a pane edge/divider starts a resize drag instead
                        // of a click — and swaps the mouse pointer to a resize arrow
                        // (OSC 22) as a live affordance, restored on release.
                        if app.begin_pane_resize(m.column, m.row) {
                            if let Some(shape) = app.resize_pointer_shape() {
                                set_pointer_shape(shape);
                                resizing = true;
                            }
                        } else {
                            app.handle_click(m.column, m.row, double);
                        }
                        last_click = Some((m.column, m.row, now));
                    }
                    MouseEventKind::Drag(MouseButton::Left) => {
                        // an active edge drag resizes the pane; otherwise scrub seek/volume
                        if !app.drag_pane_resize(m.column, m.row) {
                            app.handle_drag(m.column, m.row);
                        }
                    }
                    MouseEventKind::Up(MouseButton::Left) => {
                        app.end_pane_resize();
                        if resizing {
                            reset_pointer_shape(); // restore the pointer the drag replaced
                            resizing = false;
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
            app.mark_dirty(); // any handled input → repaint next iteration
        }
        // drain worker messages — a result means visible state may have changed
        let mut got = false;
        while let Ok(ev) = lib_rx.try_recv() {
            app.on_library_event(ev);
            got = true;
        }
        while let Ok(info) = info_rx.try_recv() {
            app.on_info_result(info);
            got = true;
        }
        while let Ok(pod) = pod_rx.try_recv() {
            app.on_podcast_result(pod);
        }
        while let Ok(lyr) = lyr_rx.try_recv() {
            app.on_lyrics_result(lyr);
            got = true;
        }
        while let Ok(tr) = tr_rx.try_recv() {
            app.on_translate_result(tr);
            got = true;
        }
        while let Ok(cov) = cov_rx.try_recv() {
            app.on_cover_result(cov);
            got = true;
        }
        while let Ok(tag) = tag_rx.try_recv() {
            app.on_tag_result(tag);
            got = true;
        }
        while let Ok(sp) = sp_rx.try_recv() {
            app.on_spotify_result(sp);
            got = true;
        }
        while let Ok(art) = spart_rx.try_recv() {
            app.on_spotify_art(art);
            got = true;
        }
        while let Ok(thumb) = art_rx.try_recv() {
            app.on_art_result(thumb);
            got = true;
        }
        while let Ok(st) = radio_rx.try_recv() {
            app.on_radio_result(st);
            got = true;
        }
        // OS media controls: service the platform run loop (macOS) so its command
        // callbacks fire, then apply any transport command it delivered.
        media.pump();
        while let Some(cmd) = media.poll_command() {
            app.on_media_command(cmd);
            got = true;
        }
        if got {
            app.mark_dirty();
        }
        // a settings change or [R] can request a re-sync at runtime
        if let Some(dirs) = app.take_rescan()
            && !dirs.is_empty()
        {
            scanner::spawn(dirs, app.cache_map(), lib_tx.clone());
        }
        app.pump_audio(); // sets dirty itself on audio events
        app.pump_spotify(); // drain Spotify auth/resume events
        if last.elapsed() >= period {
            app.set_frame_dt(last.elapsed()); // real cadence drives the position clocks
            app.update(Action::Tick);
            last = Instant::now();
        }
        // Mirror the current playback to the OS "Now Playing" surface. `publish`
        // diffs against the last push, so it only actually talks to the OS on a
        // track / play-state / seek change — never per frame.
        media.publish(app.now_playing_snapshot().as_ref());
    }
    Ok(())
}

/// Does this terminal render the **iTerm2 inline-image protocol**? Covers iTerm2
/// itself and WezTerm, which implements the same protocol.
///
/// Each is identified by a signal that survives a multiplexer, because `tmux`
/// overwrites `TERM_PROGRAM` with its own name: `LC_TERMINAL` for iTerm2,
/// `WEZTERM_EXECUTABLE` for WezTerm. Without that, detection inside tmux falls to
/// ratatui-image's own comment-documented "risky guess" at the outer terminal —
/// which happens to land on iTerm2 today, but is a heuristic, not identification.
fn wants_iterm2_protocol() -> bool {
    iterm2_protocol_from(
        std::env::var("TERM_PROGRAM").ok().as_deref(),
        std::env::var("LC_TERMINAL").ok().as_deref(),
        std::env::var("WEZTERM_EXECUTABLE").ok().as_deref(),
    )
}

/// Pure half of [`wants_iterm2_protocol`], so the env-var precedence is
/// unit-testable.
fn iterm2_protocol_from(
    term_program: Option<&str>,
    lc_terminal: Option<&str>,
    wezterm_exe: Option<&str>,
) -> bool {
    term_program.is_some_and(|p| p.contains("iTerm") || p.contains("WezTerm"))
        || lc_terminal.is_some_and(|t| t.contains("iTerm"))
        || wezterm_exe.is_some_and(|w| !w.is_empty())
}

/// Resolve the inline-image protocol for this terminal.
///
/// The stdin handshake is authoritative *except* on terminals that answer the
/// Kitty support query affirmatively but don't implement the half of the Kitty
/// protocol ratatui-image actually renders with — **unicode placeholders**. On
/// those, images transmit successfully and are then never placed: album art
/// silently renders as nothing at all (not even a fallback), while the rest of
/// the UI looks perfect.
///
/// iTerm2 ≥ 3.6 is exactly that case — it replies `ESC _Gi=31;OK ESC \` to the
/// Kitty query, so detection picks Kitty and every cover disappears. Its own
/// (iTerm2) protocol renders correctly, so we override the queried protocol
/// there. ratatui-image handles WezTerm and Konsole (same broken-placeholder
/// story) via `QueryStdioOptions::blacklist_protocols`, but that route doesn't
/// work for iTerm2: its device attributes also advertise sixel, so removing
/// Kitty alone selects Sixel — a palette-quantised renderer with binary rather
/// than alpha transparency, which the round cover art depends on — and removing
/// both makes the query fail outright, falling back to halfblocks *and* losing
/// the queried font size (which sizes every image rect). Overriding after a
/// normal query keeps that font size and picks the one protocol that works.
fn build_picker() -> ratatui_image::picker::Picker {
    use ratatui_image::picker::{Picker, ProtocolType};

    let mut picker = Picker::from_query_stdio().unwrap_or_else(|_| guess_picker());
    if wants_iterm2_protocol() {
        picker.set_protocol_type(ProtocolType::Iterm2);
    }

    // Escape hatch for terminals whose self-reported capabilities are wrong (or
    // that we haven't characterised yet): LYRFIN_IMAGE_PROTOCOL=kitty|iterm2|
    // sixel|halfblocks. Applied over the detected picker so the queried font
    // size — which sizes every image rect — is preserved.
    if let Some(proto) = forced_protocol() {
        picker.set_protocol_type(proto);
    }
    picker
}

/// The `LYRFIN_IMAGE_PROTOCOL` override, if set to a name we recognise.
/// An unset/empty/`auto`/unknown value means "trust detection".
fn forced_protocol() -> Option<ratatui_image::picker::ProtocolType> {
    protocol_from_name(&std::env::var("LYRFIN_IMAGE_PROTOCOL").ok()?)
}

/// Pure half of [`forced_protocol`], so the accepted names are unit-testable.
fn protocol_from_name(name: &str) -> Option<ratatui_image::picker::ProtocolType> {
    use ratatui_image::picker::ProtocolType;
    match name.trim().to_ascii_lowercase().as_str() {
        "kitty" => Some(ProtocolType::Kitty),
        "iterm2" => Some(ProtocolType::Iterm2),
        "sixel" => Some(ProtocolType::Sixel),
        "halfblocks" => Some(ProtocolType::Halfblocks),
        _ => None,
    }
}

/// Fallback when the terminal's protocol handshake fails — it's a racy stdin
/// round-trip, so on an unlucky run it errors and we'd otherwise drop to blocky
/// halfblocks. Guess the graphics protocol from the environment instead: Ghostty
/// and Kitty speak the Kitty protocol; iTerm2 and WezTerm speak iTerm2. Only
/// genuinely unknown terminals fall back to halfblocks.
fn guess_picker() -> ratatui_image::picker::Picker {
    use ratatui_image::picker::{Picker, ProtocolType};
    // `halfblocks()` seeds the same 10×20 fallback font size and tmux detection;
    // the protocol type is then set explicitly from the environment below.
    let mut p = Picker::halfblocks();
    let term = std::env::var("TERM").unwrap_or_default();
    let prog = std::env::var("TERM_PROGRAM").unwrap_or_default();
    // iTerm2 first: it also answers the Kitty query, but only its own protocol
    // actually places images (see `build_picker`).
    if wants_iterm2_protocol() {
        p.set_protocol_type(ProtocolType::Iterm2);
    } else if std::env::var_os("KITTY_WINDOW_ID").is_some()
        || std::env::var_os("GHOSTTY_RESOURCES_DIR").is_some()
        || prog.eq_ignore_ascii_case("ghostty")
        || term.contains("kitty")
        || term.contains("ghostty")
    {
        p.set_protocol_type(ProtocolType::Kitty);
    }
    p
}

fn convert_key(k: ratatui::crossterm::event::KeyEvent) -> Key {
    let code = match k.code {
        CKeyCode::Char(c) => KeyCode::Char(c),
        CKeyCode::Enter => KeyCode::Enter,
        CKeyCode::Esc => KeyCode::Esc,
        CKeyCode::Tab => KeyCode::Tab,
        CKeyCode::BackTab => KeyCode::BackTab,
        CKeyCode::Backspace => KeyCode::Backspace,
        CKeyCode::Delete => KeyCode::Delete,
        CKeyCode::Left => KeyCode::Left,
        CKeyCode::Right => KeyCode::Right,
        CKeyCode::Up => KeyCode::Up,
        CKeyCode::Down => KeyCode::Down,
        CKeyCode::Home => KeyCode::Home,
        CKeyCode::End => KeyCode::End,
        CKeyCode::PageUp => KeyCode::PageUp,
        CKeyCode::PageDown => KeyCode::PageDown,
        CKeyCode::F(n) => KeyCode::F(n),
        _ => KeyCode::Char('\0'),
    };
    let m = k.modifiers;
    Key {
        code,
        mods: Mods {
            ctrl: m.contains(KeyModifiers::CONTROL),
            alt: m.contains(KeyModifiers::ALT),
            shift: m.contains(KeyModifiers::SHIFT),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{iterm2_protocol_from, osc22_seq, protocol_from_name};
    use ratatui_image::picker::ProtocolType;

    #[test]
    fn detects_iterm2_from_term_program() {
        assert!(iterm2_protocol_from(Some("iTerm.app"), None, None));
    }

    #[test]
    fn detects_iterm2_through_tmux_via_lc_terminal() {
        // The regression this guards: inside tmux, TERM_PROGRAM is overwritten
        // with "tmux", so LC_TERMINAL is the only surviving iTerm2 signal.
        assert!(iterm2_protocol_from(Some("tmux"), Some("iTerm2"), None));
    }

    /// WezTerm speaks the same protocol, and `WEZTERM_EXECUTABLE` survives tmux —
    /// without it, detection inside tmux relies on upstream guessing the outer
    /// terminal rather than identifying WezTerm at all.
    #[test]
    fn detects_wezterm_through_tmux() {
        assert!(iterm2_protocol_from(Some("WezTerm"), None, None));
        assert!(iterm2_protocol_from(
            Some("tmux"),
            None,
            Some("/opt/homebrew/bin/wezterm-gui")
        ));
        assert!(!iterm2_protocol_from(Some("tmux"), None, Some("")));
    }

    #[test]
    fn other_terminals_are_not_iterm2() {
        assert!(!iterm2_protocol_from(Some("ghostty"), None, None));
        assert!(!iterm2_protocol_from(None, None, None));
    }

    #[test]
    fn protocol_override_names() {
        assert_eq!(protocol_from_name("iterm2"), Some(ProtocolType::Iterm2));
        assert_eq!(protocol_from_name("Kitty"), Some(ProtocolType::Kitty));
        assert_eq!(protocol_from_name(" sixel "), Some(ProtocolType::Sixel));
        assert_eq!(
            protocol_from_name("halfblocks"),
            Some(ProtocolType::Halfblocks)
        );
    }

    #[test]
    fn unknown_protocol_override_falls_back_to_detection() {
        assert_eq!(protocol_from_name(""), None);
        assert_eq!(protocol_from_name("auto"), None);
        assert_eq!(protocol_from_name("nonsense"), None);
    }

    #[test]
    fn osc22_is_raw_outside_tmux() {
        // plain OSC 22 set (the form Ghostty documents) + explicit `default` reset
        assert_eq!(osc22_seq("ew-resize", false), "\x1b]22;ew-resize\x1b\\");
        assert_eq!(osc22_seq("default", false), "\x1b]22;default\x1b\\");
    }

    #[test]
    fn osc22_is_tmux_wrapped_with_doubled_escapes() {
        // inside tmux the sequence is wrapped in the passthrough DCS and every
        // inner ESC (0x1b) is doubled, so tmux forwards it to the outer terminal
        assert_eq!(
            osc22_seq("ns-resize", true),
            "\x1bPtmux;\x1b\x1b]22;ns-resize\x1b\x1b\\\x1b\\"
        );
    }
}
