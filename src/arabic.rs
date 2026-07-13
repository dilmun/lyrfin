//! Minimal Arabic shaping for the terminal: join letters into their contextual
//! presentation forms (Arabic Presentation Forms-B) and reorder runs for RTL
//! display. Terminals render code points left-to-right with no shaping, so raw
//! Arabic looks reversed and "disconnected" — this makes it readable.
//!
//! It's intentionally small: a single-letter joiner + lam-alef ligatures + a
//! light bidi pass (RTL base, with embedded Latin/number runs kept in order).
//! Not a full UAX#9 implementation, but good for names and prose.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Does the text contain any Arabic-script characters?
pub fn contains_arabic(s: &str) -> bool {
    s.chars().any(is_arabic)
}

fn is_arabic(c: char) -> bool {
    matches!(c as u32,
        0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF)
}

fn is_diacritic(c: char) -> bool {
    matches!(c as u32, 0x064B..=0x0652 | 0x0670)
}

const LAM: char = '\u{0644}';

/// (isolated, final, initial, medial) for a letter, and whether it is
/// dual-joining (connects on its left to a following letter). Right-joining
/// letters only connect to the preceding letter (final form).
fn forms(c: char) -> Option<(bool, [char; 4])> {
    let r = |i: char, f: char| Some((false, [i, f, i, f]));
    let d = |i: char, f: char, ini: char, m: char| Some((true, [i, f, ini, m]));
    match c {
        '\u{0622}' => r('\u{FE81}', '\u{FE82}'), // آ
        '\u{0623}' => r('\u{FE83}', '\u{FE84}'), // أ
        '\u{0624}' => r('\u{FE85}', '\u{FE86}'), // ؤ
        '\u{0625}' => r('\u{FE87}', '\u{FE88}'), // إ
        '\u{0626}' => d('\u{FE89}', '\u{FE8A}', '\u{FE8B}', '\u{FE8C}'), // ئ
        '\u{0627}' => r('\u{FE8D}', '\u{FE8E}'), // ا
        '\u{0628}' => d('\u{FE8F}', '\u{FE90}', '\u{FE91}', '\u{FE92}'), // ب
        '\u{0629}' => r('\u{FE93}', '\u{FE94}'), // ة
        '\u{062A}' => d('\u{FE95}', '\u{FE96}', '\u{FE97}', '\u{FE98}'), // ت
        '\u{062B}' => d('\u{FE99}', '\u{FE9A}', '\u{FE9B}', '\u{FE9C}'), // ث
        '\u{062C}' => d('\u{FE9D}', '\u{FE9E}', '\u{FE9F}', '\u{FEA0}'), // ج
        '\u{062D}' => d('\u{FEA1}', '\u{FEA2}', '\u{FEA3}', '\u{FEA4}'), // ح
        '\u{062E}' => d('\u{FEA5}', '\u{FEA6}', '\u{FEA7}', '\u{FEA8}'), // خ
        '\u{062F}' => r('\u{FEA9}', '\u{FEAA}'), // د
        '\u{0630}' => r('\u{FEAB}', '\u{FEAC}'), // ذ
        '\u{0631}' => r('\u{FEAD}', '\u{FEAE}'), // ر
        '\u{0632}' => r('\u{FEAF}', '\u{FEB0}'), // ز
        '\u{0633}' => d('\u{FEB1}', '\u{FEB2}', '\u{FEB3}', '\u{FEB4}'), // س
        '\u{0634}' => d('\u{FEB5}', '\u{FEB6}', '\u{FEB7}', '\u{FEB8}'), // ش
        '\u{0635}' => d('\u{FEB9}', '\u{FEBA}', '\u{FEBB}', '\u{FEBC}'), // ص
        '\u{0636}' => d('\u{FEBD}', '\u{FEBE}', '\u{FEBF}', '\u{FEC0}'), // ض
        '\u{0637}' => d('\u{FEC1}', '\u{FEC2}', '\u{FEC3}', '\u{FEC4}'), // ط
        '\u{0638}' => d('\u{FEC5}', '\u{FEC6}', '\u{FEC7}', '\u{FEC8}'), // ظ
        '\u{0639}' => d('\u{FEC9}', '\u{FECA}', '\u{FECB}', '\u{FECC}'), // ع
        '\u{063A}' => d('\u{FECD}', '\u{FECE}', '\u{FECF}', '\u{FED0}'), // غ
        '\u{0641}' => d('\u{FED1}', '\u{FED2}', '\u{FED3}', '\u{FED4}'), // ف
        '\u{0642}' => d('\u{FED5}', '\u{FED6}', '\u{FED7}', '\u{FED8}'), // ق
        '\u{0643}' => d('\u{FED9}', '\u{FEDA}', '\u{FEDB}', '\u{FEDC}'), // ك
        '\u{0644}' => d('\u{FEDD}', '\u{FEDE}', '\u{FEDF}', '\u{FEE0}'), // ل
        '\u{0645}' => d('\u{FEE1}', '\u{FEE2}', '\u{FEE3}', '\u{FEE4}'), // م
        '\u{0646}' => d('\u{FEE5}', '\u{FEE6}', '\u{FEE7}', '\u{FEE8}'), // ن
        '\u{0647}' => d('\u{FEE9}', '\u{FEEA}', '\u{FEEB}', '\u{FEEC}'), // ه
        '\u{0648}' => r('\u{FEED}', '\u{FEEE}'), // و
        '\u{0649}' => r('\u{FEEF}', '\u{FEF0}'), // ى
        '\u{064A}' => d('\u{FEF1}', '\u{FEF2}', '\u{FEF3}', '\u{FEF4}'), // ي
        _ => None,
    }
}

/// lam + alef → ligature (isolated, final).
fn lam_alef(next: char) -> Option<(char, char)> {
    match next {
        '\u{0622}' => Some(('\u{FEF5}', '\u{FEF6}')), // لآ
        '\u{0623}' => Some(('\u{FEF7}', '\u{FEF8}')), // لأ
        '\u{0625}' => Some(('\u{FEF9}', '\u{FEFA}')), // لإ
        '\u{0627}' => Some(('\u{FEFB}', '\u{FEFC}')), // لا
        _ => None,
    }
}

/// True if the letter can connect to a *following* letter (dual-joining).
fn joins_left(c: char) -> bool {
    forms(c).map(|(dual, _)| dual).unwrap_or(false)
}
/// True if the letter can connect to a *preceding* letter (any joining letter).
fn joins_right(c: char) -> bool {
    forms(c).is_some()
}

/// Replace Arabic letters with their contextual presentation forms (joining).
pub fn reshape(s: &str) -> String {
    let cs: Vec<char> = s.chars().collect();
    let n = cs.len();
    let mut out = String::with_capacity(n);

    let prev_letter =
        |i: usize| -> Option<char> { cs[..i].iter().rev().copied().find(|c| !is_diacritic(*c)) };
    let next_letter =
        |i: usize| -> Option<char> { cs[i + 1..].iter().copied().find(|c| !is_diacritic(*c)) };

    let mut i = 0;
    while i < n {
        let c = cs[i];
        // lam-alef ligature
        if c == LAM
            && let Some(nx) = next_letter(i)
            && let Some((iso, fin)) = lam_alef(nx)
        {
            let joins_prev = prev_letter(i).map(joins_left).unwrap_or(false);
            out.push(if joins_prev { fin } else { iso });
            // skip to (and over) the alef we consumed. `next_letter` already
            // found a non-diacritic after `i`, so `position` is always `Some`;
            // fall back to a single step rather than panicking if that ever fails.
            i = match cs[i + 1..].iter().position(|c| !is_diacritic(*c)) {
                Some(p) => i + 1 + p + 1,
                None => i + 1,
            };
            continue;
        }

        match forms(c) {
            Some((dual, f)) => {
                let jp = prev_letter(i).map(joins_left).unwrap_or(false);
                let jn = dual && next_letter(i).map(joins_right).unwrap_or(false);
                let form = if dual {
                    match (jp, jn) {
                        (true, true) => f[3],
                        (false, true) => f[2],
                        (true, false) => f[1],
                        (false, false) => f[0],
                    }
                } else if jp {
                    f[1]
                } else {
                    f[0]
                };
                out.push(form);
            }
            None => out.push(c),
        }
        i += 1;
    }
    out
}

#[derive(Clone, Copy, PartialEq)]
enum Dir {
    L,
    R,
    N,
}

fn dir(c: char) -> Dir {
    if c.is_ascii_alphanumeric() || matches!(c as u32, 0x00C0..=0x024F) {
        Dir::L
    } else if is_arabic(c) {
        Dir::R
    } else {
        Dir::N
    }
}

/// Bidi-mirrored glyph for paired punctuation (used inside RTL runs so brackets
/// point the right way).
fn mirror(c: char) -> char {
    match c {
        '(' => ')',
        ')' => '(',
        '[' => ']',
        ']' => '[',
        '{' => '}',
        '}' => '{',
        '<' => '>',
        '>' => '<',
        '\u{00AB}' => '\u{00BB}', // «
        '\u{00BB}' => '\u{00AB}', // »
        '\u{2039}' => '\u{203A}', // ‹
        '\u{203A}' => '\u{2039}', // ›
        _ => c,
    }
}

/// Base paragraph direction: RTL when Arabic characters are at least as common
/// as Latin ones, else LTR (so mostly-English text flows left-to-right).
pub fn base_rtl(s: &str) -> bool {
    let (mut l, mut r) = (0usize, 0usize);
    for c in s.chars() {
        match dir(c) {
            Dir::L => l += 1,
            Dir::R => r += 1,
            Dir::N => {}
        }
    }
    r > 0 && r >= l
}

/// Reorder a (reshaped) logical line for display. `base_rtl` sets the paragraph
/// direction; opposite-direction runs are flipped in place (Latin kept readable,
/// Arabic reversed + mirrored).
pub fn reorder(s: &str, base_rtl: bool) -> String {
    let cs: Vec<char> = s.chars().collect();
    if cs.is_empty() {
        return String::new();
    }
    let base = if base_rtl { Dir::R } else { Dir::L };

    // resolve neutrals: keep the surrounding direction only when both sides
    // agree, otherwise fall back to the base direction.
    let mut cls: Vec<Dir> = cs.iter().map(|&c| dir(c)).collect();
    let mut i = 0;
    while i < cls.len() {
        if cls[i] == Dir::N {
            let start = i;
            while i < cls.len() && cls[i] == Dir::N {
                i += 1;
            }
            let before = if start == 0 { base } else { cls[start - 1] };
            let after = if i >= cls.len() { base } else { cls[i] };
            let resolved = if before == after { before } else { base };
            for slot in &mut cls[start..i] {
                *slot = resolved;
            }
        } else {
            i += 1;
        }
    }

    // group into maximal L / R runs
    let mut runs: Vec<(bool, Vec<char>)> = Vec::new(); // (is_l, chars)
    for (k, &c) in cs.iter().enumerate() {
        let l = cls[k] == Dir::L;
        match runs.last_mut() {
            Some((rl, v)) if *rl == l => v.push(c),
            _ => runs.push((l, vec![c])),
        }
    }

    // RTL base → emit runs right-to-left; LTR base → left-to-right. Either way,
    // RTL runs are reversed + mirrored internally.
    let mut out = String::with_capacity(cs.len());
    let emit = |out: &mut String, l: bool, v: Vec<char>| {
        if l {
            out.extend(v);
        } else {
            out.extend(v.into_iter().rev().map(mirror));
        }
    };
    if base_rtl {
        for (l, v) in runs.into_iter().rev() {
            emit(&mut out, l, v);
        }
    } else {
        for (l, v) in runs {
            emit(&mut out, l, v);
        }
    }
    out
}

/// Single-field convenience: shape `s` for display only when `shape` is on and
/// `s` actually contains Arabic, else return it unchanged. The renderers use this
/// to shape one label without repeating the contains-check (mirrors the local
/// artist panel's gate).
pub fn shaped(s: &str, shape: bool) -> String {
    if shape && contains_arabic(s) {
        shape_line(s, true)
    } else {
        s.to_string()
    }
}

/// Shape one logical line for terminal display (reshape + reorder by base dir).
/// `shape == false` returns the raw logical string untouched — for terminals
/// that shape Arabic themselves (Ghostty / Kitty / WezTerm), where pre-shaping
/// would double-process and leave the letters disconnected.
pub fn shape_line(s: &str, shape: bool) -> String {
    if shape {
        reorder(&reshape(s), base_rtl(s))
    } else {
        s.to_string()
    }
}

/// Word-wrap `text` to `width` (display columns), shape each line, and align by
/// base direction: RTL paragraphs are right-aligned, LTR (mostly-English) are
/// left-aligned. When `justify` is set, LTR lines are space-justified to the full
/// width (except the last, ragged line). `shape == false` only wraps (no
/// reshape/reorder), leaving Arabic to the terminal's own shaper — see
/// [`shape_line`].
pub fn display_lines(text: &str, width: usize, shape: bool, justify: bool) -> Vec<String> {
    let width = width.max(1);
    let is_ltr = !shape || !contains_arabic(text);
    // fill-hyphenate only on the justified LTR path (Arabic doesn't hyphenate).
    let wrapped = word_wrap(text, width, justify && is_ltr);
    if is_ltr {
        // LTR / non-Arabic (incl. CJK): optionally justify every line but the last.
        if justify {
            let n = wrapped.len();
            return wrapped
                .into_iter()
                .enumerate()
                .map(|(i, line)| {
                    if i + 1 < n {
                        justify_line(&line, width)
                    } else {
                        line
                    }
                })
                .collect();
        }
        return wrapped;
    }
    let rtl = base_rtl(text);
    wrapped
        .into_iter()
        .map(|line| {
            let shaped = reorder(&reshape(&line), rtl);
            if rtl {
                let pad = width.saturating_sub(shaped.width());
                format!("{}{}", " ".repeat(pad), shaped)
            } else {
                shaped
            }
        })
        .collect()
}

/// A narrow (single-column) alphanumeric — the only kind we hyphenate across a
/// hard word break. Wide glyphs (CJK) break cleanly with no hyphen.
fn is_hyphenatable(c: char) -> bool {
    c.width() == Some(1) && c.is_alphanumeric()
}

/// Greedy word-wrap by **display width** (CJK/wide glyphs count as two columns).
/// Whitespace-delimited words stay whole when they fit; a word (or a spaceless
/// CJK run) wider than the line is hard-broken via [`break_long_word`] — with a
/// hyphen at a Latin break, a clean break between wide glyphs.
///
/// When `fill` is set (the justify path), a word that won't fit *whole* is instead
/// hyphenated to pack the current line tight (as much as fits + `-`, remainder to
/// the next line) — so lines end near the margin with single spaces, and the
/// justify pass barely has to stretch. Without it, the word moves down whole.
fn word_wrap(text: &str, width: usize, fill: bool) -> Vec<String> {
    use std::borrow::Cow;
    use std::collections::VecDeque;
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let mut queue: VecDeque<Cow<str>> = para.split_whitespace().map(Cow::Borrowed).collect();
        let mut cur = String::new();
        let mut cur_w = 0usize;
        while let Some(word) = queue.pop_front() {
            let ww = word.width();
            if cur_w == 0 {
                if ww <= width {
                    cur = word.into_owned();
                    cur_w = ww;
                } else {
                    break_long_word(&word, width, &mut lines, &mut cur, &mut cur_w);
                }
            } else if cur_w + 1 + ww <= width {
                cur.push(' ');
                cur.push_str(&word);
                cur_w += 1 + ww;
            } else {
                // the whole word doesn't fit — hyphenate it to fill the line first
                // (justify path), else move it down whole.
                if fill
                    && let Some((head, tail)) =
                        try_hyphenate(&word, width.saturating_sub(cur_w + 1))
                {
                    cur.push(' ');
                    cur.push_str(&head); // head already carries the trailing '-'
                    lines.push(std::mem::take(&mut cur));
                    cur_w = 0;
                    queue.push_front(Cow::Owned(tail)); // continue with the remainder
                    continue;
                }
                lines.push(std::mem::take(&mut cur));
                cur_w = 0;
                if ww <= width {
                    cur = word.into_owned();
                    cur_w = ww;
                } else {
                    break_long_word(&word, width, &mut lines, &mut cur, &mut cur_w);
                }
            }
        }
        lines.push(cur);
    }
    lines
}

/// Split `word` for a mid-line break so a `-`-terminated head fits in `room`
/// display columns, leaving the tail for the next line. Only breaks between two
/// narrow Latin letters, with ≥2 chars each side — so short/punctuated tokens
/// (`U.S.`, `CIA`) move down whole instead of turning into `CI-A`. Returns
/// `(head + '-', tail)` or `None` when no sensible break fits.
fn try_hyphenate(word: &str, room: usize) -> Option<(String, String)> {
    const MIN: usize = 2; // min chars kept on each side of the break
    if room <= MIN {
        return None; // no room for ≥MIN chars plus the hyphen column
    }
    let chars: Vec<char> = word.chars().collect();
    if chars.len() < MIN * 2 {
        return None;
    }
    // take as many leading chars as fit beside the reserved hyphen column
    let mut head_w = 0usize;
    let mut split = 0usize;
    for (i, &c) in chars.iter().enumerate() {
        let cw = c.width().unwrap_or(0);
        if head_w + cw + 1 > room {
            break;
        }
        head_w += cw;
        split = i + 1;
    }
    if split < MIN
        || chars.len() - split < MIN
        || !is_hyphenatable(chars[split - 1])
        || !is_hyphenatable(chars[split])
    {
        return None;
    }
    let mut head: String = chars[..split].iter().collect();
    head.push('-');
    let tail: String = chars[split..].iter().collect();
    Some((head, tail))
}

/// Hard-break a word wider than `width` display columns, pushing full lines to
/// `lines` and leaving the trailing partial in `cur`/`cur_w`. Assumes it starts a
/// fresh line (caller flushes first). A break between two narrow letters gets a
/// trailing hyphen; wide (CJK) glyphs break with none. Called only for an
/// over-wide token, so the loop always makes progress.
fn break_long_word(
    word: &str,
    width: usize,
    lines: &mut Vec<String>,
    cur: &mut String,
    cur_w: &mut usize,
) {
    let chars: Vec<char> = word.chars().collect();
    let mut seg = String::new();
    let mut seg_w = 0usize;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let cw = c.width().unwrap_or(0);
        // reserve a column for a hyphen when breaking mid-word (letter↔letter)
        let reserve = usize::from(
            is_hyphenatable(c) && chars.get(i + 1).copied().is_some_and(is_hyphenatable),
        );
        if seg_w > 0 && seg_w + cw + reserve > width {
            if seg_w < width
                && is_hyphenatable(c)
                && seg.chars().last().is_some_and(is_hyphenatable)
            {
                seg.push('-');
            }
            lines.push(std::mem::take(&mut seg));
            seg_w = 0;
            continue; // re-place `c` on the fresh segment
        }
        seg.push(c);
        seg_w += cw;
        i += 1;
    }
    *cur = seg;
    *cur_w = seg_w;
}

/// Space-justify one already-wrapped line to `width` display columns by padding
/// the gaps between words as evenly as possible (leftmost gaps take any
/// remainder). Lines with one word — or already at/over width — are returned
/// unchanged (nothing to distribute, and CJK runs have no gaps to widen).
fn justify_line(line: &str, width: usize) -> String {
    let words: Vec<&str> = line.split(' ').filter(|w| !w.is_empty()).collect();
    if words.len() < 2 {
        return line.to_string();
    }
    let text_w: usize = words.iter().map(|w| w.width()).sum();
    if text_w >= width {
        return line.to_string();
    }
    let gaps = words.len() - 1;
    let pad = width - text_w;
    // Cap the stretch: distributing a big gap over few words blows the spacing out
    // (unreadable). Only tighten to the margin when it stays ≤ 2 spaces per gap;
    // otherwise keep single spaces (ragged) — the fill-wrap already packs most
    // lines full enough that this rarely fires.
    if pad.div_ceil(gaps) > 2 {
        return line.to_string();
    }
    let (base, extra) = (pad / gaps, pad % gaps);
    let mut out = String::with_capacity(line.len() + pad);
    for (i, w) in words.iter().enumerate() {
        out.push_str(w);
        if i < gaps {
            out.push_str(&" ".repeat(base + usize::from(i < extra)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_and_reshapes() {
        assert!(contains_arabic("أصالة"));
        assert!(!contains_arabic("Assala"));
        // every char of a pure-Arabic word becomes a presentation form (FE70+)
        let shaped = reshape("بسم");
        assert!(shaped.chars().all(|c| (c as u32) >= 0xFE70));
    }

    #[test]
    fn latin_runs_stay_in_order() {
        // pure-latin string is unchanged either way
        assert_eq!(reorder("Assala Nasri", true), "Assala Nasri");
        assert_eq!(reorder("Assala Nasri", false), "Assala Nasri");
    }

    #[test]
    fn base_direction_by_majority() {
        // mostly Arabic → RTL base
        assert!(base_rtl("\u{0628}\u{0633}\u{0645} hi"));
        // mostly English → LTR base
        assert!(!base_rtl("hello world \u{0628}"));
        // no Arabic → LTR
        assert!(!base_rtl("hello"));
    }

    #[test]
    fn brackets_mirror_in_rtl() {
        // an opening paren inside an RTL run displays as a closing glyph
        let out = reorder("(\u{0628})", true);
        let chars: Vec<char> = out.chars().collect();
        // reversed + mirrored: ") <ba> (" → first glyph is "(", last is ")"
        assert_eq!(chars.first(), Some(&'('));
        assert_eq!(chars.last(), Some(&')'));
    }

    #[test]
    fn wraps_cjk_by_display_width() {
        // a spaceless CJK run (7 hangul × 2 cols = 14) into width 10 breaks at the
        // column boundary instead of overflowing.
        let out = display_lines("안녕하세요세계", 10, false, false);
        assert!(
            out.iter().all(|l| l.width() <= 10),
            "no line exceeds the width: {out:?}"
        );
        assert!(out.len() >= 2, "the CJK run wraps: {out:?}");
    }

    #[test]
    fn hard_breaks_and_hyphenates_a_long_word() {
        let out = display_lines("supercalifragilistic", 10, false, false);
        assert!(
            out.iter().all(|l| l.width() <= 10),
            "fits the width: {out:?}"
        );
        assert!(
            out[0].ends_with('-'),
            "a mid-word Latin break is hyphenated: {out:?}"
        );
        let joined: String = out.iter().map(|l| l.trim_end_matches('-')).collect();
        assert_eq!(
            joined, "supercalifragilistic",
            "round-trips without the hyphens"
        );
    }

    #[test]
    fn justifies_tight_lines_but_leaves_sparse_ones_ragged() {
        // a nearly-full line tightens to the margin (≤ 2-space gaps)
        let out = display_lines("ab cd ef gh ij", 12, false, true);
        assert!(out.len() >= 2, "{out:?}");
        assert_eq!(out[0].width(), 12, "a tight line fills the width: {out:?}");
        assert!(
            out.last().unwrap().width() <= 12,
            "the last line stays ragged: {out:?}"
        );
        // a sparse line (few short words) keeps single spaces instead of blowing the
        // gaps out — readability wins over a flush edge.
        let sparse = display_lines("the quick brown fox", 11, false, true);
        assert_eq!(
            sparse[0], "the quick",
            "sparse line stays single-spaced: {sparse:?}"
        );
    }

    #[test]
    fn fill_hyphenates_a_boundary_word_to_pack_a_justified_line() {
        // justify path: a long word that won't fit whole is hyphenated to fill the
        // line (single spaces) rather than moved down whole (leaving a big gap).
        let out = display_lines("aa bbbbbbbb", 8, false, true);
        assert_eq!(
            out[0], "aa bbbb-",
            "boundary word hyphenated to fill: {out:?}"
        );
        assert_eq!(out[1], "bbbb", "the remainder continues next line: {out:?}");
    }
}
