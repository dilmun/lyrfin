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

    /// Reveal the highlighted setting in the full Settings overlay (the `→` key),
    /// focused on its row in context with the rest of its group. Non-setting rows
    /// (plain actions) have nothing to reveal, so it's a no-op there.
    pub(crate) fn palette_reveal_selected(&mut self) {
        let Some(p) = self.palette.as_ref() else {
            return;
        };
        if !matches!(p.ctx, PaletteCtx::Root) {
            return;
        }
        let sel = p.sel;
        let entries = self.palette_entries();
        let Some(&idx) = self.palette_matches().get(sel) else {
            return;
        };
        if let Action::PaletteOpenSetting(s) = &entries[idx].action {
            let s = *s;
            self.palette = None;
            self.open_settings_group(s.group());
            if let Some(i) = self.settings_group_items().iter().position(|&x| x == s) {
                self.settings.sel = i;
            }
        }
    }

    /// Activate the current palette row: at the root, open/run the fuzzy-selected
    /// entry; in a value picker, apply the chosen value.
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

    /// Run a `RunCommand` action's payload. Only the tracklist `sort:` spec flows
    /// through here now (its palette rows); the old typed setting verbs are gone —
    /// settings are changed via the guided rows / value pickers.
    pub(crate) fn run_command(&mut self, input: &str) -> String {
        let input = input.trim();
        let (verb, rest) = match input.find(|c: char| c.is_whitespace() || c == ':') {
            Some(i) => (input[..i].to_lowercase(), input[i + 1..].trim().to_string()),
            None => (input.to_lowercase(), String::new()),
        };
        match verb.as_str() {
            "sort" => self.cmd_sort(&rest),
            other => format!("Unknown command: {other}"),
        }
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
