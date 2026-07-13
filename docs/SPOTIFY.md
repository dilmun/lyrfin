# Spotify

lyrfin can browse and play Spotify directly in the terminal, using
[librespot](https://github.com/librespot-org/librespot) for audio (routed through
lyrfin's own engine) and the Spotify Web API for browsing.

## Requirements

- A **Spotify Premium** account for playback. This is a librespot limitation —
  free accounts can't stream. Browsing and search work on any account, but lyrfin
  will refuse to start playback on a non-Premium account and reports the reason.

## Logging in

1. Launch lyrfin and press <kbd>7</kbd> to open the Spotify view.
2. Press <kbd>Enter</kbd> to log in. Your default browser opens Spotify's consent
   page (OAuth **Authorization Code + PKCE**, so no client secret is stored).
3. Approve access. Spotify redirects to `http://127.0.0.1:8898/login`, which lyrfin
   is listening on; it captures the code and completes the exchange.
4. You're connected. The access + refresh tokens are cached under the config
   directory (`spotify_token.json`) and refreshed automatically — you won't need
   to log in again on future launches.

The requested scopes cover streaming, reading/modifying your library and
playlists, your profile, playback state, recently played and top tracks.

## Using your own client ID (recommended)

By default lyrfin uses a shared public client ID for Web-API calls. That works, but
the shared quota can hit rate limits (HTTP 429) or registration errors (403). For
reliable browsing, register your own free app:

1. Open the [Spotify Developer Dashboard](https://developer.spotify.com/dashboard)
   and create an app.
2. In the app's settings, add the redirect URI **`http://127.0.0.1:8898/login`**.
3. Copy the **Client ID** and paste it into lyrfin's Spotify view (press `;` →
   **Account** tab → Client ID), or set `spotify_client_id` in `config.toml`.

lyrfin stores the client ID in a dedicated `spotify_client_id` file, separate from
`config.toml`, so a config parse error can never wipe it. Your own ID is
preferred over the shared one once set.

## What works

- Browse home/search results, your library, playlists, and artist pages
  (popular tracks, albums, related artists).
- Play tracks through lyrfin's engine — the visualizer, volume and speed controls
  all apply. Stream quality is configurable (`spotify_bitrate`: 96/160/320 kbps).
- Like / unlike tracks; create, rename, delete and edit playlists.

## Known limitations

- **Premium required for playback** (see above).
- **Podcasts:** Spotify does not grant podcast decryption keys to third-party
  clients, so Spotify-hosted episodes can't be decoded directly. lyrfin instead
  matches an episode to its **public RSS feed** and streams the open MP3 — which
  works for syndicated shows but not Spotify-exclusive podcasts.
- **Web-API scope:** some catalog endpoints are restricted for apps in Spotify's
  development mode; lyrfin leans on librespot's own metadata paths where it can.
- Single-account login (no in-app account switcher yet).

## Troubleshooting

- **The browser page is blank or errors on the Spotify consent screen** — that's
  a browser-side issue with Spotify's page, not lyrfin. The auth flow itself still
  completes; try a different browser if it persists.
- **"Unable to load encrypted file" / playback stalls** — usually a transient
  Spotify CDN issue; lyrfin already fails over across CDN nodes. When a track that's
  already playing stalls mid-way (librespot reports "unable to get next packet …
  Deadline expired" under a slow link), lyrfin re-buffers the **same** track in
  place at the stall point rather than skipping — so a network hiccup no longer
  flips through the queue. A segment that keeps stalling at one spot is skipped
  after a few tries.
- **Rate-limit / 403 errors while browsing** — set your own client ID (above).
- To see librespot's own logs: `RUST_LOG=librespot=info lyrfin 2>/tmp/lyrfin.log`
  (the TUI uses stdout, so logs go to stderr and can be redirected cleanly).
