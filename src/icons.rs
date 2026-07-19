//! Transport icon sets: built-in presets, an optional Nerd Font set, and
//! per-glyph custom overrides from the `[icons]` config table.
//!
//! Glyphs render in whatever font the terminal uses, so the plain-Unicode
//! presets work everywhere; the `nerd` preset only looks right with a Nerd Font
//! installed. Resolution: start from the named preset, then apply any overrides.

use serde::Deserialize;

/// The resolved glyph for every transport control.
#[derive(Debug, Clone)]
pub struct Icons {
    pub play: String,
    pub pause: String,
    pub prev: String,
    pub next: String,
    pub shuffle: String,
    pub repeat: String,
    pub repeat_one: String,
    pub seek_back: String,
    pub seek_fwd: String,
    pub volume: String,
    pub volume_mute: String,
}

/// Per-glyph overrides parsed from the `[icons]` table in `config.toml`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct IconOverrides {
    pub play: Option<String>,
    pub pause: Option<String>,
    pub prev: Option<String>,
    pub next: Option<String>,
    pub shuffle: Option<String>,
    pub repeat: Option<String>,
    pub repeat_one: Option<String>,
    pub seek_back: Option<String>,
    pub seek_fwd: Option<String>,
    pub volume: Option<String>,
    pub volume_mute: Option<String>,
}

impl Icons {
    /// Selectable built-in preset names (in cycle order). `outline` leads because
    /// it is the safe default; `nerd` is last because it is the only one that
    /// needs a font the user may not have.
    pub const PRESETS: [&'static str; 5] = ["outline", "triangles", "skip", "ascii", "nerd"];

    fn make(g: [&str; 11]) -> Icons {
        Icons {
            play: g[0].into(),
            pause: g[1].into(),
            prev: g[2].into(),
            next: g[3].into(),
            shuffle: g[4].into(),
            repeat: g[5].into(),
            repeat_one: g[6].into(),
            seek_back: g[7].into(),
            seek_fwd: g[8].into(),
            volume: g[9].into(),
            volume_mute: g[10].into(),
        }
    }

    /// A built-in preset by name (unknown → `outline`).
    /// Order: play, pause, prev, next, shuffle, repeat, repeat_one, rewind, fwd,
    /// volume, volume_mute. prev/next are *track* skip; rewind/fwd are *seek*.
    pub fn preset(name: &str) -> Icons {
        match name.to_lowercase().as_str() {
            // Nerd Font (Font Awesome glyphs) — proper, full-size media icons.
            "nerd" => Self::make([
                "\u{f04b}", "\u{f04c}", "\u{f048}", "\u{f051}", "\u{f074}", "\u{f01e}", "\u{f01e}",
                "\u{f04a}", "\u{f04e}", "\u{f028}", "\u{f026}",
            ]),
            // plain-Unicode fallbacks (work without a Nerd Font)
            "triangles" => Self::make([
                "▶", "❚❚", "▏◀", "▶▏", "⤨", "↻", "↻", "◀◀", "▶▶", "VOL", "MUT",
            ]),
            "skip" => Self::make(["▶", "❚❚", "↞", "↠", "⤭", "⟳", "⟳", "↶", "↷", "VOL", "MUT"]),
            // pure ASCII — the last-resort set. Every other preset still relies on
            // Unicode symbols that a sparse font or an old terminal can miss; this
            // one cannot fail, so it's the answer to "everything renders as boxes".
            "ascii" => Self::make([
                ">", "||", "|<", ">|", "><", "()", "()", "<<", ">>", "VOL", "MUT",
            ]),
            // outline: standard media symbols; prev/next carry the bar at the tip.
            _ => Self::make(["▶", "⏸", "⏮", "⏭", "⇄", "↻", "↻", "⏪", "⏩", "VOL", "MUT"]),
        }
    }

    /// A one-line sample of the transport glyphs, for the settings picker.
    ///
    /// Whether a font has a given glyph can't be queried from a terminal, so the
    /// only honest way to choose an icon set is to *look* at it: this renders each
    /// preset inline in the value list, and the one showing boxes is the one this
    /// font can't display.
    pub fn sample(name: &str) -> String {
        let i = Self::preset(name);
        format!(
            "{} {} {} {} {} {}",
            i.play, i.pause, i.prev, i.next, i.shuffle, i.repeat
        )
    }

    /// Resolve a preset with custom overrides applied (empty overrides ignored).
    pub fn resolve(name: &str, ov: &IconOverrides) -> Icons {
        let mut i = Self::preset(name);
        let set = |slot: &mut String, o: &Option<String>| {
            if let Some(v) = o
                && !v.is_empty()
            {
                *slot = v.clone();
            }
        };
        set(&mut i.play, &ov.play);
        set(&mut i.pause, &ov.pause);
        set(&mut i.prev, &ov.prev);
        set(&mut i.next, &ov.next);
        set(&mut i.shuffle, &ov.shuffle);
        set(&mut i.repeat, &ov.repeat);
        set(&mut i.repeat_one, &ov.repeat_one);
        set(&mut i.seek_back, &ov.seek_back);
        set(&mut i.seek_fwd, &ov.seek_fwd);
        set(&mut i.volume, &ov.volume);
        set(&mut i.volume_mute, &ov.volume_mute);
        i
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_preset_falls_back_to_outline() {
        let a = Icons::preset("nope");
        let b = Icons::preset("outline");
        assert_eq!(a.play, b.play);
        assert_eq!(a.prev, "⏮");
    }

    /// Every glyph in `ascii` must be plain ASCII — it is the last-resort set for
    /// "everything renders as boxes", so it cannot itself depend on font coverage.
    #[test]
    fn ascii_preset_needs_no_special_font() {
        let i = Icons::preset("ascii");
        for g in [
            &i.play,
            &i.pause,
            &i.prev,
            &i.next,
            &i.shuffle,
            &i.repeat,
            &i.repeat_one,
            &i.seek_back,
            &i.seek_fwd,
            &i.volume,
            &i.volume_mute,
        ] {
            assert!(g.is_ascii(), "{g:?} is not ASCII");
        }
    }

    /// The shipped default must render without a Nerd Font installed. A terminal
    /// can't be asked which glyphs its font has, so an undetectable dependency
    /// must never be the out-of-box choice — `nerd` is opt-in only.
    #[test]
    fn default_icon_set_needs_no_nerd_font() {
        let c = crate::config::Config::default();
        assert_ne!(c.icon_set, "nerd");
        assert!(
            Icons::PRESETS.contains(&c.icon_set.as_str()),
            "default {:?} is not a known preset",
            c.icon_set
        );
        assert!(!c.powerline, "Powerline glyphs are a font dependency too");
    }

    #[test]
    fn sample_renders_every_preset() {
        for p in Icons::PRESETS {
            assert!(!Icons::sample(p).is_empty(), "{p} has no sample");
        }
    }

    #[test]
    fn overrides_win_over_preset() {
        let ov = IconOverrides {
            play: Some("X".into()),
            pause: Some(String::new()), // empty → ignored
            ..Default::default()
        };
        let i = Icons::resolve("outline", &ov);
        assert_eq!(i.play, "X");
        assert_eq!(i.pause, "⏸", "empty override keeps the preset glyph");
    }
}
