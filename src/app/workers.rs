//! Artist-info + lyrics worker wiring; audio drain methods on `AppState` (extracted from app/mod.rs).

use super::*;

/// How close the engine's re-opened position must get to a streamed scrub target
/// before the bar unlocks and resumes the live clock — absorbs the re-open latency
/// so it doesn't flash the old position first. See `spotify_seek`'s `sp_seek_target`.
const SP_SEEK_CONFIRM_TOL: f64 = 2.0;

impl AppState {
    /// Attach the online artist-info worker and fetch for the current track.
    pub fn set_info_sender(
        &mut self,
        tx: crossbeam_channel::Sender<crate::artistinfo::InfoRequest>,
    ) {
        self.workers.info = Some(tx);
        self.request_artist_info();
    }

    /// The artist (and its album) whose info the active source's pane shows: the
    /// Spotify now-playing artist in the Spotify view, else the local track's
    /// album artist (the act, not the per-track artist). The first listed artist
    /// reads best against the bio source ("A, B" → "A"). `None` when nothing is
    /// playing for that source. This single derivation is the bio's identity key —
    /// shared by the fetch ([`Self::request_artist_info`]) and the source-gated
    /// accessors ([`Self::current_artist_info`]) so they can never disagree.
    pub(crate) fn active_artist_target(&self) -> Option<(&str, &str)> {
        if self.showing_spotify() {
            let tr = self.spov.now_spotify.as_ref()?;
            let artist = tr.primary_artist();
            (!artist.is_empty()).then_some((artist, tr.album.as_str()))
        } else {
            let t = self.current_track()?;
            let artist = t.album_artist.as_str();
            (!artist.is_empty()).then_some((artist, t.album.as_str()))
        }
    }

    /// The active source's current artist name. See [`Self::active_artist_target`].
    pub(crate) fn active_artist(&self) -> Option<&str> {
        self.active_artist_target().map(|(artist, _)| artist)
    }

    /// Request artist info (bio/formed/country) for the active source. Skips when
    /// that artist panel is hidden or the artist is unchanged, so there's no
    /// network when nothing's shown.
    pub(crate) fn request_artist_info(&mut self) {
        // clone the target up front, releasing the borrow before we mutate `meta`
        let Some((artist, album)) = self
            .active_artist_target()
            .map(|(a, al)| (a.to_string(), al.to_string()))
        else {
            return;
        };
        let layout = if self.showing_spotify() {
            Layout::Spotify
        } else {
            Layout::Dashboard
        };
        if !self.panel_in(layout, Panel::Artist).shown {
            return;
        }
        if self.meta.info_artist.as_deref() == Some(artist.as_str()) {
            return;
        }
        self.meta.info_artist = Some(artist.clone());
        self.meta.artist_info = None;
        self.scroll.artist.set(0); // new artist → scroll the bio back to the top
        self.meta.info_pending = true;
        if let Some(tx) = &self.workers.info {
            let _ = tx.send(crate::artistinfo::InfoRequest { artist, album });
        }
    }

    /// The fetched artist info, but only when it belongs to the artist the active
    /// source's pane is *currently* showing. Both artist panes read the single
    /// shared `meta.artist_info` slot, so this identity gate is what stops a bio
    /// fetched for one source (local vs Spotify) from bleeding into the other
    /// pane — independent of when the async fetch lands.
    pub fn current_artist_info(&self) -> Option<&crate::artistinfo::ArtistInfo> {
        let artist = self.active_artist()?;
        (self.meta.info_artist.as_deref() == Some(artist))
            .then_some(self.meta.artist_info.as_ref())
            .flatten()
    }

    /// Whether an artist-info fetch for the *currently shown* artist is in flight
    /// (so a pane shows "loading…" only for its own artist, never for a stale
    /// fetch left over from the other source).
    pub fn artist_info_loading(&self) -> bool {
        self.meta.info_pending && self.meta.info_artist.as_deref() == self.active_artist()
    }

    /// Apply a fetched artist-info result (ignored if the artist moved on). Matches
    /// the requested artist (`info_artist`), so it works for local + Spotify alike.
    pub fn on_info_result(&mut self, res: crate::artistinfo::InfoResult) {
        if self.meta.info_artist.as_deref() == Some(res.artist.as_str()) {
            self.meta.artist_info = res.info;
            self.meta.info_pending = false;
        }
    }

    /// Attach the online lyrics worker and look up the current track.
    pub fn set_lyrics_sender(
        &mut self,
        tx: crossbeam_channel::Sender<crate::lyricsfetch::LyricsRequest>,
    ) {
        self.workers.lyrics = Some(tx);
        // a restored Spotify track in the Spotify view wants its lyrics, not the
        // local track's — force a load now that the worker can serve online misses
        self.reload_lyrics();
    }

    /// Load lyrics for whatever source is active (Spotify's now-playing in the
    /// Spotify view, else the local track), replacing the shared `meta.lyrics` slot.
    pub(crate) fn reload_lyrics(&mut self) {
        // must agree with `active_lyrics_pane`, which decides which slot the result
        // is cached under — disagreeing would fetch one source's words and store
        // them against the other's key
        if self.lyrics_source_is_spotify() {
            self.load_spotify_lyrics();
        } else {
            self.load_lyrics();
        }
    }

    /// Re-target the shared lyrics slot at the active source on a view switch, so
    /// the pane never keeps the other source's lyrics (mirrors `request_artist_info`).
    /// Dedups when the slot already holds the active track's lyrics, so switching
    /// views doesn't re-fetch.
    pub(crate) fn request_lyrics(&mut self) {
        let key = self.lyrics_key_for_pane(self.active_lyrics_pane());
        if key.is_some() && self.meta.lyrics_for == key {
            return;
        }
        self.reload_lyrics();
    }

    /// Which source currently owns playback / the now-bar — the pane the shared
    /// lyrics slot is loaded for.
    pub(crate) fn active_lyrics_pane(&self) -> crate::app::LyricsPane {
        if self.lyrics_source_is_spotify() {
            crate::app::LyricsPane::Spotify
        } else {
            crate::app::LyricsPane::Local
        }
    }

    /// Whether lyrics should track the Spotify track rather than the local one.
    ///
    /// The Spotify *view* always does. The player views (Now Playing / Lyrics /
    /// Concert) have no source of their own, so they follow whatever is playing —
    /// otherwise opening Lyrics while Spotify streams would fetch the *local*
    /// track's lyrics and overwrite the Spotify slot that the shared cache keys.
    pub(crate) fn lyrics_source_is_spotify(&self) -> bool {
        match self.layout {
            // a player view follows the audio
            l if l.is_player_view() => {
                self.now_playing_source() == Some(crate::app::NpSource::Spotify)
                    && self.spov.now_spotify.is_some()
            }
            Layout::Spotify => self.spov.now_spotify.is_some(),
            // Home/Library dock a Local lyrics pane; radio has no per-track
            // identity to key a lookup on
            _ => false,
        }
    }

    /// The lyrics cache key for `pane`'s track (`None` when that source has no
    /// track). Matches the keys `load_lyrics`/`load_spotify_lyrics` store in
    /// `meta.lyrics_for`, so a pane can tell whether the loaded lyrics are its own.
    fn lyrics_key_for_pane(&self, pane: crate::app::LyricsPane) -> Option<String> {
        match pane {
            crate::app::LyricsPane::Spotify => {
                let tr = self.spov.now_spotify.as_ref()?;
                Some(crate::lyrics::cache_key(tr.primary_artist(), &tr.name))
            }
            crate::app::LyricsPane::Local => {
                let t = self.current_track()?;
                Some(crate::lyrics::cache_key(&t.artist, &t.title))
            }
        }
    }

    /// The lyrics for `pane`, source-gated: the shared `meta.lyrics` only when it
    /// was loaded for that pane's track. This is what stops a local track's lyrics
    /// leaking into the Spotify pane (and vice-versa) — both read one slot.
    pub fn lyrics_for_pane(&self, pane: crate::app::LyricsPane) -> Option<&crate::lyrics::Lyrics> {
        let key = self.lyrics_key_for_pane(pane)?;
        (self.meta.lyrics_for.as_deref() == Some(key.as_str()))
            .then_some(self.meta.lyrics.as_ref())
            .flatten()
    }

    /// Whether `pane`'s source has a track to follow — drives the pane's
    /// "no lyrics found" vs the idle "—".
    pub fn lyrics_pane_has_track(&self, pane: crate::app::LyricsPane) -> bool {
        self.lyrics_key_for_pane(pane).is_some()
    }

    /// Whether a lyrics fetch for `pane`'s own track is in flight, so it shows
    /// "searching…" only for its source, never a stale fetch from the other.
    pub fn lyrics_pane_loading(&self, pane: crate::app::LyricsPane) -> bool {
        self.meta.lyrics_pending && self.meta.lyrics_for == self.lyrics_key_for_pane(pane)
    }

    /// Apply a fetched-lyrics result (ignored if the track moved on).
    pub fn on_lyrics_result(&mut self, res: crate::lyricsfetch::LyricsResult) {
        if self.meta.lyrics_for.as_deref() != Some(res.key.as_str()) {
            return;
        }
        self.meta.lyrics_pending = false;
        if let Some(text) = &res.text {
            crate::lyrics::Lyrics::save_cache(&self.config.dir, &res.key, text);
            let l = crate::lyrics::Lyrics::parse(text);
            if !l.lines.is_empty() {
                self.meta.lyrics = Some(l);
            }
        }
        self.maybe_request_translation();
    }

    /// Attach the translation worker; returns nothing (fire-and-forget like the
    /// other senders). Translates whatever lyrics are already loaded.
    pub fn set_translate_sender(
        &mut self,
        tx: crossbeam_channel::Sender<crate::translate::TranslateRequest>,
    ) {
        self.workers.translate = Some(tx);
        self.maybe_request_translation();
    }

    /// Kick off a machine translation for the loaded lyrics when a target language
    /// is set and there's no human (bilingual-`.lrc`) translation already — from
    /// the on-disk cache if present, else the worker thread. Cheap + idempotent, so
    /// it's safe to call after every lyrics (re)load.
    pub(crate) fn maybe_request_translation(&mut self) {
        let target = self.config.lyrics_translate_to.clone();
        if target.is_empty() {
            return; // translation off
        }
        let Some(key) = self.meta.lyrics_for.clone() else {
            return;
        };
        let Some(lyr) = self.meta.lyrics.as_ref() else {
            return;
        };
        // a human translation wins; and there's nothing to do for empty lyrics
        if lyr.lines.is_empty() || lyr.trans.iter().any(Option::is_some) {
            return;
        }
        let lines: Vec<String> = lyr.lines.iter().map(|(_, s)| s.clone()).collect();
        // cache hit → apply immediately, no network
        if let Some(tr) = crate::translate::load_cached(&self.config.dir, &key, &target) {
            self.apply_translation(&key, &target, tr);
            return;
        }
        if let Some(tx) = &self.workers.translate {
            self.meta.lyrics_translate_pending = true;
            let _ = tx.send(crate::translate::TranslateRequest { key, target, lines });
        }
    }

    /// A translation result arrived: cache it and apply it to the current lyrics.
    pub fn on_translate_result(&mut self, res: crate::translate::TranslateResult) {
        self.meta.lyrics_translate_pending = false;
        let Some(lines) = res.lines else {
            return; // lookup failed → just show the originals
        };
        crate::translate::save_cached(&self.config.dir, &res.key, &res.target, &lines);
        self.apply_translation(&res.key, &res.target, lines);
    }

    /// Fill the current lyrics' per-line translations from `tr`, but only if they're
    /// still the lyrics we asked about (`key`) and the target hasn't changed. A line
    /// whose translation equals the original (e.g. English→English) is left blank so
    /// the dual view doesn't show a redundant duplicate.
    fn apply_translation(&mut self, key: &str, target: &str, tr: Vec<String>) {
        if self.meta.lyrics_for.as_deref() != Some(key) || self.config.lyrics_translate_to != target
        {
            return; // stale — the track or target changed
        }
        let Some(lyr) = self.meta.lyrics.as_mut() else {
            return;
        };
        if lyr.trans.iter().any(Option::is_some) {
            return; // a human translation arrived meanwhile — keep it
        }
        for (i, slot) in lyr.trans.iter_mut().enumerate() {
            let t = tr.get(i).map(|s| s.trim()).unwrap_or("");
            let orig = lyr.lines.get(i).map(|(_, s)| s.trim()).unwrap_or("");
            *slot = (!t.is_empty() && t != orig).then(|| t.to_string());
        }
        self.dirty = true;
    }

    /// Attach the podcast resolver (episode → public MP3).
    pub fn set_podcast_sender(
        &mut self,
        tx: crossbeam_channel::Sender<crate::podcastfetch::PodcastRequest>,
    ) {
        self.workers.podcast = Some(tx);
    }

    /// A resolved episode MP3 arrived (ignored if the episode moved on): stream it
    /// through the engine, or report the episode unplayable if nothing was found.
    pub fn on_podcast_result(&mut self, res: crate::podcastfetch::PodcastResult) {
        // stale unless it's still the now-playing episode
        if self.spov.now_spotify.as_ref().map(|t| t.uri.as_str()) != Some(res.key.as_str()) {
            return;
        }
        match res.url {
            Some(url) => self.spotify_stream_episode(url),
            None => self.spotify_episode_unplayable(),
        }
        self.dirty = true;
    }

    pub fn pump_audio(&mut self) {
        let mut evs = Vec::new();
        while let Some(ev) = self.engine.try_recv() {
            evs.push(ev);
        }
        if !evs.is_empty() {
            self.dirty = true; // an audio event changed visible state
        }
        for ev in evs {
            match ev {
                AudioEvent::Progress(d) => {
                    // a streamed podcast episode drives the Spotify clock; the local
                    // player stays frozen (it's a preserved overlay). Radio likewise
                    // owns the engine clock — freeze local there too.
                    if self.spov.sp_stream {
                        let pos = d.as_secs_f64();
                        // While a scrub is settling, keep the bar LOCKED to the target
                        // and ignore the engine's lagging position — so it can't jump
                        // back — until the re-open lands near the target, then unlock.
                        if let Some(target) = self.spov.sp_seek_target {
                            self.spov.sp_started = true;
                            self.last_audio_progress = self.tick;
                            if (pos - target).abs() <= SP_SEEK_CONFIRM_TOL {
                                self.spov.sp_seek_target = None; // reached → resume clock
                            } else {
                                continue; // stale position — don't pull the bar back
                            }
                        }
                        self.spov.sp_pos = pos;
                        // Podcasts with dynamically-inserted ads under-declare their
                        // length (the header reflects the ad-free original), so the
                        // real audio runs past it. Grow the total to track playback
                        // instead of freezing the seeker or overrunning the bar.
                        if pos > self.spov.sp_dur {
                            self.spov.sp_dur = pos;
                        }
                        self.spov.sp_started = true; // first audio → out of "buffering…"
                        self.last_audio_progress = self.tick;
                    } else if self.rnow.now_station.is_none() {
                        self.player.elapsed = d;
                        self.last_audio_progress = self.tick;
                        self.enforce_ab_loop();
                    } else if let Some(dvr) = self.rnow.dvr.as_mut() {
                        // a timeshifted live stream: while following the live edge the
                        // play-head stays pinned to `live`; once rewound, track the
                        // real play position within the DVR window (local stays frozen).
                        if dvr.following {
                            dvr.pos = dvr.live;
                        } else {
                            dvr.pos = d.as_secs_f64();
                        }
                        self.last_audio_progress = self.tick;
                    }
                }
                AudioEvent::DvrWindow { start, live } => self.on_dvr_window(start, live),
                AudioEvent::Duration(d) => {
                    if self.spov.sp_stream {
                        if !d.is_zero() {
                            self.spov.sp_dur = d.as_secs_f64();
                        }
                    } else if self.rnow.now_station.is_none() && !d.is_zero() {
                        self.player.duration = d;
                    }
                }
                AudioEvent::Finished => {
                    if self.spov.sp_stream {
                        // episode ended → stop in place (don't touch the local queue)
                        self.spov.spotify_paused = true;
                        self.spov.sp_started = false;
                    } else if self.rnow.now_station.is_some() {
                        // a radio stream ended/dropped — mark the overlay stopped;
                        // the local player (queue) is untouched.
                        self.rnow.radio_paused = true;
                    } else {
                        self.advance_after_finish();
                    }
                }
                AudioEvent::Advanced => self.soft_advance(),
                AudioEvent::Spectrum(s) => self.player.spectrum = s,
                AudioEvent::IcyTitle(t) => self.on_icy_title(&t),
                AudioEvent::Error(e) => {
                    if self.spov.sp_stream {
                        // the episode's stream couldn't open/decode — stop the spinner
                        self.spov.spotify_paused = true;
                        self.spov.sp_started = false;
                        self.notify("Couldn't play this episode (stream unavailable)".into());
                    } else if self.rnow.now_station.is_some() {
                        // the stream couldn't open/decode — show it stopped, not LIVE
                        self.rnow.radio_paused = true;
                        self.notify("Couldn't play this station (unsupported or offline)".into());
                    } else {
                        self.notify(format!("Audio: {e}"));
                    }
                }
            }
        }
    }
}
