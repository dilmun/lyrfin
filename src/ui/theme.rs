//! Themes: a palette + gradient stops that every widget reads from. M2 loads
//! user themes from `themes/*.toml`; the gradient accent is sampled per-cell to
//! produce the smooth progress bars / visualizer fills seen in the mockups.

/// 24-bit color. Bridges to `ratatui::style::Color::Rgb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl From<Rgb> for ratatui::style::Color {
    fn from(c: Rgb) -> Self {
        ratatui::style::Color::Rgb(c.0, c.1, c.2)
    }
}

impl Rgb {
    /// Blend toward another color by `t` in 0.0..=1.0.
    pub fn mix(self, other: Rgb, t: f32) -> Rgb {
        Rgb(
            lerp(self.0, other.0, t),
            lerp(self.1, other.1, t),
            lerp(self.2, other.2, t),
        )
    }
    /// Desaturated (neutral grey) version at the same perceived brightness.
    pub fn grey(self) -> Rgb {
        let l = ((self.0 as u16 + self.1 as u16 + self.2 as u16) / 3) as u8;
        Rgb(l, l, l)
    }
}

/// Semantic colour roles every widget reads through. Each is optional: `None`
/// means "use the computed default" (derived from the base palette), so existing
/// themes keep working and a theme file only overrides what it names.
#[derive(Debug, Clone, Default)]
pub struct Roles {
    /// Panel / box titles (structure, not accent).
    pub title: Option<Rgb>,
    /// Track/item TITLE text — every list, the column table, and grid cards.
    pub track_title: Option<Rgb>,
    /// Track METADATA text — artist · album · year · time and every non-title
    /// column, one calm tier below the title.
    pub track_meta: Option<Rgb>,
    /// The whole current-playing row (all columns).
    pub now_playing: Option<Rgb>,
    /// Selected-row text. A user-settable theme role, not yet rendered.
    #[allow(dead_code)]
    pub selection_fg: Option<Rgb>,
    /// Active toggle buttons / on-state indicators.
    pub toggle_on: Option<Rgb>,
    /// Inactive toggles.
    pub toggle_off: Option<Rgb>,
    /// Sliders (single-colour meters like volume).
    pub slider: Option<Rgb>,
    /// Special sidebar items (smart lists), distinct from the artist tree.
    pub special: Option<Rgb>,
    /// Marked-row accent.
    pub marked: Option<Rgb>,
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub bg: Rgb,
    pub panel: Rgb,
    pub border: Rgb,
    pub border_focus: Rgb,
    pub text: Rgb,
    pub text_dim: Rgb,
    pub text_faint: Rgb,
    pub selection: Rgb,
    /// Accent gradient stops (cyan → indigo → pink for Aurora). Sampled across
    /// progress bars, visualizer bars, and highlights.
    pub accent: [Rgb; 3],
    pub warning: Rgb,
    pub good: Rgb,
    /// Semantic colour roles (overrides; defaults computed from the palette).
    pub roles: Roles,
}

impl Theme {
    // ---- semantic role accessors (the only colours widgets should read) ----
    /// Panel / box title colour. Unfocused panels dim toward the panel bg.
    pub fn title_color(&self, focused: bool) -> Rgb {
        let base = self
            .roles
            .title
            .unwrap_or_else(|| self.text_dim.mix(self.text, 0.6));
        if focused {
            base
        } else {
            base.mix(self.panel, 0.4)
        }
    }
    /// The TITLE tier — the track/album/artist title in every view (rows, the
    /// column table, grid cards, browse names). One shared colour so a title reads
    /// the same everywhere, a step brighter than the metadata below it. Theme role:
    /// `track_title`.
    pub fn title_text(&self) -> Rgb {
        self.roles
            .track_title
            .unwrap_or_else(|| self.text_dim.mix(self.text, 0.6))
    }
    /// The METADATA tier — artist · album · year · time and every non-title column,
    /// one calm tier below the title and identical across all views. Theme role:
    /// `track_meta`.
    pub fn meta_text(&self) -> Rgb {
        self.roles.track_meta.unwrap_or(self.text_faint)
    }
    /// The current-playing row (every column).
    pub fn now_playing_color(&self) -> Rgb {
        self.roles.now_playing.unwrap_or(self.accent[0])
    }
    pub fn toggle_on(&self) -> Rgb {
        self.roles.toggle_on.unwrap_or(self.accent[1])
    }
    pub fn toggle_off(&self) -> Rgb {
        self.roles
            .toggle_off
            .unwrap_or_else(|| self.text_faint.grey())
    }
    pub fn slider_color(&self) -> Rgb {
        self.roles.slider.unwrap_or(self.accent[0])
    }
    pub fn marked_color(&self) -> Rgb {
        self.roles.marked.unwrap_or(self.accent[2])
    }
}

/// A terminal's own colours, read at startup via an OSC palette query (see
/// `crate::termquery`). Every field is optional — a terminal may answer some
/// queries and not others, or none at all.
#[derive(Debug, Clone, Default)]
pub struct TerminalPalette {
    /// Default foreground (OSC 10).
    pub fg: Option<Rgb>,
    /// Default background (OSC 11).
    pub bg: Option<Rgb>,
    /// The 16 ANSI colours (OSC 4): 0–7 normal, 8–15 bright.
    pub ansi: [Option<Rgb>; 16],
}

impl TerminalPalette {
    /// Build the `auto` theme from the queried colours, or `None` if the terminal
    /// didn't return the essentials (default fg + bg). The neutral tiers
    /// (panel/border/dim/faint/selection) are blends of fg↔bg, so they track the
    /// terminal whether it's light or dark; the accent + status roles come from the
    /// ANSI colours (bright variant preferred), keeping every gradient/blend on real
    /// RGB rather than un-interpolatable symbolic colours.
    pub fn to_theme(&self) -> Option<Theme> {
        let bg = self.bg?;
        let fg = self.fg?;
        // an ANSI slot, preferring the bright variant, then normal, then fg
        let pick =
            |bright: usize, normal: usize| self.ansi[bright].or(self.ansi[normal]).unwrap_or(fg);
        let red = pick(9, 1);
        let green = pick(10, 2);
        let yellow = pick(11, 3);
        let blue = pick(12, 4);
        let magenta = pick(13, 5);
        let cyan = pick(14, 6);
        Some(Theme {
            name: "auto".into(),
            bg,
            panel: bg.mix(fg, 0.05),
            border: bg.mix(fg, 0.20),
            border_focus: blue,
            text: fg,
            text_dim: fg.mix(bg, 0.35),
            text_faint: fg.mix(bg, 0.55),
            selection: bg.mix(fg, 0.16),
            accent: [cyan, blue, magenta],
            warning: yellow,
            good: green,
            roles: Roles {
                now_playing: Some(blue),
                special: Some(magenta),
                marked: Some(red),
                ..Roles::default()
            },
        })
    }
}

/// The built-in themes, in cycle order.
pub const BUILTIN_THEMES: [&str; 5] =
    ["aurora", "cyberpunk", "glacier", "monolith", "highcontrast"];

impl Theme {
    pub fn by_name(name: &str) -> Theme {
        match name.to_lowercase().as_str() {
            "cyberpunk" => Self::cyberpunk(),
            "glacier" => Self::glacier(),
            "monolith" => Self::monolith(),
            "highcontrast" | "high-contrast" => Self::high_contrast(),
            _ => Self::aurora(),
        }
    }

    /// Accessibility theme: pure black bg, bright high-contrast foregrounds.
    pub fn high_contrast() -> Theme {
        Theme {
            name: "highcontrast".into(),
            bg: Rgb(0x00, 0x00, 0x00),
            panel: Rgb(0x00, 0x00, 0x00),
            border: Rgb(0xAA, 0xAA, 0xAA),
            border_focus: Rgb(0xFF, 0xFF, 0x00),
            text: Rgb(0xFF, 0xFF, 0xFF),
            text_dim: Rgb(0xD0, 0xD0, 0xD0),
            text_faint: Rgb(0x9A, 0x9A, 0x9A),
            selection: Rgb(0x00, 0x33, 0x66),
            accent: [
                Rgb(0x00, 0xFF, 0xFF),
                Rgb(0xFF, 0xFF, 0x00),
                Rgb(0xFF, 0x00, 0xFF),
            ],
            warning: Rgb(0xFF, 0xD7, 0x00),
            good: Rgb(0x00, 0xFF, 0x00),
            roles: Roles::default(),
        }
    }

    /// Resolve a theme by name: a user `themes/<name>.toml` wins over a built-in.
    /// `auto` is built from the terminal query at startup (see [`TerminalPalette`]);
    /// here it's the fallback used before that / when the terminal can't answer.
    pub fn resolve(name: &str, themes_dir: &std::path::Path) -> Theme {
        if name == "auto" {
            let mut t = Self::by_name("aurora");
            t.name = "auto".into();
            return t;
        }
        let path = themes_dir.join(format!("{name}.toml"));
        if let Ok(text) = std::fs::read_to_string(&path)
            && let Ok(t) = Self::from_toml(name, &text)
        {
            return t;
        }
        Self::by_name(name)
    }

    /// Parse a theme TOML; missing fields fall back to Aurora.
    pub fn from_toml(name: &str, text: &str) -> Result<Theme, String> {
        let f: ThemeFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let d = Theme::aurora();
        let p = |o: Option<String>, fb: Rgb| o.and_then(|s| parse_hex(&s)).unwrap_or(fb);
        let accent = f
            .accent
            .and_then(|v| {
                if v.len() == 3 {
                    Some([
                        parse_hex(&v[0]).unwrap_or(d.accent[0]),
                        parse_hex(&v[1]).unwrap_or(d.accent[1]),
                        parse_hex(&v[2]).unwrap_or(d.accent[2]),
                    ])
                } else {
                    None
                }
            })
            .unwrap_or(d.accent);
        Ok(Theme {
            name: f.name.unwrap_or_else(|| name.to_string()),
            bg: p(f.bg, d.bg),
            panel: p(f.panel, d.panel),
            border: p(f.border, d.border),
            border_focus: p(f.border_focus, d.border_focus),
            text: p(f.text, d.text),
            text_dim: p(f.text_dim, d.text_dim),
            text_faint: p(f.text_faint, d.text_faint),
            selection: p(f.selection, d.selection),
            accent,
            warning: p(f.warning, d.warning),
            good: p(f.good, d.good),
            roles: Roles {
                title: f.title.and_then(|s| parse_hex(&s)),
                track_title: f.track_title.and_then(|s| parse_hex(&s)),
                track_meta: f.track_meta.and_then(|s| parse_hex(&s)),
                now_playing: f.now_playing.and_then(|s| parse_hex(&s)),
                selection_fg: f.selection_fg.and_then(|s| parse_hex(&s)),
                toggle_on: f.toggle_on.and_then(|s| parse_hex(&s)),
                toggle_off: f.toggle_off.and_then(|s| parse_hex(&s)),
                slider: f.slider.and_then(|s| parse_hex(&s)),
                special: f.special.and_then(|s| parse_hex(&s)),
                marked: f.marked.and_then(|s| parse_hex(&s)),
            },
        })
    }

    pub fn aurora() -> Theme {
        Theme {
            name: "aurora".into(),
            bg: Rgb(0x0A, 0x0C, 0x14),
            panel: Rgb(0x0E, 0x11, 0x1B),
            border: Rgb(0x22, 0x28, 0x38),
            border_focus: Rgb(0x33, 0x40, 0x5E),
            text: Rgb(0xE7, 0xEA, 0xF4),
            text_dim: Rgb(0x9A, 0xA3, 0xB8),
            text_faint: Rgb(0x64, 0x6D, 0x83),
            selection: Rgb(0x18, 0x20, 0x32),
            accent: [
                Rgb(0x48, 0xE6, 0xD6),
                Rgb(0x7E, 0x8C, 0xF7),
                Rgb(0xF4, 0x7C, 0xC0),
            ],
            warning: Rgb(0xF7, 0xC4, 0x5A),
            good: Rgb(0x54, 0xDD, 0xA0),
            roles: Roles::default(),
        }
    }

    pub fn cyberpunk() -> Theme {
        Theme {
            name: "cyberpunk".into(),
            bg: Rgb(0x0A, 0x05, 0x12),
            panel: Rgb(0x14, 0x0A, 0x1F),
            border: Rgb(0x46, 0x21, 0x5F),
            border_focus: Rgb(0x7B, 0x53, 0x90),
            text: Rgb(0xFB, 0xEE, 0xFF),
            text_dim: Rgb(0xC7, 0x9B, 0xDA),
            text_faint: Rgb(0x7B, 0x53, 0x90),
            selection: Rgb(0x24, 0x12, 0x33),
            accent: [
                Rgb(0x00, 0xE5, 0xFF),
                Rgb(0x9B, 0x5C, 0xFF),
                Rgb(0xFF, 0x3D, 0xEC),
            ],
            warning: Rgb(0xFF, 0xC4, 0x5A),
            good: Rgb(0x54, 0xDD, 0xA0),
            roles: Roles::default(),
        }
    }

    pub fn glacier() -> Theme {
        Theme {
            name: "glacier".into(),
            bg: Rgb(0x08, 0x11, 0x1C),
            panel: Rgb(0x0D, 0x1B, 0x2A),
            border: Rgb(0x28, 0x45, 0x6A),
            border_focus: Rgb(0x5F, 0xB3, 0xFF),
            text: Rgb(0xEA, 0xF4, 0xFF),
            text_dim: Rgb(0x9F, 0xBA, 0xDA),
            text_faint: Rgb(0x5C, 0x76, 0xA0),
            selection: Rgb(0x12, 0x2A, 0x40),
            accent: [
                Rgb(0x21, 0xD4, 0xFD),
                Rgb(0x2A, 0x7C, 0xFF),
                Rgb(0x21, 0x52, 0xFF),
            ],
            warning: Rgb(0xF7, 0xC4, 0x5A),
            good: Rgb(0x54, 0xDD, 0xA0),
            roles: Roles::default(),
        }
    }

    pub fn monolith() -> Theme {
        Theme {
            name: "monolith".into(),
            bg: Rgb(0x0E, 0x0F, 0x12),
            panel: Rgb(0x16, 0x18, 0x1C),
            border: Rgb(0x2C, 0x2F, 0x36),
            border_focus: Rgb(0x64, 0x69, 0x77),
            text: Rgb(0xF2, 0xF3, 0xF6),
            text_dim: Rgb(0xA4, 0xA8, 0xB2),
            text_faint: Rgb(0x64, 0x69, 0x77),
            selection: Rgb(0x20, 0x22, 0x27),
            accent: [
                Rgb(0x9A, 0xA3, 0xB8),
                Rgb(0xC0, 0xC6, 0xD2),
                Rgb(0xE7, 0xEA, 0xF4),
            ],
            warning: Rgb(0xD6, 0xDA, 0xE2),
            good: Rgb(0xC0, 0xC6, 0xD2),
            roles: Roles::default(),
        }
    }

    /// Drive the accent gradient (and focus border) from a single base colour —
    /// used by the dynamic accent derived from album art. Lifts very dark bases
    /// so they still read against the panel, then fans out a light→base→deep ramp.
    ///
    /// Also re-tints every accent-derived role (now-playing row, toggles, slider,
    /// special items, marked) so the whole UI follows the album art — overriding
    /// any accent-family colours a theme set. Neutral roles (titles, column text,
    /// selection) are left alone. Toggling dynamic accent off re-resolves the
    /// theme, restoring its colours.
    pub fn set_accent(&mut self, base: Rgb) {
        let lum = base.0 as u16 + base.1 as u16 + base.2 as u16;
        let base = if lum < 180 {
            base.mix(Rgb(255, 255, 255), 0.35)
        } else {
            base
        };
        self.accent = [
            base.mix(Rgb(255, 255, 255), 0.30),
            base,
            base.mix(self.bg, 0.30),
        ];
        self.border_focus = base;
        // cascade onto the accent-family roles (override any theme values)
        self.roles.now_playing = Some(self.accent[0]);
        self.roles.toggle_on = Some(self.accent[1]);
        self.roles.slider = Some(self.accent[0]);
        self.roles.special = Some(self.accent[1]);
        self.roles.marked = Some(self.accent[2]);
    }

    /// Linear sample of the 3-stop accent gradient at `t` in 0.0..=1.0.
    pub fn accent_at(&self, t: f32) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        let (a, b, seg) = if t < 0.5 {
            (self.accent[0], self.accent[1], t / 0.5)
        } else {
            (self.accent[1], self.accent[2], (t - 0.5) / 0.5)
        };
        Rgb(
            lerp(a.0, b.0, seg),
            lerp(a.1, b.1, seg),
            lerp(a.2, b.2, seg),
        )
    }
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}

/// On-disk theme: every field optional, hex strings (`#RRGGBB`).
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default)]
struct ThemeFile {
    name: Option<String>,
    bg: Option<String>,
    panel: Option<String>,
    border: Option<String>,
    border_focus: Option<String>,
    text: Option<String>,
    text_dim: Option<String>,
    text_faint: Option<String>,
    selection: Option<String>,
    accent: Option<Vec<String>>,
    warning: Option<String>,
    good: Option<String>,
    // semantic roles (optional)
    title: Option<String>,
    track_title: Option<String>,
    track_meta: Option<String>,
    now_playing: Option<String>,
    selection_fg: Option<String>,
    toggle_on: Option<String>,
    toggle_off: Option<String>,
    slider: Option<String>,
    special: Option<String>,
    marked: Option<String>,
}

fn parse_hex(s: &str) -> Option<Rgb> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Rgb(r, g, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_themes_parse_and_have_valid_hex() {
        for (name, body) in crate::config::BUNDLED_THEMES {
            // valid TOML that resolves to a Theme carrying its declared name
            let theme = Theme::from_toml(name, body)
                .unwrap_or_else(|e| panic!("bundled theme `{name}` failed to parse: {e}"));
            assert_eq!(
                &theme.name, name,
                "theme `{name}` declares a mismatched name"
            );

            // Every colour LITERAL (`"#rrggbb"`) must be a well-formed 6-digit hex — a
            // typo (5 digits, a non-hex char) parses fine as TOML but silently falls back
            // to the aurora default for that one field, so `from_toml` alone can't catch
            // it. Only `#` immediately preceded by a quote is a value (skips the header
            // comment's `#`).
            let bytes = body.as_bytes();
            for (i, _) in body.match_indices('#') {
                if i == 0 || bytes[i - 1] != b'"' {
                    continue; // not a quoted colour literal
                }
                let hex = &body[i + 1..(i + 7).min(body.len())];
                let closed = bytes.get(i + 7) == Some(&b'"');
                assert!(
                    hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) && closed,
                    "theme `{name}` has a malformed colour near `#{hex}`"
                );
            }
        }
    }

    #[test]
    fn roles_default_to_palette() {
        let t = Theme::aurora();
        // metadata defaults to the faint tier; the title sits one step brighter
        assert_eq!(
            t.meta_text(),
            t.text_faint,
            "metadata defaults to text_faint"
        );
        assert_eq!(t.title_text(), t.text_dim.mix(t.text, 0.6));
        assert_eq!(t.now_playing_color(), t.accent[0]);
        assert_eq!(t.toggle_on(), t.accent[1]);
    }

    #[test]
    fn theme_file_roles_override_title_and_meta() {
        let toml = r##"
            name = "x"
            track_title = "#112233"
            track_meta = "#445566"
            now_playing = "#aabbcc"
        "##;
        let t = Theme::from_toml("x", toml).unwrap();
        assert_eq!(t.title_text(), Rgb(0x11, 0x22, 0x33), "track_title honored");
        assert_eq!(t.meta_text(), Rgb(0x44, 0x55, 0x66), "track_meta honored");
        assert_eq!(t.now_playing_color(), Rgb(0xAA, 0xBB, 0xCC));
        // an omitted role still falls back to the palette default
        assert_eq!(t.toggle_on(), t.accent[1]);
    }

    #[test]
    fn dynamic_accent_overrides_accent_family_roles() {
        // a theme that pins its own now-playing / toggle colours
        let toml = r##"
            name = "x"
            now_playing = "#101010"
            toggle_on = "#202020"
            title = "#303030"
        "##;
        let mut t = Theme::from_toml("x", toml).unwrap();
        assert_eq!(t.now_playing_color(), Rgb(0x10, 0x10, 0x10));
        // album-art accent takes over the accent family...
        t.set_accent(Rgb(0xC0, 0x40, 0x80));
        assert_eq!(t.now_playing_color(), t.accent[0]);
        assert_eq!(t.toggle_on(), t.accent[1]);
        assert_eq!(t.marked_color(), t.accent[2]);
        // ...but neutral roles (title) are untouched
        assert_eq!(t.roles.title, Some(Rgb(0x30, 0x30, 0x30)));
    }
}
