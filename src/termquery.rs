//! Read the terminal's own colours at startup so the `auto` theme can match the
//! user's terminal colorscheme.
//!
//! We ask the terminal directly with OSC queries — `OSC 10` (default foreground),
//! `OSC 11` (default background), and `OSC 4;n` (ANSI palette entry `n`) — then a
//! `CSI c` (Primary Device Attributes) query as a sentinel, since every terminal
//! answers DA. We do NOT assume strict ordering (the DA reply can race ahead of
//! the colour replies): we stop once the colours we need are parsed, or once the
//! essentials are in and DA has arrived — so a fast terminal returns in a few ms
//! without waiting out the timeout. A terminal that doesn't support the colour
//! queries never answers them, and we fall back.
//!
//! Reading the replies needs raw mode (so bytes aren't line-buffered) and a
//! timeout (so an unsupported terminal can't hang startup); we restore the prior
//! raw-mode state afterwards. Unix only — the one target that matters (Ghostty on
//! macOS/Linux) is Unix; elsewhere this is a no-op and `auto` falls back.

use crate::ui::theme::TerminalPalette;
// `Rgb` is only referenced by the unix query path and the parse helpers/tests
// below; keep its import on the same cfg so the non-unix binary has no unused
// import under `-D warnings`.
#[cfg(any(unix, test))]
use crate::ui::theme::Rgb;

/// Query the terminal for its foreground, background, and 16 ANSI colours. Returns
/// an empty palette (all `None`) if the terminal can't answer within `timeout` or
/// the platform can't read the tty; the caller falls back to a default theme.
#[cfg(unix)]
pub fn query(timeout: std::time::Duration, log_dir: Option<&std::path::Path>) -> TerminalPalette {
    use std::io::Write;

    use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode, is_raw_mode_enabled};

    // Replies arrive on stdin a byte at a time — needs raw mode. Leave it as we
    // found it (ratatui::init enables it later for the real UI).
    let was_raw = is_raw_mode_enabled().unwrap_or(false);
    if !was_raw && enable_raw_mode().is_err() {
        return TerminalPalette::default();
    }

    // fg, bg, the 16 palette colours, then the DA sentinel.
    let mut q = String::from("\x1b]10;?\x1b\\\x1b]11;?\x1b\\");
    for n in 0..16 {
        q.push_str(&format!("\x1b]4;{n};?\x1b\\"));
    }
    q.push_str("\x1b[c");
    {
        let mut out = std::io::stdout();
        let _ = out.write_all(q.as_bytes());
        let _ = out.flush();
    }

    let buf = drain_replies(timeout, |buf| {
        // Decide completion from the PARSED colours, not the DA sentinel alone: a
        // terminal can send its DA reply BEFORE some colour replies, so breaking the
        // instant DA appears would drop them (the intermittent "auto went dark"
        // bug). Stop once every colour is in, or once the essentials (fg + bg, all
        // `to_theme` needs) are in AND the terminal has signalled it's done (DA).
        let p = parse(buf);
        let essentials = p.fg.is_some() && p.bg.is_some();
        let complete = essentials && p.ansi.iter().all(Option::is_some);
        complete || (essentials && has_da_reply(buf))
    });

    if !was_raw {
        let _ = disable_raw_mode();
    }
    let pal = parse(&buf);
    // Record a diagnostic only when the user is actually on `auto` (log_dir is
    // Some) AND detection FAILED — so the startup probe never litters a log for
    // someone who isn't using the auto theme, and a success leaves no file behind.
    if let Some(dir) = log_dir
        && pal.to_theme().is_none()
    {
        write_debug(dir, &buf, &pal);
    }
    pal
}

/// Read stdin until `is_done` accepts what's arrived or `timeout` elapses.
/// Raw mode must already be on (replies arrive unbuffered, byte by byte). Shared
/// by the palette query and [`terminal_name`].
#[cfg(unix)]
fn drain_replies(timeout: std::time::Duration, is_done: impl Fn(&[u8]) -> bool) -> Vec<u8> {
    let deadline = std::time::Instant::now() + timeout;
    let mut buf: Vec<u8> = Vec::with_capacity(1024);
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let ms = remaining.as_millis().min(i32::MAX as u128) as i32;
        let mut pfd = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pfd is a valid, initialized pollfd for the lifetime of the call.
        let ready = unsafe { libc::poll(&mut pfd, 1, ms) };
        if ready <= 0 || pfd.revents & libc::POLLIN == 0 {
            break;
        }
        let mut chunk = [0u8; 1024];
        // SAFETY: reading up to chunk.len() bytes into the owned buffer.
        let n = unsafe {
            libc::read(
                libc::STDIN_FILENO,
                chunk.as_mut_ptr() as *mut libc::c_void,
                chunk.len(),
            )
        };
        if n <= 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n as usize]);
        if is_done(&buf) {
            break;
        }
    }
    buf
}

/// Ask the terminal what it *is* (XTVERSION, `CSI > q`) and return its self-
/// reported name, e.g. `"iTerm2 3.6.11"`, `"WezTerm 20260623-..."`, `"ghostty ..."`.
///
/// This exists because environment variables cannot identify the terminal under a
/// multiplexer: tmux overwrites `TERM_PROGRAM` with its own name, and the tmux
/// *server* hands every pane the environment of whichever terminal happened to
/// start it — so a session started from one terminal and attached from another
/// reports the wrong one, indefinitely. Asking over the wire always describes the
/// terminal actually attached right now.
///
/// Inside tmux the query is wrapped in tmux's passthrough DCS (verified to reach
/// the outer terminal and return its real reply), which needs `allow-passthrough`
/// — on by default since tmux 3.3a. Terminals without XTVERSION simply never
/// answer and we return `None`, so every caller must have an env-based fallback.
#[cfg(unix)]
pub fn terminal_name(timeout: std::time::Duration) -> Option<String> {
    use std::io::Write;

    use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode, is_raw_mode_enabled};

    let was_raw = is_raw_mode_enabled().unwrap_or(false);
    if !was_raw && enable_raw_mode().is_err() {
        return None;
    }
    let q = xtversion_query(std::env::var_os("TMUX").is_some());
    {
        let mut out = std::io::stdout();
        let _ = out.write_all(q.as_bytes());
        let _ = out.flush();
    }
    // The reply is a DCS string terminated by ST; stop as soon as it's whole.
    let buf = drain_replies(timeout, |b| parse_xtversion(b).is_some());
    if !was_raw {
        let _ = disable_raw_mode();
    }
    parse_xtversion(&buf)
}

#[cfg(not(unix))]
pub fn terminal_name(_timeout: std::time::Duration) -> Option<String> {
    None
}

/// Build the XTVERSION query, wrapped in tmux passthrough when inside tmux (every
/// inner `ESC` doubled). Pure, so the escaping is unit-testable.
#[cfg(any(unix, test))]
fn xtversion_query(in_tmux: bool) -> String {
    let q = "\x1b[>q";
    if in_tmux {
        format!("\x1bPtmux;{}\x1b\\", q.replace('\x1b', "\x1b\x1b"))
    } else {
        q.to_string()
    }
}

/// Extract the name from an XTVERSION reply: `DCS > | <name> ST`, where ST is
/// either `ESC \` or BEL. Returns `None` until the whole reply has arrived, which
/// is what lets the read loop stop at exactly the right moment.
#[cfg(any(unix, test))]
fn parse_xtversion(buf: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(buf);
    let start = s.find("\x1bP>|")? + 4;
    let rest = &s[start..];
    let end = rest.find("\x1b\\").or_else(|| rest.find('\x07'))?;
    let name = rest[..end].trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Measure the terminal cell size in the units its *image protocol* uses, by
/// asking for the text area both ways and dividing:
///
/// * `CSI 14t` -> text area in pixels
/// * `CSI 18t` -> text area in characters
///
/// Why not just use the cell size from `CSI 16t`, which ratatui-image already
/// queries? Because on a HiDPI display the two disagree: iTerm2 answers `16t` in
/// *physical* pixels (22x46 on a 2x Retina panel) while sizing inline images in
/// *points* (11x23) — so every image is transmitted at twice its intended size and
/// spills far outside the cells it was given. Deriving the cell from `14t / 18t`
/// keeps us in one unit system, whichever that turns out to be.
///
/// Returns `None` unless both replies arrive and divide cleanly, so a terminal
/// that answers neither (or only one) keeps ratatui-image's own detection.
#[cfg(unix)]
pub fn measured_cell_size(timeout: std::time::Duration) -> Option<(u16, u16)> {
    use std::io::Write;

    use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode, is_raw_mode_enabled};

    let was_raw = is_raw_mode_enabled().unwrap_or(false);
    if !was_raw && enable_raw_mode().is_err() {
        return None;
    }
    {
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x1b[14t\x1b[18t");
        let _ = out.flush();
    }
    let buf = drain_replies(timeout, |b| parse_cell_size(b).is_some());
    if !was_raw {
        let _ = disable_raw_mode();
    }
    parse_cell_size(&buf)
}

#[cfg(not(unix))]
pub fn measured_cell_size(_timeout: std::time::Duration) -> Option<(u16, u16)> {
    None
}

/// Pull `CSI 4 ; h ; w t` (pixels) and `CSI 8 ; rows ; cols t` (characters) out of
/// a reply buffer and divide them into a cell size. Pure, so it is unit-testable.
#[cfg(any(unix, test))]
fn parse_cell_size(buf: &[u8]) -> Option<(u16, u16)> {
    let s = String::from_utf8_lossy(buf);
    let triple = |lead: &str| -> Option<(u32, u32)> {
        let at = s.find(lead)? + lead.len();
        let rest = &s[at..];
        let end = rest.find('t')?;
        let mut it = rest[..end].split(';');
        Some((
            it.next()?.trim().parse().ok()?,
            it.next()?.trim().parse().ok()?,
        ))
    };
    let (px_h, px_w) = triple("\x1b[4;")?;
    let (rows, cols) = triple("\x1b[8;")?;
    if rows == 0 || cols == 0 {
        return None;
    }
    let (cw, ch) = (px_w / cols, px_h / rows);
    // A degenerate answer (some terminals report 0, or a 1px cell) is worse than
    // no answer — leave detection alone rather than sizing every image from it.
    (cw >= 2 && ch >= 2).then_some((cw as u16, ch as u16))
}

/// Write a diagnostic of a FAILED `auto` query to `autotheme.log`: the raw reply
/// (escaped) and what we parsed from it. Lets a terminal that won't answer be told
/// apart from a parse gap without guessing.
#[cfg(unix)]
fn write_debug(dir: &std::path::Path, raw: &[u8], pal: &TerminalPalette) {
    let mut s = format!("raw {} bytes: ", raw.len());
    for &b in raw {
        match b {
            0x1b => s.push_str("\\e"),
            0x20..=0x7e => s.push(b as char),
            other => s.push_str(&format!("\\x{other:02x}")),
        }
    }
    let hex = |c: Option<Rgb>| {
        c.map(|r| format!("#{:02x}{:02x}{:02x}", r.0, r.1, r.2))
            .unwrap_or_else(|| "none".into())
    };
    s.push_str(&format!("\nfg: {}  bg: {}\n", hex(pal.fg), hex(pal.bg)));
    for (i, c) in pal.ansi.iter().enumerate() {
        s.push_str(&format!("ansi[{i:>2}]: {}\n", hex(*c)));
    }
    s.push_str(&format!("theme built: {}\n", pal.to_theme().is_some()));
    let _ = std::fs::write(dir.join("autotheme.log"), s);
}

#[cfg(not(unix))]
pub fn query(_timeout: std::time::Duration, _log_dir: Option<&std::path::Path>) -> TerminalPalette {
    TerminalPalette::default()
}

/// Does `buf` contain a Primary Device Attributes reply (`CSI ? … c`)? Its arrival
/// signals the terminal has finished answering the colour queries sent before it.
#[cfg(any(unix, test))]
fn has_da_reply(buf: &[u8]) -> bool {
    let mut i = 0;
    while i + 2 < buf.len() {
        if buf[i] == 0x1b && buf[i + 1] == b'[' && buf[i + 2] == b'?' {
            for &c in &buf[i + 3..] {
                if c == b'c' {
                    return true;
                }
                if !(c.is_ascii_digit() || c == b';') {
                    break;
                }
            }
        }
        i += 1;
    }
    false
}

/// Extract the OSC colour replies from a raw response buffer. Each reply is an OSC
/// string (`ESC ] … BEL` or `ESC ] … ESC \`) whose payload is `10;<spec>` (fg),
/// `11;<spec>` (bg), or `4;<n>;<spec>` (palette entry).
#[cfg(any(unix, test))]
fn parse(buf: &[u8]) -> TerminalPalette {
    let mut pal = TerminalPalette::default();
    let mut i = 0;
    while i + 1 < buf.len() {
        if buf[i] == 0x1b && buf[i + 1] == b']' {
            let start = i + 2;
            let mut end = None;
            let mut j = start;
            while j < buf.len() {
                if buf[j] == 0x07 {
                    end = Some(j);
                    break;
                }
                if buf[j] == 0x1b && j + 1 < buf.len() && buf[j + 1] == b'\\' {
                    end = Some(j);
                    break;
                }
                j += 1;
            }
            let Some(e) = end else { break };
            if let Ok(payload) = std::str::from_utf8(&buf[start..e]) {
                interpret(payload, &mut pal);
            }
            i = e + 1;
        } else {
            i += 1;
        }
    }
    pal
}

#[cfg(any(unix, test))]
fn interpret(payload: &str, pal: &mut TerminalPalette) {
    if let Some(spec) = payload.strip_prefix("10;") {
        pal.fg = parse_color(spec);
    } else if let Some(spec) = payload.strip_prefix("11;") {
        pal.bg = parse_color(spec);
    } else if let Some(rest) = payload.strip_prefix("4;")
        && let Some((idx, spec)) = rest.split_once(';')
        && let Ok(n) = idx.parse::<usize>()
        && n < 16
    {
        pal.ansi[n] = parse_color(spec);
    }
}

/// Parse an X color spec: `rgb:RR/GG/BB` or `rgb:RRRR/GGGG/BBBB` (1–4 hex digits
/// per channel, scaled to 8 bits), or a `#RRGGBB` / `#RRRRGGGGBBBB` triplet.
#[cfg(any(unix, test))]
fn parse_color(spec: &str) -> Option<Rgb> {
    let spec = spec.trim();
    if let Some(hex) = spec.strip_prefix('#') {
        return match hex.len() {
            6 => Some(Rgb(hx(&hex[0..2])?, hx(&hex[2..4])?, hx(&hex[4..6])?)),
            12 => Some(Rgb(hx(&hex[0..2])?, hx(&hex[4..6])?, hx(&hex[8..10])?)),
            _ => None,
        };
    }
    let rest = spec
        .strip_prefix("rgb:")
        .or_else(|| spec.strip_prefix("rgba:"))?;
    let mut it = rest.split('/');
    let r = channel(it.next()?)?;
    let g = channel(it.next()?)?;
    let b = channel(it.next()?)?;
    Some(Rgb(r, g, b))
}

/// One `rgb:` channel of 1–4 hex digits, scaled to 8 bits (the high byte).
#[cfg(any(unix, test))]
fn channel(h: &str) -> Option<u8> {
    let h = h.trim();
    let v = u16::from_str_radix(h, 16).ok()?;
    Some(match h.len() {
        1 => (v * 0x11) as u8, // 4-bit → 8-bit (0xN → 0xNN)
        2 => v as u8,          // already 8-bit
        3 => (v >> 4) as u8,   // 12-bit → 8-bit
        4 => (v >> 8) as u8,   // 16-bit → 8-bit
        _ => return None,
    })
}

/// A two-digit hex byte.
#[cfg(any(unix, test))]
fn hx(s: &str) -> Option<u8> {
    u8::from_str_radix(s, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derives_cell_size_from_area_replies() {
        // real capture from iTerm2 on a 2x Retina panel: 1066x612 px over 96x25
        // cells -> 11x24, while CSI 16t reports 22x46 (physical pixels).
        let reply = b"\x1b[4;612;1066t\x1b[8;25;96t";
        assert_eq!(parse_cell_size(reply), Some((11, 24)));
    }

    #[test]
    fn cell_size_needs_both_replies() {
        assert_eq!(parse_cell_size(b"\x1b[4;612;1066t"), None);
        assert_eq!(parse_cell_size(b"\x1b[8;25;96t"), None);
        assert_eq!(parse_cell_size(b""), None);
    }

    #[test]
    fn degenerate_cell_size_is_rejected() {
        // a 0-cell or 1px answer would mis-size every image — worse than no answer
        assert_eq!(parse_cell_size(b"\x1b[4;612;1066t\x1b[8;0;0t"), None);
        assert_eq!(parse_cell_size(b"\x1b[4;25;96t\x1b[8;25;96t"), None);
    }

    #[test]
    fn parses_xtversion_reply() {
        assert_eq!(
            parse_xtversion(b"\x1bP>|iTerm2 3.6.11\x1b\\"),
            Some("iTerm2 3.6.11".to_string())
        );
        // BEL is a valid string terminator too
        assert_eq!(
            parse_xtversion(b"\x1bP>|WezTerm 20260623\x07"),
            Some("WezTerm 20260623".to_string())
        );
    }

    #[test]
    fn xtversion_incomplete_reply_is_none() {
        // no terminator yet -> keep reading rather than truncating the name
        assert_eq!(parse_xtversion(b"\x1bP>|iTerm2 3.6"), None);
        assert_eq!(parse_xtversion(b""), None);
    }

    #[test]
    fn xtversion_query_is_tmux_wrapped() {
        assert_eq!(xtversion_query(false), "\x1b[>q");
        // inside tmux: passthrough DCS with every inner ESC doubled
        assert_eq!(xtversion_query(true), "\x1bPtmux;\x1b\x1b[>q\x1b\\");
    }

    #[test]
    fn parses_fg_bg_and_palette_from_a_response() {
        // fg (OSC 10), bg (OSC 11, BEL-terminated), and palette 4 (blue).
        let buf = b"\x1b]10;rgb:e7e7/eaea/f4f4\x1b\\\x1b]11;rgb:0a0a/0c0c/1414\x07\x1b]4;4;rgb:7e7e/8c8c/f7f7\x1b\\";
        let p = parse(buf);
        assert_eq!(p.fg, Some(Rgb(0xe7, 0xea, 0xf4)));
        assert_eq!(p.bg, Some(Rgb(0x0a, 0x0c, 0x14)));
        assert_eq!(p.ansi[4], Some(Rgb(0x7e, 0x8c, 0xf7)));
        assert_eq!(p.ansi[5], None);
    }

    #[test]
    fn parses_two_digit_and_hash_specs() {
        assert_eq!(parse_color("rgb:1a/1b/26"), Some(Rgb(0x1a, 0x1b, 0x26)));
        assert_eq!(parse_color("#1a1b26"), Some(Rgb(0x1a, 0x1b, 0x26)));
        assert_eq!(
            parse_color("rgb:1a2a/1b2b/263a"),
            Some(Rgb(0x1a, 0x1b, 0x26))
        );
    }

    #[test]
    fn da_reply_is_detected() {
        assert!(has_da_reply(b"junk\x1b[?62;1;6c"));
        assert!(!has_da_reply(b"\x1b]11;rgb:0a/0c/14\x07"));
    }

    #[test]
    fn into_theme_needs_fg_and_bg() {
        // just an ANSI colour, no fg/bg → can't build a theme
        let mut p = TerminalPalette::default();
        p.ansi[4] = Some(Rgb(0x7e, 0x8c, 0xf7));
        assert!(p.to_theme().is_none());

        p.fg = Some(Rgb(0xe7, 0xea, 0xf4));
        p.bg = Some(Rgb(0x0a, 0x0c, 0x14));
        let t = p.to_theme().expect("fg+bg is enough");
        assert_eq!(t.name, "auto");
        assert_eq!(t.bg, Rgb(0x0a, 0x0c, 0x14));
        assert_eq!(t.text, Rgb(0xe7, 0xea, 0xf4));
        assert_eq!(t.accent[1], Rgb(0x7e, 0x8c, 0xf7), "blue drives accent mid");
    }
}
