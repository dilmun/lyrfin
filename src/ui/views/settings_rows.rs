//! The per-row `(label, value)` mapping for every [`Setting`] — a flat,
//! exhaustive match split out of `settings_view` so the list layout stays small.
//! Boolean settings yield a [`SettingValue::Toggle`] (drawn as a switch);
//! everything else yields a display [`SettingValue::Text`].

use crate::app::{AppState, Setting};
use crate::ui::components;

/// The right-hand value of a settings row: a boolean rendered as a toggle switch,
/// or a plain text value (theme name, "320 kbps", "⌫ del", …).
pub enum SettingValue {
    Toggle(bool),
    Text(String),
}

/// Value labels shared by the settings-row renderer AND the palette's value
/// suggestions (`crate::app::settings_choices`), indexed by the config value, so the
/// two surfaces never drift. Each array's index == the stored `u8`.
pub(crate) const REPLAYGAIN_LABELS: [&str; 3] = ["off", "track", "album"];
pub(crate) const LYRICS_ALIGN_LABELS: [&str; 3] = ["center", "left", "right"];
pub(crate) const LYRICS_COLOR_LABELS: [&str; 5] = ["accent", "violet", "pink", "amber", "white"];

/// A per-appearance theme picker's value: the theme name, marked `· active` when
/// it's the one the follow-system switch is currently showing (so the user can see
/// which of light/dark is live without guessing).
fn theme_slot_value(app: &AppState, name: &str) -> String {
    if app.applied_sys_theme.as_deref() == Some(name) {
        format!("{name}  · active")
    } else {
        name.to_string()
    }
}

/// The (label, value) display pair for one settings row.
pub(crate) fn setting_label_value(app: &AppState, item: &Setting) -> (String, SettingValue) {
    use SettingValue::{Text, Toggle};
    match item {
        Setting::MusicDir(d) => (
            format!("♪ {}", app.config.music_dirs[*d].display()),
            Text("⌫ del".to_string()),
        ),
        Setting::AddDir => ("＋ Add music directory…".into(), Text(String::new())),
        Setting::Rescan => ("↻ Rescan now".into(), Text(String::new())),
        Setting::Theme => ("Theme".into(), Text(app.config.theme.clone())),
        Setting::ThemeFollowSystem => (
            "Follow system light/dark".into(),
            Toggle(app.config.theme_follows_system),
        ),
        Setting::LightTheme => (
            "Light theme".into(),
            Text(theme_slot_value(app, &app.config.light_theme)),
        ),
        Setting::DarkTheme => (
            "Dark theme".into(),
            Text(theme_slot_value(app, &app.config.dark_theme)),
        ),
        Setting::AlbumArt => (
            "Album art (inline images)".into(),
            Toggle(app.config.album_art),
        ),
        Setting::DynamicAccent => (
            "Accent from album art".into(),
            Toggle(app.config.dynamic_accent),
        ),
        Setting::IconSet => (
            "Transport icons".into(),
            // Show the whole glyph run, not just `play`: the point of this row is
            // to reveal which glyphs this font is missing, and one icon can't.
            Text(format!(
                "{}  {}",
                app.config.icon_set,
                crate::icons::Icons::sample(&app.config.icon_set)
            )),
        ),
        Setting::Powerline => (
            "Powerline selection caps".into(),
            Toggle(app.config.powerline),
        ),
        Setting::PlayerViz => ("Playback visualizer".into(), Toggle(app.config.player_viz)),
        Setting::PanesLayout => (
            "Side panes".into(),
            Text(
                if app.config.panes_horizontal {
                    "horizontal"
                } else {
                    "vertical"
                }
                .into(),
            ),
        ),
        Setting::GridList => {
            // grid on/off is runtime per-section state, per the view on screen
            let grid_on = if app.layout == crate::app::Layout::Spotify {
                app.spotify.grid
            } else {
                app.local.grid
            };
            (
                "View".into(),
                Text(if grid_on { "grid" } else { "list" }.into()),
            )
        }
        Setting::GridShape => (
            "Grid card shape".into(),
            Text(
                if app.config.grid_circle {
                    "circle"
                } else {
                    "rounded"
                }
                .into(),
            ),
        ),
        Setting::GridSize => (
            "Grid card size".into(),
            Text(app.config.grid_card_size.label().into()),
        ),
        Setting::TrackColumns => (
            "Track layout".into(),
            Text(
                if app.config.track_columns {
                    "columns"
                } else {
                    "rows"
                }
                .into(),
            ),
        ),
        Setting::PlayerVizMode => (
            "Playback visualizer mode".into(),
            Text(
                components::VIZ_MODES
                    [app.config.player_viz_mode as usize % components::VIZ_MODES.len()]
                .to_string(),
            ),
        ),
        Setting::Mouse => ("Mouse support".into(), Toggle(app.config.mouse)),
        Setting::NextHint => (
            "Status bar “Next:” hint".into(),
            Toggle(app.config.next_hint),
        ),
        Setting::OsMediaControls => (
            "OS media controls".into(),
            Toggle(app.config.os_media_controls),
        ),
        Setting::TouchpadScroll => (
            "Touchpad scroll speed".into(),
            Text(app.config.touchpad_speed.label().into()),
        ),
        Setting::GridScrollLock => (
            "Lock grid scroll to row".into(),
            Toggle(app.config.grid_scroll_lock),
        ),
        Setting::OverlaySize => (
            "Overlay size".into(),
            Text(
                crate::config::OVERLAY_SIZE_LABELS
                    .get(app.config.overlay_size as usize)
                    .copied()
                    .unwrap_or("Small")
                    .into(),
            ),
        ),
        Setting::ReducedMotion => ("Reduced motion".into(), Toggle(app.config.reduced_motion)),
        Setting::PeakCaps => ("Visualizer peak caps".into(), Toggle(app.config.peak_caps)),
        Setting::Fps => ("Frame rate".into(), Text(format!("{} fps", app.config.fps))),
        Setting::RadioRefresh => (
            "Radio directory refresh".into(),
            Text(match app.config.radio_refresh_days {
                0 => "manual".to_string(),
                1 => "daily".to_string(),
                7 => "weekly".to_string(),
                14 => "fortnightly".to_string(),
                30 => "monthly".to_string(),
                n => format!("every {n} days"),
            }),
        ),
        Setting::ArabicShaping => (
            "Arabic text shaping".into(),
            Toggle(app.config.arabic_shaping),
        ),
        Setting::RadioDvr => ("Radio timeshift (DVR)".into(), Toggle(app.config.radio_dvr)),
        Setting::Gapless => ("Gapless playback".into(), Toggle(app.config.gapless)),
        Setting::SilenceSkip => (
            "Skip silence between tracks".into(),
            Toggle(app.config.silence_skip),
        ),
        Setting::Crossfade => (
            "Crossfade".into(),
            Text(if app.config.crossfade_ms == 0 {
                "off".to_string()
            } else {
                format!("{} ms", app.config.crossfade_ms)
            }),
        ),
        Setting::ReplayGain => (
            "ReplayGain (normalize)".into(),
            Text(REPLAYGAIN_LABELS[app.config.replaygain.min(2) as usize].to_string()),
        ),
        Setting::ColIndex => ("Track number".into(), Toggle(app.config.columns.index)),
        Setting::ColArtist => ("Artist".into(), Toggle(app.config.columns.artist)),
        Setting::ColAlbumArtist => (
            "Album artist".into(),
            Toggle(app.config.columns.album_artist),
        ),
        Setting::ColAlbum => ("Album".into(), Toggle(app.config.columns.album)),
        Setting::ColYear => ("Year".into(), Toggle(app.config.columns.year)),
        Setting::ColGenre => ("Genre".into(), Toggle(app.config.columns.genre)),
        Setting::ColComposer => ("Composer".into(), Toggle(app.config.columns.composer)),
        Setting::ColFormat => ("Format (type)".into(), Toggle(app.config.columns.format)),
        Setting::ColBitrate => ("Bitrate (kbps)".into(), Toggle(app.config.columns.bitrate)),
        Setting::ColRating => ("Rating".into(), Toggle(app.config.columns.rating)),
        Setting::ColTime => ("Time".into(), Toggle(app.config.columns.time)),
        Setting::ColPlays => ("Play count".into(), Toggle(app.config.columns.plays)),
        Setting::ColComment => ("Comment".into(), Toggle(app.config.columns.comment)),
        Setting::LyricsAlign => (
            "Lyrics alignment".into(),
            Text(LYRICS_ALIGN_LABELS[app.config.lyrics_align.min(2) as usize].to_string()),
        ),
        Setting::LyricsGap => (
            "Lyrics line spacing".into(),
            Text(match app.config.lyrics_gap {
                0 => "compact".to_string(),
                n => format!("{n} blank"),
            }),
        ),
        Setting::LyricsGradient => (
            "Lyrics highlight".into(),
            Text(
                if app.config.lyrics_gradient {
                    "gradient"
                } else {
                    "solid"
                }
                .into(),
            ),
        ),
        Setting::LyricsColor => (
            "Lyrics color (solid)".into(),
            Text(LYRICS_COLOR_LABELS[app.config.lyrics_color.min(4) as usize].to_string()),
        ),
        Setting::LyricsKaraoke => (
            "Lyrics: karaoke wipe".into(),
            Toggle(app.config.lyrics_karaoke),
        ),
        Setting::LyricsDual => (
            "Lyrics: translations".into(),
            Toggle(app.config.lyrics_dual),
        ),
        Setting::LyricsTranslate => (
            "Lyrics: translate to".into(),
            Text(crate::translate::lang_label(&app.config.lyrics_translate_to).to_string()),
        ),
        Setting::LyricsTeleprompter => (
            "Lyrics: teleprompter".into(),
            Toggle(app.config.lyrics_teleprompter),
        ),
        Setting::PanelShow(p) => (format!("Show {}", p.label()), Toggle(app.panel(*p).shown)),
        Setting::PanelDock(p) => (
            format!("{} position", p.label()),
            Text(format!("◧ {}", app.panel(*p).dock.label())),
        ),
        Setting::PanelSize(p) => (
            format!("{} size", p.label()),
            Text(format!("{}%", app.panel(*p).size)),
        ),
        Setting::SpotifyLogout => {
            let connected = matches!(
                app.spotify.conn,
                crate::spotify::ConnState::Connected { .. }
            );
            let label = if connected {
                "⏏ Log out of Spotify"
            } else {
                "↺ Reset cached Spotify login"
            };
            (label.into(), Text(String::new()))
        }
        Setting::SpotifyClientId => {
            let value = if app.config.spotify_client_id.is_empty() {
                "shared (rate-limited)".to_string()
            } else {
                "custom ✓".to_string()
            };
            ("🔑 Spotify client id".into(), Text(value))
        }
        Setting::SpotifyReauth => {
            let label = if matches!(
                app.spotify.conn,
                crate::spotify::ConnState::Connected { .. }
            ) {
                "↻ Re-authenticate / switch account"
            } else {
                "↻ Log in to Spotify"
            };
            (label.into(), Text(String::new()))
        }
        Setting::SpotifyBitrate => (
            "♬ Streaming quality".into(),
            Text(format!("{} kbps", app.config.spotify_bitrate)),
        ),
        Setting::SpotifyShowAccount => (
            "◉ Show account on header".into(),
            Toggle(app.config.spotify_show_account),
        ),
        Setting::Keybind(i) => {
            let action = crate::keymap::configurable_actions()
                .get(*i)
                .copied()
                .unwrap_or("");
            let value = if app.settings.rebinding.as_deref() == Some(action) {
                "press a key…  esc".to_string()
            } else {
                app.config
                    .keymap
                    .label_for(action)
                    .unwrap_or_else(|| "—".into())
            };
            (crate::keymap::keybind_desc(action), Text(value))
        }
    }
}
