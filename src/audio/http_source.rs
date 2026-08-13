//! HTTP audio sources for streaming playback, extracted from the audio engine:
//! `HttpStream` (a non-seekable ICY radio stream that strips inline metadata and
//! emits `StreamTitle`s) and `HttpRangeSource` (a seekable ranged-HTTP source for
//! externally-hosted podcast episodes). Both are symphonia [`MediaSource`]s; the
//! engine's controller opens one and feeds it to a decoder.

use std::io::{self, Read, Seek, SeekFrom};

use crossbeam_channel::Sender;
use symphonia::core::io::MediaSource;

use crate::audio::AudioEvent;

/// A non-seekable [`MediaSource`] over an HTTP response body (radio stream).
/// When the server interleaves ICY metadata (`icy-metaint`), this strips the
/// metadata blocks out of the audio byte stream and emits the parsed
/// `StreamTitle` as an [`AudioEvent::IcyTitle`].
pub(crate) struct HttpStream {
    pub(crate) reader: Box<dyn Read + Send + Sync>,
    /// Audio bytes between metadata blocks (0 = stream carries no metadata).
    pub(crate) metaint: usize,
    /// Audio bytes left before the next metadata block.
    pub(crate) until_meta: usize,
    pub(crate) evt_tx: Sender<AudioEvent>,
    pub(crate) last_title: String,
}

impl HttpStream {
    /// Read + parse one metadata block (1 length byte ×16, then that many bytes).
    fn consume_metadata(&mut self) -> io::Result<()> {
        let mut lenb = [0u8; 1];
        self.reader.read_exact(&mut lenb)?;
        let len = lenb[0] as usize * 16;
        if len == 0 {
            return Ok(());
        }
        let mut meta = vec![0u8; len];
        self.reader.read_exact(&mut meta)?;
        if let Some(title) = parse_stream_title(&meta)
            && title != self.last_title
        {
            self.last_title = title.clone();
            let _ = self.evt_tx.send(AudioEvent::IcyTitle(title));
        }
        Ok(())
    }
}

/// Pull `StreamTitle='…';` out of an ICY metadata block (lossy UTF-8).
pub(crate) fn parse_stream_title(meta: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(meta);
    let start = s.find("StreamTitle='")? + "StreamTitle='".len();
    let rest = &s[start..];
    let end = rest.find("';").unwrap_or(rest.len());
    let title = rest[..end].trim();
    (!title.is_empty()).then(|| title.to_string())
}

impl Read for HttpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.metaint == 0 {
            return self.reader.read(buf); // no interleaved metadata
        }
        if self.until_meta == 0 {
            self.consume_metadata()?;
            self.until_meta = self.metaint;
        }
        // never read past the next metadata boundary in one call
        let want = buf.len().min(self.until_meta);
        let n = self.reader.read(&mut buf[..want])?;
        self.until_meta -= n;
        Ok(n)
    }
}

impl Seek for HttpStream {
    fn seek(&mut self, _: SeekFrom) -> io::Result<u64> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "radio stream is not seekable",
        ))
    }
}

impl MediaSource for HttpStream {
    fn is_seekable(&self) -> bool {
        false
    }
    fn byte_len(&self) -> Option<u64> {
        None
    }
}

/// A **seekable** [`MediaSource`] over a finite HTTP audio file (a podcast episode
/// MP3, vs the live radio stream). It knows the total size (Content-Length) and
/// re-issues a ranged GET (`Range: bytes=N-`) on seek, so symphonia's seek-by-time
/// works — the user can scrub an episode like a local file.
pub(crate) struct HttpRangeSource {
    pub(crate) agent: ureq::Agent,
    pub(crate) url: String,
    pub(crate) len: u64,
    pub(crate) pos: u64,
    pub(crate) reader: Box<dyn Read + Send + Sync>,
}

/// Where a ranged request actually landed, given what we asked for and the status
/// the server answered with.
///
/// A server is free to ignore `Range` and reply `200` with the whole body — some
/// podcast CDNs and ad-inserting proxies do. The bytes then start at 0 while the
/// request said otherwise, so believing the request would leave the source
/// reporting a position it is not at: the episode plays from the beginning while
/// the bar sits at the seek target, and every later seek compounds the error.
fn landed_pos(requested: u64, status: u16) -> u64 {
    // 206 Partial Content is the only promise that the range was honoured; a
    // request from byte 0 needs no promise.
    if requested == 0 || status == 206 {
        requested
    } else {
        0
    }
}

impl HttpRangeSource {
    /// (Re)open the body at byte `pos` via a ranged request, replacing the reader.
    /// Returns the byte the new reader actually starts at — not necessarily `pos`,
    /// see [`landed_pos`].
    fn open_at(&mut self, pos: u64) -> io::Result<u64> {
        let pos = pos.min(self.len);
        let resp = self
            .agent
            .get(&self.url)
            .header("Range", &format!("bytes={pos}-"))
            .call()
            .map_err(|e| io::Error::other(e.to_string()))?;
        let landed = landed_pos(pos, resp.status().as_u16());
        self.reader = Box::new(resp.into_body().into_reader());
        self.pos = landed;
        Ok(landed)
    }
}

impl Read for HttpRangeSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.reader.read(buf)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for HttpRangeSource {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let target = match from {
            SeekFrom::Start(n) => n,
            SeekFrom::End(n) => (self.len as i64 + n).max(0) as u64,
            SeekFrom::Current(n) => (self.pos as i64 + n).max(0) as u64,
        };
        if target != self.pos {
            self.open_at(target)?;
        }
        Ok(self.pos)
    }
}

impl MediaSource for HttpRangeSource {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

#[cfg(test)]
mod tests {
    use super::{landed_pos, parse_stream_title};

    /// Seeking a podcast episode is only honest if the server actually honoured
    /// the range. A `200` means the body restarted at 0 whatever we asked for.
    #[test]
    fn a_range_the_server_ignored_reports_the_real_position() {
        assert_eq!(landed_pos(5_000_000, 206), 5_000_000, "honoured");
        assert_eq!(
            landed_pos(5_000_000, 200),
            0,
            "ignored → the body starts at 0"
        );
        assert_eq!(
            landed_pos(0, 200),
            0,
            "no range asked for, nothing to honour"
        );
    }

    #[test]
    fn icy_stream_title_parsed() {
        let meta = b"StreamTitle='Miles Davis - So What';StreamUrl='http://x';\0\0";
        assert_eq!(
            parse_stream_title(meta).as_deref(),
            Some("Miles Davis - So What")
        );
        // empty title → None; missing field → None
        assert_eq!(parse_stream_title(b"StreamTitle='';"), None);
        assert_eq!(parse_stream_title(b"\0\0\0"), None);
    }
}
