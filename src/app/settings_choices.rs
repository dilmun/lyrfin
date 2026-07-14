//! The command palette's value-suggestion data: for a given [`Setting`], the
//! enumerable candidate values (each with a display label + the concrete value to
//! apply), and the single "set to this exact value" apply site. Both are keyed on
//! the one `Setting` vocabulary the Settings overlay already uses, so the palette
//! adds no parallel command universe — a new setting flows to the overlay AND the
//! palette from one place. Values are enumerated from the app's existing arrays and
//! enums (themes, icon presets, viz modes, …), never re-listed here.

use super::*;
use crate::ui::views::settings_rows::{
    LYRICS_ALIGN_LABELS, LYRICS_COLOR_LABELS, REPLAYGAIN_LABELS, SettingValue, setting_label_value,
};

/// One selectable value for a setting: what to show, what to apply, and whether it's
/// the value currently in effect (so the palette can mark it).
pub struct Choice {
    pub label: String,
    pub value: ChoiceValue,
    pub current: bool,
}

/// The concrete value a [`Choice`] applies, dispatched by [`AppState::apply_setting_value`].
#[derive(Clone)]
pub enum ChoiceValue {
    Bool(bool),
    Int(i64),
    Str(String),
}

/// The shape of a setting's values, driving how the palette presents them.
pub enum SettingChoices {
    /// A fixed list to pick from (themes, viz modes, replaygain modes, …).
    Discrete(Vec<Choice>),
    /// A number in `[min, max]`, chosen from a few handy presets or typed directly
    /// (the palette clamps a typed value to the range). The preset matching the
    /// current value is marked `current`.
    Bounded {
        min: i64,
        max: i64,
        presets: Vec<Choice>,
    },
    /// A plain on/off — the palette flips it in place rather than drilling.
    Toggle,
    /// Free text entered through the setting's own prompt (a path, a client id).
    FreeText,
    /// No value to pick — activating the setting just runs it (Rescan, a rebind, …).
    None,
}

/// Discrete choices from labels indexed by a stored `u8`/index (`current` = index).
fn idx_choices(labels: &[&str], current: usize) -> SettingChoices {
    SettingChoices::Discrete(
        labels
            .iter()
            .enumerate()
            .map(|(i, l)| Choice {
                label: (*l).to_string(),
                value: ChoiceValue::Int(i as i64),
                current: i == current,
            })
            .collect(),
    )
}

/// Discrete choices from string values (`current` = the matching value).
fn str_choices(items: &[&str], current: &str) -> SettingChoices {
    SettingChoices::Discrete(
        items
            .iter()
            .map(|v| Choice {
                label: (*v).to_string(),
                current: *v == current,
                value: ChoiceValue::Str((*v).to_string()),
            })
            .collect(),
    )
}

/// Discrete choices from `(value, label)` pairs keyed on the actual stored number
/// (not an index) — e.g. `320` kbps, `7` days.
fn int_choices(pairs: &[(i64, &str)], current: i64) -> Vec<Choice> {
    pairs
        .iter()
        .map(|(v, l)| Choice {
            label: (*l).to_string(),
            value: ChoiceValue::Int(*v),
            current: *v == current,
        })
        .collect()
}

/// A two-value, bool-backed setting shown with names rather than on/off
/// (e.g. circle/rounded, list/grid). `false` label first.
fn bool_choices(off: &str, on: &str, current: bool) -> SettingChoices {
    SettingChoices::Discrete(vec![
        Choice {
            label: off.to_string(),
            value: ChoiceValue::Bool(false),
            current: !current,
        },
        Choice {
            label: on.to_string(),
            value: ChoiceValue::Bool(true),
            current,
        },
    ])
}

impl AppState {
    /// The candidate values for `s`, enumerated from the app's existing value
    /// vocabularies, with the value currently in effect marked `current`.
    pub fn setting_choices(&self, s: Setting) -> SettingChoices {
        use Setting::*;
        match s {
            // theme slots — the shared `auto + builtins + custom` list
            Theme => self.theme_choices(&self.config.theme),
            LightTheme => self.theme_choices(&self.config.light_theme),
            DarkTheme => self.theme_choices(&self.config.dark_theme),

            IconSet => str_choices(&crate::icons::Icons::PRESETS, &self.config.icon_set),
            PlayerVizMode => idx_choices(
                &crate::ui::components::VIZ_MODES,
                self.config.player_viz_mode as usize,
            ),
            GridSize => {
                use crate::config::GridCardSize;
                let labels: Vec<&str> = GridCardSize::ALL.iter().map(|g| g.label()).collect();
                let cur = GridCardSize::ALL
                    .iter()
                    .position(|&g| g == self.config.grid_card_size)
                    .unwrap_or(1);
                idx_choices(&labels, cur)
            }
            TouchpadScroll => {
                use crate::config::TouchpadSpeed;
                let labels: Vec<&str> = TouchpadSpeed::ALL.iter().map(|t| t.label()).collect();
                let cur = TouchpadSpeed::ALL
                    .iter()
                    .position(|&t| t == self.config.touchpad_speed)
                    .unwrap_or(1);
                idx_choices(&labels, cur)
            }
            OverlaySize => idx_choices(
                &crate::config::OVERLAY_SIZE_LABELS,
                self.config.overlay_size as usize,
            ),
            ReplayGain => idx_choices(&REPLAYGAIN_LABELS, self.config.replaygain.min(2) as usize),
            LyricsAlign => idx_choices(
                &LYRICS_ALIGN_LABELS,
                self.config.lyrics_align.min(2) as usize,
            ),
            LyricsColor => idx_choices(
                &LYRICS_COLOR_LABELS,
                self.config.lyrics_color.min(4) as usize,
            ),
            LyricsGap => idx_choices(
                &["compact", "1 blank", "2 blank", "3 blank"],
                self.config.lyrics_gap.min(3) as usize,
            ),
            LyricsTranslate => SettingChoices::Discrete(
                crate::translate::LANGS
                    .iter()
                    .map(|(code, name)| Choice {
                        label: (*name).to_string(),
                        current: *code == self.config.lyrics_translate_to,
                        value: ChoiceValue::Str((*code).to_string()),
                    })
                    .collect(),
            ),

            // discrete by the actual stored number
            RadioRefresh => SettingChoices::Discrete(int_choices(
                &[
                    (0, "manual"),
                    (1, "daily"),
                    (3, "every 3 days"),
                    (7, "weekly"),
                    (14, "fortnightly"),
                    (30, "monthly"),
                ],
                self.config.radio_refresh_days as i64,
            )),
            SpotifyBitrate => SettingChoices::Discrete(int_choices(
                &[(96, "96 kbps"), (160, "160 kbps"), (320, "320 kbps")],
                self.config.spotify_bitrate as i64,
            )),

            // bounded numeric with presets
            Crossfade => SettingChoices::Bounded {
                min: 0,
                max: 12000,
                presets: int_choices(
                    &[(0, "off"), (2000, "2 s"), (4000, "4 s"), (8000, "8 s")],
                    self.config.crossfade_ms as i64,
                ),
            },
            Fps => SettingChoices::Bounded {
                min: 10,
                max: 144,
                presets: int_choices(
                    &[(30, "30 fps"), (60, "60 fps"), (120, "120 fps")],
                    self.config.fps as i64,
                ),
            },

            // bool-backed but shown with names (setting_label_value renders these as Text)
            GridShape => bool_choices("rounded", "circle", self.config.grid_circle),
            GridList => {
                let on = if self.layout == Layout::Spotify {
                    self.spotify.grid
                } else {
                    self.local.grid
                };
                bool_choices("list", "grid", on)
            }
            TrackColumns => bool_choices("rows", "columns", self.config.track_columns),
            PanesLayout => bool_choices("vertical", "horizontal", self.config.panes_horizontal),
            LyricsGradient => bool_choices("solid", "gradient", self.config.lyrics_gradient),

            // free-text entered through the setting's own prompt
            SpotifyClientId => SettingChoices::FreeText,

            // everything else: a plain toggle (detected generically) or a value-less
            // action (Rescan / a rebind / Spotify re-auth / a dock or size step / …)
            _ => match setting_label_value(self, &s).1 {
                SettingValue::Toggle(_) => SettingChoices::Toggle,
                SettingValue::Text(_) => SettingChoices::None,
            },
        }
    }

    fn theme_choices(&self, current: &str) -> SettingChoices {
        SettingChoices::Discrete(
            self.all_themes()
                .into_iter()
                .map(|name| Choice {
                    current: name == current,
                    label: name.clone(),
                    value: ChoiceValue::Str(name),
                })
                .collect(),
        )
    }

    /// Set `s` to the exact `value` a [`Choice`] carries, reusing the existing
    /// mutators so each setting's side effects + save-on-write are inherited. The one
    /// "set to X" site in the app; value-less settings fall through to
    /// [`Self::activate_setting`].
    pub(crate) fn apply_setting_value(&mut self, s: Setting, value: &ChoiceValue) {
        use ChoiceValue::{Bool, Int, Str};
        use Setting::*;
        match (s, value) {
            // ---- theme slots (string) ----
            (Theme, Str(name)) => {
                self.set_theme(name);
                self.config.save();
            }
            (LightTheme, Str(name)) => {
                self.config.light_theme = name.clone();
                self.config.save();
                self.apply_appearance(crate::appearance::detect());
            }
            (DarkTheme, Str(name)) => {
                self.config.dark_theme = name.clone();
                self.config.save();
                self.apply_appearance(crate::appearance::detect());
            }
            (IconSet, Str(name)) => self.set_icon_set(name),
            (LyricsTranslate, Str(code)) => {
                self.config.lyrics_translate_to = code.clone();
                self.config.save();
                self.reload_lyrics();
            }

            // ---- indexed discrete (a u8 into a labels array) ----
            (PlayerVizMode, Int(i)) => {
                self.config.player_viz_mode = *i as u8;
                self.config.save();
            }
            (GridSize, Int(i)) => {
                self.config.grid_card_size = crate::config::GridCardSize::ALL[(*i as usize).min(2)];
                self.config.save();
            }
            (TouchpadScroll, Int(i)) => {
                self.config.touchpad_speed =
                    crate::config::TouchpadSpeed::ALL[(*i as usize).min(2)];
                self.config.save();
            }
            (OverlaySize, Int(i)) => {
                self.config.overlay_size =
                    (*i as u8).min(crate::config::OVERLAY_SIZE_COUNT.saturating_sub(1));
                self.config.save();
            }
            (ReplayGain, Int(i)) => {
                self.config.replaygain = (*i as u8).min(2);
                self.config.save();
                self.refresh_replaygain();
            }
            (LyricsAlign, Int(i)) => {
                self.config.lyrics_align = (*i as u8).min(2);
                self.config.save();
            }
            (LyricsColor, Int(i)) => {
                self.config.lyrics_color = (*i as u8).min(4);
                self.config.save();
            }
            (LyricsGap, Int(i)) => {
                self.config.lyrics_gap = (*i as u8).min(3);
                self.config.save();
            }

            // ---- discrete by the actual stored number ----
            (RadioRefresh, Int(days)) => {
                self.config.radio_refresh_days = (*days).max(0) as u32;
                self.config.save();
            }
            (SpotifyBitrate, Int(b)) => {
                self.config.spotify_bitrate = *b as u16;
                self.config.save();
            }

            // ---- bounded numeric ----
            (Crossfade, Int(ms)) => self.set_crossfade((*ms).clamp(0, 12000) as u32),
            (Fps, Int(f)) => {
                self.config.fps = (*f).clamp(10, 144) as u8;
                self.config.save();
            }

            // ---- bool-backed, name-labelled (each needs the right side-effecting write) ----
            (GridShape, Bool(t)) => {
                if self.config.grid_circle != *t {
                    self.toggle_grid_shape();
                }
            }
            (GridList, Bool(t)) => {
                let cur = if self.layout == Layout::Spotify {
                    self.spotify.grid
                } else {
                    self.local.grid
                };
                if cur != *t {
                    self.toggle_grid_current();
                }
            }
            (TrackColumns, Bool(t)) => self.set_setting(|c| &mut c.track_columns, *t),
            (PanesLayout, Bool(t)) => self.set_setting(|c| &mut c.panes_horizontal, *t),
            (LyricsGradient, Bool(t)) => self.set_setting(|c| &mut c.lyrics_gradient, *t),

            // ---- generic toggle: flip to the target through activate_setting so its
            // side effects (reload cover, re-arm gapless, …) run exactly as from the overlay
            (_, Bool(target)) => {
                let cur = matches!(setting_label_value(self, &s).1, SettingValue::Toggle(true));
                if cur != *target {
                    self.activate_setting(s);
                }
            }

            // ---- no value to set (FreeText prompt / value-less action) ----
            _ => self.activate_setting(s),
        }
    }
}
