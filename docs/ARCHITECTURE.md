# lyrfin — Architecture

## Goals

- **Keyboard-first, responsive, low-overhead.** UI never blocks on audio or disk.
- **Modular & testable.** Core logic is a pure function of state; no terminal or
  audio backend leaks into it.
- **Cross-platform** (Linux, macOS, Windows) via crossterm + cpal.

## Core pattern — unidirectional data flow (The Elm Architecture)

```
        ┌─────────── events ───────────┐
        │                              ▼
   inputs/audio/library  ──►  keymap/dispatch  ──►  Action
                                                      │
                                                      ▼
                                           AppState::update(action)     (the ONLY mutator)
                                                      │
                                                      ▼
                                           ui::render(&AppState)        (pure draw)
                                                      │
                                                      ▼
                                       commands ──► audio / library workers
```

- **`event.rs`** — backend-agnostic `Event` (key/mouse/tick/audio/library). The
  terminal backend (crossterm) is adapted onto these so the core never depends
  on it directly.
- **`action.rs`** — `Action`, the exhaustive list of intents. Keys map to
  actions through the configurable keymap.
- **`app.rs`** — `AppState` is the single source of truth; `update` is the sole
  place state changes. Pure `(state, action) -> state` ⇒ trivially unit-testable.
- **`ui/`** — pure render. Reads state, draws widgets, returns nothing.

This separation is what keeps the app fast: the render + update loop only
touches in-memory state, while slow work (decoding, scanning) happens on worker
threads that communicate via channels.

## Threading model

Long-lived threads, joined by channels (`crossbeam-channel`):

| Thread        | Owns                                   | Talks via |
|---------------|----------------------------------------|-----------|
| **UI/main**   | `AppState`, terminal, event loop       | receives `Event`s, sends `*Command`s |
| **Audio**     | decoder (symphonia) + output (cpal)    | `AudioCommand` in / `AudioEvent` out |
| **Library**   | scanner + SQLite store                 | scan requests in / `LibraryEvent` out |
| **Artwork**   | off-thread cover decode / online fetch | `ArtRequest` in / `ArtResult` out |

The audio thread also pushes PCM into a lock-free ring buffer that the
visualizer FFT reads, so analysis never contends with playback. Further
on-demand worker threads (Spotify Web API / librespot session, lyrics, online
tagging) follow the same `(Sender<Request>, Receiver<Result>)` + drain-in-the-loop
pattern, keeping all network/decode work off the UI thread.

**OS "Now Playing"** (`media/`, via `souvlaki`) is the one integration that is
*not* a worker thread: macOS delivers media-key / Control Center callbacks on the
**main thread's Cocoa run loop**, which the crossterm poll loop never runs, so the
`MediaBridge` is owned by the event loop (not `AppState`) and `pump()`s that run
loop non-blocking each iteration. It stays platform-neutral to the core: the app
exposes a pure `now_playing_snapshot()` (published on change) and
`on_media_command()` (routed to the same per-source transport helpers the on-screen
buttons use), so no `souvlaki`/objc type ever reaches `AppState`. macOS +
Linux (MPRIS) are wired; Windows (SMTC) is a compiled no-op.

## Modules & responsibilities

- **`core/model.rs`** — `Track`, `Album`, `Artist`, `Playlist`, typed ids,
  `AudioInfo`. Pure data.
- **`core/player.rs`** — `PlayerState` (status, queue, volume, speed, repeat,
  shuffle, spectrum). Logical *intent*; the audio thread mirrors it.
- **`audio/`** — `AudioEngine` trait + `AudioCommand`/`AudioEvent`. Implementations:
  `NullEngine` (tests), `CpalEngine` (M4). `visualizer.rs` does FFT → bands.
- **`library/`** — `scanner.rs` (walkdir + lofty tags), `mod.rs` in-memory index
  + query helpers, `search.rs` (nucleo fuzzy), SQLite persistence (M3).
- **`config.rs`** — `Config`, `Theme` selection, `Keymap`. TOML via serde (M2).
- **`ui/theme.rs`** — palette + 3-stop accent gradient sampled per cell.
- **`ui/layout.rs`** — which regions are visible per `Layout` + responsive
  breakpoints; resolves to ratatui `Rect`s in M1.
- **`ui/components.rs`** — reusable widgets (tracklist table, sidebar, queue,
  transport bar, visualizer, lyrics, status bar) + block-char meter helpers.
- **`ui/views.rs`** — compose components into the 8 layouts.

## Layouts ↔ mockups

`app::Layout` variants map 1:1 to `design/mockups/`. `ui::render` dispatches on
the active layout; users switch instantly with number keys without interrupting
playback (only `AppState.layout` changes; audio thread is untouched).

## Source views (the standardized shell)

Every "source view" — the local library (Dashboard), Spotify, Radio, and any
future source (e.g. Apple Music) — presents the same chrome: a sidebar, an
inline search row, a main list, movable Queue / Artist / Lyrics panes, a
now-playing bar, and a status bar. Rather than reimplement that per source, the
chrome is a set of **shared renderers fed by per-source adapters** — sources
differ only in the *content* and a few navigation hooks, not the layout. (There
is deliberately no `SourceView` trait: a source is a code path over the single
`AppState`, not an object, so a trait would just wrap `&mut AppState` functions
in ceremony — the dispatch is a plain `match self.layout` instead.)

**Shared chrome** (`ui/components/`):

- `shell::browser_shell` — the docked-panes + `[ sidebar | main ]` frame. It owns
  the borders, the responsive split (percentage-derived sidebar), and the
  collapse-to-single-pane below `COLLAPSE_WIDTH` (~80 cols). Panes are freely
  movable (the 4-edge `Dock`) with a vertical/horizontal stacking setting.
- `queue::queue_pane` draws the played / now-playing / upcoming list over a
  uniform `QueueRow`; `artist::{artist_panel, spotify_artist_panel}` share
  `artist_scroll_region` + `bio_lines`; `now_bar::playback_bar` draws the
  transport from a source-agnostic `NowPlaying` snapshot (`::local` / `::spotify`);
  `table::columns_table` is the responsive columnar engine; `lyrics_panel` +
  `status_bar` are shared as-is.
- `search_bar::search_bar` is the one inline search row every source draws over
  its list while searching (focus bar · magnifier/spinner · caret-aware query ·
  placeholder · right-aligned scope label + result count). Each view fills a
  `SearchBar` from its own query state; the query no longer lives in the pane
  title. Local, Spotify, and Radio all route their typing through the same
  `keymap::text_capture`, so the box behaves identically everywhere.

**Unified navigation** — one focus model for every view:

- `app::Focus { Sidebar, Main, Pane(Panel), Search }` on the single `app.focus`
  field. `layout::focus_order` is the per-view Tab ring (Sidebar + Main + each
  shown movable pane); `cycle_focus` / `set_focus` drive Tab/BackTab; `set_layout`
  calls `clamp_focus` so focus always lands on a region the view exposes.
- `navigation::{move_selection, activate}` are single dispatchers that branch by
  `layout` then `focus`. In the keymap, `keymap::text_capture` is the shared
  search/filter typing handler; browse-mode list keys (j/k/g/G/arrows/page) fall
  through to the global table, so each per-source handler declares only its own
  keys.

**Adding a source** is then mechanical: add a `Layout` variant + its state, write
a render fn composing `browser_shell` with the shared panes (supplying rows from
the new source's state), add a `focus_order` arm, add `move_selection` /
`activate` arms, and a `keymap` handler that reuses `text_capture` for its search
box. No new shell, focus enum, or transport bar.

## Cover-art grid

The Albums/Artists sections (and the drilled-in artist page's ALBUMS region, and
Spotify's Albums/Artists) render as a grid of cover thumbnails. It follows the
performance rule — artwork is decoded/fetched **off the UI thread** and only ever
for on-screen cards.

**Artwork worker** (`artwork.rs`) — a long-lived thread, `spawn() -> (Sender<ArtRequest>,
Receiver<ArtResult>)`, drained in the event loop into `on_art_result`. It decodes
+ thumbnails (and optionally `circle_crop`s) a `DynamicImage` off-thread; the main
thread builds the inline-image protocol (the `ratatui_image` picker lives there).
A request is keyed by `ArtKey { Album(id) | Artist(id) | ArtistPhoto(id) | Remote(hash) }`
(`Copy`) and sourced by `ArtSource { Embedded(path) | Artist{name,fallback} | Url(s) }`:
local albums decode an embedded cover, local artists fetch an online (Deezer)
photo disk-cached under `cache/artists/`, Spotify covers download by image URL
(`ArtKey::remote(url)` hashes it). The now-playing artist's **pane** photo uses a
distinct `ArtistPhoto` key from the grid card's `Artist` key so the two render
sizes never thrash one `StatefulProtocol`.

**Cache** — `app.grid_art: RefCell<HashMap<ArtKey, (tick, ArtThumb)>>`
(`ArtThumb = Pending | Ready(protocol) | Missing`). `request_art` (called every
frame for each visible card) coalesces by key and bumps the recency `tick` from
`grid_art_clock`; inserting past `GRID_ART_CAP` evicts the least-recently-used
(off-screen) entry, so memory stays bounded for huge libraries.

**Render** is source-agnostic (`ui/components/grid.rs`): `render_grid(GridData)`
lays out cells (`grid_cells`), and each card is a `GridCard { name, subtitle, art }`
built on demand for visible cells only — local cards from `LocalItem`, Spotify
cards from `api::Item`. `render_thumb` draws the placeholder + photo into one
centred **square-in-pixels** rect and fills it (`cover::render_proto_filled`, the
same Scale-to-fill path the big cover and artist pane use), so the image always
covers the placeholder for both circle and rounded shapes. Card size is a fixed
`config.grid_card_size` target (so covers don't resize as a side pane is dragged —
only the column count does); the block is centred in the pane on both axes;
`card_h` derives the cover height from the real cell aspect (`image_font`).

**State / nav** — `LocalBrowse.grid` + `Spotify.grid` (per-section, persisted),
`local_grid_active` / `spotify_grid_active` / `artist_grid_from` gate where a grid
shows; `#` toggles it (`ToggleGridView`), `hjkl` move 2-D (`GridMove` → `grid_step`),
the last-rendered column count is stashed in a `cols: Cell<usize>` written by
render and read by nav. The artist page mixes a POPULAR track **list** above an
ALBUMS **grid**; `grid_move` keeps moves inside the album region but lets a move
off its top row fall back into the track list.

**Track layout (rows vs columns)** — one shared `config.track_columns` toggle
("Track layout" on the `;` popup's Tracklist tab) reshapes *every* track list,
local and Spotify alike: `true` (default) = the responsive **column table**,
`false` = compact one-line **rows** (name · artist · album · year … time).
Each list dispatches on it — `local_tracklist` → `track_table` / `track_rows`,
`spotify_main_body` → `spotify_tracks` / `spotify_track_rows`,
`render_popular_region` → `render_popular_columns` / `render_popular_row`. The
rows form is the shared `compact_track_line` geometry; both forms honor the
per-column show/hide toggles (`row_meta` for rows, `tracklist_cols` /
`spotify_col_specs` for the table). Column **curation is per source**: the local
library offers every column, Spotify only the metadata it carries (#, Artist,
Album, Year, Time) — enforced at render (`spotify_col_specs`) and in the popup's
Columns tab (`supported_col_rows`).

## Persistence

- **Library DB** (SQLite, M3): tracks/albums/artists, ratings, favorites, play
  counts, recently-played — fast cold-start without a full rescan.
- **Config/themes/keybindings** (TOML, M2): `~/.config/lyrfin/`.
- **Session** (optional): last view/layout/queue restored on launch.

## Performance notes

- Render only on real change + a capped tick (config `fps`); diffed by ratatui.
- Decode/scan off-thread; UI thread stays allocation-light per frame.
- Visualizer reads a ring buffer (no locks on the audio path).
- `--release` with thin-LTO for shipping.

## Testing strategy

- Pure `update` reducer ⇒ table-driven tests over `(state, action)`.
- `NullEngine` + fixture library for headless integration tests.
- Snapshot tests of rendered frames via ratatui's `TestBackend` (M5+).
