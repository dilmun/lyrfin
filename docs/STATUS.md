# Implementation status

An honest map of what works today, what's partial, and the known limitations.
`cargo build`, `cargo test`, `cargo clippy -D warnings` and `cargo fmt --check`
all pass across the CI matrix (Linux/macOS/Windows).

## Working

| Area | State | Notes |
|------|-------|-------|
| Terminal UI & event loop | ✅ | ratatui/crossterm, 7 layouts, Elm-style `Event → Action → update → render`. |
| Config / themes / keybindings | ✅ | TOML in `~/.config/lyrfin/`; 5 built-in + 8 bundled themes + custom; data-driven keymap. |
| Library scan / search | ✅ | Off-thread incremental scanner, lofty tags, binary cache, nucleo fuzzy search, Miller-column browser. |
| Audio engine | ✅ | symphonia decode → cpal output; play/pause/seek/volume/mute; queue; repeat/shuffle; gapless; speed; crossfade; ReplayGain. |
| Now-playing / queue / ratings | ✅ | Favorites, 0–5★ ratings, history, queue reorder/remove, snapshot-tested. |
| Visualizer | ✅ | rustfft spectrum from live PCM, multiple modes, peak caps, waterfall. |
| Lyrics | ✅ | Synced `.lrc` (sidecar/embedded) + online (LRCLIB/NetEase/JioSaavn), karaoke/teleprompter, translation. |
| Album art | ✅ | Inline images (ratatui-image: Kitty/sixel/iTerm2) with half-block fallback; embedded + online cover search; artist photos; cover-art grid. |
| Tag editor | ✅ | Writes tags via lofty (single track or whole album); filename ↔ tags patterns; find & replace; online auto-tag; cover embed. |
| Spotify | ✅ | OAuth PKCE login; librespot playback; browse/search/library/playlists/artist pages. Requires Premium for playback. |
| Internet radio | ✅ | Radio Browser directory; search, country/genre filters, sort, favorites. |
| Podcasts | ✅ | Episode → public RSS feed → open MP3 stream. |
| Mouse & responsive UI | ✅ | Clicks/scroll/drag-resize; movable/dockable panes; collapse on narrow terminals. |
| OS "Now Playing" | ✅ | macOS Control Center + lock screen + media keys / AirPods; Linux MPRIS. Reports track/art/position and accepts play/pause/next/prev/seek (`souvlaki`); toggle with `os_media_controls`. |
| CLI & session restore | ✅ | `lyrfin [PATHS] --theme --snapshot --size`; last view/queue/panel layout restored. |
| Cross-platform CI + release | ✅ | GitHub Actions matrix + tag-triggered binary release + man page. |

## Known limitations / honest caveats

- **Spotify playback needs Premium** (a librespot requirement); browsing works on
  any account. The default Web-API client ID is shared and can hit rate limits —
  set your own for reliability. See [`SPOTIFY.md`](SPOTIFY.md).
- **Podcasts** only resolve for shows syndicated to a public RSS feed; Spotify
  can't grant podcast decryption keys to third-party clients, so Spotify-exclusive
  episodes won't play.
- **Playable formats** are the symphonia-enabled set (FLAC, MP3, AAC/M4A, OGG
  Vorbis, WAV/PCM); other formats' *tags* may be readable but they won't decode.
- **Audio polish needs real-hardware listening checks** — gapless, pitch-preserved
  speed, and crossfade are implemented and adjustable (crossfade has a Settings
  slider + presets), but want verification on real audio devices.
- **Inline album art** depends on terminal support (Kitty/sixel/iTerm2); elsewhere
  it renders as half-blocks.
- **OS "Now Playing"** — verified on macOS **and** Linux. macOS: the Control Center
  tile (title/artist/album, artwork, transport) and media keys / AirPods all work
  from a bare terminal launch; a future background `.app` bundle would only swap the
  generic source-app icon badge for a proper lyrfin icon (cosmetic). Linux: lyrfin
  registers as an MPRIS player and `playerctl` reads its metadata and drives
  play/pause + next (verified headless under a D-Bus session bus). Windows (SMTC)
  isn't wired yet — it needs a hidden message-pump window a console app lacks.
- **Terminal testing** is currently done **only on [Ghostty](https://ghostty.org)**.
  lyrfin targets widely-supported standards so it should work on other modern
  terminals, but they aren't officially verified yet — broader terminal support is
  on the [roadmap](ROADMAP.md).

## Verify locally

```sh
cargo run                       # launch the TUI
cargo run -- ~/Music            # scan a real folder
cargo run -- --snapshot         # headless render of every layout
cargo test                      # snapshot + reducer tests
```
