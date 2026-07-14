//! Command palette methods on `AppState` (extracted from app/mod.rs).

use super::*;
use crate::ui::views::settings_rows::{SettingValue, setting_label_value};

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

    /// Every root-level palette row: the static action commands, then every reachable
    /// setting (each shown with its current value), a "bookmark this search" entry
    /// when a search is active, and one quick-jump entry per saved bookmark. This is
    /// the guided entry point — you fuzzy-find a setting and drill into its values,
    /// rather than remembering a command syntax.
    pub fn palette_entries(&self) -> Vec<PaletteEntry> {
        let mut v: Vec<PaletteEntry> = palette_commands()
            .into_iter()
            .map(|(cat, l, a)| PaletteEntry {
                category: cat.to_string(),
                label: l.to_string(),
                value: None,
                action: a,
            })
            .collect();
        // every reachable setting, shown with its current value. The per-index
        // Keybind / MusicDir rows are dropped — rebinding and dir removal belong in
        // the settings overlay, and would flood this list.
        for s in self.settings_items() {
            if matches!(s, Setting::Keybind(_) | Setting::MusicDir(_)) {
                continue;
            }
            let (label, val) = setting_label_value(self, &s);
            let value = match val {
                SettingValue::Toggle(b) => Some(onoff(b).to_string()),
                SettingValue::Text(t) if t.is_empty() => None,
                SettingValue::Text(t) => Some(t),
            };
            v.push(PaletteEntry {
                category: s.group().to_string(),
                label,
                value,
                action: Action::PaletteOpenSetting(s),
            });
        }
        if !self.search.query.trim().is_empty() {
            v.push(PaletteEntry {
                category: "Library".into(),
                label: "Bookmark current search".into(),
                value: None,
                action: Action::BookmarkSearch,
            });
        }
        for b in &self.bookmarks {
            v.push(PaletteEntry {
                category: "Library".into(),
                label: format!("★ {}", b.name),
                value: None,
                action: Action::RunSearch(b.query.clone()),
            });
        }
        v
    }

    /// The selectable value rows for a setting's drill-in picker: its discrete choices
    /// or its bounded presets. Empty for value-less settings (they never drill).
    pub(crate) fn drill_choices(&self, s: Setting) -> Vec<Choice> {
        match self.setting_choices(s) {
            SettingChoices::Discrete(v) => v,
            SettingChoices::Bounded { presets, .. } => presets,
            SettingChoices::Toggle | SettingChoices::FreeText | SettingChoices::None => Vec::new(),
        }
    }

    /// Row indices matching the palette query (best first) at the current level —
    /// the root entry list, or a setting's value list.
    pub fn palette_matches(&self) -> Vec<usize> {
        let Some(p) = self.palette.as_ref() else {
            return Vec::new();
        };
        let labels: Vec<String> = match p.ctx {
            PaletteCtx::Root => self
                .palette_entries()
                .into_iter()
                .map(|e| e.label)
                .collect(),
            PaletteCtx::Setting(s) => self.drill_choices(s).into_iter().map(|c| c.label).collect(),
        };
        let refs: Vec<&str> = labels.iter().map(|l| l.as_str()).collect();
        crate::library::search::rank_labels(&refs, &p.query)
    }

    /// Open a setting from the root list: drill into its value picker, or — for a
    /// plain toggle / value-less action — run it immediately and close.
    pub(crate) fn palette_open_setting(&mut self, s: Setting) {
        match self.setting_choices(s) {
            SettingChoices::Discrete(_) | SettingChoices::Bounded { .. } => {
                if let Some(p) = self.palette.as_mut() {
                    p.ctx = PaletteCtx::Setting(s);
                    p.query.clear();
                    p.sel = 0;
                }
            }
            // a toggle flips in place; a value-less setting runs its own action
            // (Rescan, the Spotify prompts, a dock / size step) — either way, done.
            SettingChoices::Toggle | SettingChoices::None | SettingChoices::FreeText => {
                self.activate_setting(s);
                self.palette = None;
            }
        }
    }

    /// Activate the current palette row: at the root, run the fuzzy-selected entry (or
    /// a typed `verb args` command); in a value picker, apply the chosen value.
    pub(crate) fn palette_activate(&mut self) {
        let Some(p) = self.palette.as_ref() else {
            return;
        };
        let (sel, query) = (p.sel, p.query.trim().to_string());
        match p.ctx {
            PaletteCtx::Root => self.palette_activate_root(sel, query),
            PaletteCtx::Setting(s) => self.palette_apply_choice(s, sel, query),
        }
    }

    fn palette_activate_root(&mut self, sel: usize, query: String) {
        // a typed command line is "verb arg…" (interior whitespace) or the colon form
        // "verb:args" (e.g. sort:artist,album) — but only when the first word is an
        // actual command verb, so multi-word entry *labels* ("Search Tags", "Play
        // random album") still pick their palette entry. This is the power-user fast
        // path; the guided setting rows are the primary way in.
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
        let labels: Vec<&str> = entries.iter().map(|e| e.label.as_str()).collect();
        let matches = crate::library::search::rank_labels(&labels, &query);
        let Some(&idx) = matches.get(sel) else {
            self.palette = None;
            return;
        };
        match entries[idx].action.clone() {
            // opening a setting manages the palette itself (drill in, or run + close)
            a @ Action::PaletteOpenSetting(_) => self.update(a),
            // a template pre-fills the input and keeps the palette open
            a @ Action::PalettePrefill(_) => self.update(a),
            a => {
                self.palette = None;
                self.update(a);
            }
        }
    }

    fn palette_apply_choice(&mut self, s: Setting, sel: usize, query: String) {
        // a bounded numeric setting also accepts a typed value (clamped to range)
        if let SettingChoices::Bounded { min, max, .. } = self.setting_choices(s)
            && let Ok(n) = query.parse::<i64>()
        {
            self.apply_setting_value(s, &ChoiceValue::Int(n.clamp(min, max)));
            self.palette = None;
            return;
        }
        let choices = self.drill_choices(s);
        let labels: Vec<&str> = choices.iter().map(|c| c.label.as_str()).collect();
        let matches = crate::library::search::rank_labels(&labels, &query);
        if let Some(&idx) = matches.get(sel) {
            let value = choices[idx].value.clone();
            self.apply_setting_value(s, &value);
        }
        self.palette = None;
    }

    /// Map a typed `set <name>` / `toggle <name>` param to the [`Setting`] it drives,
    /// so the typed command path and the palette apply through one site. Non-setting
    /// params (volume / speed / shuffle / pane widths / the panel toggles) are handled
    /// inline by the callers, and `theme` has its own [`Self::cmd_theme`].
    fn setting_for_param(name: &str) -> Option<Setting> {
        use Setting::*;
        Some(match name {
            "fps" => Fps,
            "crossfade" | "crossfade_ms" => Crossfade,
            "icons" | "iconset" | "icon_set" => IconSet,
            "replaygain" | "rg" => ReplayGain,
            "spotify_quality" | "spotify_bitrate" | "sp_quality" => SpotifyBitrate,
            "gapless" => Gapless,
            "silence" | "silence_skip" => SilenceSkip,
            "mouse" => Mouse,
            "next" | "next_hint" => NextHint,
            "album_art" | "art" => AlbumArt,
            "reduced_motion" => ReducedMotion,
            "peak_caps" => PeakCaps,
            "karaoke" => LyricsKaraoke,
            "dual" | "translation" | "translations" => LyricsDual,
            "teleprompter" => LyricsTeleprompter,
            "accent" | "dynamic_accent" => DynamicAccent,
            "player_viz" | "playerviz" => PlayerViz,
            _ => return None,
        })
    }

    /// A short "Label: value" status toast for a setting's current state.
    fn setting_status(&self, s: Setting) -> String {
        let (label, val) = setting_label_value(self, &s);
        match val {
            SettingValue::Toggle(b) => format!("{label}: {}", onoff(b)),
            SettingValue::Text(t) => format!("{label}: {t}"),
        }
    }

    /// A usage hint listing a setting's valid values (for a rejected typed value).
    fn setting_value_hint(&self, s: Setting, key: &str) -> String {
        match self.setting_choices(s) {
            SettingChoices::Discrete(cs) => {
                let opts: Vec<&str> = cs.iter().map(|c| c.label.as_str()).collect();
                format!("{key}: {}", opts.join(" | "))
            }
            SettingChoices::Bounded { min, max, .. } => format!("{key}: {min}–{max}"),
            SettingChoices::Toggle => format!("{key}: on | off"),
            SettingChoices::FreeText | SettingChoices::None => format!("{key}: ?"),
        }
    }

    /// Parse `val` into the right value for `s` (reusing its [`Self::setting_choices`])
    /// and apply it through the one [`Self::apply_setting_value`] site. Returns the
    /// status toast, or `None` if `val` isn't a valid value for `s`.
    fn apply_setting_from_str(&mut self, s: Setting, val: &str) -> Option<String> {
        let value = match self.setting_choices(s) {
            SettingChoices::Discrete(cs) => cs
                .iter()
                .find(|c| {
                    c.label.eq_ignore_ascii_case(val)
                        || match &c.value {
                            ChoiceValue::Str(t) => t.eq_ignore_ascii_case(val),
                            ChoiceValue::Int(n) => val.parse::<i64>().ok() == Some(*n),
                            ChoiceValue::Bool(_) => false,
                        }
                })
                .map(|c| c.value.clone())?,
            SettingChoices::Bounded { min, max, .. } => {
                ChoiceValue::Int(val.parse::<i64>().ok()?.clamp(min, max))
            }
            SettingChoices::Toggle => ChoiceValue::Bool(parse_onoff(val)?),
            SettingChoices::FreeText | SettingChoices::None => return None,
        };
        self.apply_setting_value(s, &value);
        Some(self.setting_status(s))
    }

    pub(crate) fn cmd_theme(&mut self, name: &str) -> String {
        let name = name.trim();
        if name.is_empty() {
            return "Usage: theme <name>".into();
        }
        // route through the one apply site; the valid set is `all_themes()` (via
        // setting_choices), so there's no separate builtin list to drift.
        self.apply_setting_from_str(Setting::Theme, name)
            .unwrap_or_else(|| {
                format!(
                    "No theme '{name}' (try: {})",
                    crate::ui::theme::BUILTIN_THEMES.join(", ")
                )
            })
    }

    pub(crate) fn cmd_set(&mut self, rest: &str) -> String {
        let Some((key, val)) = rest.split_once(char::is_whitespace) else {
            return "Usage: set <param> <value>".into();
        };
        let key = key.to_lowercase().replace('-', "_");
        let val = val.trim();
        // setting-backed params share the palette's one apply site
        if let Some(s) = Self::setting_for_param(&key) {
            return self
                .apply_setting_from_str(s, val)
                .unwrap_or_else(|| self.setting_value_hint(s, &key));
        }
        // non-setting params (transport + pane geometry) stay inline
        match key.as_str() {
            "theme" => self.cmd_theme(val),
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
            "sidebar" | "sidebar_width" => match val.parse::<u16>() {
                Ok(v) => {
                    self.config.dash_sidebar_w = v.clamp(16, 50);
                    self.config.save();
                    format!("Sidebar width {}", self.config.dash_sidebar_w)
                }
                Err(_) => "sidebar must be a number".into(),
            },
            "artist_width" => match val.parse::<u16>() {
                Ok(v) => {
                    self.config.dash_artist_w = v.clamp(20, 70);
                    self.config.save();
                    format!("Artist width {}", self.config.dash_artist_w)
                }
                Err(_) => "artist_width must be a number".into(),
            },
            _ => format!("Unknown setting: {key}"),
        }
    }

    pub(crate) fn cmd_toggle(&mut self, key: &str) -> String {
        let key = key.trim().to_lowercase().replace('-', "_");
        // setting-backed booleans flip through the one apply path (activate_setting)
        if let Some(s) = Self::setting_for_param(&key) {
            return if matches!(self.setting_choices(s), SettingChoices::Toggle) {
                self.activate_setting(s);
                self.setting_status(s)
            } else {
                format!("Can't toggle: {key}")
            };
        }
        // non-setting toggles: player shuffle + the two panel-visibility toggles
        match key.as_str() {
            "shuffle" => {
                self.player.toggle_shuffle();
                format!("Shuffle: {}", onoff(self.player.shuffle))
            }
            "lyrics" => {
                self.toggle_panel(Panel::Lyrics);
                format!("Lyrics pane: {}", onoff(self.panel(Panel::Lyrics).shown))
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

/// Parse an on/off word for `set <toggle> <value>` (`on`/`off`, plus the usual
/// aliases). `None` if it isn't a recognisable boolean.
fn parse_onoff(s: &str) -> Option<bool> {
    match s.trim().to_lowercase().as_str() {
        "on" | "true" | "1" | "yes" => Some(true),
        "off" | "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

/// Command-palette state. `ctx` is the current level: the root action + settings
/// list, or a specific setting's value picker (drill-in).
pub struct Palette {
    pub query: String,
    pub sel: usize,
    pub ctx: PaletteCtx,
}

/// The palette's current level.
#[derive(Clone, Copy)]
pub enum PaletteCtx {
    /// The flat list of actions + every reachable setting (the entry point).
    Root,
    /// A value picker for one setting (its `setting_choices`).
    Setting(Setting),
}

/// One root-level palette row: an action, or a setting shown with its current value.
pub struct PaletteEntry {
    pub category: String,
    pub label: String,
    pub value: Option<String>,
    pub action: Action,
}
