# Configuration

lyrfin is configured through TOML files under its config directory. Everything has
a sensible default, and most options can also be changed live from the in-app
settings popup (<kbd>;</kbd>) — hand-editing is never required.

## Config directory

| Platform | Location |
|----------|----------|
| Linux / macOS | `$XDG_CONFIG_HOME/lyrfin`, else `~/.config/lyrfin` |
| Windows | `%APPDATA%\lyrfin` |

The main file is `config.toml`, created with defaults on first run. If it fails
to parse, lyrfin surfaces the error and refuses to overwrite it (so a typo can't
wipe your settings).

## `config.toml` reference

### Appearance

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `theme` | string | `"aurora"` | Theme name (built-in or a file in `themes/`). |
| `album_art` | bool | `true` | Show inline cover art. |
| `dynamic_accent` | bool | `false` | Drive the accent gradient from the current album art. |
| `panes_horizontal` | bool | `false` | Stack Queue/Artist/Lyrics panes side-by-side instead of vertically. |
| `grid_circle` | bool | `true` | Render grid cards as circles (vs rounded squares). |
| `grid_card_size` | `small`\|`medium`\|`large` | `medium` | Cover-grid card size. |
| `track_columns` | bool | `true` | Show tracks as a column table (default) instead of compact rows. |
| `icon_set` | string | `"outline"` | Transport icon preset: `outline`, `triangles`, `skip`, `ascii`, `nerd`. |
| `powerline` | bool | `false` | Powerline glyphs (U+E0Bx) for the rounded selection pill. |
| `arabic_shaping` | bool | `true` | Pre-shape Arabic text; disable on terminals that shape natively. |

### Audio & playback

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `volume` | int (0–100) | `72` | Startup volume. |
| `gapless` | bool | `true` | Gapless track transitions. |
| `crossfade_ms` | int | `0` | Crossfade duration in ms (`0` disables). |
| `replaygain` | int (0–2) | `0` | ReplayGain: 0 = off, 1 = track, 2 = album. |
| `replaygain_preamp` | float | `0.0` | Pre-amp in dB (−15.0 … +15.0). |
| `sort_order` | string | `"artist,year,album"` | Default tracklist sort keys. |
| `spotify_bitrate` | int | `160` | Spotify stream bitrate: `96`, `160`, or `320`. |

### Library

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `music_dirs` | list of paths | system audio dir | Library roots to scan. |

### UI & interaction

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `mouse` | bool | `true` | Enable mouse support. |
| `os_media_controls` | bool | `true` | Publish playback to the OS "Now Playing" (macOS Control Center + lock screen + media keys / AirPods, Linux MPRIS) and accept its transport commands. No effect on Windows yet. |
| `fps` | int (10–144) | `60` | UI refresh cap. |
| `overlay_size` | int (0–3) | `0` | Settings-overlay size step. |
| `touchpad_speed` | `slow`\|`normal`\|`fast` | `normal` | Two-finger scroll speed. |
| `grid_scroll_lock` | bool | `true` | Lock touchpad horizontal scroll to the current row. |
| `player_viz` | bool | `true` | Show the mini visualizer in the playback bar. |
| `player_viz_mode` | int | `0` | Playback-bar visualizer mode index. |
| `radio_refresh_days` | int | `7` | Auto-refresh the radio directory after N days (`0` = manual). |
| `reduced_motion` | bool | `false` | Disable idle visualizer animation. |

### `[visualizer]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `peak_caps` | bool | `true` | Show falling peak caps above the bars. |
| `gravity` | float (0–1) | `0.004` | Peak-cap fall speed. |
| `peak_hang` | int | `10` | Frames a cap hangs at its peak before falling. |

### `[layout]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `sidebar_width` | int (16–50) | `26` | Library sidebar width. |
| `artist_width` | int (20–70) | `38` | Artist panel width. |

### `[columns]`

Booleans that toggle each column of the track table (`track_columns = true`):
`index`, `artist`, `album_artist`, `album`, `year`, `genre`, `composer`,
`format`, `bitrate`, `rating`, `time`, `plays`, `comment`.

### `[lyrics]`

| Key | Type | Default | Description |
|-----|------|---------|-------------|
| `align` | int (0–2) | `0` | 0 = center, 1 = left, 2 = right. |
| `gap` | int (0–3) | `0` | Blank lines between lyric lines. |
| `gradient` | bool | `true` | Rainbow gradient on the active line. |
| `color` | int (0–4) | `0` | Active-line color when the gradient is off. |
| `karaoke` | bool | `true` | Word-by-word wipe highlight. |
| `dual` | bool | `true` | Show bilingual translations. |
| `teleprompter` | bool | `false` | Show only the current (+ next) line. |
| `offset` | int (ms) | `0` | Manual sync nudge (−5000 … +5000). |
| `translate_to` | string | `"en"` | Machine-translate lyrics to this language code. |

### `[icons]` (optional)

Override individual glyphs (`play`, `pause`, `prev`, `next`, `shuffle`,
`repeat`, `repeat_one`, `seek_back`, `seek_fwd`, `volume`, `volume_mute`) with
your own strings.

### Symbols showing as boxes?

A terminal offers **no way to ask which glyphs its font actually has** — a
missing glyph occupies exactly the same width as a present one, so lyrfin cannot
detect this and pick for you. Instead it defaults to glyphs that need no special
font, and lets you *look* at the alternatives:

<kbd>;</kbd> → **Transport icons** renders every preset inline, so you pick the
row that displays correctly:

| Preset | Needs | Glyphs |
|--------|-------|--------|
| `outline` (default) | plain Unicode | ▶ ⏸ ⏮ ⏭ ⇄ ↻ |
| `triangles` | plain Unicode | ▶ ❚❚ ▏◀ ▶▏ ⤨ ↻ |
| `skip` | plain Unicode | ▶ ❚❚ ↞ ↠ ⤭ ⟳ |
| `ascii` | nothing — pure ASCII | `>` `\|\|` `\|<` `>\|` `><` `()` |
| `nerd` | a [Nerd Font](https://www.nerdfonts.com) | Font Awesome media icons |

If even `outline` shows boxes, use `ascii` — it cannot fail. `powerline` is a
separate toggle because Powerline (U+E0Bx) and Nerd Font (U+F0xx) coverage are
independent: many fonts have one without the other.

> Note this is a *font* question, not only a terminal one. iTerm2, for example,
> won't substitute another installed font for these codepoints, so a Nerd Font
> that works in Ghostty can still render as boxes there unless it's the font the
> profile actually uses.

### `[spotify]` / client ID

`spotify_client_id` may be set here, but lyrfin also stores it in a dedicated
`spotify_client_id` file so a `config.toml` parse error can never wipe it. See
[`SPOTIFY.md`](SPOTIFY.md).

## Inline album art

lyrfin detects the terminal's inline-image protocol at startup and picks the best
one available (Kitty, iTerm2, sixel, or a half-block fallback). `album_art`
controls whether covers are shown at all.

Detection trusts what the terminal reports, which is occasionally wrong — a
terminal can advertise a protocol it only partly implements, and the usual
symptom is that art silently renders as *nothing* while the rest of the UI looks
fine. Override it with an environment variable:

```sh
LYRFIN_IMAGE_PROTOCOL=iterm2 lyrfin     # kitty · iterm2 · sixel · halfblocks
```

Unset (or `auto`, or an unrecognised name) means "trust detection". The override
only changes the protocol — the terminal's queried font size, which sizes every
image, is kept either way.

> **iTerm2** advertises Kitty graphics support but doesn't implement the
> unicode-placeholder placement lyrfin renders with, so lyrfin selects iTerm2's own
> protocol there automatically. No configuration needed.

## Running under tmux

lyrfin renders the same inside tmux as it does natively — same protocol, same
image scaling — but tmux needs **two settings** first. Add them to `~/.tmux.conf`
and restart the tmux server (`tmux kill-server`):

```tmux
# Inline album art. tmux hides escape sequences it doesn't recognise, so images
# (and lyrfin's terminal queries) never reach the terminal without this.
# It is OFF by default — nothing works until you set it.
set -g allow-passthrough on

# 24-bit colour. Without this tmux downgrades to 256 colours and every theme
# gradient banks; match the value to your `default-terminal`.
set -g default-terminal "tmux-256color"
set -ag terminal-overrides ",tmux-256color:Tc"
```

| Setting | Default | Why lyrfin needs it |
|---------|---------|---------------------|
| `allow-passthrough` | **off** | inline images, and the terminal-identity query |
| `terminal-overrides … :Tc` | not set | true colour; themes are 24-bit RGB throughout |

Without `allow-passthrough`, art degrades to half-blocks at best — and because
lyrfin also identifies your terminal through that channel, detection falls back to
guessing from environment variables, which tmux is unable to answer correctly
(the tmux server hands every pane the environment of whichever terminal *started*
it, so attaching the same session from a different terminal reports the old one).

> Ghostty, iTerm2 and WezTerm all report themselves correctly through tmux once
> passthrough is on, so a session started in one and attached from another still
> picks the right image protocol.

## Themes

### Auto (match the terminal)

`auto` builds a theme from your terminal's own colours — it asks the terminal for
its foreground, background, and 16 ANSI colours at startup (an OSC palette query)
and maps them onto lyrfin's roles, so lyrfin adopts whatever colorscheme your terminal
runs. Because the queried values are real RGB, gradients and blends still work.
Needs a terminal that answers OSC 4/10/11 (Ghostty does); if it doesn't reply in
time, `auto` falls back to the default theme. Set `theme = "auto"` or cycle to it.

### Built-in

`aurora` (default), `cyberpunk`, `glacier`, `monolith`, `highcontrast`.

### Bundled community themes

Seeded into `~/.config/lyrfin/themes/` on first run: `tokyonight-night`,
`tokyonight-storm`, `tokyonight-moon`, `tokyonight-day`, `foxnight`,
`foxnight-dusk`, `foxnight-tera`, `foxnight-dawn`, `foxnight-day`,
`foxnight-nord`, `foxnight-carbon`.

### Custom theme format

Drop a `~/.config/lyrfin/themes/<name>.toml` file; it overrides a built-in of the
same name. Any omitted field falls back to the Aurora default. `accent` must
have exactly three stops (light → mid → deep) and is sampled per-cell across
progress bars and the visualizer.

```toml
name          = "my-theme"                       # optional; defaults to the filename
bg            = "#0A0C14"
panel         = "#0E111B"
border        = "#222838"
border_focus  = "#33405E"
text          = "#E7EAF4"
text_dim      = "#9AA3B8"
text_faint    = "#646D83"
selection     = "#182032"
accent        = ["#48E6D6", "#7E8CF7", "#F47CC0"]  # [light, mid, deep]
warning       = "#F7C45A"
good          = "#54DDA0"

# Optional semantic role overrides (empty = computed default):
title         = "#9AA3B8"
column        = "#9AA3B8"
column_title  = "#C5D0E8"
now_playing   = "#48E6D6"
selection_fg  = "#E7EAF4"
toggle_on     = "#7E8CF7"
toggle_off    = "#646D83"
slider        = "#48E6D6"
special       = "#7E8CF7"
marked        = "#F47CC0"

# Optional per-column colour overrides:
[columns]
year = "#888888"
```

## Files & data paths

Everything lyrfin persists lives under the config directory:

| File / dir | Purpose |
|------------|---------|
| `config.toml` | Main settings. |
| `keybindings.toml` | Custom keybindings. |
| `themes/` | Custom & bundled theme files. |
| `library.bin` | Binary cache of the scanned catalogue (fast startup). |
| `store.json` | Per-track user data: ratings, favorites, play counts, last-played. |
| `playlists.json` | User playlists and smart playlists. |
| `bookmarks.json` | Saved searches. |
| `history.json` | Listening history (drives stats). |
| `radio_favorites.json` | Starred radio stations. |
| `spotify_token.json` | Cached Spotify OAuth tokens. |
| `spotify_client_id` | Your Spotify client ID (if set). |
| `spotify_view.json` | Last Spotify browse state (instant restore). |
| `session.json` | Last view, focus, queue and panel layout. |
| `cache/` | Artwork, cover and radio-directory caches. |

To reset lyrfin completely, delete the config directory (it will be recreated with
defaults on the next launch).
