//! Equalizer overlay: behaviour (open/adjust/preset/save) + a render smoke test.

use super::*;
use crate::action::Action;

/// Open the EQ overlay via its default key path (the `OpenEqualizer` action).
fn open_eq(app: &mut AppState) {
    app.update(Action::OpenEqualizer);
    assert!(app.eq_open(), "EQ overlay opens");
}

#[test]
fn eq_opens_toggles_and_closes() {
    let mut app = demo();
    open_eq(&mut app);
    // the same action toggles it shut
    app.update(Action::OpenEqualizer);
    assert!(!app.eq_open(), "OpenEqualizer toggles the overlay closed");
    // Esc also closes
    open_eq(&mut app);
    app.update(Action::Back);
    assert!(!app.eq_open(), "Esc closes the overlay");
}

#[test]
fn adjusting_a_band_persists_enables_and_clamps() {
    let mut app = demo();
    open_eq(&mut app);
    // select the 3rd band (index 2 = 125 Hz) and boost it
    app.update(Action::EqSelect(1));
    app.update(Action::EqSelect(1));
    assert_eq!(app.eq.sel, 2);
    let before = app.config.eq_bands[2];
    app.update(Action::EqAdjust(1));
    assert_eq!(app.config.eq_bands[2], before + 1.0, "↑ adds 1 dB");
    assert!(app.config.eq_enabled, "editing a band switches the EQ on");
    // clamp at the ceiling no matter how many nudges
    for _ in 0..40 {
        app.update(Action::EqAdjust(1));
    }
    assert_eq!(
        app.config.eq_bands[2],
        crate::audio::eq::EQ_MAX_DB,
        "a band clamps at +12 dB"
    );
    // and at the floor
    for _ in 0..60 {
        app.update(Action::EqAdjust(-1));
    }
    assert_eq!(app.config.eq_bands[2], crate::audio::eq::EQ_MIN_DB);
}

#[test]
fn preamp_is_the_last_control() {
    let mut app = demo();
    open_eq(&mut app);
    // step left once from band 0 wraps to the preamp (the last control)
    app.update(Action::EqSelect(-1));
    assert_eq!(app.eq.sel, crate::app::EQ_PREAMP, "wraps onto the preamp");
    let before = app.config.eq_preamp;
    app.update(Action::EqAdjust(-1));
    assert_eq!(app.config.eq_preamp, before - 1.0, "↓ cuts the preamp 1 dB");
}

#[test]
fn power_toggle_is_independent_and_reset_flattens() {
    let mut app = demo();
    open_eq(&mut app);
    app.update(Action::EqTogglePower);
    assert!(app.config.eq_enabled, "power on");
    app.update(Action::EqTogglePower);
    assert!(!app.config.eq_enabled, "power off");
    // build a curve, then reset → flat
    app.update(Action::EqAdjust(1));
    app.update(Action::EqSelect(1));
    app.update(Action::EqAdjust(1));
    assert!(app.config.eq_bands.iter().any(|&b| b != 0.0));
    app.update(Action::EqReset);
    assert!(
        app.config.eq_bands.iter().all(|&b| b == 0.0) && app.config.eq_preamp == 0.0,
        "reset flattens every band + the preamp"
    );
    assert_eq!(app.config.eq_preset, "Flat");
}

#[test]
fn cycling_presets_applies_a_builtin_curve() {
    let mut app = demo();
    open_eq(&mut app);
    // Flat → Bass Boost (the next built-in)
    app.update(Action::EqCyclePreset(1));
    assert_eq!(app.config.eq_preset, "Bass Boost");
    let (_, curve) = crate::audio::eq::BUILTIN_EQ_PRESETS[1];
    assert_eq!(app.config.eq_bands, curve, "the preset's curve is applied");
    assert!(
        app.config.eq_enabled,
        "applying a preset switches the EQ on"
    );
}

#[test]
fn saving_a_custom_preset_round_trips_and_shows_up_in_the_cycle() {
    let mut app = demo();
    open_eq(&mut app);
    app.update(Action::EqAdjust(1)); // a distinctive curve (band 0 = +1)
    app.update(Action::EqBeginSave);
    for c in "My Mix".chars() {
        let s = app.eq.naming.clone().unwrap_or_default();
        app.update(Action::EqNameInput(format!("{s}{c}")));
    }
    app.update(Action::EqSavePreset);
    assert!(app.eq.naming.is_none(), "save closes the name field");
    assert!(
        app.eq_presets.iter().any(|p| p.name == "My Mix"),
        "the custom preset is stored"
    );
    assert_eq!(app.config.eq_preset, "My Mix");
    // it's reachable in the preset cycle (built-ins + customs)
    assert!(app.eq_preset_names().contains(&"My Mix".to_string()));
    // reloading the config from disk finds it (persisted to eq_presets.toml)
    assert!(
        app.config
            .load_eq_presets()
            .iter()
            .any(|p| p.name == "My Mix"),
        "the preset persisted to disk"
    );
}

#[test]
fn overlay_renders_faders_labels_and_preset() {
    let mut app = demo();
    open_eq(&mut app);
    app.update(Action::EqCyclePreset(1)); // Bass Boost, so bars are visible
    let out = render_layout(&mut app, Layout::Dashboard, 90, 30);
    assert!(out.contains("Equalizer"), "title shows:\n{out}");
    assert!(out.contains("Bass Boost"), "active preset shows");
    assert!(out.contains('█'), "fader bars are drawn");
    // the frequency labels and the preamp column label
    for label in ["31", "16k", "Pre"] {
        assert!(out.contains(label), "band label `{label}` shows:\n{out}");
    }
}
