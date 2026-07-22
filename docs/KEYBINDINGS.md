# Keybindings

These are lyrfin's **default** keybindings. Everything is rebindable in
`~/.config/lyrfin/keybindings.toml` (edit by hand, or from the in-app settings
popup with <kbd>;</kbd> → Keys). The in-app help (<kbd>?</kbd>) always reflects
your current bindings and is the authoritative reference.

Many keys are **context-aware**: the same key can do different things depending
on the focused pane or the active view (noted below where relevant).

## Global

| Key | Action |
|-----|--------|
| <kbd>q</kbd> / <kbd>Ctrl</kbd>+<kbd>C</kbd> | Quit |
| <kbd>?</kbd> | Toggle the help / info overlay |
| <kbd>:</kbd> | Command palette — type a name to find any action or setting; <kbd>⏎</kbd> runs / opens its value list, <kbd>→</kbd> reveals a setting in the Settings overlay |
| <kbd>Tab</kbd> / <kbd>Shift</kbd>+<kbd>Tab</kbd> | Focus next / previous pane |
| <kbd>Esc</kbd> | Back / up one level / close overlay |
| <kbd>Ctrl</kbd>+<kbd>B</kbd> / <kbd>Ctrl</kbd>+<kbd>F</kbd> | **Back / forward** through your browsing history |
| <kbd>Ctrl</kbd>+<kbd>[</kbd> / <kbd>Ctrl</kbd>+<kbd>]</kbd> | Same, but only in terminals using the kitty keyboard protocol (Ghostty, Kitty) — see below |
| <kbd>Ctrl</kbd>+<kbd>H</kbd> <kbd>J</kbd> <kbd>K</kbd> <kbd>L</kbd> | Focus the pane left / below / above / right |

> **Focus scoping.** While a side-pane holds the focus it owns the keyboard: its own
> keys (e.g. Queue reorder, Lyrics format) plus the universal keys — navigation,
> playback transport, and app chrome (<kbd>:</kbd> <kbd>?</kbd> <kbd>/</kbd> quit,
> view-switch <kbd>1</kbd>–<kbd>7</kbd>) — always work. Other view-content shortcuts
> (theme, EQ, panel toggles…) are reachable from the main area — press <kbd>Tab</kbd>
> to return there.

> **Why <kbd>Ctrl</kbd>+<kbd>B</kbd>/<kbd>F</kbd> and not the brackets?** A terminal
> encodes <kbd>Ctrl</kbd>+letter as one byte in the `0x01`–`0x1A` range, which every
> terminal agrees on. Brackets fall outside it: <kbd>Ctrl</kbd>+<kbd>[</kbd> is
> `0x1B`, which *is* Escape, and <kbd>Ctrl</kbd>+<kbd>]</kbd> is `0x1D`, which a
> legacy terminal reports as <kbd>Ctrl</kbd>+<kbd>5</kbd>. Only terminals speaking
> the kitty keyboard protocol can tell them apart. <kbd>Ctrl</kbd>+<kbd>[</kbd>
> still works as back everywhere simply because Escape already is back. The same
> problem rules out vim's <kbd>Ctrl</kbd>+<kbd>I</kbd> for forward — that byte is
> <kbd>Tab</kbd>.

> **Moving between panes.** <kbd>Tab</kbd> cycles them in order; <kbd>Ctrl</kbd> +
> <kbd>h</kbd>/<kbd>j</kbd>/<kbd>k</kbd>/<kbd>l</kbd> jumps to the pane lying in that
> direction, using where panes are actually drawn — so stacked panes and the
> Library's three columns work without thinking about it. These always move focus,
> whatever the focused pane does with the plain keys, so a cover grid or list that
> uses <kbd>j</kbd>/<kbd>k</kbd> for its own selection can never trap you. In the
> miniplayer, where one card fills the screen, <kbd>Ctrl</kbd>+<kbd>h</kbd>/<kbd>l</kbd>
> step through the cards.

> **Narrow terminals.** Under 64 columns the browse views (Home, Library, Radio,
> Spotify) show one pane at a time as a full-width card. There, <kbd>h</kbd> and
> <kbd>l</kbd> walk that card stack — <kbd>h</kbd> backs out of an album into
> whatever you opened it from, <kbd>l</kbd> goes deeper or redoes a step you backed
> out of — instead of moving focus between panes, since there are no side-by-side
> panes to move between. Inside a cover grid they still move the selection.

## Layouts

| Key | View |
|-----|------|
| <kbd>1</kbd> | Dashboard (Home) |
| <kbd>2</kbd> | Library (Artists ▸ Albums ▸ Tracks) |
| <kbd>3</kbd> | Full Player |
| <kbd>4</kbd> | Lyrics |
| <kbd>5</kbd> | Concert (distraction-free) |
| <kbd>6</kbd> | Internet Radio |
| <kbd>7</kbd> | Spotify |

## Transport & playback

| Key | Action |
|-----|--------|
| <kbd>Space</kbd> | Play / pause |
| <kbd>n</kbd> / <kbd>p</kbd> | Next / previous track |
| <kbd>.</kbd> / <kbd>,</kbd> | Seek forward / backward (±5s) — in the Lyrics view/pane these nudge the lyric sync instead |
| <kbd>+</kbd> / <kbd>=</kbd> | Volume up (+5%) |
| <kbd>-</kbd> | Volume down (−5%) |
| <kbd>s</kbd> | Toggle shuffle |
| <kbd>r</kbd> | Cycle repeat (off → one → all) |
| <kbd>]</kbd> / <kbd>[</kbd> | Playback speed up / down |
| <kbd>o</kbd> | A–B loop (set A → set B → clear) |
| <kbd>Shift</kbd>+<kbd>T</kbd> | Sleep timer (off → 15 → 30 → 45 → 60 min) |

## Navigation

| Key | Action |
|-----|--------|
| <kbd>j</kbd> / <kbd>↓</kbd> | Move down |
| <kbd>k</kbd> / <kbd>↑</kbd> | Move up |
| <kbd>g</kbd> / <kbd>G</kbd> | Jump to top / bottom |
| <kbd>Ctrl</kbd>+<kbd>N</kbd> / <kbd>Ctrl</kbd>+<kbd>P</kbd> | Move down / up (works in any context) |
| <kbd>Ctrl</kbd>+<kbd>D</kbd> / <kbd>Ctrl</kbd>+<kbd>U</kbd> | Scroll a page down / up (any list) |
| <kbd>PageDown</kbd> / <kbd>PageUp</kbd> | Page down / up |
| <kbd>Enter</kbd> | Activate / play the selection |
| <kbd>h</kbd> / <kbd>l</kbd> (and <kbd>←</kbd> / <kbd>→</kbd>) | Focus left / right — move between the sidebar, list, and panes. In the Library browser they switch column; in cover grids they move a card; in the Settings overlay they adjust the selected value |

## Library, search & views

| Key | Action |
|-----|--------|
| <kbd>/</kbd> | Search the current view |
| <kbd>#</kbd> | Toggle cover-art grid ↔ list (Albums / Artists) |
| <kbd>b</kbd> | Toggle sidebar |
| <kbd>u</kbd> | Toggle queue pane |
| <kbd>i</kbd> | Toggle artist panel |
| <kbd>Shift</kbd>+<kbd>L</kbd> | Toggle lyrics pane |
| <kbd>Shift</kbd>+<kbd>V</kbd> | Toggle visualizer pane |
| <kbd>v</kbd> | Cycle visualizer mode |
| <kbd>Shift</kbd>+<kbd>F</kbd> | Cycle lyrics format (plain → karaoke → teleprompter) — Lyrics view / pane |
| <kbd>t</kbd> | Cycle theme |
| <kbd>;</kbd> | Per-view settings popup |
| <kbd>Shift</kbd>+<kbd>I</kbd> | Stats & metadata overlay |
| <kbd>y</kbd> | Copy the last error to the clipboard (OSC 52) |

## Panes (docking & resize)

| Key | Action |
|-----|--------|
| <kbd>m</kbd> | Move the focused pane to the next dock edge (L → T → R → B) |
| <kbd>&gt;</kbd> / <kbd>&lt;</kbd> | Widen / narrow the focused pane |
| <kbd>}</kbd> / <kbd>{</kbd> | Grow / shrink the focused pane's height |
| <kbd>Ctrl</kbd>+<kbd>Q</kbd> | Cycle the queue pane's dock position |
| <kbd>z</kbd> | Re-fit pane sizes to the window |
| <kbd>Shift</kbd>+<kbd>Z</kbd> | Reset the view's panels to defaults |

## Ratings & favorites

| Key | Action |
|-----|--------|
| <kbd>f</kbd> | Toggle favorite |
| <kbd>)</kbd> / <kbd>(</kbd> | Rate up / down (0–5★) |

## Queue (when the Queue pane is focused)

| Key | Action |
|-----|--------|
| <kbd>Shift</kbd>+<kbd>J</kbd> / <kbd>Shift</kbd>+<kbd>K</kbd> | Move the selected upcoming track down / up |
| <kbd>d</kbd> | Remove the selected upcoming track |
| <kbd>Shift</kbd>+<kbd>D</kbd> | Clear everything after the now-playing track |
| <kbd>c</kbd> | Clear the queue |
| <kbd>a</kbd> | Play the selected track's album |

## Playlists (Dashboard → Playlists)

| Key | Action |
|-----|--------|
| <kbd>n</kbd> | New playlist |
| <kbd>Shift</kbd>+<kbd>S</kbd> | New smart playlist (from a search) |
| <kbd>e</kbd> / <kbd>r</kbd> | Rename playlist |
| <kbd>Shift</kbd>+<kbd>D</kbd> | Delete playlist |
| <kbd>a</kbd> | Add the now-playing track to the selected playlist |
| <kbd>Shift</kbd>+<kbd>B</kbd> | Bookmark the current search |

## Radio view

| Key | Action |
|-----|--------|
| <kbd>/</kbd> | Search stations |
| <kbd>n</kbd> / <kbd>p</kbd> | Next / previous station |
| <kbd>f</kbd> | Star / unstar the selected station(s) (saved to Favorites) |
| <kbd>a</kbd> | Add the selected station(s) to a playlist |
| <kbd>x</kbd> / <kbd>Shift</kbd>+<kbd>V</kbd> | Mark station / visual range — `f` and `a` then apply to the whole selection |
| <kbd>o</kbd> | Cycle sort order |
| <kbd>Shift</kbd>+<kbd>R</kbd> | Refresh the station directory |
| <kbd>Enter</kbd> on **Countries** / **Genres** | Browse / filter by country or genre (sidebar sections) |

`g` / `G` are jump-to-top / bottom here too — country and genre live in the
sidebar's **Countries** and **Genres** sections rather than on a shortcut key.

## Spotify view

| Key | Action |
|-----|--------|
| <kbd>Enter</kbd> | Log in (when disconnected) |
| <kbd>/</kbd> | Search |
| <kbd>n</kbd> / <kbd>p</kbd> | Next / previous track |
| <kbd>f</kbd> | Like the now-playing track (or the whole selection) |
| <kbd>a</kbd> | Add the selected track(s) to a playlist |
| <kbd>x</kbd> / <kbd>Shift</kbd>+<kbd>V</kbd> | Mark track / visual range (on a track list) — `f` likes and `a` adds the whole selection |
| <kbd>n</kbd> (Playlists) | New playlist |
| <kbd>e</kbd> / <kbd>r</kbd> | Rename the selected playlist |
| <kbd>Shift</kbd>+<kbd>D</kbd> | Delete (unfollow) the selected playlist |
| <kbd>d</kbd> (inside a playlist) | Remove the selected track(s) from the playlist |

## Tag editor

Opened on the focused track (from the library). Field navigation with
<kbd>j</kbd>/<kbd>k</kbd>; begin editing a field with <kbd>i</kbd> or
<kbd>Enter</kbd>.

| Key | Action |
|-----|--------|
| <kbd>s</kbd> | Save changes to the file |
| <kbd>a</kbd> | Apply to every track on the album (with confirmation) |
| <kbd>Esc</kbd> / <kbd>q</kbd> | Cancel |
| <kbd>Ctrl</kbd>+<kbd>T</kbd> / <kbd>Ctrl</kbd>+<kbd>U</kbd> / <kbd>Ctrl</kbd>+<kbd>L</kbd> | Title-case / UPPERCASE / lowercase the field |
| <kbd>Ctrl</kbd>+<kbd>N</kbd> | Auto-number tracks |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> / <kbd>Ctrl</kbd>+<kbd>R</kbd> | Filename → tags / tags → filename pattern |
| <kbd>Ctrl</kbd>+<kbd>E</kbd> | Find & replace within the field |
| <kbd>Ctrl</kbd>+<kbd>D</kbd> | Delete the field from all target tracks |

## Settings & overlays

| Key | Action |
|-----|--------|
| <kbd>Delete</kbd> / <kbd>Ctrl</kbd>+<kbd>D</kbd> | Delete the selected row (e.g. a music directory) |
| <kbd>Ctrl</kbd>+<kbd>R</kbd> | Restore default keybindings (Keys settings) |
