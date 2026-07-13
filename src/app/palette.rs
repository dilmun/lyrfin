//! Command palette methods on `AppState` (extracted from app/mod.rs).

use super::*;

impl AppState {
    /// `sort` command: set the tracklist sort (e.g. `sort:artist,album,year,track`,
    /// `sort:off`). Empty spec re-applies the default. Persists to config.
    pub(crate) fn cmd_sort(&mut self, rest: &str) -> String {
        let rest = rest.trim();
        if rest.eq_ignore_ascii_case("off") || rest.eq_ignore_ascii_case("none") {
            self.sort.clear();
            self.config.sort_order = String::new();
            self.config.save();
            return "Sort: off".into();
        }
        let spec_str = if rest.is_empty() {
            "artist,year,album"
        } else {
            rest
        };
        let spec = parse_sort(spec_str);
        if spec.is_empty() {
            return format!("Unknown sort field(s): {spec_str}");
        }
        self.sort = spec;
        self.config.sort_order = self.sort_order_string();
        self.config.save();
        self.sort_browse();
        self.selection = 0;
        format!("Sorted by {}", self.sort_describe())
    }

    /// Command indices matching the palette query (best first).
    /// All palette entries: the static commands, a "bookmark this search" entry
    /// when a search is active, and one quick-jump entry per saved bookmark.
    pub fn palette_entries(&self) -> Vec<(String, String, Action)> {
        let mut v: Vec<(String, String, Action)> = palette_commands()
            .into_iter()
            .map(|(cat, l, a)| (cat.to_string(), l.to_string(), a))
            .collect();
        if !self.search.query.trim().is_empty() {
            v.push((
                "Library".into(),
                "Bookmark current search".into(),
                Action::BookmarkSearch,
            ));
        }
        for b in &self.bookmarks {
            v.push((
                "Library".into(),
                format!("★ {}", b.name),
                Action::RunSearch(b.query.clone()),
            ));
        }
        // command-line templates: selecting one pre-fills the input so you can
        // type the argument (then Enter runs the typed command).
        for (cat, label, prefix) in [
            ("Settings", "» theme <name>", "theme "),
            (
                "Settings",
                "» set <param> <value>  (volume/speed/fps/crossfade…)",
                "set ",
            ),
            (
                "Settings",
                "» toggle <param>  (gapless/mouse/shuffle/lyrics…)",
                "toggle ",
            ),
            ("Audio", "» replaygain <off|track|album>", "replaygain "),
            ("Audio", "» sleep <minutes|off>", "sleep "),
            ("Playback", "» repeat <off|one|all|album|artist>", "repeat "),
            ("View", "» layout <name>", "layout "),
            ("Library", "» sort:artist,year,album", "sort:"),
        ] {
            v.push((
                cat.into(),
                label.into(),
                Action::PalettePrefill(prefix.into()),
            ));
        }
        v
    }

    pub fn palette_matches(&self) -> Vec<usize> {
        let q = self
            .palette
            .as_ref()
            .map(|p| p.query.as_str())
            .unwrap_or("");
        let entries = self.palette_entries();
        let labels: Vec<&str> = entries.iter().map(|(_, l, _)| l.as_str()).collect();
        crate::library::search::rank_labels(&labels, q)
    }

    /// Activate the palette: a typed `verb args` line runs as a command; a bare
    /// query runs the fuzzy-selected entry (which may pre-fill a command template).
    pub(crate) fn palette_activate(&mut self) {
        let Some(p) = self.palette.as_ref() else {
            return;
        };
        let query = p.query.trim().to_string();
        // a command line is "verb arg…" (interior whitespace) or the colon form
        // "verb:args" (e.g. sort:artist,album) — but only when the first word is
        // an actual command verb, so multi-word entry *labels* ("Search Tags",
        // "Play random album") still pick their palette entry.
        let first = query
            .split(|c: char| c.is_whitespace() || c == ':')
            .next()
            .unwrap_or("")
            .to_lowercase();
        let is_verb = matches!(
            first.as_str(),
            "theme"
                | "set"
                | "toggle"
                | "volume"
                | "vol"
                | "speed"
                | "replaygain"
                | "rg"
                | "sleep"
                | "repeat"
                | "shuffle"
                | "layout"
                | "go"
                | "sort"
                | "stats"
                | "insights"
        );
        if is_verb && (query.split_whitespace().count() >= 2 || query.contains(':')) {
            self.palette = None;
            let msg = self.run_command(&query);
            self.notify(msg);
            return;
        }
        let entries = self.palette_entries();
        let labels: Vec<&str> = entries.iter().map(|(_, l, _)| l.as_str()).collect();
        let matches = crate::library::search::rank_labels(&labels, &p.query);
        let Some(&idx) = matches.get(p.sel) else {
            self.palette = None;
            return;
        };
        let action = entries[idx].2.clone();
        match action {
            // a template pre-fills the input and keeps the palette open
            Action::PalettePrefill(_) => self.update(action),
            _ => {
                self.palette = None;
                self.update(action);
            }
        }
    }

    pub(crate) fn cmd_theme(&mut self, name: &str) -> String {
        let name = name.trim();
        if name.is_empty() {
            return "Usage: theme <name>".into();
        }
        const BUILTIN: [&str; 6] = [
            "aurora",
            "cyberpunk",
            "glacier",
            "monolith",
            "highcontrast",
            "high-contrast",
        ];
        let file = self.config.themes_dir().join(format!("{name}.toml"));
        if !BUILTIN.contains(&name.to_lowercase().as_str()) && !file.exists() {
            return format!("No theme '{name}' (try: aurora, cyberpunk, glacier, monolith)");
        }
        self.set_theme(name);
        self.config.save();
        format!("Theme: {}", self.theme.name)
    }

    pub(crate) fn cmd_set(&mut self, rest: &str) -> String {
        let Some((key, val)) = rest.split_once(char::is_whitespace) else {
            return "Usage: set <param> <value>".into();
        };
        let key = key.to_lowercase().replace('-', "_");
        let val = val.trim();
        match key.as_str() {
            "volume" | "vol" => match val.parse::<u8>() {
                Ok(v) => {
                    self.update(Action::SetVolume(v.min(100)));
                    format!("Volume {}", self.player.volume)
                }
                Err(_) => "volume must be 0–100".into(),
            },
            "speed" => match val.parse::<f32>() {
                Ok(s) => {
                    let s = s.clamp(0.25, 2.0);
                    self.update(Action::SetSpeed(s));
                    format!("Speed {s:.2}×")
                }
                Err(_) => "speed must be 0.5–2.0".into(),
            },
            "fps" => match val.parse::<u8>() {
                Ok(v) => {
                    self.config.fps = v.clamp(10, 144);
                    self.config.save();
                    format!("FPS {}", self.config.fps)
                }
                Err(_) => "fps must be a number".into(),
            },
            "crossfade" | "crossfade_ms" => match val.parse::<u32>() {
                Ok(v) => {
                    self.set_crossfade(v);
                    format!("Crossfade {} ms", self.config.crossfade_ms)
                }
                Err(_) => "crossfade must be milliseconds".into(),
            },
            "sidebar" | "sidebar_width" => match val.parse::<u16>() {
                Ok(v) => {
                    self.config.dash_sidebar_w = v.clamp(16, 50);
                    self.config.save();
                    format!("Sidebar width {}", self.config.dash_sidebar_w)
                }
                Err(_) => "sidebar must be a number".into(),
            },
            "icons" | "iconset" | "icon_set" => {
                let v = val.trim().to_lowercase();
                if crate::icons::Icons::PRESETS.contains(&v.as_str()) {
                    self.set_icon_set(&v);
                    format!("Icons: {v}")
                } else {
                    "icons: outline | triangles | skip | nerd".into()
                }
            }
            "artist_width" => match val.parse::<u16>() {
                Ok(v) => {
                    self.config.dash_artist_w = v.clamp(20, 70);
                    self.config.save();
                    format!("Artist width {}", self.config.dash_artist_w)
                }
                Err(_) => "artist_width must be a number".into(),
            },
            "replaygain" | "rg" => {
                let mode = match val.to_lowercase().as_str() {
                    "off" | "none" | "0" => 0,
                    "track" | "1" => 1,
                    "album" | "2" => 2,
                    _ => return "replaygain: off | track | album".into(),
                };
                self.config.replaygain = mode;
                self.config.save();
                self.refresh_replaygain();
                format!("ReplayGain: {}", ["Off", "Track", "Album"][mode as usize])
            }
            "spotify_quality" | "spotify_bitrate" | "sp_quality" => {
                match val.trim().trim_end_matches("kbps").trim().parse::<u16>() {
                    Ok(b) if matches!(b, 96 | 160 | 320) => {
                        self.config.spotify_bitrate = b;
                        self.config.save();
                        let live = self.spov.session_cmd.is_some();
                        let note = if live { " (reconnect to apply)" } else { "" };
                        format!("Spotify quality: {b} kbps{note}")
                    }
                    _ => "spotify_quality: 96 | 160 | 320".into(),
                }
            }
            "theme" => self.cmd_theme(val),
            _ => format!("Unknown setting: {key}"),
        }
    }

    pub(crate) fn cmd_toggle(&mut self, key: &str) -> String {
        let key = key.trim().to_lowercase().replace('-', "_");
        match key.as_str() {
            "gapless" => {
                self.config.gapless = !self.config.gapless;
                self.config.save();
                self.update_gapless_next();
                format!("Gapless: {}", onoff(self.config.gapless))
            }
            "silence" | "silence_skip" => {
                self.config.silence_skip = !self.config.silence_skip;
                self.config.save();
                self.apply_silence_skip();
                format!("Silence-skip: {}", onoff(self.config.silence_skip))
            }
            "mouse" => {
                self.config.mouse = !self.config.mouse;
                self.config.save();
                format!("Mouse: {}", onoff(self.config.mouse))
            }
            "next" | "next_hint" => {
                self.config.next_hint = !self.config.next_hint;
                self.config.save();
                format!("Next hint: {}", onoff(self.config.next_hint))
            }
            "album_art" | "art" => {
                self.config.album_art = !self.config.album_art;
                self.config.save();
                self.reload_cover();
                format!("Album art: {}", onoff(self.config.album_art))
            }
            "reduced_motion" => {
                self.config.reduced_motion = !self.config.reduced_motion;
                self.config.save();
                format!("Reduced motion: {}", onoff(self.config.reduced_motion))
            }
            "peak_caps" => {
                self.config.peak_caps = !self.config.peak_caps;
                self.config.save();
                format!("Peak caps: {}", onoff(self.config.peak_caps))
            }
            "shuffle" => {
                self.player.toggle_shuffle();
                format!("Shuffle: {}", onoff(self.player.shuffle))
            }
            "lyrics" => {
                self.toggle_panel(Panel::Lyrics);
                format!("Lyrics pane: {}", onoff(self.panel(Panel::Lyrics).shown))
            }
            "karaoke" => {
                self.config.lyrics_karaoke = !self.config.lyrics_karaoke;
                self.config.save();
                format!("Karaoke: {}", onoff(self.config.lyrics_karaoke))
            }
            "dual" | "translation" | "translations" => {
                self.config.lyrics_dual = !self.config.lyrics_dual;
                self.config.save();
                format!("Translations: {}", onoff(self.config.lyrics_dual))
            }
            "teleprompter" => {
                self.config.lyrics_teleprompter = !self.config.lyrics_teleprompter;
                self.config.save();
                format!("Teleprompter: {}", onoff(self.config.lyrics_teleprompter))
            }
            "accent" | "dynamic_accent" => {
                self.config.dynamic_accent = !self.config.dynamic_accent;
                self.config.save();
                self.reload_cover();
                format!("Dynamic accent: {}", onoff(self.config.dynamic_accent))
            }
            "player_viz" | "playerviz" => {
                self.config.player_viz = !self.config.player_viz;
                self.config.save();
                format!("Playback visualizer: {}", onoff(self.config.player_viz))
            }
            "artist_info" | "info" => {
                let d = Layout::Dashboard.default_panel(Panel::Artist);
                let e = self
                    .views
                    .panels
                    .entry((Layout::Dashboard, Panel::Artist))
                    .or_insert(d);
                e.shown = !e.shown;
                let shown = e.shown;
                if shown {
                    self.request_artist_info();
                }
                format!("Artist info: {}", onoff(shown))
            }
            _ => format!("Can't toggle: {key}"),
        }
    }

    /// Execute a typed command line (e.g. `theme cyberpunk`, `set volume 80`,
    /// `toggle gapless`). Returns a status message for the toast.
    pub(crate) fn run_command(&mut self, input: &str) -> String {
        let input = input.trim();
        // verb ends at the first whitespace OR ':' (so `sort:a,b` and `set x y`
        // both work); the separator itself is dropped.
        let (verb, rest) = match input.find(|c: char| c.is_whitespace() || c == ':') {
            Some(i) => (input[..i].to_lowercase(), input[i + 1..].trim().to_string()),
            None => (input.to_lowercase(), String::new()),
        };
        match verb.as_str() {
            "theme" => self.cmd_theme(&rest),
            "set" => self.cmd_set(&rest),
            "toggle" => self.cmd_toggle(&rest),
            "volume" | "vol" => self.cmd_set(&format!("volume {rest}")),
            "speed" => self.cmd_set(&format!("speed {rest}")),
            "replaygain" | "rg" => self.cmd_set(&format!("replaygain {rest}")),
            "sleep" => {
                let m = if rest.eq_ignore_ascii_case("off") || rest.is_empty() {
                    0
                } else {
                    match rest.parse::<u32>() {
                        Ok(m) => m,
                        Err(_) => return "sleep <minutes|off>".into(),
                    }
                };
                self.set_sleep_timer(m);
                if m == 0 {
                    "Sleep timer off".into()
                } else {
                    format!("Sleep timer: {m} min")
                }
            }
            "repeat" => {
                self.player.repeat = match rest.to_lowercase().as_str() {
                    "off" => Repeat::Off,
                    "all" => Repeat::All,
                    "one" => Repeat::One,
                    _ => return "repeat: off | all | one".into(),
                };
                format!("Repeat: {rest}")
            }
            "shuffle" => {
                self.player.shuffle = match rest.to_lowercase().as_str() {
                    "on" | "true" | "1" => true,
                    "off" | "false" | "0" => false,
                    "" => !self.player.shuffle,
                    _ => return "shuffle: on | off".into(),
                };
                format!("Shuffle: {}", onoff(self.player.shuffle))
            }
            "layout" | "go" => {
                if matches!(rest.to_lowercase().as_str(), "settings" | "config") {
                    self.open_settings();
                    return "Settings".into();
                }
                let l = match rest.to_lowercase().as_str() {
                    "dashboard" | "home" => Layout::Dashboard,
                    "player" | "nowplaying" => Layout::FullPlayer,
                    "lyrics" => Layout::LyricsFocus,
                    "visualizer" | "viz" => Layout::FullPlayer, // viz lives in Now Playing
                    "concert" | "focus" => Layout::Concert,
                    _ => {
                        return "layout: dashboard|player|library|lyrics|visualizer|concert".into();
                    }
                };
                self.update(Action::SwitchLayout(l));
                format!("Layout: {rest}")
            }
            "sort" => self.cmd_sort(&rest),
            "stats" | "insights" => {
                self.toggle_info(crate::app::InfoTab::Stats);
                let open = matches!(
                    self.info.as_ref().map(|i| i.tab),
                    Some(crate::app::InfoTab::Stats)
                );
                format!("Stats: {}", onoff(open))
            }
            "" => "Type a command, e.g. `theme cyberpunk`".into(),
            other => format!("Unknown command: {other}"),
        }
    }
}

/// Command-palette state (fuzzy-runnable actions).
pub struct Palette {
    pub query: String,
    pub sel: usize,
}
