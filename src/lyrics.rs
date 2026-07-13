//! Lyrics: embedded tags, `.lrc` sidecars, an on-disk cache of online lookups,
//! and parsing of both synced (`[mm:ss.xx]` timestamped) and plain text.
//!
//! Resolution order at play time (see `app::play_current`): sidecar `.lrc` →
//! embedded tag → cache → online lookup (LRCLIB → NetEase, on a worker thread).

use std::path::Path;
use std::time::Duration;

use lofty::prelude::*;

#[derive(Debug, Clone, Default)]
pub struct Lyrics {
    /// (timestamp, text). For plain lyrics the timestamps are placeholders and
    /// `synced` is false; the UI then scrolls by playback progress instead.
    pub lines: Vec<(Duration, String)>,
    /// Per-line translation (bilingual `.lrc`: a second line at the same
    /// timestamp). Aligned 1:1 with `lines`; `None` when there's no translation.
    pub trans: Vec<Option<String>>,
    pub synced: bool,
}

impl Lyrics {
    /// Local lyrics for a track: a sidecar `<track>.lrc`, else embedded tag text.
    pub fn load_for(path: &Path) -> Option<Lyrics> {
        if let Ok(text) = std::fs::read_to_string(path.with_extension("lrc")) {
            let l = Self::parse(&text);
            if !l.lines.is_empty() {
                return Some(l);
            }
        }
        Self::embedded(path)
    }

    /// Lyrics embedded in the file's tags (USLT / LYRICS — plain or LRC).
    pub fn embedded(path: &Path) -> Option<Lyrics> {
        let tagged = lofty::read_from_path(path).ok()?;
        let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
        let text = tag.get_string(ItemKey::Lyrics)?;
        let l = Self::parse(text);
        (!l.lines.is_empty()).then_some(l)
    }

    /// Parse LRC (timestamped) or plain lyrics. If no `[mm:ss]` tags are found
    /// the text is treated as plain (one entry per non-empty line).
    pub fn parse(text: &str) -> Lyrics {
        let mut timed: Vec<(Duration, String)> = Vec::new();
        for raw in text.lines() {
            let mut rest = raw;
            let mut stamps: Vec<Duration> = Vec::new();
            while rest.starts_with('[') {
                let Some(end) = rest.find(']') else { break };
                if let Some(d) = parse_timestamp(&rest[1..end]) {
                    stamps.push(d);
                }
                rest = &rest[end + 1..];
            }
            let content = rest.trim().to_string();
            for d in stamps {
                timed.push((d, content.clone()));
            }
        }

        if !timed.is_empty() {
            timed.sort_by_key(|(d, _)| *d);
            // bilingual `.lrc`: a second line at the *same* timestamp is the
            // translation of the first — pair them so the UI can show both.
            let mut lines = Vec::new();
            let mut trans = Vec::new();
            let mut i = 0;
            while i < timed.len() {
                let (t, text) = timed[i].clone();
                if i + 1 < timed.len() && timed[i + 1].0 == t {
                    lines.push((t, text));
                    trans.push(Some(timed[i + 1].1.clone()));
                    i += 2;
                } else {
                    lines.push((t, text));
                    trans.push(None);
                    i += 1;
                }
            }
            return Lyrics {
                lines,
                trans,
                synced: true,
            };
        }

        // plain text → one line each (placeholder timestamps; UI uses progress)
        let lines: Vec<(Duration, String)> = text
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .enumerate()
            .map(|(i, l)| (Duration::from_secs(i as u64), l.to_string()))
            .collect();
        let trans = vec![None; lines.len()];
        Lyrics {
            lines,
            trans,
            synced: false,
        }
    }

    /// Index of the active line at `elapsed` (synced lyrics).
    pub fn active_index(&self, elapsed: Duration) -> Option<usize> {
        if self.lines.is_empty() {
            return None;
        }
        let mut idx = 0;
        for (i, (t, _)) in self.lines.iter().enumerate() {
            if *t <= elapsed {
                idx = i;
            } else {
                break;
            }
        }
        Some(idx)
    }

    /// Active line index plus how far through it we are (0.0..1.0), interpolated
    /// between this line's timestamp and the next line's. Drives the karaoke
    /// "wipe" so words light up across the line as it's sung. Plain (unsynced)
    /// lyrics have no timing → fraction 0.
    pub fn active_progress(&self, elapsed: Duration) -> Option<(usize, f32)> {
        let idx = self.active_index(elapsed)?;
        if !self.synced {
            return Some((idx, 0.0));
        }
        let start = self.lines[idx].0;
        // end of this line = the next line's start; for the last line assume 5s
        let end = self
            .lines
            .get(idx + 1)
            .map(|(t, _)| *t)
            .unwrap_or(start + Duration::from_secs(5));
        let span = end.saturating_sub(start).as_secs_f32().max(0.001);
        let into = elapsed.saturating_sub(start).as_secs_f32();
        Some((idx, (into / span).clamp(0.0, 1.0)))
    }

    // ---- on-disk cache of online lookups --------------------------------
    pub fn load_cached(dir: &Path, key: &str) -> Option<Lyrics> {
        let text = std::fs::read_to_string(dir.join("lyrics").join(format!("{key}.lrc"))).ok()?;
        let l = Self::parse(&text);
        (!l.lines.is_empty()).then_some(l)
    }

    /// Write raw fetched LRC/plain text to the cache.
    pub fn save_cache(dir: &Path, key: &str, text: &str) {
        let d = dir.join("lyrics");
        let _ = std::fs::create_dir_all(&d);
        let _ = std::fs::write(d.join(format!("{key}.lrc")), text);
    }
}

/// Stable, filesystem-safe cache key from artist + title.
pub fn cache_key(artist: &str, title: &str) -> String {
    let raw = format!(
        "{}-{}",
        artist.trim().to_lowercase(),
        title.trim().to_lowercase()
    );
    raw.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

fn parse_timestamp(tag: &str) -> Option<Duration> {
    // mm:ss.xx  or  mm:ss
    let (m, rest) = tag.split_once(':')?;
    let minutes: u64 = m.trim().parse().ok()?;
    let seconds: f64 = rest.trim().parse().ok()?;
    if seconds.is_nan() || seconds < 0.0 {
        return None;
    }
    Some(Duration::from_secs_f64(minutes as f64 * 60.0 + seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_synced_lrc() {
        let l = Lyrics::parse("[00:12.50]Hello\n[00:15.00]World");
        assert!(l.synced);
        assert_eq!(l.lines.len(), 2);
        assert_eq!(l.lines[0].1, "Hello");
        assert_eq!(l.lines[0].0, Duration::from_secs_f64(12.5));
        assert_eq!(l.active_index(Duration::from_secs(13)), Some(0));
        assert_eq!(l.active_index(Duration::from_secs(16)), Some(1));
    }

    #[test]
    fn active_progress_interpolates_within_a_line() {
        let l = Lyrics::parse("[00:10.00]one\n[00:20.00]two");
        // 10s into the song = start of line 0 → 0% through it
        assert_eq!(l.active_progress(Duration::from_secs(10)), Some((0, 0.0)));
        // 15s = halfway between line 0 (10s) and line 1 (20s) → ~50%
        let (i, f) = l.active_progress(Duration::from_secs(15)).unwrap();
        assert_eq!(i, 0);
        assert!((f - 0.5).abs() < 0.01, "halfway, got {f}");
        // past the last line → clamps to 1.0 within line 1
        let (i, f) = l.active_progress(Duration::from_secs(22)).unwrap();
        assert_eq!(i, 1);
        assert!(f > 0.0);
    }

    #[test]
    fn pairs_bilingual_lines_by_timestamp() {
        // same timestamp twice → second line is the first's translation
        let l = Lyrics::parse("[00:10.00]Bonjour\n[00:10.00]Hello\n[00:20.00]Au revoir");
        assert_eq!(l.lines.len(), 2, "the pair collapses to one line");
        assert_eq!(l.lines[0].1, "Bonjour");
        assert_eq!(l.trans[0].as_deref(), Some("Hello"));
        assert_eq!(l.lines[1].1, "Au revoir");
        assert_eq!(l.trans[1], None);
    }

    #[test]
    fn parses_plain_text() {
        let l = Lyrics::parse("Line one\n\n  Line two  \n");
        assert!(!l.synced);
        assert_eq!(l.lines.len(), 2); // blank line dropped, trimmed
        assert_eq!(l.lines[1].1, "Line two");
    }

    #[test]
    fn cache_key_is_filesystem_safe() {
        let k = cache_key("Amr Diab", "Tamally Maak?");
        assert!(k.chars().all(|c| c.is_alphanumeric() || c == '_'));
        assert_eq!(k, cache_key(" amr diab ", "TAMALLY MAAK?")); // case/space stable
    }
}
