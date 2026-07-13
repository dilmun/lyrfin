//! Shared value types: navigation, layout/panels, the sidebar tree, and the
//! settings vocabulary. Pure data (no `AppState`) used across app/UI/keymap.

use crate::core::model::PlaylistId;

/// Top-level destinations (the "what am I looking at"). Some variants are part
/// of the navigation model but not yet reachable via a binding.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Dashboard,
    Library,
    NowPlaying,
    Lyrics,
    Visualizer,
    Queue,
    Search,
    Settings,
}

/// Which edge a movable panel is docked to. Cycles L → T → R → B.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dock {
    Left,
    #[default]
    Right,
    Top,
    Bottom,
}

impl Dock {
    pub fn cycle(self) -> Dock {
        match self {
            Dock::Left => Dock::Top,
            Dock::Top => Dock::Right,
            Dock::Right => Dock::Bottom,
            Dock::Bottom => Dock::Left,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Dock::Left => "left",
            Dock::Right => "right",
            Dock::Top => "top",
            Dock::Bottom => "bottom",
        }
    }
    pub fn from_label(s: &str) -> Dock {
        match s {
            "left" => Dock::Left,
            "top" => Dock::Top,
            "bottom" => Dock::Bottom,
            _ => Dock::Right,
        }
    }
}

/// Which source a lyrics pane represents. The local views and the Spotify view
/// share one `lyrics_panel` component reading one `meta.lyrics` slot, so each
/// call declares its source — the pane then only shows lyrics loaded for *that*
/// source, never the other's (see `AppState::lyrics_for_pane`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LyricsPane {
    Local,
    Spotify,
}

/// A movable, toggleable panel that a view can host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Panel {
    Queue,
    Sidebar,
    Artist,
    Visualizer,
    /// Synced/plain lyrics as a dockable pane (distinct from `Layout::LyricsFocus`,
    /// which is the whole-view lyrics screen).
    Lyrics,
}

impl Panel {
    pub fn label(self) -> &'static str {
        match self {
            Panel::Queue => "Queue",
            Panel::Sidebar => "Sidebar",
            Panel::Artist => "Artist panel",
            Panel::Visualizer => "Visualizer",
            Panel::Lyrics => "Lyrics",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Panel::Queue => "queue",
            Panel::Sidebar => "sidebar",
            Panel::Artist => "artist",
            Panel::Visualizer => "viz",
            Panel::Lyrics => "lyrics",
        }
    }

    /// Collapse priority when the terminal runs out of room: panes with a LOWER
    /// rank are hidden first (less important). The main content is never a pane,
    /// so it is always kept; the sidebar (navigation) is kept longest.
    pub fn collapse_rank(self) -> u8 {
        match self {
            Panel::Lyrics => 0,
            Panel::Visualizer => 1,
            Panel::Artist => 2,
            Panel::Queue => 3,
            Panel::Sidebar => 4,
        }
    }
}

/// Per-view state of one panel: shown, which edge it's docked to, and its size
/// as a **percentage** of the window along the dock axis (width for left/right,
/// height for top/bottom) — never an absolute cell count, so every pane scales
/// with the terminal and collapses gracefully as it shrinks.
#[derive(Debug, Clone, Copy)]
pub struct PanelCfg {
    pub shown: bool,
    pub dock: Dock,
    /// Percentage (1–100) of the window along the dock axis — the band thickness
    /// (column width for left/right, row height for top/bottom).
    pub size: u16,
    /// Relative weight (a share, not a percentage) along the *cross* axis when
    /// this pane shares its edge with others: height for left/right docks, width
    /// for top/bottom. Equal weights (the default) split the band evenly; raising
    /// one makes that pane taller/wider relative to its column-mates. Ignored when
    /// the pane is alone on its edge (it fills the band).
    pub len: u16,
}

/// Switchable screen arrangements (maps 1:1 to the mockups in `design/`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layout {
    FullPlayer,
    LyricsFocus,
    Dashboard,
    /// Library: a 3-column "Miller" browser — ARTISTS → ALBUMS (of the selected
    /// artist) → TRACKS (of the selected album). Bound to `2`. Optional
    /// Queue/Artist/Lyrics panes dock around the columns via the shared shell.
    LibraryFocus,
    /// Fullscreen, distraction-free now-playing (the "Concert" wow-mode).
    Concert,
    /// Internet radio: search + browse stations (its own list, not the library).
    Radio,
    /// Spotify: a librespot-backed client (library/search/playback) for a
    /// personal Premium account. Its own context, not the local library.
    Spotify,
}

/// Fixed character width of the status-bar view-name slot: every [`Layout::title`]
/// fits (longest is "Library"/"Playing"/"Concert"), so switching views never
/// reflows the bar.
pub const VIEW_NAME_W: u16 = 7;

impl Layout {
    /// Human-facing name shown in the status bar and command palette. Kept short
    /// and ≤ [`VIEW_NAME_W`] so the status-bar view slot is a fixed width.
    pub fn title(self) -> &'static str {
        match self {
            Layout::Dashboard => "Home",
            Layout::LibraryFocus => "Library",
            Layout::FullPlayer => "Playing",
            Layout::LyricsFocus => "Lyrics",
            Layout::Concert => "Concert",
            Layout::Radio => "Radio",
            Layout::Spotify => "Spotify",
        }
    }

    /// Movable panels this view hosts (order = how they're laid out / listed).
    pub fn panels(self) -> &'static [Panel] {
        match self {
            // The library/section Sidebar is a movable dock pane too (defaults to
            // the left edge), alongside Queue/Artist/Lyrics — listed first so it
            // docks outermost. Same set for Spotify so every source view matches.
            Layout::Dashboard => &[Panel::Sidebar, Panel::Queue, Panel::Artist, Panel::Lyrics],
            // The 3 columns are the navigation (no Sidebar); Queue/Artist/Lyrics
            // dock optionally around them.
            Layout::LibraryFocus => &[Panel::Queue, Panel::Artist, Panel::Lyrics],
            Layout::FullPlayer => &[Panel::Visualizer, Panel::Queue],
            Layout::LyricsFocus => &[Panel::Visualizer, Panel::Queue],
            Layout::Spotify => &[Panel::Sidebar, Panel::Queue, Panel::Artist, Panel::Lyrics],
            _ => &[],
        }
    }

    /// Whether `panel` can be moved/resized in this view. The Now Playing
    /// visualizer is the main content (fills the view), so it's fixed.
    pub fn panel_movable(self, panel: Panel) -> bool {
        !matches!((self, panel), (Layout::FullPlayer, Panel::Visualizer))
    }

    /// Default state of `panel` for this view (used until the user changes it).
    pub fn default_panel(self, panel: Panel) -> PanelCfg {
        use Dock::*;
        let (shown, dock) = match (self, panel) {
            // Home (the default view) mirrors the hero layout: the LIBRARY sidebar
            // and Artist panel stacked on the left, Queue and Lyrics stacked on the
            // right, with the tracklist in the centre.
            (Layout::Dashboard, Panel::Sidebar) => (true, Left),
            (Layout::Dashboard, Panel::Artist) => (true, Left),
            (Layout::Dashboard, Panel::Queue) => (true, Right),
            (Layout::Dashboard, Panel::Lyrics) => (true, Right),
            (Layout::Spotify, Panel::Sidebar) => (true, Left),
            (_, Panel::Queue) => (false, Right),
            (Layout::LyricsFocus, Panel::Visualizer) => (false, Left),
            (Layout::FullPlayer, Panel::Visualizer) => (true, Top),
            _ => (false, Right),
        };
        // size = percentage of the window along the dock axis (see `PanelCfg`)
        let size = match panel {
            Panel::Sidebar => 22,
            Panel::Queue => 26,
            Panel::Artist => 26,
            Panel::Lyrics => 30,
            Panel::Visualizer => 40,
        };
        PanelCfg {
            shown,
            dock,
            size,
            len: 50, // equal cross-axis share until the user adjusts it
        }
    }
}

/// Focusable regions of a source view; keyboard input routes to the focused one.
/// Unified across every view (local / Spotify / Radio) — each view's focus ring
/// (`focus_order`) picks which regions it actually exposes. `Main` is the view's
/// primary content (tracklist / Spotify result list / radio stations / now-playing
/// card / lyrics); `Pane` is a movable dock pane (Queue / Artist / Lyrics / …);
/// `Search` is the text-input search mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Main,
    Pane(Panel),
    Search,
}

/// Smart/discovery track lists. Four of these (Recently Added/Played, Most
/// Played, Favorites) back the local library's section sidebar via `smart_ids`;
/// the rest (All Tracks, On This Day, Forgotten, Duplicates, Needs Tags) are
/// computed discovery views that the new sections model doesn't surface yet — the
/// resolver (`smart_ids` / `smart_count`) stays complete so they re-wire trivially.
#[allow(dead_code)] // discovery variants kept for the resolver; not all are surfaced yet
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmartList {
    AllTracks,
    RecentlyAdded,
    RecentlyPlayed,
    MostPlayed,
    Favorites,
    OnThisDay,
    Forgotten,
    Duplicates,
    Untagged,
}

/// Target of a text-entry: create/rename a playlist, or add a music directory.
#[derive(Debug, Clone, Copy)]
pub enum NameTarget {
    New,
    Rename(PlaylistId),
    AddMusicDir,
    /// Name a bookmark for the current search query.
    Bookmark,
    /// Name a smart playlist for the current search query.
    SmartPlaylist,
    /// Paste your private Spotify Client ID (escapes the shared rate limit).
    SpotifyClientId,
}

/// A selectable row in the Settings tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    MusicDir(usize),
    AddDir,
    Rescan,
    Theme,
    /// Follow the OS light/dark appearance (macOS + Linux): pick `LightTheme`/`DarkTheme`
    /// to match and switch live. When on, the single `Theme` row is hidden in favour of
    /// the two per-appearance pickers below.
    ThemeFollowSystem,
    /// The theme used while the system is in Light mode (shown only when following).
    LightTheme,
    /// The theme used while the system is in Dark mode (shown only when following).
    DarkTheme,
    AlbumArt,
    DynamicAccent,
    IconSet,
    PlayerViz,
    PlayerVizMode,
    /// Side panes (Queue/Artist/Lyrics) stack vertically or sit side-by-side.
    PanesLayout,
    /// Grid vs list for the current browse section (Albums/Artists, + Spotify
    /// Playlists) — the settings-row twin of the `#` key. Runtime per-section
    /// state, not a `Config` bool (see `toggle_grid_current`).
    GridList,
    /// Albums/Artists grid cards: circle (round avatars) vs rounded square.
    GridShape,
    /// Albums/Artists grid card size (small / medium / large).
    GridSize,
    /// Track-list layout, shared across every view: compact rows vs a column
    /// table. Drives the local + Spotify main lists and the artist POPULAR list.
    TrackColumns,
    Mouse,
    /// Show the "▶ Next:" up-next hint in the status bar.
    NextHint,
    /// Report playback to the OS "Now Playing" surface + accept its media keys
    /// (macOS Control Center / AirPods, Linux MPRIS). See `crate::media`.
    OsMediaControls,
    /// Touchpad two-finger scroll speed over cover grids / carousels
    /// (slow / normal / fast) — cycles [`crate::config::TouchpadSpeed`].
    TouchpadScroll,
    /// Lock touchpad horizontal grid scroll to the current row/carousel (a sideways
    /// swipe clamps at the row end instead of wrapping onto the next row).
    GridScrollLock,
    /// How large the big overlays (Settings / Info / Tag editor) open — a step
    /// from a compact card up to a semi-full one (`f` cycles it).
    OverlaySize,
    ReducedMotion,
    PeakCaps,
    Fps,
    RadioRefresh,
    Gapless,
    Crossfade,
    SilenceSkip,
    ReplayGain,
    ColIndex,
    ColArtist,
    ColAlbumArtist,
    ColAlbum,
    ColYear,
    ColGenre,
    ColComposer,
    ColFormat,
    ColBitrate,
    ColRating,
    ColTime,
    ColPlays,
    ColComment,
    LyricsAlign,
    LyricsGap,
    LyricsGradient,
    LyricsColor,
    LyricsKaraoke,
    LyricsDual,
    /// Machine-translate lyrics into a chosen language (cycles `translate::LANGS`).
    LyricsTranslate,
    LyricsTeleprompter,
    /// Per-view panel controls (the "Panes" group): show it, which edge it docks
    /// to, and its size. The panels offered follow the current view (`layout.panels`).
    PanelShow(Panel),
    PanelDock(Panel),
    PanelSize(Panel),
    /// Log out of Spotify + clear the cached token (shown only in the Spotify
    /// view's `;` popup while connected — replaces the old `L` shortcut).
    SpotifyLogout,
    /// Set / clear your private Spotify client id (opens the paste prompt). Needed
    /// when switching to an account registered on a different dev app.
    SpotifyClientId,
    /// Re-run the Spotify login (browser, or cached-token resume) — e.g. after
    /// changing the client id, or to switch accounts.
    SpotifyReauth,
    /// Spotify streaming quality (96 / 160 / 320 kbps); applies on next reconnect.
    SpotifyBitrate,
    /// Show the connected account (display name) on the Spotify header line —
    /// toggle off for privacy. The Info overlay always shows it regardless.
    SpotifyShowAccount,
    /// A configurable key binding (index into `keymap::configurable_actions`).
    Keybind(usize),
}

/// Top-level settings groups, in the global overlay's tab order. The global
/// Settings overlay and the per-view `;` popup share this one vocabulary, so a
/// group name means the same thing in both places (the popup just curates a
/// subset per view). The overlay drops a group only when the current view has no
/// rows for it — in practice only "Panes" (Radio/Concert host no movable panels);
/// see [`crate::app::AppState::settings_tabs`].
pub const SETTINGS_GROUPS: [&str; 11] = [
    "General",
    "Panes",
    "Grid",
    "Tracklist",
    "Audio",
    "Visualizer",
    "Lyrics",
    "Theme",
    "Spotify",
    "Library",
    "Keys",
];

impl Setting {
    /// The top-level group this setting belongs to (one of `SETTINGS_GROUPS`).
    pub fn group(self) -> &'static str {
        use Setting::*;
        match self {
            Mouse | NextHint | OsMediaControls | TouchpadScroll | GridScrollLock | OverlaySize
            | ReducedMotion | Fps | IconSet | RadioRefresh => "General",
            // the movable side panels (show / dock / size) + their stacking axis
            PanelShow(_) | PanelDock(_) | PanelSize(_) | PanesLayout => "Panes",
            // the cover-art grid: list⇄grid, card shape, card size
            GridList | GridShape | GridSize => "Grid",
            // the track table: the rows⇄columns layout switch leads the per-column
            // visibility toggles it governs
            TrackColumns | ColIndex | ColArtist | ColAlbumArtist | ColAlbum | ColYear
            | ColGenre | ColComposer | ColFormat | ColBitrate | ColRating | ColTime | ColPlays
            | ColComment => "Tracklist",
            Gapless | Crossfade | SilenceSkip | ReplayGain => "Audio",
            PlayerViz | PlayerVizMode | PeakCaps => "Visualizer",
            LyricsAlign | LyricsGap | LyricsGradient | LyricsColor | LyricsKaraoke | LyricsDual
            | LyricsTranslate | LyricsTeleprompter => "Lyrics",
            Theme | ThemeFollowSystem | LightTheme | DarkTheme | AlbumArt | DynamicAccent => {
                "Theme"
            }
            SpotifyLogout | SpotifyClientId | SpotifyReauth | SpotifyBitrate
            | SpotifyShowAccount => "Spotify",
            MusicDir(_) | AddDir | Rescan => "Library",
            Keybind(_) => "Keys",
        }
    }
}

impl Panel {
    /// The size setting paired with this panel (shown under it in the Panes group).
    pub fn size_setting(self) -> Setting {
        Setting::PanelSize(self)
    }
}
