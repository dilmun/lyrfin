//! Equalizer overlay state + logic on `AppState`: open/close, band selection and
//! adjustment, preset cycling, saving/deleting custom presets, and pushing the
//! resulting curve to the audio engine. Pure state transitions (Action →
//! `update`); the DSP lives in [`crate::audio::eq`] and the rendering in
//! [`crate::ui::views::equalizer_view`]. The config holds the persisted values
//! (`config.eq_*`); this module only orchestrates edits + the engine hand-off.

use super::*;
use crate::audio::eq::{BUILTIN_EQ_PRESETS, EQ_BANDS, EQ_MAX_DB, EQ_MIN_DB, matching_preset};
use crate::config::EqPreset;

/// Selectable controls in the overlay: the 10 bands, then the preamp (last).
pub const EQ_CONTROLS: usize = EQ_BANDS + 1;
/// Index of the preamp within the selectable control row (after the bands).
pub const EQ_PREAMP: usize = EQ_BANDS;

/// Equalizer overlay UI state. Pure presentation state — the dB values live in
/// `config` (`eq_bands` / `eq_preamp` / `eq_enabled` / `eq_preset`).
#[derive(Default)]
pub struct EqUi {
    /// The overlay is open (a modal — captures its own keys, gates the view).
    pub open: bool,
    /// Highlighted control: `0..EQ_BANDS` selects a band, `EQ_PREAMP` the preamp.
    pub sel: usize,
    /// `Some(buffer)` while typing a name to save the current curve as a preset.
    pub naming: Option<String>,
}

impl AppState {
    /// Whether the Equalizer overlay owns the screen.
    pub fn eq_open(&self) -> bool {
        self.eq.open
    }

    /// Toggle the Equalizer overlay (opening resets the cursor + any name entry).
    pub(crate) fn toggle_equalizer(&mut self) {
        if self.eq.open {
            self.close_equalizer();
        } else {
            self.eq.open = true;
            self.eq.sel = 0;
            self.eq.naming = None;
        }
    }

    /// Close the overlay (and abandon a half-typed preset name).
    pub(crate) fn close_equalizer(&mut self) {
        self.eq.open = false;
        self.eq.naming = None;
    }

    /// Push the current EQ curve to the audio engine. Cheap (a single command);
    /// called after every edit and on track load, so playback applies it live
    /// without a restart.
    pub(crate) fn apply_eq(&self) {
        self.engine
            .send(AudioCommand::SetEq(self.config.eq_config()));
    }

    /// Move the selection left/right across the bands + preamp (wraps).
    pub(crate) fn eq_select(&mut self, dir: i32) {
        let n = EQ_CONTROLS as i32;
        self.eq.sel = (((self.eq.sel as i32 + dir) % n + n) % n) as usize;
    }

    /// Adjust the selected control by `dir` dB (±1), clamped to the valid range.
    /// Editing implies wanting to hear it, so the EQ is switched on if it was off.
    pub(crate) fn eq_adjust(&mut self, dir: i32) {
        let delta = dir as f32;
        if self.eq.sel == EQ_PREAMP {
            self.config.eq_preamp = (self.config.eq_preamp + delta).clamp(EQ_MIN_DB, EQ_MAX_DB);
        } else if let Some(b) = self.config.eq_bands.get_mut(self.eq.sel) {
            *b = (*b + delta).clamp(EQ_MIN_DB, EQ_MAX_DB);
        }
        self.eq_touched();
    }

    /// Reset the selected control to 0 dB.
    pub(crate) fn eq_reset_selected(&mut self) {
        if self.eq.sel == EQ_PREAMP {
            self.config.eq_preamp = 0.0;
        } else if let Some(b) = self.config.eq_bands.get_mut(self.eq.sel) {
            *b = 0.0;
        }
        self.eq_touched();
    }

    /// Reset every band and the preamp to flat (leaves the on/off state as-is).
    pub(crate) fn eq_reset(&mut self) {
        self.config.eq_bands = [0.0; EQ_BANDS];
        self.config.eq_preamp = 0.0;
        self.config.eq_preset = "Flat".into();
        self.config.save();
        self.apply_eq();
        self.notify("Equalizer: flat".into());
    }

    /// Toggle the EQ on/off (the master power switch).
    pub(crate) fn eq_toggle_power(&mut self) {
        self.config.eq_enabled = !self.config.eq_enabled;
        self.config.save();
        self.apply_eq();
        self.notify(
            if self.config.eq_enabled {
                "Equalizer on"
            } else {
                "Equalizer off"
            }
            .into(),
        );
    }

    /// After any band/preamp edit: refresh the preset label, enable the EQ (so the
    /// change is audible), persist, and re-apply live.
    fn eq_touched(&mut self) {
        self.config.eq_preset = self.current_preset_name();
        self.config.eq_enabled = true;
        self.config.save();
        self.apply_eq();
    }

    /// The name of whatever preset the current bands match — a built-in first,
    /// then a custom one — else "Custom". (Preset identity is the band curve only;
    /// the preamp isn't part of the match.)
    fn current_preset_name(&self) -> String {
        if let Some(n) = matching_preset(&self.config.eq_bands) {
            return n.to_string();
        }
        if let Some(p) = self
            .eq_presets
            .iter()
            .find(|p| p.bands == self.config.eq_bands)
        {
            return p.name.clone();
        }
        "Custom".to_string()
    }

    /// Every preset name in cycle order: the built-ins, then the user's customs.
    pub(crate) fn eq_preset_names(&self) -> Vec<String> {
        BUILTIN_EQ_PRESETS
            .iter()
            .map(|(n, _)| n.to_string())
            .chain(self.eq_presets.iter().map(|p| p.name.clone()))
            .collect()
    }

    /// Cycle to the next/previous preset and apply it (live preview).
    pub(crate) fn eq_cycle_preset(&mut self, dir: i32) {
        let names = self.eq_preset_names();
        if names.is_empty() {
            return;
        }
        let cur = names
            .iter()
            .position(|n| *n == self.config.eq_preset)
            .unwrap_or(0) as i32;
        let n = names.len() as i32;
        let idx = (((cur + dir) % n + n) % n) as usize;
        let name = names[idx].clone();
        self.apply_preset_by_name(&name);
    }

    /// Apply a preset (built-in or custom) by name: set the bands (custom presets
    /// also carry a preamp), switch the EQ on, persist, and re-apply live.
    pub(crate) fn apply_preset_by_name(&mut self, name: &str) {
        if let Some((_, curve)) = BUILTIN_EQ_PRESETS.iter().find(|(n, _)| *n == name) {
            self.config.eq_bands = *curve;
            // built-in presets define band gains only — keep the user's preamp
        } else if let Some(p) = self.eq_presets.iter().find(|p| p.name == name) {
            self.config.eq_bands = p.bands;
            self.config.eq_preamp = p.preamp;
        } else {
            return;
        }
        self.config.eq_preset = name.to_string();
        self.config.eq_enabled = true; // applying a preset implies using it
        self.config.save();
        self.apply_eq();
        self.notify(format!("Equalizer: {name}"));
    }

    /// Begin naming a new custom preset from the current curve (inline name field).
    pub(crate) fn eq_begin_save(&mut self) {
        self.eq.naming = Some(String::new());
    }

    /// Update the save-preset name buffer (typing).
    pub(crate) fn eq_name_input(&mut self, s: String) {
        if self.eq.naming.is_some() {
            self.eq.naming = Some(s);
        }
    }

    /// Commit the pending custom preset from the name field. Overwrites an
    /// existing custom of the same name; refuses an empty name or a built-in name.
    pub(crate) fn eq_save_preset(&mut self) {
        let Some(name) = self.eq.naming.take() else {
            return;
        };
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        if BUILTIN_EQ_PRESETS
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(&name))
        {
            self.notify(format!("“{name}” is a built-in preset name"));
            return;
        }
        let preset = EqPreset {
            name: name.clone(),
            preamp: self.config.eq_preamp,
            bands: self.config.eq_bands,
        };
        match self.eq_presets.iter_mut().find(|p| p.name == name) {
            Some(existing) => *existing = preset,
            None => self.eq_presets.push(preset),
        }
        self.config.save_eq_presets(&self.eq_presets);
        self.config.eq_preset = name.clone();
        self.config.save();
        self.notify(format!("Saved preset “{name}”"));
    }

    /// Delete the active preset if it's a custom one (built-ins can't be deleted).
    pub(crate) fn eq_delete_preset(&mut self) {
        let name = self.config.eq_preset.clone();
        if let Some(pos) = self.eq_presets.iter().position(|p| p.name == name) {
            self.eq_presets.remove(pos);
            self.config.save_eq_presets(&self.eq_presets);
            self.config.eq_preset = self.current_preset_name();
            self.config.save();
            self.notify(format!("Deleted preset “{name}”"));
        } else {
            self.notify("Only custom presets can be deleted".into());
        }
    }
}
