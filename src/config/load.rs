//! Config persistence: load `config.toml` over the defaults, write the current
//! settings back, seed the first-run scaffold, and resolve the platform config
//! directory. All filesystem + environment access for `Config` lives here; the
//! schema (the `Config` data model) stays in the parent module.

use std::path::{Path, PathBuf};

use super::{
    BUNDLED_THEMES, Config, ConfigFile, EqPreset, EqPresetsFile, GridCardSize, Keymap,
    TouchpadSpeed,
};
use crate::audio::eq::{EQ_BANDS, EQ_MAX_DB, EQ_MIN_DB};

impl Config {
    pub fn load_or_default() -> Self {
        let mut cfg = Config::default();
        let dir = config_dir();
        cfg.dir = dir.clone();
        Self::ensure_scaffold(&dir);

        let path = dir.join("config.toml");
        if let Ok(text) = std::fs::read_to_string(&path) {
            match toml::from_str::<ConfigFile>(&text) {
                Ok(f) => cfg.apply(f),
                Err(e) => {
                    // Run on in-memory defaults, but DON'T touch config.toml (no
                    // backup, no overwrite) — saves are blocked while this is set,
                    // so the user's fixable file is preserved. Surface it (with the
                    // parse location) in the status bar. The first error line is the
                    // "TOML parse error at line N, column M" locator.
                    let loc = e.to_string();
                    let loc = loc.lines().next().unwrap_or("parse error");
                    cfg.config_error = Some(format!(
                        "config.toml — {loc}. Running on defaults; fix it and restart \
                         (settings won't be saved until then)."
                    ));
                    eprintln!("lyrfin: {}\n{e}", cfg.config_error.as_deref().unwrap_or(""));
                }
            }
        }
        cfg.keymap = Keymap::load(&dir);
        cfg
    }

    fn apply(&mut self, f: ConfigFile) {
        if let Some(v) = f.music_dirs {
            self.music_dirs = v;
        }
        if let Some(v) = f.theme {
            self.theme = v;
        }
        if let Some(v) = f.theme_follows_system {
            self.theme_follows_system = v;
        }
        if let Some(v) = f.light_theme {
            self.light_theme = v;
        }
        if let Some(v) = f.dark_theme {
            self.dark_theme = v;
        }
        if let Some(v) = f.volume {
            self.volume = v.min(100);
        }
        if let Some(v) = f.gapless {
            self.gapless = v;
        }
        if let Some(v) = f.crossfade_ms {
            self.crossfade_ms = v;
        }
        if let Some(v) = f.silence_skip {
            self.silence_skip = v;
        }
        if let Some(v) = f.radio_dvr {
            self.radio_dvr = v;
        }
        if let Some(v) = f.radio_dvr_minutes {
            self.radio_dvr_minutes = v;
        }
        if let Some(v) = f.replaygain {
            self.replaygain = v.min(2);
        }
        if let Some(v) = f.replaygain_preamp {
            self.replaygain_preamp = v.clamp(-15.0, 15.0);
        }
        if let Some(v) = f.sort_order {
            self.sort_order = v;
        }
        if let Some(v) = f.album_art {
            self.album_art = v;
        }
        if let Some(v) = f.dynamic_accent {
            self.dynamic_accent = v;
        }
        if let Some(v) = f.panes_horizontal {
            self.panes_horizontal = v;
        }
        if let Some(v) = f.grid_circle {
            self.grid_circle = v;
        }
        if let Some(v) = f
            .grid_card_size
            .as_deref()
            .and_then(GridCardSize::from_label)
        {
            self.grid_card_size = v;
        }
        // `track_columns` (current) wins; fall back to the legacy `popular_columns`
        // key so an upgraded config keeps its rows/columns choice.
        if let Some(v) = f.track_columns.or(f.popular_columns) {
            self.track_columns = v;
        }
        if let Some(v) = f.arabic_shaping {
            self.arabic_shaping = v;
        }
        if let Some(v) = f.spotify_client_id {
            self.spotify_client_id = v;
        }
        if let Some(v) = f.spotify_bitrate {
            // only librespot's three valid steps; anything else falls back to 160
            self.spotify_bitrate = if matches!(v, 96 | 160 | 320) { v } else { 160 };
        }
        if let Some(v) = f.icon_set {
            self.icon_set = v;
        }
        if let Some(v) = f.powerline {
            self.powerline = v;
        }
        if let Some(v) = f.icons {
            self.icons = v;
        }
        if let Some(v) = f.player_viz {
            self.player_viz = v;
        }
        if let Some(v) = f.player_viz_mode {
            self.player_viz_mode = v % crate::ui::components::VIZ_MODES.len() as u8;
        }
        if let Some(v) = f.mouse {
            self.mouse = v;
        }
        if let Some(v) = f.next_hint {
            self.next_hint = v;
        }
        if let Some(v) = f.os_media_controls {
            self.os_media_controls = v;
        }
        if let Some(v) = f.spotify_show_account {
            self.spotify_show_account = v;
        }
        if let Some(v) = f
            .touchpad_speed
            .as_deref()
            .and_then(TouchpadSpeed::from_label)
        {
            self.touchpad_speed = v;
        }
        if let Some(v) = f.grid_scroll_lock {
            self.grid_scroll_lock = v;
        }
        if let Some(v) = f.overlay_size {
            self.overlay_size = v.min(crate::config::OVERLAY_SIZE_COUNT - 1);
        }
        if let Some(v) = f.fps {
            self.fps = v.clamp(10, 144);
        }
        if let Some(v) = f.radio_refresh_days {
            self.radio_refresh_days = v.min(365);
        }
        if let Some(v) = f.reduced_motion {
            self.reduced_motion = v;
        }
        if let Some(vz) = f.visualizer {
            if let Some(fr) = vz.frame_rate {
                self.fps = fr.clamp(10, 144);
            }
            if let Some(b) = vz.peak_caps {
                self.peak_caps = b;
            }
            if let Some(g) = vz.gravity {
                self.viz_gravity = g.clamp(0.0, 1.0);
            }
            if let Some(h) = vz.peak_hang {
                self.viz_peak_hang = h.min(240);
            }
        }
        if let Some(l) = f.layout {
            if let Some(w) = l.sidebar_width {
                self.dash_sidebar_w = w.clamp(16, 50);
            }
            if let Some(w) = l.artist_width {
                self.dash_artist_w = w.clamp(20, 70);
            }
        }
        if let Some(c) = f.columns {
            let col = &mut self.columns;
            for (slot, v) in [
                (&mut col.index, c.index),
                (&mut col.artist, c.artist),
                (&mut col.album_artist, c.album_artist),
                (&mut col.album, c.album),
                (&mut col.year, c.year),
                (&mut col.genre, c.genre),
                (&mut col.composer, c.composer),
                (&mut col.format, c.format),
                (&mut col.bitrate, c.bitrate),
                (&mut col.rating, c.rating),
                (&mut col.time, c.time),
                (&mut col.plays, c.plays),
                (&mut col.comment, c.comment),
            ] {
                if let Some(v) = v {
                    *slot = v;
                }
            }
        }
        if let Some(l) = f.lyrics {
            if let Some(v) = l.align {
                self.lyrics_align = v.min(2);
            }
            if let Some(v) = l.gap {
                self.lyrics_gap = v.min(3);
            }
            if let Some(v) = l.gradient {
                self.lyrics_gradient = v;
            }
            if let Some(v) = l.color {
                self.lyrics_color = v.min(4);
            }
            if let Some(v) = l.karaoke {
                self.lyrics_karaoke = v;
            }
            if let Some(v) = l.dual {
                self.lyrics_dual = v;
            }
            if let Some(v) = l.teleprompter {
                self.lyrics_teleprompter = v;
            }
            if let Some(v) = l.queue {
                self.lyrics_queue = v;
            }
            if let Some(v) = l.viz {
                self.lyrics_viz = v;
            }
            if let Some(v) = l.offset {
                self.lyrics_offset_ms = v.clamp(-5000, 5000);
            }
            if let Some(v) = l.translate_to {
                self.lyrics_translate_to = v;
            }
        }
        if let Some(e) = f.eq {
            if let Some(v) = e.enabled {
                self.eq_enabled = v;
            }
            if let Some(v) = e.preamp {
                self.eq_preamp = v.clamp(EQ_MIN_DB, EQ_MAX_DB);
            }
            if let Some(bands) = e.bands {
                // clamp + copy up to EQ_BANDS; missing entries stay flat
                for (slot, b) in self.eq_bands.iter_mut().zip(bands.iter()) {
                    *slot = b.clamp(EQ_MIN_DB, EQ_MAX_DB);
                }
            }
            if let Some(v) = e.preset {
                self.eq_preset = v;
            }
        }
    }

    /// Path of the custom-EQ-presets file (siblings the main `config.toml`).
    pub fn eq_presets_path(&self) -> PathBuf {
        self.dir.join("eq_presets.toml")
    }

    /// Load the user's saved custom EQ presets (empty when the file is absent or
    /// unparsable — a bad presets file must never block startup). Each preset's
    /// bands are clamped to the valid dB range and normalized to `EQ_BANDS`.
    pub fn load_eq_presets(&self) -> Vec<EqPreset> {
        let text = match std::fs::read_to_string(self.eq_presets_path()) {
            Ok(t) => t,
            Err(_) => return Vec::new(),
        };
        let parsed: EqPresetsFile = toml::from_str(&text).unwrap_or_default();
        parsed
            .preset
            .into_iter()
            .filter_map(|p| {
                if p.name.trim().is_empty() {
                    return None;
                }
                let mut bands = [0.0f32; EQ_BANDS];
                for (slot, b) in bands.iter_mut().zip(p.bands.iter()) {
                    *slot = b.clamp(EQ_MIN_DB, EQ_MAX_DB);
                }
                Some(EqPreset {
                    name: p.name,
                    preamp: p.preamp.clamp(EQ_MIN_DB, EQ_MAX_DB),
                    bands,
                })
            })
            .collect()
    }

    /// Write the custom EQ presets back to `eq_presets.toml` (atomic, like the main
    /// config). A no-op in the never-write states (empty dir / failed-to-load config).
    pub fn save_eq_presets(&self, presets: &[EqPreset]) {
        if self.dir.as_os_str().is_empty() || self.config_error.is_some() {
            return;
        }
        let mut s = String::from(
            "# lyrfin equalizer presets — managed by the in-app Equalizer.\n\
             # Custom presets you save appear here; values are dB (-12..12).\n",
        );
        for p in presets {
            let bands = p
                .bands
                .iter()
                .map(|b| b.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            s.push_str(&format!(
                "\n[[preset]]\nname = {:?}\npreamp = {}\nbands = [{}]\n",
                p.name, p.preamp, bands
            ));
        }
        let _ = std::fs::create_dir_all(&self.dir);
        atomic_write(&self.eq_presets_path(), &s);
    }

    pub fn themes_dir(&self) -> PathBuf {
        self.dir.join("themes")
    }

    /// Write the bundled custom themes into `themes/` (only ones not already
    /// present, so user edits are never clobbered). Also drops the documented
    /// `example.toml` reference.
    pub fn seed_bundled_themes(&self) {
        let dir = self.themes_dir();
        let _ = std::fs::create_dir_all(&dir);
        for (name, body) in BUNDLED_THEMES {
            let path = dir.join(format!("{name}.toml"));
            if !path.exists() {
                let _ = std::fs::write(&path, body);
            }
        }
    }

    /// Custom themes available in `themes/` (file stems, minus built-ins and the
    /// example reference), sorted for a stable cycle order.
    pub fn custom_themes(&self) -> Vec<String> {
        let mut v = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.themes_dir()) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) != Some("toml") {
                    continue;
                }
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str())
                    && stem != "example"
                    && !crate::ui::theme::BUILTIN_THEMES.contains(&stem)
                {
                    v.push(stem.to_string());
                }
            }
        }
        v.sort();
        v
    }

    /// Write the current settings back to `config.toml` (managed by the in-app
    /// Settings tab). Overwrites the file — comments in the scaffold are not kept.
    pub fn save(&self) {
        // a default-constructed Config has an empty dir (see `Config::default`) —
        // never write in that state, so a test (or stray default) can't land on the
        // real `~/.config/lyrfin` or pollute the CWD.
        if self.dir.as_os_str().is_empty() {
            return;
        }
        // config.toml exists but failed to parse → never overwrite it with our
        // in-memory defaults; the user is meant to fix the file by hand.
        if self.config_error.is_some() {
            return;
        }
        let dirs = self
            .music_dirs
            .iter()
            .map(|p| format!("{:?}", p.to_string_lossy()))
            .collect::<Vec<_>>()
            .join(", ");
        let s = format!(
            "# lyrfin configuration — managed by the in-app Settings tab (press 7)\n\
             music_dirs = [{dirs}]\n\
             theme = {:?}\n\
             theme_follows_system = {}\n\
             light_theme = {:?}\n\
             dark_theme = {:?}\n\
             volume = {}\n\
             album_art = {}\n\
             dynamic_accent = {}\n\
             panes_horizontal = {}\n\
             grid_circle = {}\n\
             grid_card_size = {:?}\n\
             track_columns = {}\n\
             arabic_shaping = {}\n\
             \n# Spotify: paste your own app's Client ID (developer.spotify.com/dashboard,\n\
             # redirect URI http://127.0.0.1:8898/login) to avoid the shared rate limit.\n\
             spotify_client_id = {:?}\n\
             spotify_bitrate = {}\n\
             spotify_show_account = {}\n\
             icon_set = {:?}\n\
             powerline = {}\n\
             player_viz = {}\n\
             player_viz_mode = {}\n\
             mouse = {}\n\
             next_hint = {}\n\
             os_media_controls = {}\n\
             touchpad_speed = {:?}\n\
             grid_scroll_lock = {}\n\
             overlay_size = {}\n\
             fps = {}\n\
             radio_refresh_days = {}\n\
             reduced_motion = {}\n\
             gapless = {}\n\
             crossfade_ms = {}\n\
             silence_skip = {}\n\
             radio_dvr = {}\n\
             radio_dvr_minutes = {}\n\
             replaygain = {}\n\
             replaygain_preamp = {}\n\
             sort_order = {:?}\n\
             \n[visualizer]\n\
             peak_caps = {}\n\
             gravity = {}\n\
             peak_hang = {}\n\
             \n[layout]\n\
             sidebar_width = {}\n\
             artist_width = {}\n\
             \n[columns]\n\
             index = {}\n\
             artist = {}\n\
             album_artist = {}\n\
             album = {}\n\
             year = {}\n\
             genre = {}\n\
             composer = {}\n\
             format = {}\n\
             bitrate = {}\n\
             rating = {}\n\
             time = {}\n\
             plays = {}\n\
             comment = {}\n\
             \n[lyrics]\n\
             align = {}\n\
             gap = {}\n\
             gradient = {}\n\
             color = {}\n\
             karaoke = {}\n\
             dual = {}\n\
             teleprompter = {}\n\
             queue = {}\n\
             viz = {}\n\
             offset = {}\n\
             translate_to = {:?}\n",
            self.theme,
            self.theme_follows_system,
            self.light_theme,
            self.dark_theme,
            self.volume,
            self.album_art,
            self.dynamic_accent,
            self.panes_horizontal,
            self.grid_circle,
            self.grid_card_size.label(),
            self.track_columns,
            self.arabic_shaping,
            self.spotify_client_id,
            self.spotify_bitrate,
            self.spotify_show_account,
            self.icon_set,
            self.powerline,
            self.player_viz,
            self.player_viz_mode,
            self.mouse,
            self.next_hint,
            self.os_media_controls,
            self.touchpad_speed.label(),
            self.grid_scroll_lock,
            self.overlay_size,
            self.fps,
            self.radio_refresh_days,
            self.reduced_motion,
            self.gapless,
            self.crossfade_ms,
            self.silence_skip,
            self.radio_dvr,
            self.radio_dvr_minutes,
            self.replaygain,
            self.replaygain_preamp,
            self.sort_order,
            self.peak_caps,
            self.viz_gravity,
            self.viz_peak_hang,
            self.dash_sidebar_w,
            self.dash_artist_w,
            self.columns.index,
            self.columns.artist,
            self.columns.album_artist,
            self.columns.album,
            self.columns.year,
            self.columns.genre,
            self.columns.composer,
            self.columns.format,
            self.columns.bitrate,
            self.columns.rating,
            self.columns.time,
            self.columns.plays,
            self.columns.comment,
            self.lyrics_align,
            self.lyrics_gap,
            self.lyrics_gradient,
            self.lyrics_color,
            self.lyrics_karaoke,
            self.lyrics_dual,
            self.lyrics_teleprompter,
            self.lyrics_queue,
            self.lyrics_viz,
            self.lyrics_offset_ms,
            self.lyrics_translate_to,
        );
        // [eq] — the 10-band equalizer state (bands as an inline dB array)
        let eq_bands = self
            .eq_bands
            .iter()
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let s = format!(
            "{s}\n[eq]\nenabled = {}\npreamp = {}\npreset = {:?}\nbands = [{eq_bands}]\n",
            self.eq_enabled, self.eq_preamp, self.eq_preset,
        );
        // custom transport glyph overrides — only emit the ones that are set
        let ov = &self.icons;
        let mut rows = String::new();
        for (k, v) in [
            ("play", &ov.play),
            ("pause", &ov.pause),
            ("prev", &ov.prev),
            ("next", &ov.next),
            ("shuffle", &ov.shuffle),
            ("repeat", &ov.repeat),
            ("repeat_one", &ov.repeat_one),
            ("seek_back", &ov.seek_back),
            ("seek_fwd", &ov.seek_fwd),
            ("volume", &ov.volume),
            ("volume_mute", &ov.volume_mute),
        ] {
            if let Some(g) = v.as_ref().filter(|g| !g.is_empty()) {
                rows.push_str(&format!("{k} = {g:?}\n"));
            }
        }
        let s = if rows.is_empty() {
            s
        } else {
            format!("{s}\n[icons]\n{rows}")
        };
        let _ = std::fs::create_dir_all(&self.dir);
        atomic_write(&self.dir.join("config.toml"), &s);
    }

    /// Write default config files on first run (never overwrites existing ones).
    fn ensure_scaffold(dir: &Path) {
        let _ = std::fs::create_dir_all(dir.join("themes"));
        write_if_absent(
            &dir.join("config.toml"),
            include_str!("../../assets/config.toml"),
        );
        write_if_absent(
            &dir.join("keybindings.toml"),
            include_str!("../../assets/keybindings.toml"),
        );
        write_if_absent(
            &dir.join("themes/aurora.toml"),
            include_str!("../../assets/themes/aurora.toml"),
        );
    }
}

/// Write `contents` to `path` atomically: a sibling temp file + rename. A crash or
/// kill mid-write can then never leave a truncated file — which would fail to parse
/// on the next load and trigger a fall-back-to-defaults that silently clobbers the
/// user's settings (the cause of the wiped Client ID / reset visualizer).
fn atomic_write(path: &Path, contents: &str) {
    let tmp = path.with_extension("tmp"); // sibling in the same dir → rename is atomic
    if std::fs::write(&tmp, contents).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
}

fn write_if_absent(path: &Path, contents: &str) {
    if !path.exists() {
        let _ = std::fs::write(path, contents);
    }
}

/// Config lives at `$XDG_CONFIG_HOME/lyrfin` (default `~/.config/lyrfin`) on
/// Linux/macOS, and `%APPDATA%\lyrfin` on Windows.
pub(super) fn config_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(xdg).join("lyrfin");
    }
    if let Some(home) = std::env::var_os("HOME").filter(|s| !s.is_empty()) {
        return PathBuf::from(home).join(".config").join("lyrfin");
    }
    if let Some(appdata) = std::env::var_os("APPDATA").filter(|s| !s.is_empty()) {
        return PathBuf::from(appdata).join("lyrfin");
    }
    PathBuf::from(".lyrfin")
}

pub(super) fn default_music_dirs() -> Vec<PathBuf> {
    directories::UserDirs::new()
        .and_then(|u| u.audio_dir().map(|p| vec![p.to_path_buf()]))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{Config, ConfigFile};

    fn apply_toml(src: &str) -> Config {
        let mut cfg = Config::default();
        let f: ConfigFile = toml::from_str(src).expect("parse test config");
        cfg.apply(f);
        cfg
    }

    #[test]
    fn save_round_trips_client_id_and_viz_mode() {
        // the clobber bug wiped these; verify save → read → parse → apply preserves
        // them, and that the save is atomic (no leftover temp).
        let c = Config {
            spotify_client_id: "eb057fc6".into(),
            player_viz_mode: 5,
            dir: std::env::temp_dir().join("lyrfin_cfg_roundtrip"),
            ..Config::default()
        };
        let _ = std::fs::remove_dir_all(&c.dir);
        let _ = std::fs::create_dir_all(&c.dir);
        c.save();
        assert!(
            !c.dir.join("config.tmp").exists(),
            "atomic save leaves no temp file behind"
        );
        let text = std::fs::read_to_string(c.dir.join("config.toml")).expect("config written");
        let back = apply_toml(&text);
        assert_eq!(
            back.spotify_client_id, "eb057fc6",
            "client id survives save+load"
        );
        assert_eq!(
            back.player_viz_mode, 5,
            "playback viz mode survives save+load"
        );
        let _ = std::fs::remove_dir_all(&c.dir);
    }

    #[test]
    fn save_round_trips_follow_system_theme() {
        // the follow-system mode + its two per-appearance theme names must survive a
        // restart (they live only in config.toml, like the single `theme`).
        let c = Config {
            theme_follows_system: true,
            light_theme: "tokyonight-day".into(),
            dark_theme: "glacier".into(),
            dir: std::env::temp_dir().join("lyrfin_cfg_theme_follow"),
            ..Config::default()
        };
        let _ = std::fs::remove_dir_all(&c.dir);
        let _ = std::fs::create_dir_all(&c.dir);
        c.save();
        let text = std::fs::read_to_string(c.dir.join("config.toml")).expect("config written");
        let back = apply_toml(&text);
        assert!(
            back.theme_follows_system,
            "follow-system flag survives save+load"
        );
        assert_eq!(back.light_theme, "tokyonight-day", "light theme survives");
        assert_eq!(back.dark_theme, "glacier", "dark theme survives");
        let _ = std::fs::remove_dir_all(&c.dir);
    }

    #[test]
    fn save_round_trips_equalizer_state() {
        // the EQ enable / preamp / preset / per-band gains must survive a restart
        let mut bands = [0.0f32; crate::audio::eq::EQ_BANDS];
        bands[0] = 6.0;
        bands[9] = -4.5;
        let c = Config {
            eq_enabled: true,
            eq_preamp: -3.0,
            eq_bands: bands,
            eq_preset: "Bass Boost".into(),
            dir: std::env::temp_dir().join("lyrfin_cfg_eq_roundtrip"),
            ..Config::default()
        };
        let _ = std::fs::remove_dir_all(&c.dir);
        let _ = std::fs::create_dir_all(&c.dir);
        c.save();
        let text = std::fs::read_to_string(c.dir.join("config.toml")).expect("config written");
        let back = apply_toml(&text);
        assert!(back.eq_enabled, "enabled survives");
        assert_eq!(back.eq_preamp, -3.0, "preamp survives");
        assert_eq!(back.eq_preset, "Bass Boost", "preset name survives");
        assert_eq!(back.eq_bands, bands, "every band survives");
        let _ = std::fs::remove_dir_all(&c.dir);
    }

    #[test]
    fn eq_bands_are_clamped_on_load() {
        // a hand-edited file with out-of-range / short band arrays is normalized
        let c = apply_toml("[eq]\nenabled = true\nbands = [99.0, -99.0, 3.0]\n");
        assert_eq!(c.eq_bands[0], crate::audio::eq::EQ_MAX_DB, "clamped high");
        assert_eq!(c.eq_bands[1], crate::audio::eq::EQ_MIN_DB, "clamped low");
        assert_eq!(c.eq_bands[2], 3.0, "in-range kept");
        assert_eq!(c.eq_bands[3], 0.0, "missing bands stay flat");
    }

    #[test]
    fn custom_eq_presets_round_trip() {
        use crate::config::EqPreset;
        let mut bands = [0.0f32; crate::audio::eq::EQ_BANDS];
        bands[4] = 5.0;
        let dir = std::env::temp_dir().join("lyrfin_cfg_eq_presets");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let c = Config {
            dir: dir.clone(),
            ..Config::default()
        };
        assert!(c.load_eq_presets().is_empty(), "no presets file yet");
        c.save_eq_presets(&[EqPreset {
            name: "My Mix".into(),
            preamp: -2.0,
            bands,
        }]);
        let back = c.load_eq_presets();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].name, "My Mix");
        assert_eq!(back[0].preamp, -2.0);
        assert_eq!(back[0].bands, bands);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_round_trips_lyrics_offset() {
        // the manual lyric-sync nudge must persist across restarts
        let c = Config {
            lyrics_offset_ms: -320,
            dir: std::env::temp_dir().join("lyrfin_cfg_lyrics_offset"),
            ..Config::default()
        };
        let _ = std::fs::remove_dir_all(&c.dir);
        let _ = std::fs::create_dir_all(&c.dir);
        c.save();
        let text = std::fs::read_to_string(c.dir.join("config.toml")).expect("config written");
        let back = apply_toml(&text);
        assert_eq!(
            back.lyrics_offset_ms, -320,
            "lyric offset survives save+load"
        );
        let _ = std::fs::remove_dir_all(&c.dir);
    }

    #[test]
    fn default_config_never_writes_to_disk() {
        // Regression: `Config::default().dir` used to be the real `~/.config/lyrfin`,
        // so any test that constructed a default Config and saved (e.g. a tag-editor
        // test running `theme …`) silently clobbered the user's config.toml — which
        // reset the playback visualizer to Bars on every `cargo test`. The default
        // dir must be empty and `save` must be a no-op in that state.
        let c = Config::default();
        assert!(
            c.dir.as_os_str().is_empty(),
            "a default Config must not point at any real dir"
        );
        // saving is a no-op: it must not create a config.toml anywhere (CWD included)
        let before = std::path::Path::new("config.toml").exists();
        c.save();
        assert_eq!(
            std::path::Path::new("config.toml").exists(),
            before,
            "default save must not write config.toml"
        );
    }

    #[test]
    fn save_is_blocked_when_the_config_failed_to_load() {
        // a config.toml that exists but didn't parse must never be overwritten with
        // our in-memory defaults — the user is meant to fix it by hand.
        let dir = std::env::temp_dir().join("lyrfin_cfg_blocked");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("config.toml");
        std::fs::write(&path, "a broken file the user will fix").unwrap();
        let c = Config {
            config_error: Some("TOML parse error at line 3".into()),
            dir: dir.clone(),
            ..Config::default()
        };
        c.save();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "a broken file the user will fix",
            "save is a no-op when the config failed to load — the file is untouched"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_fully_replaces_the_file() {
        let dir = std::env::temp_dir().join("lyrfin_atomic_write");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("config.toml");
        let _ = std::fs::write(&path, "stale, and far longer than the new contents");
        super::atomic_write(&path, "fresh");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "fresh");
        assert!(
            !dir.join("config.tmp").exists(),
            "the temp file was renamed away, none left behind"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn spotify_bitrate_parses_valid_steps_and_clamps_invalid() {
        assert_eq!(apply_toml("spotify_bitrate = 320").spotify_bitrate, 320);
        assert_eq!(apply_toml("spotify_bitrate = 96").spotify_bitrate, 96);
        // anything outside librespot's three steps falls back to 160
        assert_eq!(apply_toml("spotify_bitrate = 256").spotify_bitrate, 160);
        // absent → the default (160), never zero
        assert_eq!(apply_toml("volume = 50").spotify_bitrate, 160);
    }

    #[test]
    fn grid_card_size_parses_label_and_defaults() {
        use super::GridCardSize;
        assert_eq!(
            apply_toml("grid_card_size = \"large\"").grid_card_size,
            GridCardSize::Large
        );
        assert_eq!(
            apply_toml("grid_card_size = \"small\"").grid_card_size,
            GridCardSize::Small
        );
        // unknown / absent → the default (medium)
        assert_eq!(
            apply_toml("grid_card_size = \"huge\"").grid_card_size,
            GridCardSize::Medium
        );
        assert_eq!(
            apply_toml("volume = 50").grid_card_size,
            GridCardSize::Medium
        );
    }

    #[test]
    fn grid_card_size_steps_and_maps_to_a_width() {
        use super::GridCardSize::{Large, Medium, Small};
        assert_eq!(Medium.step(1), Large);
        assert_eq!(Medium.step(-1), Small);
        assert_eq!(Large.step(1), Large, "saturates at the top");
        assert_eq!(Small.step(-1), Small, "saturates at the bottom");
        assert!(
            Small.card_width() < Medium.card_width() && Medium.card_width() < Large.card_width(),
            "bigger size → wider card"
        );
    }

    #[test]
    fn touchpad_settings_round_trip() {
        // both new touchpad knobs must persist across restarts
        let c = Config {
            touchpad_speed: super::TouchpadSpeed::Fast,
            grid_scroll_lock: false,
            dir: std::env::temp_dir().join("lyrfin_cfg_touchpad"),
            ..Config::default()
        };
        let _ = std::fs::remove_dir_all(&c.dir);
        let _ = std::fs::create_dir_all(&c.dir);
        c.save();
        let text = std::fs::read_to_string(c.dir.join("config.toml")).expect("config written");
        let back = apply_toml(&text);
        assert_eq!(back.touchpad_speed, super::TouchpadSpeed::Fast);
        assert!(!back.grid_scroll_lock);
        let _ = std::fs::remove_dir_all(&c.dir);
    }

    #[test]
    fn touchpad_speed_parses_label_and_defaults() {
        use super::TouchpadSpeed;
        assert_eq!(
            apply_toml("touchpad_speed = \"slow\"").touchpad_speed,
            TouchpadSpeed::Slow
        );
        assert_eq!(
            apply_toml("touchpad_speed = \"fast\"").touchpad_speed,
            TouchpadSpeed::Fast
        );
        // unknown / absent → the default (normal)
        assert_eq!(
            apply_toml("touchpad_speed = \"warp\"").touchpad_speed,
            TouchpadSpeed::Normal
        );
        assert_eq!(
            apply_toml("volume = 50").touchpad_speed,
            TouchpadSpeed::Normal
        );
    }

    #[test]
    fn touchpad_speed_steps_and_maps_to_a_threshold() {
        use super::TouchpadSpeed::{Fast, Normal, Slow};
        assert_eq!(Normal.step(1), Fast);
        assert_eq!(Normal.step(-1), Slow);
        assert_eq!(Fast.step(1), Fast, "saturates at fast");
        assert_eq!(Slow.step(-1), Slow, "saturates at slow");
        // faster speed → fewer events per step (commits sooner)
        assert!(Fast.step_events() < Normal.step_events());
        assert!(Normal.step_events() < Slow.step_events());
    }
}
