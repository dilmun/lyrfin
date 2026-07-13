//! OS "Now Playing" snapshot + remote-command routing — the `AppState` side of
//! [`crate::media`]. Platform-neutral: no `souvlaki`/objc types appear here, only
//! the shared [`MediaCommand`] / [`NowPlayingSnapshot`].
//!
//! Both the snapshot the OS *shows* and the commands it *sends* resolve through
//! one [`AppState::now_playing_source`], so Control Center / MPRIS reflects, and
//! controls, the source that is actually audible — not merely the focused view.

use std::path::Path;
use std::time::Duration;

use super::*;
use crate::core::model::TrackId;
use crate::core::player::Status;
use crate::media::{MediaCommand, NowPlayingSnapshot};

/// Which source currently owns system playback.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NpSource {
    Local,
    Spotify,
    Radio,
}

impl AppState {
    /// The source the OS "Now Playing" should reflect: the one *actively playing*
    /// wins (the overlays are mutually exclusive at the audio engine), else the
    /// on-screen context so a paused card matches what lyrfin shows. `None` when
    /// nothing is loaded anywhere.
    fn now_playing_source(&self) -> Option<NpSource> {
        // actively-playing source wins (only one drives the engine at a time)
        if self.rnow.now_station.is_some() && !self.rnow.radio_paused {
            return Some(NpSource::Radio);
        }
        if self.spov.now_spotify.is_some() && !self.spov.spotify_paused {
            return Some(NpSource::Spotify);
        }
        if self.player.status == Status::Playing {
            return Some(NpSource::Local);
        }
        // nothing playing → reflect the on-screen context, else any loaded source
        let (local, spotify, radio) = (
            self.current_track().is_some(),
            self.spov.now_spotify.is_some(),
            self.rnow.now_station.is_some(),
        );
        match self.layout {
            Layout::Radio if radio => Some(NpSource::Radio),
            Layout::Spotify if spotify => Some(NpSource::Spotify),
            _ if local => Some(NpSource::Local),
            _ if spotify => Some(NpSource::Spotify),
            _ if radio => Some(NpSource::Radio),
            _ => None,
        }
    }

    /// Build the OS "Now Playing" snapshot for the [`now_playing_source`], or
    /// `None` when nothing is loaded (the bridge then clears the OS info). Takes
    /// `&mut self` to memoise the extracted local-cover file across frames.
    ///
    /// [`now_playing_source`]: Self::now_playing_source
    pub fn now_playing_snapshot(&mut self) -> Option<NowPlayingSnapshot> {
        match self.now_playing_source()? {
            NpSource::Spotify => {
                let tr = self.spov.now_spotify.as_ref()?;
                // sp_dur tracks the live length (podcasts grow past the header); fall
                // back to the track's declared length before the stream reports it.
                let secs = if self.spov.sp_dur > 0.0 {
                    self.spov.sp_dur
                } else {
                    tr.duration_ms as f64 / 1000.0
                };
                Some(NowPlayingSnapshot {
                    title: tr.name.clone(),
                    artist: tr.primary_artist().to_string(),
                    album: tr.album.clone(),
                    duration: Duration::from_secs_f64(secs.max(0.0)),
                    elapsed: Duration::from_secs_f64(self.spov.sp_pos.max(0.0)),
                    playing: !self.spov.spotify_paused,
                    // Spotify covers are already valid https URLs the OS can fetch.
                    cover: self.spov.sp_cover_url.clone().or_else(|| tr.image.clone()),
                })
            }
            NpSource::Radio => {
                let st = self.rnow.now_station.as_ref()?;
                // the live ICY "Artist - Title" if the stream sent one, else the name
                let title = self
                    .rnow
                    .now_station_title
                    .clone()
                    .unwrap_or_else(|| st.name.clone());
                Some(NowPlayingSnapshot {
                    title,
                    artist: st.name.clone(),
                    album: String::new(),
                    duration: Duration::ZERO, // live — no fixed length
                    elapsed: Duration::ZERO,
                    playing: !self.rnow.radio_paused,
                    cover: None,
                })
            }
            NpSource::Local => {
                let t = self.current_track()?;
                let (title, artist, album) =
                    (t.title.clone(), t.artist.to_string(), t.album.to_string());
                let (id, path) = (t.id, t.path.clone()); // ends the &t borrow
                Some(NowPlayingSnapshot {
                    title,
                    artist,
                    album,
                    duration: self.player.duration,
                    elapsed: self.player.elapsed,
                    playing: self.player.status == Status::Playing,
                    cover: self.resolve_local_cover(id, &path),
                })
            }
        }
    }

    /// Apply a transport command from the OS media controls to the active source.
    /// Reuses the same per-source primitives as the on-screen transport, so there
    /// is no duplicate playback logic (see `app/playback.rs`).
    pub fn on_media_command(&mut self, cmd: MediaCommand) {
        let Some(src) = self.now_playing_source() else {
            // nothing loaded: only a play/toggle is meaningful — start the selection
            if matches!(cmd, MediaCommand::Play | MediaCommand::Toggle) {
                self.play_current();
            }
            return;
        };
        match cmd {
            MediaCommand::Toggle => self.np_toggle(src),
            MediaCommand::Play if !self.np_is_playing(src) => self.np_toggle(src),
            // Stop, like Pause, halts the active source (a distinct hard-stop isn't
            // worth a separate path for a music player).
            MediaCommand::Pause | MediaCommand::Stop if self.np_is_playing(src) => {
                self.np_toggle(src)
            }
            MediaCommand::Play | MediaCommand::Pause | MediaCommand::Stop => {} // already in state
            MediaCommand::Next => match src {
                NpSource::Spotify => self.spotify_track(1),
                NpSource::Radio => self.radio_station(1),
                NpSource::Local => self.advance_next(),
            },
            MediaCommand::Previous => match src {
                NpSource::Spotify => self.spotify_track(-1),
                NpSource::Radio => self.radio_station(-1),
                NpSource::Local => self.advance_prev(),
            },
            MediaCommand::SeekTo(secs) => self.np_seek_to(src, secs),
            MediaCommand::SeekBy(delta) => self.np_seek_by(src, delta),
        }
        self.dirty = true;
    }

    fn np_toggle(&mut self, src: NpSource) {
        match src {
            NpSource::Spotify => self.toggle_spotify_play(),
            NpSource::Radio => self.toggle_radio_play(),
            NpSource::Local => self.toggle_local_play(),
        }
    }

    fn np_is_playing(&self, src: NpSource) -> bool {
        match src {
            NpSource::Spotify => !self.spov.spotify_paused,
            NpSource::Radio => !self.rnow.radio_paused,
            NpSource::Local => self.player.status == Status::Playing,
        }
    }

    /// Absolute seek (seconds) for the active source. Radio is live → no-op.
    fn np_seek_to(&mut self, src: NpSource, secs: f64) {
        let (pos, dur) = match src {
            NpSource::Local => (secs, self.player.duration.as_secs_f64()),
            NpSource::Spotify => (secs, self.spov.sp_dur),
            NpSource::Radio => return,
        };
        if dur > 0.0 {
            let frac = (pos / dur).clamp(0.0, 1.0) as f32;
            match src {
                NpSource::Spotify => self.spotify_seek_to_fraction(frac),
                _ => self.seek_to_fraction(frac),
            }
        }
    }

    /// Relative seek (seconds, negative = back) for the active source.
    fn np_seek_by(&mut self, src: NpSource, delta: i64) {
        match src {
            NpSource::Local => self.seek_relative(delta),
            NpSource::Spotify => {
                let target = (self.spov.sp_pos + delta as f64).max(0.0);
                self.np_seek_to(NpSource::Spotify, target);
            }
            // seek_relative is DVR-aware (no-op on a live-only stream).
            NpSource::Radio => self.seek_relative(delta),
        }
    }

    /// The cover URL for a local track, memoised so the embedded art is extracted
    /// to disk only once per track change, not every frame.
    fn resolve_local_cover(&mut self, id: TrackId, path: &Path) -> Option<String> {
        if let Some((cached, url)) = &self.media_cover
            && *cached == id
        {
            return url.clone();
        }
        let url = self.extract_local_cover(path);
        self.media_cover = Some((id, url.clone()));
        url
    }

    /// Write the track's embedded cover to a cache file and return its `file://`
    /// URL, or `None` when the track has no embedded art. Ping-pongs between two
    /// filenames so consecutive tracks get distinct URLs — some desktops cache the
    /// art by URL and won't reload an unchanged path.
    fn extract_local_cover(&mut self, path: &Path) -> Option<String> {
        let (bytes, ext) = crate::cover::cover_bytes(path)?;
        let dir = self.config.dir.join("cache");
        if std::fs::create_dir_all(&dir).is_err() {
            return None;
        }
        self.media_cover_slot = !self.media_cover_slot;
        let slot = u8::from(self.media_cover_slot);
        let file = dir.join(format!("nowplaying-{slot}.{ext}"));
        std::fs::write(&file, &bytes).ok()?;
        file_url(&file)
    }
}

/// Build a percent-encoded `file://` URL for `path`. The OS resolves the cover
/// via `NSURL URLWithString:` (macOS) / a desktop URL loader (MPRIS), so a raw
/// path won't do — and the macOS config dir contains a space ("Application
/// Support"). Keeps the URL-unreserved set plus `/` literal; everything else is
/// `%XX`-escaped.
fn file_url(path: &Path) -> Option<String> {
    let s = path.to_str()?;
    let mut out = String::from("file://");
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::file_url;
    use std::path::Path;

    #[test]
    fn file_url_escapes_spaces_and_keeps_separators() {
        // the macOS config dir ("Application Support") has a space; the URL must
        // percent-encode it while leaving `/` and unreserved chars intact.
        let url = file_url(Path::new(
            "/Users/x/Application Support/lyrfin/cache/nowplaying-0.jpg",
        ))
        .unwrap();
        assert_eq!(
            url,
            "file:///Users/x/Application%20Support/lyrfin/cache/nowplaying-0.jpg"
        );
    }
}
