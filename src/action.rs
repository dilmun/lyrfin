//! `Action` — every discrete intent the app can perform.
//!
//! Input keys, mouse, and worker events are all translated into `Action`s
//! (via the keymap / dispatch layer). `AppState::update` is the *only* place
//! that mutates state in response to an `Action`. This keeps the core logic
//! pure, testable, and decoupled from crossterm/ratatui.

use crate::app::{Focus, Layout, View};
use crate::core::model::{AlbumId, ArtistId, PlaylistId, TrackId};

/// In-field text cursor movement (tag editor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caret {
    Left,
    Right,
    Home,
    End,
}

/// Movement within lists/grids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Motion {
    Up,
    Down,
    Left,
    Right,
    Top,
    Bottom,
    PageUp,
    PageDown,
}

// Several variants are scaffolded commands (Enqueue, PlayAlbum, ToggleMute,
// SeekTo, …) with `update` handling but no key binding yet — kept as the
// intended command surface they'll be wired to.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    // ---- lifecycle ----
    Quit,
    Redraw,
    Tick,
    Noop,

    // ---- navigation / focus ----
    Move(Motion),
    Activate, // enter / play selected
    Back,     // escape / pop
    /// `q`: pop one context layer (overlay / drill / selection), or quit the app
    /// if there's nothing to pop. See `AppState::go_back`.
    QuitOrBack,
    /// Copy the last error message to the clipboard (OSC 52).
    CopyError,
    /// Toggle the error-log / health overlay.
    ToggleErrorLog,
    FocusPane(Focus),
    CyclePane,            // Tab — focus the next active pane
    CyclePaneRev,         // Shift-Tab — focus the previous active pane
    NavDown,              // ctrl-n — move down in whatever list/menu is focused
    NavUp,                // ctrl-p — move up in whatever list/menu is focused
    SwitchView(View),     // jump to a top-level view
    SwitchLayout(Layout), // instant layout swap (1..8)

    // ---- transport ----
    TogglePlay,
    Stop,
    Next,
    Previous,
    Seek(i64),       // relative seconds (+/-)
    SeekTo(f32),     // absolute fraction 0.0..=1.0
    GoLive,          // jump a timeshifted live radio stream to the live edge
    GoStreamStart,   // jump a timeshifted live radio stream to the oldest buffered
    VolumeDelta(i8), // +/- percent
    SetVolume(u8),
    ToggleMute,
    SetSpeed(f32), // 0.5..=2.0
    ToggleShuffle,
    CycleRepeat,

    // ---- queue / playlists ----
    Enqueue(TrackId),
    EnqueueNext(TrackId),
    RemoveFromQueue(usize),
    QueueMove(Motion),  // reorder the selected upcoming track up/down
    QueueRemove,        // drop the selected upcoming track from the queue
    QueueClearUpcoming, // clear everything after the current track
    ClearQueue,
    PlayAlbum(AlbumId),
    PlayArtist(ArtistId),
    PlayPlaylist(PlaylistId),
    PlayCurrentAlbum,  // queue = the current/selected track's album, then play
    PlayCurrentArtist, // queue = the current/selected track's artist, then play
    RandomAlbum,       // pick a random album, browse + play it

    // ---- playlists (the sidebar's Playlists section) ----
    BeginNewPlaylist,
    NewSmartPlaylist, // create a smart (rule-based) playlist from the search
    BeginRenamePlaylist,
    DeletePlaylist,
    RemoveFromPlaylist,   // remove the selected track from the drilled-in playlist
    NameInput(String),    // text-entry buffer for new/rename
    AddCurrentToPlaylist, // add the now-playing track to the selected playlist
    AddToPlaylistPrompt,  // open the "add to playlist" picker for the selected track

    // ---- library / metadata ----
    ToggleFavorite(TrackId),
    Rate(TrackId, u8), // 0..=5 stars
    BeginSearch,
    SearchInput(String),
    ApplyFilter(String),
    // ---- internet radio ----
    OpenRadio,                // switch to the Radio view + load stations
    RadioInput(String),       // set the radio search query (live search)
    RadioActivate,            // Enter: play the station / apply the open picker
    RadioFocusSearch,         // '/' — give the search box focus (edit the query)
    RadioCancel,              // Esc — close picker / leave search / exit the view
    RadioOpenCountry,         // 'c' — open the country filter picker
    RadioOpenGenre,           // 'g' — open the genre/tag filter picker
    RadioPickerInput(String), // type into the open picker's filter box
    RadioPickerStartSearch,   // '/' inside a picker — focus its filter box
    RadioPickerEndSearch,     // Esc/Tab inside a picker — back to list navigation
    RadioToggleFavorites,     // 'f' — switch between results and the saved list
    RadioStar,                // 's' — star/unstar the selected station
    RadioCycleSort,           // 'o' — cycle the result sort order
    RadioStation(i32),        // n/p — tune the next/previous station (change channel)
    RadioRefresh,             // R — force a re-download of the station directory
    // ---- Spotify (librespot) ----
    OpenSpotify,            // switch to the Spotify view (resume cached session if any)
    SpotifyLogin,           // ⏎ on the auth panel — start the browser login flow
    SpotifyLogout,          // disconnect + clear the cached token
    SpotifyToggleSidebar,   // 'b' — jump focus between the sidebar and the list
    SpotifyCycleFocus(i32), // Tab/BackTab — cycle sidebar → list → panes
    SpotifyFocusSearch,     // '/' — focus the search box
    SpotifyInput(String),   // type into the search box (live)
    SpotifyCancel,          // Esc — leave search / leave search results
    SpotifyActivate,        // ⏎ — open/play the selection (built up over phases)
    SpotifyTrack(i32),      // n/p — next/previous track in the Spotify queue
    SpotifyLike,            // 'f' — toggle the now-playing track in Liked Songs
    SpotifyFollow,          // 'F' — follow/unfollow the selected show or artist
    SpotifyWriteConfig,     // 'c' — write the spotify_client_id placeholder to config.toml
    // Spotify playlist management (Web API writes; mirror the local Playlists keys)
    SpotifyAddToPlaylist, // 'a' — open the "add selected/now-playing track to a Spotify playlist" picker
    SpotifyBeginNewPlaylist, // 'n' inside the picker — switch to the new-playlist name prompt
    SpotifyNewPlaylist,   // 'n' in the Playlists section — create a new (empty) Spotify playlist
    SpotifyRenamePlaylist, // 'e'/'r' — rename the selected Spotify playlist
    SpotifyDeletePlaylist, // 'd' — unfollow ("delete") the selected Spotify playlist
    SpotifyRemoveFromPlaylist, // 'd'/'x' — remove the selected track from the open Spotify playlist
    SpotifyNameInput(String), // type into the Spotify playlist create/rename text field
    BookmarkSearch,       // save the current search query as a named bookmark
    RunSearch(String),    // jump to a saved search (set the query, show results)
    RescanLibrary,
    ToggleMark,   // multi-select the current track
    VisualSelect, // toggle Vim-style visual (range) selection
    // tag editor (mp3tag-style write-back)
    BeginTagEdit,
    TagEditBeginEdit, // browse → edit the focused field
    TagEditStopEdit,  // edit → browse
    TagEditMove(Motion),
    TagEditType(String), // set the whole focused field (programmatic)
    TagEditInsert(char), // insert a char at the caret
    TagEditBackspace,    // delete the char before the caret
    TagEditDelete,       // delete the char at the caret
    TagEditCaret(Caret), // move the in-field text cursor
    TagEditCase(u8),     // transform focused field: 0 Title, 1 UPPER, 2 lower
    TagEditClear,        // empty the focused field (writes it, clears <keep>)
    TagRemoveField,      // delete the focused field's frame from all targets now
    TagEditAutoNumber,   // number the target tracks 1..N (+ total)
    TagConvertBegin,     // open filename→tags pattern prompt
    TagRenameBegin,      // open tags→filename pattern prompt
    TagConvertType(String),
    TagConvertApply,
    TagConvertCancel,
    TagReplaceBegin,        // open find-&-replace prompt (focused field)
    TagReplaceType(String), // set the active (find/replace) box buffer
    TagReplaceToggle,       // switch between the find and replace boxes
    TagReplaceApply,
    TagReplaceCancel,
    TagEditSave,
    TagEditAlbumPrompt, // arm the "apply to album" confirmation
    TagEditAlbumCancel, // dismiss the album confirmation
    TagEditSaveAlbum,   // confirmed: write the manual-edit draft to the whole album
    TagEditCancel,
    SettingsRemove,     // delete the selected settings row (e.g. a music directory)
    RebindKey(String),  // bind the pending action to this key label
    RestoreKeybinds,    // reset all keybindings to their defaults
    OpenSettings,       // open the full Settings overlay (command palette only)
    OpenTags,           // open the unified Tag Edit modal (Edit tab)
    OpenCoverSearch,    // open the album-art search popup (command palette only)
    CoverMove(Motion),  // move the candidate selection in the cover popup
    CoverInput(String), // edit the cover popup's search query
    CoverActivate,      // enter: re-search (editing) or embed the selection
    OpenTagSearch,      // open the online tag/metadata search (command palette only)
    TagMove(Motion),    // move the candidate / track selection in the tag popup
    TagInput(String),   // edit the tag popup's search query
    TagActivate,        // enter: re-search (editing) or apply the active mode
    TagApplyAlbum,      // apply album-level fields to every album track
    TagToggleAlbum,     // toggle single ⇄ album compare mode
    TagSource(i32),     // album mode: cycle the matched source (±1)
    TagConfirm,         // confirm the staged tag apply
    CoverConfirm,       // confirm the staged cover embed
    CoverToggleScope,   // cover: embed to whole album ⇄ current song
    QueryInsert(char),  // insert a char at the tag/cover query caret
    QueryBackspace,     // delete before the query caret
    QueryDelete,        // delete at the query caret
    QueryCaret(Caret),  // move the query caret (←/→/Home/End)

    // ---- ui chrome ----
    ToggleLyrics,
    ToggleArtistInfo,
    ToggleQueue,            // global queue side-panel on/off
    ToggleQueueSide,        // cycle the queue dock (left/top/right/bottom)
    MoveArtistPanel,        // cycle the dashboard artist panel dock
    MoveLyricsViz,          // cycle the Lyrics-view visualizer dock
    MoveSidebar,            // cycle the Home sidebar dock
    ResizeFocusedPane(i32), // grow/shrink the focused dock pane's width (+1 / -1)
    ResizePaneHeight(i32),  // grow/shrink the focused pane's cross-axis share ({/})
    MoveFocusedPane,        // cycle the focused dock pane's edge (l→t→r→b)
    ToggleSidebar,          // Home: library/playlists sidebar on/off
    ToggleLyricsViz,
    CycleLyricsFormat, // plain → karaoke → teleprompter
    LyricsOffset(i32), // ms
    CycleVisualizer,
    CycleTheme,
    OpenViewSettings,   // open Settings at the current view's group
    OverlayTab(i32),    // Tab / Shift-Tab: step the active tabbed overlay's tab (±1)
    CycleOverlaySize,   // `f`: step the open overlay's size up (wraps at the top)
    ResetLayout,        // restore this view's panels (visibility/dock/size) to defaults
    FitLayout,          // re-fit pane sizes to default %s (keep shown/dock)
    ToggleGridView,     // '#' — Albums/Artists cover-art grid vs list
    GridMove(i32, i32), // 2-D grid navigation by (dx, dy) cards
    SetTheme(String),
    SetSleepTimer(u32), // minutes; 0 cancels
    CycleSleepTimer,    // off → 15 → 30 → 45 → 60 → off
    AbLoopCycle,        // set A → set B (activate) → clear
    CycleReplayGain,    // ReplayGain: Off → Track → Album
    ToggleHelp,         // Info overlay → Keys tab
    ToggleStats,        // Info overlay → Stats tab
    ToggleTrackInfo,    // Info overlay → Track tab (current track's tags / format)
    HelpInput(String),  // filter text for the Info overlay's Keys tab
    Notify(String),

    // ---- equalizer (self-contained overlay; keys captured while open) ----
    OpenEqualizer,       // toggle the Equalizer overlay
    EqSelect(i32),       // ←/→ : move across the 10 bands (+ the preamp)
    EqAdjust(i32),       // ↑/↓ : nudge the selected control by ±1 dB
    EqTogglePower,       // ⏎/space : turn the EQ on/off
    EqCyclePreset(i32),  // Tab / [ ] : cycle presets and apply live
    EqReset,             // reset every band + the preamp to flat
    EqResetBand,         // 0 : zero the selected control
    EqBeginSave,         // s : begin naming a custom preset from the current curve
    EqNameInput(String), // type into the save-preset name field
    EqSavePreset,        // ⏎ (while naming) : commit the custom preset
    EqDeletePreset,      // del / x : delete the active custom preset

    // ---- command palette ----
    OpenPalette,
    PaletteInput(String),
    PaletteMove(Motion),
    PaletteActivate,
    PalettePrefill(String), // replace the palette query (command template), stay open
    RunCommand(String),     // execute a typed `verb args` command line
}

impl Action {
    /// Whether this action stays live while a modal overlay/popup owns the screen
    /// (per-view settings popup, full Settings, stats / metadata). Limited to the
    /// overlay's OWN keys: navigation/adjust (`Move`/`Seek`), confirm (`Activate`),
    /// close (`Back`/`QuitOrBack`), and the overlay's own toggles
    /// (`OpenViewSettings`/`OpenSettings`/`ToggleStats`) + settings actions. Plus
    /// the force-`Quit` so ctrl-c always kills the app. Every other global command
    /// — one-key shortcuts (`v`, `space`, …) AND view switching (`1`–`7`) — is
    /// suppressed so nothing leaks through the overlay. See `crate::keymap::map`.
    pub fn allowed_in_overlay(&self) -> bool {
        use Action::*;
        matches!(
            self,
            Noop | Redraw
                | Tick
                | Move(_)
                | Seek(_)
                | Activate
                | Back
                | QuitOrBack
                | NavUp
                | NavDown
                | SettingsRemove
                | RestoreKeybinds
                | Quit
                | OpenViewSettings
                | OpenSettings
                | OverlayTab(_)
                | CycleOverlaySize
                | ToggleStats
        )
    }
}
