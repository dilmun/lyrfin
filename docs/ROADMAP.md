# lyrfin roadmap

A living plan. The **milestones M1–M7 are shipped**; what's left is a short
near-term list plus one greenfield milestone (M8). The granular record of what
works today lives in [`STATUS.md`](STATUS.md) and [`CHANGELOG.md`](../CHANGELOG.md);
this file is the forward-looking plan.

## Scope

lyrfin is a **local-first** terminal player that also pulls **online content**:
Spotify (login, playback, browse/search/library/playlists/artist pages,
podcasts), internet radio, podcast RSS, and online lyrics / cover art / tag
metadata. Those online *content* sources are in scope.

Still **out of scope** — presence, remote-control, social, and sync
integrations: Discord rich presence, MPRIS, Last.fm / ListenBrainz scrobbling,
OBS, web/mobile remote, cloud & multi-device sync, AI playlist builder,
marketplaces, and audio fingerprinting. Keep local duplicate / broken-metadata
detection instead of fingerprinting.

---

## Now — near-term

The active list — the road to the next release. Ordered by priority.

- [ ] **Terminal compatibility.** Developed and tested only on
  [**Ghostty**](https://ghostty.org). Verify and tune rendering — truecolor,
  inline-image protocols (Kitty/sixel/iTerm2), Unicode / RTL shaping, mouse — on
  Kitty, WezTerm, iTerm2, Alacritty, foot, and Windows Terminal, and document
  what's supported per terminal.
- [ ] **Audio listening checks.** Gapless, pitch-preserved speed, crossfade, and
  silence-skip are implemented, adjustable, and confirmed by ear on the desktop
  output; the remaining case is a **Bluetooth sink** — output-latency
  compensation for A/V sync and crossfade timing. Closes the last M5 gap.
- [ ] **Multi-select coverage.** v0.2.0 turned marks (`x`) + visual range (`V`)
  into a view-aware selection for bulk favourite / add-to-playlist / edit /
  remove across the local library, radio, and Spotify track lists. Extend it to
  the three surfaces it doesn't reach yet: the Library (Miller) **Tracks**
  column, the **queue** pane, and Spotify **search** track results (the
  mixed-carousel render path).
- [ ] **OS "Now Playing" polish.** The integration shipped and is verified on
  macOS (Control Center) + Linux (MPRIS). Two follow-ups: (1) a background `.app`
  bundle (`Info.plist` `LSUIElement`) so the macOS tile shows a proper lyrfin icon
  instead of the generic source-app badge; (2) Windows SMTC — needs a hidden
  message-pump window a console TUI doesn't own (souvlaki still requires an
  `hwnd`), so a hidden win32 / `winit` window driven off the event loop.

---

## Next — M8: Extensibility (local)

The one milestone not yet started.

- [ ] **Workspace profiles** — named, switchable config/layout/library sets.
- [ ] **Scripting** — Lua or Rhai hooks for user automation.
- [ ] **Custom widgets** — user-defined panes composed from existing state.

---

## Shipped

### Core player
9 layouts + responsive column/panel dropping · true-color + custom TOML themes
(5 built-in, 8 bundled, `auto` matches the terminal palette) · inline album art
+ artist images + cover-art grid · synced/plain lyrics · 6 visualizer modes ·
fuzzy search + live filter · queue + playlists (CRUD, persistent, folders) ·
shuffle/repeat · library cache + incremental indexing + instant startup ·
rating/favorite/play count + listening history · session resume · mouse +
app-wide vim navigation · cross-view multi-select (marks + visual range) for bulk
actions · first-run onboarding.

### Online layer
- **Spotify** — OAuth PKCE login, librespot playback (Premium required),
  browse/search/library/playlists/artist pages, podcast browse hub. Rate-limited
  on the shared default client ID; users can set their own.
- **Internet radio** — Radio Browser directory with a sectioned sidebar
  (Favourites, Recent, Most Played, Trending, Countries, Genres, Playlists):
  multi-field search, country / genre browse, sort, play history, favourites, and
  named station playlists.
- **Podcasts** — episode → public RSS feed → open MP3 stream (Spotify-exclusive
  DRM audio can't play in a third-party client).
- **Online metadata** — lyrics (LRCLIB → NetEase → JioSaavn + machine
  translation), cover art, and auto-tag sources (iTunes/Deezer/MusicBrainz).

### Milestones (M1–M7)
- **M1 — Tag editor.** mp3tag-style modal editor: field-masked lofty writes
  (preserving cover/lyrics/unknown frames), multi-select bulk edit, case/number
  actions, filename ↔ tags patterns, find & replace, online auto-tag, cover embed.
- **M2 — Library power.** Bulk add-to-playlist; discovery lists (All / Recently
  Added / Most Played / Favorites / Forgotten / On This Day); collapsible
  genre & year facets; duplicate & needs-tags detection; Random Album.
- **M3 — Search & control.** Command palette (`:` / Ctrl+P); advanced query
  language (field/numeric/flag filters, AND/OR/negation) that doubles as the
  smart-playlist engine; live keybinding search; named bookmarks / quick-jump.
- **M4 — Queue & playlist.** Key-driven reorder/remove/clear; real play history
  (Previous as a back button); smart shuffle; dynamic (rule-based) smart
  playlists; collapsible playlist folders.
- **M5 — Audio engine** *(effectively done — pending only a Bluetooth-sink
  listening check)*. Sleep timer; A-B loop; ReplayGain/normalization; gapless
  (next-track preload, seamless append); pitch-preserved speed (WSOLA
  time-stretch); crossfade; silence-skip.
- **M6 — Lyrics+.** Word-level karaoke; dual/translated lyrics + machine
  translation; teleprompter mode; precise ~20 Hz sync with output-latency
  compensation; manual sync offset (`,`/`.`); NetEase + JioSaavn providers.
- **M7 — Stats & "wow" modes.** Library/listening stats overlay; timestamped
  history log → streaks + weekday/hour sparklines; dynamic accent from album
  art; unified title-vs-metadata theming; Concert / Focus presentation mode.

---

## Cut / reframed (TUI reality)
- Glassmorphism / shadows / blurred background art — terminals can't composite.
- Particle/shader/rain/ambient visualizers — deferred in favour of beat
  detection + a few strong modes. Performance is first-class.
- Animated transitions — cheap fades only.
- Screen-reader support — not achievable for a TUI; invest instead in
  high-contrast + colorblind-safe palettes + large-friendly layouts.
- Audio fingerprinting — needs a service; keep local duplicate / broken-metadata
  detection instead.
- Extra presentation modes (Vinyl / Time Machine / Cockpit / Mood) — Concert
  covers the "wow" need; the rest are out of scope.
