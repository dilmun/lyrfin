//! Configuration, themes, and keybindings — loaded from the platform config
//! dir (`~/.config/lyrfin/` on Linux/macOS) with built-in defaults. On first run
//! the default `config.toml`, `keybindings.toml`, and `themes/aurora.toml` are
//! written out so users have something to edit.

use std::path::PathBuf;

use serde::Deserialize;

mod bindings;
mod load;

pub use bindings::Keymap;
use load::default_music_dirs;

/// Custom themes shipped with lyrfin, seeded into the user's `themes/` dir on first
/// run (`(file stem, TOML body)`). They're plain files the user can edit freely.
pub const BUNDLED_THEMES: &[(&str, &str)] = &[
    (
        "tokyonight-night",
        include_str!("../../themes/tokyonight-night.toml"),
    ),
    (
        "tokyonight-storm",
        include_str!("../../themes/tokyonight-storm.toml"),
    ),
    (
        "tokyonight-moon",
        include_str!("../../themes/tokyonight-moon.toml"),
    ),
    (
        "tokyonight-day",
        include_str!("../../themes/tokyonight-day.toml"),
    ),
    ("foxnight", include_str!("../../themes/foxnight.toml")),
    (
        "foxnight-dusk",
        include_str!("../../themes/foxnight-dusk.toml"),
    ),
    (
        "foxnight-tera",
        include_str!("../../themes/foxnight-tera.toml"),
    ),
    (
        "foxnight-dawn",
        include_str!("../../themes/foxnight-dawn.toml"),
    ),
    (
        "foxnight-day",
        include_str!("../../themes/foxnight-day.toml"),
    ),
    (
        "foxnight-nord",
        include_str!("../../themes/foxnight-nord.toml"),
    ),
    (
        "foxnight-carbon",
        include_str!("../../themes/foxnight-carbon.toml"),
    ),
    // Famous light/dark pairs — one light + one dark per family, so the follow-system
    // theme mode (light_theme / dark_theme) can be set to a matched set.
    ("nord-dark", include_str!("../../themes/nord-dark.toml")),
    ("nord-light", include_str!("../../themes/nord-light.toml")),
    ("one-dark", include_str!("../../themes/one-dark.toml")),
    ("one-light", include_str!("../../themes/one-light.toml")),
    (
        "catppuccin-mocha",
        include_str!("../../themes/catppuccin-mocha.toml"),
    ),
    (
        "catppuccin-latte",
        include_str!("../../themes/catppuccin-latte.toml"),
    ),
    (
        "gruvbox-dark",
        include_str!("../../themes/gruvbox-dark.toml"),
    ),
    (
        "gruvbox-light",
        include_str!("../../themes/gruvbox-light.toml"),
    ),
    (
        "solarized-dark",
        include_str!("../../themes/solarized-dark.toml"),
    ),
    (
        "solarized-light",
        include_str!("../../themes/solarized-light.toml"),
    ),
    ("github-dark", include_str!("../../themes/github-dark.toml")),
    (
        "github-light",
        include_str!("../../themes/github-light.toml"),
    ),
    (
        "rose-pine-main",
        include_str!("../../themes/rose-pine-main.toml"),
    ),
    (
        "rose-pine-dawn",
        include_str!("../../themes/rose-pine-dawn.toml"),
    ),
    ("aura-dark", include_str!("../../themes/aura-dark.toml")),
    ("aura-light", include_str!("../../themes/aura-light.toml")),
    (
        "dracula-dark",
        include_str!("../../themes/dracula-dark.toml"),
    ),
    (
        "dracula-light",
        include_str!("../../themes/dracula-light.toml"),
    ),
];

/// Overlay size steps, from a compact default (0) to a semi-full card. The
/// value stored in [`Config::overlay_size`] indexes this; `f` cycles it. The
/// level → screen-fraction mapping lives in the UI (`ui::components`), keeping
/// presentation out of config. Labels are shown by the "Overlay size" setting.
pub const OVERLAY_SIZE_LABELS: [&str; 4] = ["Small", "Medium", "Large", "X-Large"];

/// How many overlay size steps exist (the wrap point for the `f` cycle).
pub const OVERLAY_SIZE_COUNT: u8 = OVERLAY_SIZE_LABELS.len() as u8;

/// Cover-art grid card size — a FIXED target so covers don't resize as a side
/// pane is dragged (only the column count changes, discretely). Drives the card
/// width; the height keeps the cover ~square. Changed in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GridCardSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl GridCardSize {
    /// All sizes in cycle order.
    pub const ALL: [GridCardSize; 3] = [
        GridCardSize::Small,
        GridCardSize::Medium,
        GridCardSize::Large,
    ];
    /// Target card cell width. The cover is kept ~square in pixels from this
    /// (cells are ~2:1), so a wider card → a bigger circle.
    pub fn card_width(self) -> u16 {
        match self {
            GridCardSize::Small => 16,
            GridCardSize::Medium => 22,
            GridCardSize::Large => 30,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            GridCardSize::Small => "small",
            GridCardSize::Medium => "medium",
            GridCardSize::Large => "large",
        }
    }
    pub fn from_label(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s2| s2.label() == s)
    }
    /// Step to the next/previous size (saturating at the ends).
    pub fn step(self, dir: i32) -> Self {
        let i = Self::ALL.iter().position(|&s| s == self).unwrap_or(1) as i32;
        let n = Self::ALL.len() as i32;
        Self::ALL[(i + dir).clamp(0, n - 1) as usize]
    }
}

/// How fast a touchpad two-finger scroll steps through a cover grid / carousel.
/// A swipe fires many small scroll events, so this sets how many accumulate before
/// the selection steps one card/row — `Fast` = every event, `Slow` = throttled more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TouchpadSpeed {
    Slow,
    #[default]
    Normal,
    Fast,
}

impl TouchpadSpeed {
    /// All speeds in cycle order (slow → fast).
    pub const ALL: [TouchpadSpeed; 3] = [
        TouchpadSpeed::Slow,
        TouchpadSpeed::Normal,
        TouchpadSpeed::Fast,
    ];
    /// Touchpad scroll events required to step one grid card / row: a faster speed
    /// commits sooner (fewer events), a slower one throttles more. Drives the
    /// accumulator threshold in `grid_touch_scroll`.
    pub fn step_events(self) -> i32 {
        match self {
            TouchpadSpeed::Slow => 3,
            TouchpadSpeed::Normal => 2,
            TouchpadSpeed::Fast => 1,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            TouchpadSpeed::Slow => "slow",
            TouchpadSpeed::Normal => "normal",
            TouchpadSpeed::Fast => "fast",
        }
    }
    pub fn from_label(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s2| s2.label() == s)
    }
    /// Step to the next/previous speed (saturating at the ends).
    pub fn step(self, dir: i32) -> Self {
        let i = Self::ALL.iter().position(|&s| s == self).unwrap_or(1) as i32;
        let n = Self::ALL.len() as i32;
        Self::ALL[(i + dir).clamp(0, n - 1) as usize]
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub music_dirs: Vec<PathBuf>,
    /// The active theme when NOT following the system appearance (`theme_follows_system`
    /// = false): a built-in name, a custom `themes/*.toml` stem, or `"auto"`.
    pub theme: String,
    /// Follow the OS light/dark setting (macOS + Linux/XDG portal): pick `light_theme`
    /// or `dark_theme` to match, and switch live when the system flips. When false,
    /// `theme` is used.
    pub theme_follows_system: bool,
    /// Theme applied while the system is in Light mode (follow-system only).
    pub light_theme: String,
    /// Theme applied while the system is in Dark mode (follow-system only, and the
    /// fallback on platforms where the appearance can't be read).
    pub dark_theme: String,
    pub volume: u8,
    pub gapless: bool,
    pub crossfade_ms: u32,
    /// Trim leading/trailing near-silence at track boundaries so seamless
    /// (gapless/crossfade) transitions flow music→music. On by default.
    pub silence_skip: bool,
    /// Timeshift (DVR) buffering for live radio: keep a rolling window of the
    /// received stream so it can be paused, rewound, and caught up to live. On by
    /// default.
    pub radio_dvr: bool,
    /// Timeshift window in minutes — how far back a live stream can be rewound. The
    /// buffer is a *fixed* ring sized at ~`minutes × 1.4 MB`, allocated only while a
    /// live stream plays and freed when it stops (e.g. 20 min ≈ 28 MB). The real
    /// rewindable time flexes with the stream's bitrate (longer below ~192 kbps,
    /// shorter above); memory stays fixed. Raise it for a longer window at more RAM.
    pub radio_dvr_minutes: u32,
    /// Volume normalization: 0 off, 1 track gain, 2 album gain (ReplayGain tags).
    pub replaygain: u8,
    /// Pre-amp in dB applied on top of ReplayGain.
    pub replaygain_preamp: f32,
    /// 10-band equalizer: on/off, master preamp (dB), and per-band gains (dB,
    /// low→high, aligned with [`crate::audio::eq::EQ_FREQS`]). Applied to all
    /// output by the audio controller's EQ stage; managed by the in-app
    /// Equalizer overlay (see [`crate::audio::eq`]).
    pub eq_enabled: bool,
    pub eq_preamp: f32,
    pub eq_bands: [f32; crate::audio::eq::EQ_BANDS],
    /// Name of the last-applied preset (built-in or custom), for display; becomes
    /// "Custom" once the sliders diverge from every known preset.
    pub eq_preset: String,
    /// Default tracklist sort, e.g. "artist,year,album" (empty = off).
    pub sort_order: String,
    pub album_art: bool,
    /// Drive the theme accent colour from the current track's album art.
    pub dynamic_accent: bool,
    /// Side panes (Queue/Artist/Lyrics) on a Left/Right edge sit side-by-side
    /// instead of stacked vertically. Toggle in the `;` LAYOUT popup.
    pub panes_horizontal: bool,
    /// Render the Albums/Artists grid cards as circles (round avatars/covers)
    /// instead of rounded squares. Applies to every grid card. Toggle in settings.
    pub grid_circle: bool,
    /// Cover-art grid card size (fixed target so covers don't resize on pane drag).
    pub grid_card_size: GridCardSize,
    /// Render track lists (the local + Spotify main lists and the artist-page
    /// POPULAR list) as a column table — TITLE/ARTIST/ALBUM/YEAR/TIME (the default)
    /// — vs the compact `name · artist · album · year  time` rows when off. One
    /// shared "Track layout" toggle for every view; honored everywhere a tracklist
    /// renders.
    pub track_columns: bool,
    /// Pre-shape Arabic into presentation forms for terminals that don't shape
    /// it themselves. Turn OFF on shaping-capable terminals (Ghostty / Kitty /
    /// WezTerm) so the terminal renders raw Arabic with seamless joins (no
    /// per-cell cracks). ON is correct for iTerm2 and "dumb" terminals.
    pub arabic_shaping: bool,
    /// Your own Spotify app Client ID (developer.spotify.com/dashboard) — empty
    /// uses the shared public id (rate-limited). A private id has its own quota.
    /// Register redirect URI `http://127.0.0.1:8898/login` in the app.
    pub spotify_client_id: String,
    /// Spotify streaming quality (librespot `PlayerConfig.bitrate`): 96, 160, or
    /// 320 kbps. Applied when the librespot session next spawns (on reconnect).
    pub spotify_bitrate: u16,
    /// Show the connected Spotify account (display name) on the Spotify view's
    /// header line, right-aligned. Off hides it for privacy (screen-sharing,
    /// recording); the account is still shown in the Info overlay either way.
    /// On by default.
    pub spotify_show_account: bool,
    /// Transport icon preset: outline / triangles / skip / nerd.
    pub icon_set: String,
    /// Per-glyph custom icon overrides (the `[icons]` table).
    pub icons: crate::icons::IconOverrides,
    /// Show a visualizer in the playback bar (taller box; auto-hidden if short).
    pub player_viz: bool,
    /// Playback-bar visualizer mode (0..6) — independent of each view's big
    /// visualizer (which is per-view; see `AppState::viz_mode`).
    pub player_viz_mode: u8,
    pub mouse: bool,
    /// Show the "▶ Next:" up-next hint in the status bar. On by default.
    pub next_hint: bool,
    /// Publish playback to the OS "Now Playing" surface and accept its transport
    /// commands: macOS Control Center + lock screen + media keys / AirPods, Linux
    /// MPRIS. On by default; see `crate::media`. Inert on platforms without a wired
    /// backend (e.g. Windows).
    pub os_media_controls: bool,
    /// Touchpad two-finger scroll speed over cover grids / carousels (how many
    /// scroll events step one card/row). Tames a fast touchpad that would otherwise
    /// fly through the grid. See [`TouchpadSpeed`].
    pub touchpad_speed: TouchpadSpeed,
    /// Lock touchpad horizontal grid scroll to the current row/carousel: a sideways
    /// swipe clamps at the row's end instead of wrapping onto the next row (row
    /// changes then need a deliberate vertical gesture). Off = free 2-D wrap.
    pub grid_scroll_lock: bool,
    /// How large the big overlays (Settings / Info / Tag editor) open, as a step
    /// `0..OVERLAY_SIZE_COUNT`: 0 = a compact card, larger = roomier, up to a
    /// semi-full card that still clears every edge (never full-screen). Cycle with
    /// `f` in any overlay, or here. See [`OVERLAY_SIZE_LABELS`].
    pub overlay_size: u8,
    pub fps: u8,
    /// How often to refresh the cached internet-radio directory, in days.
    /// 0 = manual only (never auto-refresh in the background).
    pub radio_refresh_days: u32,
    /// Accessibility: stop the idle visualizer animation.
    pub reduced_motion: bool,
    /// Show the falling peak-cap above each visualizer bar.
    pub peak_caps: bool,
    /// How fast a cap accelerates downward once it starts falling.
    pub viz_gravity: f32,
    /// Frames a cap holds at its peak (hangs in mid-air) before sliding.
    pub viz_peak_hang: u16,
    /// Dashboard column widths (cols): the library sidebar + the artist panel.
    /// The tracklist in between fills the remainder.
    pub dash_sidebar_w: u16,
    pub dash_artist_w: u16,
    /// Which tracklist columns are shown (Title is always shown).
    pub columns: Columns,
    /// Lyrics display: alignment (0 center / 1 left / 2 right), blank lines
    /// between lines, and the rainbow gradient on the active line.
    pub lyrics_align: u8,
    pub lyrics_gap: u8,
    pub lyrics_gradient: bool,
    /// Active-line colour when the gradient is off (index into a small palette).
    pub lyrics_color: u8,
    /// Karaoke "wipe": light up the active line word-by-word as it's sung.
    pub lyrics_karaoke: bool,
    /// Show per-line translations (bilingual `.lrc`) beneath each line.
    pub lyrics_dual: bool,
    /// Teleprompter mode: show only the current line (+ the next, faint).
    pub lyrics_teleprompter: bool,
    /// Optional side panels on the Lyrics layout.
    pub lyrics_queue: bool,
    pub lyrics_viz: bool,
    /// Manual sync nudge (milliseconds) for synced lyrics: positive delays the
    /// highlight (use when it runs ahead of the vocal), negative advances it.
    /// Compensates residual output latency (e.g. Bluetooth) and `.lrc` timing
    /// conventions that the automatic clock can't know about.
    pub lyrics_offset_ms: i32,
    /// Machine-translate lyrics into this language (Google language code, e.g.
    /// `"en"`); empty = off. Shown beneath each line via the "translations" (dual)
    /// view. A human translation in a bilingual `.lrc` always wins. See
    /// `crate::translate`.
    pub lyrics_translate_to: String,
    pub keymap: Keymap,
    /// Resolved config directory (also holds `themes/`).
    pub dir: PathBuf,
    /// Set (with the parse error + line) when an existing `config.toml` failed to
    /// parse: the app runs on in-memory defaults, surfaces this in the status bar,
    /// and BLOCKS saves so the user's (fixable) file is never overwritten. Runtime
    /// only — not loaded or saved.
    pub config_error: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            music_dirs: default_music_dirs(),
            theme: "aurora".into(),
            theme_follows_system: false,
            light_theme: "tokyonight-day".into(),
            dark_theme: "aurora".into(),
            volume: 72,
            gapless: true,
            crossfade_ms: 0,
            silence_skip: true,
            radio_dvr: true,
            radio_dvr_minutes: 20,
            replaygain: 0,
            replaygain_preamp: 0.0,
            eq_enabled: false,
            eq_preamp: 0.0,
            eq_bands: [0.0; crate::audio::eq::EQ_BANDS],
            eq_preset: "Flat".into(),
            sort_order: "artist,year,album".into(),
            album_art: true,
            dynamic_accent: true,
            panes_horizontal: false,
            grid_circle: true,
            grid_card_size: GridCardSize::Medium,
            track_columns: true,
            arabic_shaping: true,
            spotify_client_id: String::new(),
            spotify_bitrate: 160,
            spotify_show_account: true,
            icon_set: "nerd".into(),
            icons: crate::icons::IconOverrides::default(),
            player_viz: true,
            player_viz_mode: 0,
            mouse: true,
            next_hint: true,
            os_media_controls: true,
            touchpad_speed: TouchpadSpeed::Normal,
            grid_scroll_lock: true,
            overlay_size: 0,
            fps: 60,
            radio_refresh_days: 7,
            reduced_motion: false,
            peak_caps: true,
            viz_gravity: 0.004,
            viz_peak_hang: 10,
            dash_sidebar_w: 26,
            dash_artist_w: 38,
            columns: Columns::default(),
            lyrics_align: 0, // center
            lyrics_gap: 0,
            lyrics_gradient: true,
            lyrics_color: 0,
            lyrics_karaoke: true,
            lyrics_dual: true,
            lyrics_teleprompter: false,
            lyrics_queue: false,
            lyrics_viz: false,
            lyrics_offset_ms: 0,
            lyrics_translate_to: "en".to_string(),
            keymap: Keymap::with_defaults(),
            // Deliberately empty, NOT `config_dir()`: a default-constructed Config
            // must never point at the real `~/.config/lyrfin`, or a test that saves
            // (or any stray `Config::default()`) would clobber the user's files.
            // `load_or_default` sets the real dir explicitly; tests set a temp dir;
            // `save` is a no-op while the dir is empty.
            dir: std::path::PathBuf::new(),
            config_error: None,
        }
    }
}

impl Config {
    /// Build the DSP payload for the audio engine from the persisted EQ state.
    pub fn eq_config(&self) -> crate::audio::eq::EqConfig {
        crate::audio::eq::EqConfig {
            enabled: self.eq_enabled,
            preamp_db: self.eq_preamp,
            bands: self.eq_bands,
        }
    }
}

/// A saved custom equalizer preset (persisted in `eq_presets.toml`), distinct
/// from the built-in [`crate::audio::eq::BUILTIN_EQ_PRESETS`]. Bands are dB,
/// low→high; `preamp` is dB.
#[derive(Debug, Clone)]
pub struct EqPreset {
    pub name: String,
    pub preamp: f32,
    pub bands: [f32; crate::audio::eq::EQ_BANDS],
}

/// Which optional tracklist columns are visible. Title is always shown.
#[derive(Debug, Clone)]
pub struct Columns {
    pub index: bool,
    pub artist: bool,
    pub album_artist: bool,
    pub album: bool,
    pub year: bool,
    pub genre: bool,
    pub composer: bool,
    pub format: bool,
    pub bitrate: bool,
    pub rating: bool,
    pub time: bool,
    pub plays: bool,
    pub comment: bool,
}

impl Default for Columns {
    fn default() -> Self {
        // default set: track number, title, artist, album, year, rating, time
        Self {
            index: true,
            artist: true,
            album_artist: false,
            album: true,
            year: true,
            genre: false,
            composer: false,
            format: false,
            bitrate: false,
            rating: true,
            time: true,
            plays: false,
            comment: false,
        }
    }
}

/// Partial on-disk representation; every field optional so a sparse file still
/// parses and merges over defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ConfigFile {
    music_dirs: Option<Vec<PathBuf>>,
    theme: Option<String>,
    theme_follows_system: Option<bool>,
    light_theme: Option<String>,
    dark_theme: Option<String>,
    volume: Option<u8>,
    gapless: Option<bool>,
    crossfade_ms: Option<u32>,
    silence_skip: Option<bool>,
    radio_dvr: Option<bool>,
    radio_dvr_minutes: Option<u32>,
    replaygain: Option<u8>,
    replaygain_preamp: Option<f32>,
    sort_order: Option<String>,
    album_art: Option<bool>,
    dynamic_accent: Option<bool>,
    panes_horizontal: Option<bool>,
    grid_circle: Option<bool>,
    grid_card_size: Option<String>,
    track_columns: Option<bool>,
    /// Legacy key (pre-"unified track layout"): read when `track_columns` is
    /// absent so an existing config keeps its rows/columns choice on upgrade.
    popular_columns: Option<bool>,
    arabic_shaping: Option<bool>,
    spotify_client_id: Option<String>,
    spotify_bitrate: Option<u16>,
    spotify_show_account: Option<bool>,
    icon_set: Option<String>,
    icons: Option<crate::icons::IconOverrides>,
    player_viz: Option<bool>,
    player_viz_mode: Option<u8>,
    mouse: Option<bool>,
    next_hint: Option<bool>,
    os_media_controls: Option<bool>,
    touchpad_speed: Option<String>,
    grid_scroll_lock: Option<bool>,
    overlay_size: Option<u8>,
    fps: Option<u8>,
    radio_refresh_days: Option<u32>,
    reduced_motion: Option<bool>,
    visualizer: Option<VizFile>,
    layout: Option<LayoutFile>,
    columns: Option<ColumnsFile>,
    lyrics: Option<LyricsFile>,
    eq: Option<EqFile>,
}

/// `[eq]` section — the 10-band equalizer state (see [`crate::audio::eq`]).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct EqFile {
    enabled: Option<bool>,
    preamp: Option<f32>,
    /// Per-band gains (dB). Extra entries are ignored; missing ones stay flat.
    bands: Option<Vec<f32>>,
    preset: Option<String>,
}

/// `eq_presets.toml` — the user's saved custom presets (an array of `[[preset]]`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct EqPresetsFile {
    pub preset: Vec<EqPresetFile>,
}

/// One `[[preset]]` entry in `eq_presets.toml`.
#[derive(Debug, Deserialize)]
pub(super) struct EqPresetFile {
    pub name: String,
    #[serde(default)]
    pub preamp: f32,
    #[serde(default)]
    pub bands: Vec<f32>,
}

/// `[lyrics]` section — lyrics display options.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LyricsFile {
    align: Option<u8>,
    gap: Option<u8>,
    gradient: Option<bool>,
    color: Option<u8>,
    karaoke: Option<bool>,
    dual: Option<bool>,
    teleprompter: Option<bool>,
    queue: Option<bool>,
    viz: Option<bool>,
    offset: Option<i32>,
    translate_to: Option<String>,
}

/// `[columns]` section — tracklist column visibility.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ColumnsFile {
    index: Option<bool>,
    artist: Option<bool>,
    album_artist: Option<bool>,
    album: Option<bool>,
    year: Option<bool>,
    genre: Option<bool>,
    composer: Option<bool>,
    format: Option<bool>,
    bitrate: Option<bool>,
    rating: Option<bool>,
    time: Option<bool>,
    plays: Option<bool>,
    comment: Option<bool>,
}

/// `[layout]` section — dashboard column widths.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct LayoutFile {
    sidebar_width: Option<u16>,
    artist_width: Option<u16>,
}

/// `[visualizer]` section.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct VizFile {
    /// Animation frame rate (also sets the global UI tick rate).
    frame_rate: Option<u8>,
    peak_caps: Option<bool>,
    gravity: Option<f32>,
    peak_hang: Option<u16>,
}
