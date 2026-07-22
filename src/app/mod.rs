//! `AppState` — the single source of truth, plus the `update` reducer.
//!
//! Everything the UI draws is derived from this struct. `update` is the only
//! mutator and is a pure function of `(state, action)`, which makes the whole
//! core unit-testable without a terminal.

use std::path::PathBuf;
use std::time::Duration;

use ratatui::layout::Rect;

use crate::action::{Action, Motion};
use crate::audio::{AudioCommand, AudioEngine, AudioEvent, NullEngine};
use crate::config::Config;
use crate::core::model::{ArtistId, PlaylistId, Track, TrackId};
// Used only by the test-only `seed_demo` fixture (session_restore.rs); imported
// under cfg(test) so non-test builds stay warning-clean.
#[cfg(test)]
use crate::core::model::{Album, AlbumId, Artist, AudioInfo, Codec, Playlist};
use crate::core::player::{PlayerState, Repeat, Status};
use crate::library::store::UserData;
use crate::library::{Library, LibraryEvent, search};
use crate::ui::theme::Theme;

fn layout_to_str(l: Layout) -> &'static str {
    match l {
        Layout::Dashboard => "dashboard",
        Layout::LibraryFocus => "library_focus",
        Layout::FullPlayer => "full_player",
        Layout::LyricsFocus => "lyrics_focus",
        Layout::Concert => "concert",
        Layout::Radio => "radio",
        Layout::Spotify => "spotify",
    }
}

/// Whether a layout renders its own large (per-view) visualizer — separate from
/// the small playback-bar visualizer that every transport bar can show. Concert
/// uses the playback-bar visualizer (toggle + mode) as its single viz, so it's
/// not a big-viz view (cycling the visualizer there cycles the playback mode).
fn layout_has_big_viz(l: Layout) -> bool {
    matches!(l, Layout::FullPlayer | Layout::LyricsFocus)
}

fn panel_from_key(s: &str) -> Option<Panel> {
    Some(match s {
        "queue" => Panel::Queue,
        "sidebar" => Panel::Sidebar,
        "artist" => Panel::Artist,
        "viz" => Panel::Visualizer,
        "lyrics" => Panel::Lyrics,
        _ => return None,
    })
}

fn layout_from_str(s: &str) -> Option<Layout> {
    Some(match s {
        "dashboard" => Layout::Dashboard,
        "library_focus" => Layout::LibraryFocus,
        "full_player" => Layout::FullPlayer,
        "lyrics_focus" => Layout::LyricsFocus,
        "concert" => Layout::Concert,
        "radio" => Layout::Radio,
        "spotify" => Layout::Spotify,
        _ => return None,
    })
}

fn focus_to_str(f: Focus) -> &'static str {
    match f {
        Focus::Sidebar => "sidebar",
        Focus::Main => "main",
        Focus::Pane(Panel::Queue) => "queue",
        Focus::Pane(_) => "main", // other panes aren't persisted as a local focus
        Focus::Search => "search",
    }
}

fn focus_from_str(s: &str) -> Option<Focus> {
    Some(match s {
        "sidebar" => Focus::Sidebar,
        // legacy variants (tracklist / now-playing / lyrics) all fold into Main
        "main" | "tracklist" | "nowplaying" | "lyrics" => Focus::Main,
        "queue" => Focus::Pane(Panel::Queue),
        "search" => Focus::Search,
        _ => return None,
    })
}

/// Expand a leading `~` to the user's home directory.
fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(s)
}

fn repeat_to_str(r: Repeat) -> &'static str {
    match r {
        Repeat::Off => "off",
        Repeat::One => "one",
        Repeat::All => "all",
    }
}

fn repeat_from_str(s: &str) -> Repeat {
    match s {
        "one" => Repeat::One,
        // "all", and the retired "album"/"artist" scope-repeat modes, all map to
        // looping the queue now (those scopes are "play album/artist" + repeat-all).
        "all" | "album" | "artist" => Repeat::All,
        _ => Repeat::Off,
    }
}

/// Internal visualizer resolution (bands are interpolated to panel width).
pub const VIZ_BANDS: usize = 96;

/// Apply a [`Motion`] to a selection index within `0..len`.
fn step(cur: usize, m: Motion, len: usize) -> usize {
    let max = len.saturating_sub(1);
    match m {
        Motion::Up => cur.saturating_sub(1),
        Motion::Down => (cur + 1).min(max),
        Motion::Top => 0,
        Motion::Bottom => max,
        Motion::PageUp => cur.saturating_sub(10),
        Motion::PageDown => (cur + 10).min(max),
        _ => cur,
    }
}

mod types;
pub use types::*;
mod browser;
pub use browser::Browser;
mod covers;
pub use covers::{CoverArt, CoverSearch, CoverStatus};
mod grid_art;
pub use grid_art::{ArtChange, ArtThumb};
mod discovery;
mod effects;
pub use effects::PlaybackFx;
mod equalizer;
pub use equalizer::{EQ_CONTROLS, EQ_PREAMP, EqUi};
mod find;
pub use find::SearchState;
mod info;
pub use info::{Info, InfoTab};
mod init;
mod layout;
pub use layout::ViewState;
mod library_sync;
pub use library_sync::ScanState;
mod local_browse;
pub use local_browse::{LocalBrowse, LocalItem, LocalSection};
pub(crate) mod release;
pub(crate) use release::ReleaseRow;
mod mouse;
mod nav;
mod navigation;
mod nowplaying;
pub use nowplaying::NpSource;
mod palette;
mod pane_resize;
pub use palette::{Palette, PaletteCtx};
mod settings_choices;
pub use settings_choices::{Choice, ChoiceValue, SettingChoices};
mod playback;
mod playlists;
pub use playlists::ModalInput;
mod radio;
pub use radio::{LIVE_EDGE_SECS, PickerKind, Radio, RadioNameTarget, RadioSection};
mod radio_playlists;
mod selection;
pub use selection::{MarkKey, MultiSelect};
mod session_restore;
mod settings;
mod sorting;
pub use settings::SettingsUi;
pub use sorting::{SortField, parse_sort};
pub(crate) mod spotify;
pub use spotify::Spotify;
mod tags;
pub use tags::{PendingApply, TagModal};
mod tick;
mod update;
mod visualizer;
pub use visualizer::Visualizer;
mod workers;

/// How the library browser is sliced (Artist/Album/Track/...).
// Some browse modes are defined but not yet reachable in the library view.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryView {
    Artists,
    Albums,
    Tracks,
    Playlists,
    Folders,
    Genres,
    RecentlyAdded,
    RecentlyPlayed,
    Favorites,
    SearchResults,
}

/// Transient banner shown in the status bar / notification system.
#[derive(Debug, Clone)]
pub struct Notification {
    pub text: String,
    pub ttl_ticks: u16,
}

/// A clickable transport control.
#[derive(Debug, Clone, Copy)]
pub enum TransportButton {
    Shuffle,
    Prev, // previous track
    PlayPause,
    Next, // next track
    Repeat,
}

/// What a mouse click on a screen region should do. The render layer registers
/// `(Rect, MouseTarget)` pairs each frame (a "hit map") that clicks resolve against.
#[derive(Debug, Clone, Copy)]
pub enum MouseTarget {
    Track(usize),           // a tracklist row (index into display_ids)
    BrowseArtist(usize),    // a Library ARTISTS column row (select)
    BrowseAlbum(usize),     // a Library ALBUMS column row (select)
    BrowseTrack(usize),     // a Library TRACKS column row (click select, dbl-click play)
    QueueRow(usize),        // a QUEUE pane row (index into queue.items)
    SpotifyQueueRow(usize), // a Spotify QUEUE pane row (click select, dbl-click play)
    SpotifyItem(usize),     // a Spotify browse item / grid card (click select, dbl-click open)
    GridScroll(usize), // a carousel ‹/› arrow: select (reveal) this card — scroll only, never activate
    OpenSpotifyDashboard, // the "developer.spotify.com/dashboard" link in the guide
    OpenSpotifyAuthUrl, // the auth URL in the login panel (opens the full URL)
    Transport(TransportButton),
    Seek,                    // the progress bar — fraction from the click x within the rect
    Volume,                  // the volume bar — level from the click y within the rect
    Tree(usize),             // a sidebar section row (selects + loads that LocalSection)
    SettingRow(usize),       // a Settings row
    RadioGoLive,             // the LIVE badge on the radio DVR bar: jump to the live edge
    RadioRow(usize),         // a station row in the Radio view (click select, dbl play)
    RadioSectionRow(usize),  // a Radio sidebar section row (index into RadioSection::ALL)
    RadioPlaylistRow(usize), // a row in the flat radio-playlist list (click select, dbl drill)
    RadioPick(usize),        // a country/genre row in the Radio picker
    RadioChip(u8),           // a Radio filter chip: 0 search·1 country·2 genre·3 sort
    PaletteRow(usize),       // a command-palette row (index into palette_matches; dbl-click runs)
    SpotifySection(usize),   // a Spotify sidebar section row (index into Section::ALL)
    /// A tab in a framed tabbed overlay (Info / global Settings) — switches that
    /// overlay's active tab. Registered per tab by `components::overlay_frame`.
    OverlayTab(usize),
    /// Whole-panel region (registered under the rows) so a scroll/click anywhere
    /// in a box — even empty space — targets that box, not the focused one.
    Scroll(ScrollBox),
}

/// Which scrollable list a panel region belongs to.
#[derive(Debug, Clone, Copy)]
pub enum ScrollBox {
    Tracklist,
    Tree,
    Queue,
    Settings,
    Artist,
    Lyrics,
    Radio,
    SpotifyQueue,
    BrowseArtists,
    BrowseAlbums,
    BrowseTracks,
}

/// What a pane-resize drag adjusts. A `Band` edge is the pane↔main boundary
/// (changes the pane's `size` %); a `Divider` is the line between two panes
/// stacked on one edge (changes their cross-axis `len` share). See `pane_resize`.
#[derive(Debug, Clone, Copy)]
pub enum ResizeKind {
    /// The pane↔main boundary — drag moves it, resizing the band's `size`.
    Band { dock: Dock },
    /// The boundary between two panes on the same edge — drag shifts the split of
    /// their shared band. `axis_y` = the divider moves vertically (stacked panes).
    Divider { a: Panel, b: Panel, axis_y: bool },
}

/// A draggable pane-resize handle registered by the render layer each frame: the
/// thin `strip` the pointer grabs, the `area` the drag is measured against (the
/// docking frame for a `Band`, the two panes' combined region for a `Divider`),
/// and the `kind` of resize. Kept in its own registry (not the click hit-map)
/// because a resize needs both the grab strip *and* that reference frame.
#[derive(Debug, Clone, Copy)]
pub struct ResizeEdge {
    pub strip: Rect,
    pub area: Rect,
    pub kind: ResizeKind,
}

/// An in-progress pane-resize drag: the handle grabbed and its reference frame,
/// captured at grab time so the resize keeps tracking once the pointer leaves the
/// strip.
#[derive(Debug, Clone, Copy)]
pub struct ResizeDrag {
    pub area: Rect,
    pub kind: ResizeKind,
}

/// The app's global interaction mode. Derived from state: editing a tag → Edit,
/// a visual range → Visual, otherwise View. Not yet surfaced in the status bar.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    View,
    Visual,
    Edit,
}

/// Category order for the command palette (groups are shown in this order).
#[allow(dead_code)]
pub const PALETTE_GROUPS: [&str; 9] = [
    "View",
    "Playback",
    "Audio",
    "Visualizer",
    "Library",
    "Playlist",
    "Tags",
    "Settings",
    "App",
];

/// All runnable commands for the palette, grouped: `(category, label, action)`.
/// Listed in `PALETTE_GROUPS` order so the palette browses as tidy sections.
pub fn palette_commands() -> Vec<(&'static str, &'static str, Action)> {
    use Action::*;
    vec![
        // View — switch the active layout / panels (in display order)
        ("View", "Home", SwitchLayout(Layout::Dashboard)),
        (
            "View",
            "Library — Artists / Albums / Tracks  (2)",
            SwitchLayout(Layout::LibraryFocus),
        ),
        ("View", "Now Playing", SwitchLayout(Layout::FullPlayer)),
        ("View", "Lyrics", SwitchLayout(Layout::LyricsFocus)),
        ("View", "Radio — internet stations  (6)", OpenRadio),
        ("View", "Spotify  (7)", OpenSpotify),
        (
            "View",
            "Concert (fullscreen)",
            SwitchLayout(Layout::Concert),
        ),
        ("Settings", "Settings…", OpenSettings),
        ("View", "Toggle library sidebar  (b)", ToggleSidebar),
        ("View", "Move library sidebar — cycle edge", MoveSidebar),
        ("View", "Toggle queue panel  (u)", ToggleQueue),
        (
            "View",
            "Move queue panel — cycle edge  (ctrl-Q)",
            ToggleQueueSide,
        ),
        ("View", "Move artist panel — cycle edge", MoveArtistPanel),
        ("View", "Move lyrics visualizer — cycle edge", MoveLyricsViz),
        (
            "View",
            "Toggle visualizer (this view)  (shift-V)",
            ToggleLyricsViz,
        ),
        ("View", "Settings for this view  (;)", OpenViewSettings),
        ("View", "Fit panes to the window  (z)", FitLayout),
        ("View", "Reset this view's layout  (shift-Z)", ResetLayout),
        ("View", "Toggle lyrics panel", ToggleLyrics),
        (
            "View",
            "Cycle lyric format (plain/karaoke/teleprompter)",
            CycleLyricsFormat,
        ),
        ("View", "Toggle artist info", ToggleArtistInfo),
        // Playback
        ("Playback", "Play / Pause", TogglePlay),
        ("Playback", "Next track", Next),
        ("Playback", "Previous track", Previous),
        ("Playback", "Toggle shuffle", ToggleShuffle),
        ("Playback", "Cycle repeat (off/one/all)", CycleRepeat),
        ("Playback", "Play this track's album", PlayCurrentAlbum),
        ("Playback", "Play this track's artist", PlayCurrentArtist),
        ("Playback", "A-B loop (set A / B / clear)", AbLoopCycle),
        ("Playback", "Clear queue", ClearQueue),
        // Audio
        ("Audio", "Equalizer  (e)", OpenEqualizer),
        ("Audio", "ReplayGain: Off / Track / Album", CycleReplayGain),
        ("Audio", "Sleep timer: 15 min", SetSleepTimer(15)),
        ("Audio", "Sleep timer: 30 min", SetSleepTimer(30)),
        ("Audio", "Sleep timer: 60 min", SetSleepTimer(60)),
        ("Audio", "Sleep timer: off", SetSleepTimer(0)),
        // Visualizer
        ("Visualizer", "Cycle visualizer mode", CycleVisualizer),
        // Library
        ("Library", "Search…", BeginSearch),
        ("Library", "Play random album", RandomAlbum),
        ("Library", "Rescan library", RescanLibrary),
        (
            "Library",
            "Sort: artist, year, album",
            RunCommand("sort:artist,year,album".into()),
        ),
        ("Library", "Sort: off", RunCommand("sort:off".into())),
        ("Library", "Library & listening stats", ToggleStats),
        // Playlist
        ("Playlist", "Add to playlist", AddToPlaylistPrompt),
        (
            "Playlist",
            "New smart playlist from search",
            NewSmartPlaylist,
        ),
        // Tags — one unified modal (Edit · Auto Tag · Cover tabs)
        ("Tags", "Tag Edit", OpenTags),
        // Settings
        ("Settings", "Cycle theme", CycleTheme),
        ("Settings", "Keybindings help", ToggleHelp),
        // App
        ("App", "Track info", ToggleTrackInfo),
        ("App", "Error log / health", ToggleErrorLog),
        ("App", "Copy last error", CopyError),
        ("App", "Quit", Quit),
    ]
}

/// Which view(s) a help entry applies to. The `?` overlay shows the Global rows
/// everywhere and the per-context rows only in their own view, so it lists just
/// what actually works where you are.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HelpScope {
    /// Every view: view switching, palette, settings, transport, volume, …
    Global,
    /// The local music player (Home / Library / Now Playing / Lyrics / Concert).
    Local,
    /// The Spotify view only.
    Spotify,
    /// The Radio view only.
    Radio,
}

impl HelpScope {
    /// Whether an entry with this scope is relevant in `layout`.
    fn shows_in(self, layout: Layout) -> bool {
        use Layout::*;
        match self {
            HelpScope::Global => true,
            HelpScope::Local => matches!(
                layout,
                Dashboard | LibraryFocus | FullPlayer | LyricsFocus | Concert
            ),
            HelpScope::Spotify => layout == Layout::Spotify,
            HelpScope::Radio => layout == Layout::Radio,
        }
    }
}

/// Keybinding reference rows (key, description, scope) for the searchable help
/// overlay. Global rows show in every view; the rest only in their own view.
/// Keep keys terse and descriptions searchable.
pub fn keybindings_help() -> Vec<(&'static str, &'static str, HelpScope)> {
    use HelpScope::*;
    vec![
        // --- Global: works in every view ---
        (
            "1-7",
            "switch view: Home · Library · Now Playing · Lyrics · Concert · Radio · Spotify",
            Global,
        ),
        (
            ": / ctrl-p",
            "command palette — Settings & Tag Edit live here; also typed commands (theme …, set …, toggle …, sort:…)",
            Global,
        ),
        (";", "settings for this view", Global),
        ("e", "equalizer — 10-band graphic EQ + presets", Global),
        ("?", "toggle this help", Global),
        ("space", "play / pause", Global),
        ("n / p", "next / previous (track / station)", Global),
        ("j / k", "move down / up", Global),
        ("ctrl-d / ctrl-u", "scroll a page down / up", Global),
        ("g / G", "jump to top / bottom", Global),
        ("tab", "cycle focus / pane / column", Global),
        ("enter", "play / open the selection", Global),
        ("/", "search", Global),
        (
            "h / l",
            "focus left / right — sidebar · list · panes (Library: switch column)",
            Global,
        ),
        (
            ", / .",
            "seek back / forward (Lyrics view: nudge sync)",
            Global,
        ),
        ("+ / -", "volume up / down", Global),
        ("t", "cycle theme", Global),
        (
            "T",
            "cycle the sleep timer (off → 15 → 30 → 45 → 60 min)",
            Global,
        ),
        ("shift-Q / ctrl-c", "quit", Global),
        // --- Local music player (Home / Library / Now Playing / Lyrics / Concert) ---
        (
            "/ …",
            "advanced search: artist: album: year>= rating>= plays> duration< fav -word OR",
            Local,
        ),
        ("f", "toggle favorite", Local),
        (") / (", "rate the current track up / down (stars)", Local),
        ("s / r", "shuffle / repeat (off / one / all)", Local),
        ("[ / ]", "speed down / up", Local),
        ("o", "A-B loop: set A, set B, then clear", Local),
        ("a", "add to playlist", Local),
        (
            "n / S",
            "playlists tab: new playlist / new smart playlist from search",
            Local,
        ),
        (
            "m / ⇧D",
            "playlists tab: move to folder / delete (a folder row ungroups it)",
            Local,
        ),
        (
            "x / V",
            "mark track / visual range — f, a, and tag-edit apply to the whole selection",
            Local,
        ),
        ("R", "play a random album", Local),
        (
            "B",
            "bookmark the current search (jump to saved searches via the palette)",
            Local,
        ),
        ("b", "toggle the library sidebar", Local),
        (
            "q / ctrl-Q",
            "toggle queue panel / cycle its edge (left/top/right/bottom)",
            Local,
        ),
        ("C", "clear queue", Local),
        ("J / K", "queue: move selected track down / up", Local),
        (
            "d / D",
            "queue: remove selected / clear all upcoming",
            Local,
        ),
        ("v", "cycle visualizer mode", Local),
        (
            "shift-V",
            "toggle this view's visualizer (independent per view)",
            Local,
        ),
        ("L", "toggle lyrics", Local),
        (
            "F",
            "lyrics: cycle format (plain / karaoke / teleprompter)",
            Local,
        ),
        ("i", "toggle artist info panel", Local),
        ("I", "library & listening stats overlay", Local),
        // --- Spotify view ---
        ("b", "focus the sections sidebar", Spotify),
        ("f", "like / unlike (Liked Songs)", Spotify),
        (
            "F",
            "follow / unfollow the selected show or artist",
            Spotify,
        ),
        ("s / r", "shuffle / repeat", Spotify),
        (
            "x / V",
            "mark track / visual range (on a track list) — f likes and a adds the whole selection",
            Spotify,
        ),
        ("c", "paste your Spotify client id", Spotify),
        ("esc", "leave search / back out of a drill-in", Spotify),
        // --- Radio view ---
        (
            "⏎ (sidebar)",
            "browse by country / genre — open the Countries / Genres sidebar sections",
            Radio,
        ),
        ("o", "cycle the result sort order", Radio),
        (
            "f",
            "star / unstar the selected station(s) (saved to Favorites)",
            Radio,
        ),
        (
            "x / V",
            "mark station / visual range — f stars and a adds the whole selection",
            Radio,
        ),
        ("R", "re-download the station directory", Radio),
        ("esc", "close a picker / leave search", Radio),
    ]
}

/// "on"/"off" for a boolean (command-line status messages).
fn onoff(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// Cached search result + the inputs it was computed from (see `ensure_search`).
#[derive(Default)]
pub(crate) struct SearchCache {
    query: String,
    sort: Vec<(SortField, bool)>,
    lib_gen: u64,
    ids: Vec<crate::core::model::TrackId>,
}

/// Title Case a string (capitalize each word; collapses runs of whitespace).
fn title_case(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// One logged error: when it happened (unix secs) + the message. Kept in a bounded
/// in-memory ring (`AppState.error_log`) and mirrored to `{dir}/errors.log`.
#[derive(Clone)]
pub struct LogEntry {
    pub ts: u64,
    pub msg: String,
}

/// How many recent errors the in-app log keeps (the on-disk `errors.log` is full).
pub const ERROR_LOG_CAP: usize = 200;

/// Online metadata for the current track + its async-fetch bookkeeping, grouped
/// out of AppState: the fetched `artist_info` bio (and which `info_artist` is
/// in-flight), and the synced `lyrics` (and which track's `lyrics_for` lookup is
/// in-flight). Request channels live in `workers`; results land via the drain.
#[derive(Default)]
pub struct TrackMeta {
    pub artist_info: Option<crate::artistinfo::ArtistInfo>,
    pub info_artist: Option<String>,
    pub info_pending: bool,
    pub lyrics: Option<crate::lyrics::Lyrics>,
    pub lyrics_for: Option<String>,
    pub lyrics_pending: bool,
    /// A machine-translation lookup for the current lyrics is in flight.
    pub lyrics_translate_pending: bool,
}

/// Sticky scroll offsets for AppState's scrollable regions, grouped out of the
/// struct. `list` keeps the centre tracklist from recentring when a visible row
/// is clicked; `items` does the same for the browse/container list (Albums &
/// Artists in list-mode, genres, playlists); `queue` for the QUEUE pane;
/// `artist`/`artist_max` are the dashboard ARTIST panel's bio scroll + its
/// last-rendered max (written by the renderer so the wheel can clamp).
/// Touchpad two-finger scroll accumulators (signed event counts) that throttle and
/// axis-lock cover-grid / carousel navigation. Horizontal events step one card only
/// once `h` crosses a threshold (a swipe fires many events → this slows it), and a
/// committed horizontal step clears `v` so sideways jitter never leaps to the next
/// row — a deliberate up/down gesture is what accumulates `v` past its threshold and
/// changes rows. See `grid_touch_scroll`.
#[derive(Default)]
pub struct GridScrollAccum {
    pub h: i32,
    pub v: i32,
}

#[derive(Default)]
pub struct ScrollOff {
    pub list: std::cell::Cell<usize>,
    pub items: std::cell::Cell<usize>,
    pub queue: std::cell::Cell<usize>,
    pub artist: std::cell::Cell<usize>,
    pub artist_max: std::cell::Cell<usize>,
    /// Manual lyrics/notes scroll: `lyrics` is the offset, `lyrics_max` the
    /// last-rendered clamp (written by the renderer). `lyrics_manual` flags that the
    /// user has taken over — until then the pane auto-follows playback; a track
    /// change resets both, restoring auto-scroll.
    pub lyrics: std::cell::Cell<usize>,
    pub lyrics_max: std::cell::Cell<usize>,
    pub lyrics_manual: std::cell::Cell<bool>,
}

/// The background-worker request channels + per-search sequence counters,
/// grouped out of the god object (slice 2 of the AppState split). Each `Option`
/// is filled once the corresponding worker thread is spawned (see the
/// `set_*_sender` methods).
#[derive(Default)]
struct Workers {
    info: Option<crossbeam_channel::Sender<crate::artistinfo::InfoRequest>>,
    lyrics: Option<crossbeam_channel::Sender<crate::lyricsfetch::LyricsRequest>>,
    /// Machine-translates lyric lines (see `crate::translate`).
    translate: Option<crossbeam_channel::Sender<crate::translate::TranslateRequest>>,
    /// Resolves a Spotify podcast episode to its public MP3 (Spotify won't let
    /// librespot decrypt episode audio — see `podcastfetch`).
    podcast: Option<crossbeam_channel::Sender<crate::podcastfetch::PodcastRequest>>,
    cover: Option<crossbeam_channel::Sender<crate::coversearch::CoverRequest>>,
    tag: Option<crossbeam_channel::Sender<crate::tagsearch::TagRequest>>,
    radio: Option<crossbeam_channel::Sender<crate::radio::RadioRequest>>,
    spotify: Option<crossbeam_channel::Sender<crate::spotify::api::SpRequest>>,
    /// Downloads the now-playing Spotify track's cover for the playback bar.
    spotify_art: Option<crossbeam_channel::Sender<crate::spotify::artwork::ArtRequest>>,
    /// Decodes/fetches library grid + Artist-pane thumbnails (album art, artist
    /// photos) off the UI thread.
    art: Option<crossbeam_channel::Sender<crate::artwork::ArtRequest>>,
    /// Monotonic counters giving each cover / tag / radio / spotify request a key.
    cover_seq: u64,
    tag_seq: u64,
    radio_seq: u64,
    spotify_seq: u64,
}

/// One row in a Radio filter picker: `(display label, choice)`. `choice` is
/// `None` for the "clear filter" row, else `(display name, api value)`.
pub type PickRow = (String, Option<(String, String)>);

/// Rank how well `name`/`code` match query `q` (lower = closer); `None` = no
/// match. An empty query matches everything at the same rank (0) so the source
/// order (popularity) is preserved.
fn match_score(name: &str, code: &str, q: &str) -> Option<u8> {
    // empty query, exact name, or exact country-code all rank best (0)
    if q.is_empty() || name == q || (!code.is_empty() && code == q) {
        Some(0)
    } else if name.starts_with(q) {
        Some(1)
    } else if name.contains(q) {
        Some(2)
    } else if !code.is_empty() && code.contains(q) {
        Some(3)
    } else {
        None
    }
}

/// Stable identity for a station (favorites de-dup): its uuid, or the stream
/// URL when the directory didn't supply one.
pub(crate) fn station_key(st: &crate::radio::Station) -> &str {
    if st.uuid.is_empty() {
        &st.url
    } else {
        &st.uuid
    }
}

pub struct AppState {
    pub running: bool,
    /// Set when state changes; the event loop only redraws when this is set (or
    /// something is animating). Keeps idle CPU near zero.
    pub dirty: bool,
    pub view: View,
    pub layout: Layout,
    pub focus: Focus,
    /// Selected library browse mode; the dedicated Library view that consumes it
    /// is not wired up yet.
    #[allow(dead_code)]
    pub library_view: LibraryView,

    pub player: PlayerState,
    pub library: Library,
    pub theme: Theme,
    /// The `auto` theme built from the terminal's own colours at startup (via an
    /// OSC palette query). `None` until queried / when the terminal can't answer;
    /// cached so switching back to `auto` re-applies it without re-querying.
    pub auto_theme: Option<Theme>,
    /// Follow-system mode: the theme name last applied by the OS light/dark switch
    /// (`None` = none applied yet). Compared each poll so a theme is only re-resolved
    /// and recolored when the appearance actually flips — never every tick.
    pub applied_sys_theme: Option<String>,
    /// Wall-clock throttle for the OS-appearance poll (`config.theme_follows_system`):
    /// checked on the render tick at ~1s so a live flip is prompt but the read cheap.
    pub appearance_at: std::time::Instant,
    /// Resolved transport glyphs (preset + custom overrides).
    pub icons: crate::icons::Icons,
    pub config: Config,

    /// index of the highlighted row in the focused list
    pub selection: usize,
    pub queue_sel: usize,
    /// Transient modal-input state: the new/rename/bookmark text-entry + buffer,
    /// and the "add to playlist" picker (targets + cursor). Inert when closed.
    pub input: ModalInput,
    /// Library-browser state: the browsed track list + the 3-column cursors.
    pub browser: Browser,
    /// Local library drill-in browse state (flat sections + nav_stack), the local
    /// analogue of `spotify`'s browse model.
    pub local: LocalBrowse,
    /// Saved searches (quick-jump), persisted to `bookmarks.json`.
    pub bookmarks: Vec<crate::library::store::Bookmark>,
    /// Listening history: unix timestamps of plays (`history.json`), oldest→newest.
    pub play_history: Vec<u64>,
    /// Active tracklist sort keys (field, descending). Empty = natural order.
    pub sort: Vec<(SortField, bool)>,
    /// Library load/scan lifecycle: the session pending restore, a pending rescan
    /// request, and the active scan's progress (the state behind library_sync.rs).
    pub scan: ScanState,
    /// xorshift64 PRNG state for Random Album (seeded lazily from the clock).
    rng: u64,
    /// Playback effects/timers runtime: sleep-timer deadline, A-B loop markers,
    /// ReplayGain status (the state behind effects.rs).
    pub fx: PlaybackFx,
    /// Settings overlay/popup UI state (cursor, active group tab, key-rebind capture).
    pub settings: SettingsUi,
    /// Equalizer overlay UI state (open flag, selected control, save-name buffer).
    /// The values themselves live in `config` (`eq_*`); see `crate::app::equalizer`.
    pub eq: EqUi,
    /// The user's saved custom EQ presets (loaded from `eq_presets.toml`).
    pub eq_presets: Vec<crate::config::EqPreset>,
    /// The unified Tag Edit modal: which tab is active + each tab's popup state
    /// (manual editor / online tag search / album-art search). All `None` = closed.
    pub tags: TagModal,
    /// Internet-radio view (search + station list).
    pub radio: Radio,
    /// Radio playback-overlay state (tuned station, live ICY title, paused) —
    /// a separate context; the local `player` is preserved while it streams.
    pub rnow: radio::RadioNow,
    /// Spotify view + connection state (librespot-backed; built in phases).
    pub spotify: Spotify,
    /// Spotify playback-overlay state (now-playing track, position, queue,
    /// session handles, like/shuffle/repeat) — grouped to keep AppState lean.
    pub spov: spotify::SpOverlay,
    /// Per-Layout view state: cursor memory (tracklist + queue selection, restored
    /// on view switch), the big-visualizer mode, and the panel show/dock config.
    pub views: ViewState,
    /// Background worker request channels + per-search sequence counters.
    workers: Workers,
    /// Per-frame clickable regions, populated by the render layer.
    pub hit: std::cell::RefCell<Vec<(Rect, MouseTarget)>>,
    /// The source that last actually produced audio. Resolves the ambiguity when
    /// several sources hold a paused item at once: pausing Spotify and opening the
    /// Lyrics view should still show Spotify, not whatever local track is loaded.
    /// Updated each tick from [`AppState::audible_source`].
    pub(crate) last_source: Option<nowplaying::NpSource>,
    /// Focusable regions drawn this frame, as `(rect, focus)` — populated by the
    /// render layer alongside the mouse hit-map, and read by directional focus
    /// movement (`ctrl+h/j/k/l`). Geometry rather than dock config, so panes
    /// sharing an edge (Queue stacked over Lyrics on the right) and the Library's
    /// three columns are all handled without special cases.
    pub focus_rects: std::cell::RefCell<Vec<(Rect, Focus)>>,
    /// The frame rect from the last render. Interior mutability, same as `hit`:
    /// written by the render layer (which only holds `&AppState`) and read by the
    /// input layer, which needs to know whether the mini card layout is on screen
    /// before it can decide what `h`/`l` mean. Defaults to a comfortably wide rect
    /// so a never-rendered state (tests, startup) reads as the normal layout.
    pub frame: std::cell::Cell<Rect>,
    /// Index in `hit` where the topmost open overlay's regions begin (set by
    /// `mark_overlay_hits` just before overlays draw). While a modal overlay is
    /// open, only regions at/after this index are clickable, so clicks on the view
    /// behind the modal are ignored.
    pub overlay_hits: std::cell::Cell<usize>,
    /// Per-frame pane-resize handles, populated by the render layer (a registry
    /// parallel to `hit`). Empty when mouse support is off. See `pane_resize`.
    pub resize_edges: std::cell::RefCell<Vec<ResizeEdge>>,
    /// The pane edge currently being dragged, if any — set on mouse-down over a
    /// resize handle, cleared on release.
    pub resize_drag: Option<ResizeDrag>,
    /// Local-library search state: the search box (active + query), the cached
    /// result, the prefix-token index, and the `lib_gen` cache-invalidation key.
    pub search: SearchState,
    /// Sticky scroll offsets for the scrollable regions (kept stable so clicking a
    /// visible row selects it without the viewport recentring underneath).
    pub scroll: ScrollOff,
    /// Touchpad two-finger scroll accumulators for grid/carousel navigation, so a
    /// swipe steps cards gently (throttled) and stays on its row until a deliberate
    /// vertical gesture. See `grid_touch_scroll`.
    pub grid_scroll: GridScrollAccum,
    pub playlist_name: String,
    /// Plays recorded since the history file was last flushed (debounce counter).
    plays_since_flush: u32,
    /// The unified read-only Info overlay (Keys / Stats / Health / Track tabs),
    /// `Some` while open. Replaces the old help / stats / error-log / metadata
    /// overlays. See `crate::app::info`.
    pub info: Option<Info>,
    /// Command palette (modal), if open.
    pub palette: Option<Palette>,
    /// Bulk multi-select state: the marked-track set (`x` toggles) + the Vim
    /// visual-mode anchor (`Some` = active; live range is anchor..=selection).
    pub marks: MultiSelect,
    /// Animated bar-visualizer state (levels + peak caps + adaptive gain).
    pub viz: Visualizer,
    /// Inline album-art rendering state: the image-protocol picker, the current
    /// track's full + transport-bar protocols, and the cover's pixel dims.
    pub art: CoverArt,
    /// Cover/photo thumbnails for the library grid + Artist pane, keyed per
    /// album/artist (+ recency tick for LRU eviction). Built from the `artwork`
    /// worker's decoded images on the main thread (interior mut: requested during
    /// render, filled in the event loop).
    pub grid_art: std::cell::RefCell<grid_art::ArtCache>,
    /// Monotonic clock bumped on each `request_art`, stamping a thumbnail's recency
    /// so off-screen entries age out (see `grid_art`'s LRU eviction).
    pub grid_art_clock: std::cell::Cell<u64>,
    /// Online metadata for the current track (artist bio/info + synced lyrics) and
    /// its async-fetch bookkeeping. The request channels live in `workers`.
    pub meta: TrackMeta,
    pub notification: Option<Notification>,
    /// The most recent error message (set by `notify_error` / a connect failure),
    /// kept after its toast fades so it can be copied (`CopyError`).
    pub last_error: Option<String>,
    /// Bounded ring of recent errors, shown in the error-log overlay (`overlays.errors`).
    pub error_log: std::collections::VecDeque<LogEntry>,
    pub tick: u64,

    /// Audio engine handle (NullEngine until a real device attaches in `tui`).
    pub engine: Box<dyn AudioEngine>,
    pub engine_active: bool,
    /// Track currently loaded in the engine (to avoid reloading on resume).
    loaded_track: Option<TrackId>,
    /// Tick at which the last real audio Progress arrived (gates the demo clock).
    last_audio_progress: u64,
    /// Real wall-clock elapsed since the previous tick, set by the event loop.
    /// The extrapolated clocks (Spotify position, the local between-Progress
    /// interpolation) advance by *this*, not an assumed `1/fps`, so they don't
    /// drift when the frame cadence wanders. Defaults to one frame at 60 fps so
    /// tests that pump `Action::Tick` synchronously still advance deterministically.
    frame_dt: Duration,

    /// OS "Now Playing" local-cover memo: the track its cache file was extracted
    /// for + the resulting `file://` URL (`None` = that track had no embedded art).
    /// Avoids re-reading the audio file every frame. See `app/nowplaying.rs`.
    media_cover: Option<(TrackId, Option<String>)>,
    /// Ping-pong slot for the cover cache file, flipped on each track change so
    /// consecutive tracks get distinct URLs (defeats desktop art-URL caching).
    media_cover_slot: bool,
}

impl AppState {
    /// Attach the terminal image-protocol picker (called from `tui`).
    pub fn set_picker(&mut self, picker: ratatui_image::picker::Picker) {
        self.art.picker = Some(picker);
        // Must precede `reload_cover`: the underlay is baked in when a protocol is
        // built, so setting it afterwards would leave the first cover unflattened.
        self.sync_art_background();
        self.reload_cover(); // show the current track's cover immediately
    }

    /// Re-derive the dynamic accent from the current cover (after a theme change).
    /// No-op unless dynamic accent is on. Decodes the cover.
    fn apply_accent(&mut self) {
        if !self.config.dynamic_accent {
            return;
        }
        if let Some(path) = self.current_track().map(|t| t.path.clone())
            && let Some(img) = crate::cover::load_cover(&path)
        {
            self.theme.set_accent(crate::cover::dominant_color(&img));
        }
    }

    /// Clear last frame's regions (called at the start of each render).
    pub fn clear_hits(&self) {
        self.hit.borrow_mut().clear();
        self.overlay_hits.set(0);
        self.resize_edges.borrow_mut().clear();
        self.focus_rects.borrow_mut().clear();
    }

    /// Record a focusable region drawn this frame, for directional focus movement.
    /// Called by the render layer; a region drawn more than once (or not at all)
    /// simply changes what `ctrl+<dir>` can reach, never correctness elsewhere.
    pub fn register_focus(&self, rect: Rect, focus: Focus) {
        if rect.width > 0 && rect.height > 0 {
            self.focus_rects.borrow_mut().push((rect, focus));
        }
    }

    /// Mark the boundary between the base view's click regions and an overlay's,
    /// called right before overlays draw. Everything registered after this is an
    /// overlay control; see [`Self::overlay_hits`].
    pub fn mark_overlay_hits(&self) {
        self.overlay_hits.set(self.hit.borrow().len());
    }

    /// Whether a modal overlay owns input right now. Gates both global one-key
    /// shortcuts and base-view mouse clicks so only the overlay reacts.
    pub fn modal_open(&self) -> bool {
        self.tags_open()
            || self.palette.is_some()
            || self.settings.overlay
            || self.settings.popup.is_some()
            || self.info.is_some()
            || !self.input.add_targets.is_empty()
            || self.input.naming.is_some()
            || self.input.confirm_delete.is_some()
            || self.spotify.pl_modal.is_some()
            || self.spotify.pl_confirm_delete.is_some()
            || self.settings.rebinding.is_some()
    }

    /// A new track: drop any manual lyrics scroll so the pane re-anchors and resumes
    /// auto-follow. Called by both lyrics loaders (local + Spotify).
    pub(crate) fn reset_lyrics_scroll(&self) {
        self.scroll.lyrics.set(0);
        self.scroll.lyrics_manual.set(false);
    }

    /// Resolve lyrics for the current track: sidecar/embedded → cache → online.
    fn load_lyrics(&mut self) {
        self.reset_lyrics_scroll();
        self.meta.lyrics_pending = false;
        let Some(t) = self.current_track() else {
            self.meta.lyrics = None;
            self.meta.lyrics_for = None;
            return;
        };
        let (path, artist, title, album, dur) = (
            t.path.clone(),
            t.artist.to_string(),
            t.title.clone(),
            t.album.to_string(),
            t.duration().as_secs(),
        );
        let key = crate::lyrics::cache_key(&artist, &title);
        self.meta.lyrics_for = Some(key.clone());

        // local sidecar .lrc / embedded tag, else a cached online lookup
        if let Some(l) = crate::lyrics::Lyrics::load_for(&path)
            .or_else(|| crate::lyrics::Lyrics::load_cached(&self.config.dir, &key))
        {
            self.meta.lyrics = Some(l);
            self.maybe_request_translation();
            return;
        }
        // request an online lookup
        self.meta.lyrics = None;
        if let Some(tx) = &self.workers.lyrics {
            self.meta.lyrics_pending = true;
            let _ = tx.send(crate::lyricsfetch::LyricsRequest {
                artist,
                title,
                album,
                duration_secs: dur,
                translate_to: self.config.lyrics_translate_to.clone(),
                key,
            });
        }
    }

    /// The query string + caret of whichever tag/cover field is being edited.
    fn active_query(&mut self) -> Option<(&mut String, &mut usize)> {
        if let Some(ts) = self.tags.search.as_mut().filter(|t| t.editing) {
            Some((&mut ts.query, &mut ts.qcaret))
        } else if let Some(cs) = self.tags.cover.as_mut().filter(|c| c.editing) {
            Some((&mut cs.query, &mut cs.qcaret))
        } else {
            None
        }
    }

    /// Attach a real audio engine (called from `tui` once a device is found).
    pub fn attach_engine(&mut self, engine: Box<dyn AudioEngine>) {
        self.engine = engine;
        self.engine_active = true;
        self.engine
            .send(AudioCommand::SetVolume(self.player.volume));
        self.engine.send(AudioCommand::SetSpeed(self.player.speed));
        self.apply_eq(); // seed the engine with the persisted equalizer curve
    }

    /// Visualizer mode for the current view (per-layout; 0 by default).
    pub fn viz_mode(&self) -> u8 {
        self.views.viz_modes.get(&self.layout).copied().unwrap_or(0)
    }

    /// Show a browsed album/playlist in the centre list (without playing it).
    fn browse(&mut self, title: String, ids: Vec<TrackId>) {
        self.browser.title = title;
        self.browser.list = self.sort_ids(ids);
        self.selection = 0;
        self.search.active = false;
        self.search.query.clear();
        self.focus = Focus::Main;
    }

    fn activate_selection(&mut self) {
        let ids = self.display_ids();
        if let Some(&id) = ids.get(self.selection) {
            // activating from a filtered/search list builds the queue from it
            self.player.queue.items = ids;
            self.player.queue.position = self.selection;
            self.player.current = Some(id);
            self.search.active = false;
            self.play_current();
        }
    }

    /// Max stations materialised into the visible result list at once. The table
    /// only renders a window, and nobody scrolls 50k rows; the true match count
    /// is shown separately.
    const LOCAL_RESULT_CAP: usize = 1500;

    /// Whether the UI changes on its own over time (so it must redraw on a timer
    /// rather than only on input). Everything else lets the loop idle.
    pub fn is_animating(&self) -> bool {
        self.player.status == Status::Playing
            || (self.rnow.now_station.is_some() && !self.rnow.radio_paused) // radio streaming
            || (self.spov.now_spotify.is_some() && !self.spov.spotify_paused) // spotify streaming
            || self.notification.is_some()
            || self.fx.sleep_until.is_some()
            || self.radio_busy() // tighten the loop so radio results land promptly
            || self.spotify_busy() // …and Spotify auth/resume
    }

    /// Record the real wall-clock time since the previous tick (from the event
    /// loop), used to advance the extrapolated position clocks. Clamped to 1s so a
    /// stall or a suspended process can't fling the clock forward by a huge jump.
    pub fn set_frame_dt(&mut self, dt: Duration) {
        self.frame_dt = dt.min(Duration::from_secs(1));
    }

    /// Flag that the UI needs a redraw on the next loop iteration.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Consume the dirty flag (returns whether a redraw is needed).
    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    /// Reference track for the "play current album/artist" scope actions: the
    /// now-playing track if any, else the track under the tracklist cursor.
    fn scope_ref(&self) -> Option<TrackId> {
        self.player
            .current
            .or_else(|| self.display_ids().get(self.selection).copied())
    }

    fn notify(&mut self, text: String) {
        self.notification = Some(Notification {
            text,
            ttl_ticks: 180,
        });
    }

    /// Record an error: append it to the in-app log ring (shown in the error-log
    /// overlay) + the on-disk `errors.log` (survives restarts/crashes), and stash it
    /// as `last_error` for copying. Does NOT show a toast — use `notify_error` for
    /// that. Every error surface (toasts, connect failures, …) funnels through here.
    pub(crate) fn log_error(&mut self, msg: String) {
        let ts = crate::datetime::now_unix();
        // best-effort on-disk append so errors outlive the session
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.config.dir.join("errors.log"))
        {
            use std::io::Write;
            let _ = writeln!(f, "{ts}\t{msg}");
        }
        self.error_log.push_back(LogEntry {
            ts,
            msg: msg.clone(),
        });
        while self.error_log.len() > ERROR_LOG_CAP {
            self.error_log.pop_front();
        }
        self.last_error = Some(msg);
    }

    /// Like `notify`, but for errors: log it (see `log_error`) and show a toast that
    /// lingers ~25s (fps-independent) so it can be read, copied (`CopyError`), or
    /// reviewed later in the error-log overlay.
    pub(crate) fn notify_error(&mut self, text: String) {
        self.log_error(text.clone());
        self.notification = Some(Notification {
            text,
            ttl_ticks: (self.config.fps as u16).saturating_mul(25).max(180),
        });
    }

    /// Copy the last error message to the system clipboard via the OSC 52 terminal
    /// escape (no extra dependency; works over SSH + in modern terminals).
    pub(crate) fn copy_last_error(&mut self) {
        match self.last_error.clone() {
            Some(err) => {
                use base64::Engine;
                use std::io::Write;
                let b64 = base64::engine::general_purpose::STANDARD.encode(&err);
                let mut out = std::io::stdout();
                let _ = write!(out, "\x1b]52;c;{b64}\x07");
                let _ = out.flush();
                self.notify("Error copied to clipboard".into());
            }
            None => self.notify("No error to copy".into()),
        }
    }

    /// Keybinding rows matching the help filter (substring over key + action).
    pub fn help_matches(&self) -> Vec<(&'static str, &'static str)> {
        let q = self
            .info
            .as_ref()
            .map(|i| i.keys_query.trim().to_lowercase())
            .unwrap_or_default();
        let layout = self.layout;
        // show only what applies to the current view (Global rows + this view's),
        // then narrow by the search query
        keybindings_help()
            .into_iter()
            .filter(|(_, _, scope)| scope.shows_in(layout))
            .filter(|(k, d, _)| {
                q.is_empty() || k.to_lowercase().contains(&q) || d.to_lowercase().contains(&q)
            })
            .map(|(k, d, _)| (k, d))
            .collect()
    }

    /// Currently-playing track, resolved against the library.
    pub fn current_track(&self) -> Option<&Track> {
        self.player.current.and_then(|id| self.library.track(id))
    }
}

#[cfg(test)]
mod tag_editor_tests;
