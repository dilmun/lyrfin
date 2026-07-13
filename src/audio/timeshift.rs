//! Source-agnostic timeshift / DVR buffer for **live** streams.
//!
//! A live stream (internet radio, and any future live source) is a forward-only
//! firehose: the past isn't stored server-side and the future doesn't exist yet.
//! This layer keeps a sliding window of the *compressed* bytes we've already
//! received (not decoded PCM — far cheaper) in a fixed-capacity ring, so playback
//! can be paused, rewound anywhere in the window, and caught back up to the live
//! edge, while a background producer keeps the buffer filling.
//!
//! It's deliberately generic — the producer just writes bytes and the consumer is
//! a plain [`MediaSource`] reading at a movable cursor — so it isn't radio-specific
//! and future live source types can reuse it unchanged.
//!
//! ## Model
//! Absolute byte offsets from the first byte received (`0`). The ring retains a
//! sliding window `[tail, head)`; `read` is the decoder's cursor, always kept
//! inside it. Bytes ↔ seconds via a measured `byte_rate` (constant-bitrate — true
//! for essentially all live radio). The producer, consumer, and status readers
//! coordinate through one small mutex plus a condvar (the consumer waits on it when
//! it reaches the live edge with nothing buffered ahead).

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use symphonia::core::io::MediaSource;

/// Nominal compressed byte-rate (≈192 kbps) used to size the ring and map time
/// before the real rate is measured from the first decoded packet. Sizing the ring
/// by this means the retained *duration* flexes with the real bitrate (a 128 kbps
/// stream buffers longer than requested; a 320 kbps one, shorter) while memory
/// stays fixed.
const NOMINAL_BYTE_RATE: u64 = 24_000;

/// How long a consumer blocks waiting for the producer before re-checking shutdown,
/// so a torn-down stream never wedges the controller thread.
const WAIT_SLICE: Duration = Duration::from_millis(150);

/// Fixed-capacity byte ring holding the most-recent `cap` bytes of a live stream.
/// Pure data structure — no threading, no I/O — so the window/seek math is unit-
/// testable in isolation. `head`/`tail`/`read` are absolute, monotonic byte offsets.
struct Ring {
    buf: Box<[u8]>,
    cap: u64,
    /// Total bytes ever written — the exclusive end (live edge) of the window.
    head: u64,
    /// Oldest retained byte — the inclusive start of the window.
    tail: u64,
    /// Decoder read cursor, always kept within `[tail, head]`.
    read: u64,
    /// The producer finished or errored — no more bytes will ever arrive.
    eof: bool,
}

impl Ring {
    fn new(cap: usize) -> Self {
        let cap = cap.max(1);
        Self {
            buf: vec![0u8; cap].into_boxed_slice(),
            cap: cap as u64,
            head: 0,
            tail: 0,
            read: 0,
            eof: false,
        }
    }

    /// Append `src`, dropping the oldest bytes once the window would exceed `cap`.
    /// If that drop passes the read cursor (the decoder fell more than a full window
    /// behind live), the cursor is pulled forward to the new tail — that rewind
    /// position has expired. O(n) via at most two `copy_from_slice`s (ring wrap).
    fn write(&mut self, src: &[u8]) {
        // A single write larger than the whole ring can only keep its last `cap`.
        let src = if src.len() as u64 > self.cap {
            &src[src.len() - self.cap as usize..]
        } else {
            src
        };
        let start = (self.head % self.cap) as usize;
        let first = src.len().min(self.cap as usize - start);
        self.buf[start..start + first].copy_from_slice(&src[..first]);
        if src.len() > first {
            self.buf[..src.len() - first].copy_from_slice(&src[first..]);
        }
        self.head += src.len() as u64;
        if self.head - self.tail > self.cap {
            self.tail = self.head - self.cap;
            self.read = self.read.max(self.tail);
        }
    }

    /// Copy up to `dst.len()` buffered bytes from the read cursor forward and
    /// advance it; returns the count (0 when the cursor sits at the live edge —
    /// nothing buffered ahead yet).
    fn read(&mut self, dst: &mut [u8]) -> usize {
        let avail = (self.head - self.read).min(dst.len() as u64) as usize;
        if avail == 0 {
            return 0;
        }
        let start = (self.read % self.cap) as usize;
        let first = avail.min(self.cap as usize - start);
        dst[..first].copy_from_slice(&self.buf[start..start + first]);
        if avail > first {
            dst[first..avail].copy_from_slice(&self.buf[..avail - first]);
        }
        self.read += avail as u64;
        avail
    }

    /// Move the read cursor to `pos`, clamped into the retained window.
    fn seek(&mut self, pos: u64) -> u64 {
        self.read = pos.clamp(self.tail, self.head);
        self.read
    }
}

/// A snapshot of the timeshift window, in seconds, for the UI/engine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    /// Oldest seekable position (seconds since tune-in).
    pub start: f64,
    /// Decoder read cursor (seconds since tune-in) — the seek/decode position, a
    /// little ahead of what's actually heard.
    pub read: f64,
    /// Live edge (seconds since tune-in): the newest buffered audio.
    pub live: f64,
}

/// Shared timeshift buffer: the producer writes, the consumer (a [`MediaSource`])
/// reads at a movable cursor, and the engine/UI query the window. Cheap to size:
/// one `cap`-byte allocation. Cloneable handle via [`std::sync::Arc`].
pub struct Timeshift {
    inner: Mutex<Ring>,
    /// Signalled on every write and on EOF so a consumer parked at the live edge
    /// wakes promptly.
    data: Condvar,
    /// Compressed bytes per second (CBR assumption), for byte ↔ time mapping.
    byte_rate: Mutex<u64>,
    /// The engine asked the producer to stop (stream torn down / re-tuned).
    shutdown: AtomicBool,
}

impl Timeshift {
    /// A timeshift buffer retaining `window` of audio at the nominal bitrate (memory
    /// = `window` × [`NOMINAL_BYTE_RATE`], fixed). `window` of zero is treated as a
    /// minimal 1-second buffer so the ring is always valid.
    pub fn new(window: Duration) -> Self {
        let cap = (window.as_secs().max(1) * NOMINAL_BYTE_RATE) as usize;
        Self {
            inner: Mutex::new(Ring::new(cap)),
            data: Condvar::new(),
            byte_rate: Mutex::new(NOMINAL_BYTE_RATE),
            shutdown: AtomicBool::new(false),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Ring> {
        // The ring is always in a consistent state (plain bytes + offsets), so a
        // producer/consumer panic can't corrupt it — recover through poisoning
        // rather than propagating a panic onto the audio thread.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn rate(&self) -> u64 {
        (*self.byte_rate.lock().unwrap_or_else(|e| e.into_inner())).max(1)
    }

    /// Record the real compressed byte-rate once the decoder has measured it, so the
    /// window's seconds line up with actual playback. Ignored if not positive.
    pub fn set_byte_rate(&self, bytes_per_sec: u64) {
        if bytes_per_sec > 0 {
            *self.byte_rate.lock().unwrap_or_else(|e| e.into_inner()) = bytes_per_sec;
        }
    }

    /// Producer: append received (already ICY-stripped) audio bytes.
    pub fn write(&self, bytes: &[u8]) {
        self.lock().write(bytes);
        self.data.notify_all();
    }

    /// Producer: no more bytes will arrive (network EOF/error). Wakes any waiter.
    pub fn set_eof(&self) {
        self.lock().eof = true;
        self.data.notify_all();
    }

    /// Ask the producer to stop and release any parked consumer.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        self.data.notify_all();
    }

    fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }

    /// Move the read cursor to `secs` (seconds since tune-in), clamped to the
    /// retained window; returns the landed position in seconds. The decoder is then
    /// (re)opened at this cursor — see the engine's DVR seek.
    pub fn seek_secs(&self, secs: f64) -> f64 {
        let rate = self.rate();
        let pos = (secs.max(0.0) * rate as f64) as u64;
        let landed = self.lock().seek(pos);
        landed as f64 / rate as f64
    }

    /// The current window in seconds (`start` ≤ `read` ≤ `live`).
    pub fn window(&self) -> Window {
        let rate = self.rate() as f64;
        let ring = self.lock();
        Window {
            start: ring.tail as f64 / rate,
            read: ring.read as f64 / rate,
            live: ring.head as f64 / rate,
        }
    }
}

/// The consumer side: a seekable [`MediaSource`] over a [`Timeshift`]. symphonia
/// reads it sequentially and seeks it by byte (mapping time→byte from the codec's
/// running bitrate), so the existing decode/seek path works unchanged — the source
/// just serves from, and moves within, the retained window.
pub struct TimeshiftSource {
    ts: std::sync::Arc<Timeshift>,
}

impl TimeshiftSource {
    pub fn new(ts: std::sync::Arc<Timeshift>) -> Self {
        Self { ts }
    }
}

impl Read for TimeshiftSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut ring = self.ts.lock();
        loop {
            let n = ring.read(buf);
            if n > 0 {
                return Ok(n);
            }
            // At the live edge with nothing buffered ahead: EOF is a real end of
            // stream; otherwise park until the producer writes (or we're torn down),
            // re-checking on a slice so shutdown is always honoured.
            if ring.eof || self.ts.is_shutdown() {
                return Ok(0);
            }
            ring = self
                .ts
                .data
                .wait_timeout(ring, WAIT_SLICE)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

impl Seek for TimeshiftSource {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let mut ring = self.ts.lock();
        let target = match from {
            SeekFrom::Start(n) => n,
            SeekFrom::Current(n) => (ring.read as i64 + n).max(0) as u64,
            SeekFrom::End(n) => (ring.head as i64 + n).max(0) as u64,
        };
        Ok(ring.seek(target))
    }
}

impl MediaSource for TimeshiftSource {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        None // live: unbounded, no fixed length
    }
}

/// Background producer: read clean (already ICY-stripped) audio bytes from `reader`
/// into `ts` until EOF/error or shutdown. Its own thread, so the buffer keeps
/// filling to the live edge even while playback is paused or rewound.
pub fn spawn_producer(ts: std::sync::Arc<Timeshift>, mut reader: Box<dyn Read + Send>) {
    let _ = std::thread::Builder::new()
        .name("lyrfin-timeshift".into())
        .spawn(move || {
            let mut chunk = vec![0u8; 16 * 1024];
            while !ts.is_shutdown() {
                match reader.read(&mut chunk) {
                    Ok(0) => break, // network EOF
                    Ok(n) => ts.write(&chunk[..n]),
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break, // network error → stop; consumer surfaces it
                }
            }
            ts.set_eof();
        });
    // If the thread can't spawn, the stream simply never buffers; the consumer's
    // first read returns EOF and the engine recovers — no panic on the audio path.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_writes_and_reads_back_in_order() {
        let mut r = Ring::new(16);
        r.write(&[1, 2, 3, 4]);
        let mut dst = [0u8; 4];
        assert_eq!(r.read(&mut dst), 4);
        assert_eq!(dst, [1, 2, 3, 4]);
        // caught up to the live edge → nothing more to read
        assert_eq!(r.read(&mut dst), 0);
    }

    #[test]
    fn ring_wraps_across_the_capacity_boundary() {
        let mut r = Ring::new(8);
        r.write(&[1, 2, 3, 4, 5, 6]); // read some, then write past the wrap
        let mut a = [0u8; 4];
        assert_eq!(r.read(&mut a), 4);
        assert_eq!(a, [1, 2, 3, 4]);
        r.write(&[7, 8, 9, 10]); // head now wraps; window still ≤ cap
        let mut b = [0u8; 6];
        let n = r.read(&mut b);
        assert_eq!(&b[..n], &[5, 6, 7, 8, 9, 10]);
    }

    #[test]
    fn ring_drops_oldest_and_pulls_a_stale_read_cursor_forward() {
        let mut r = Ring::new(4);
        r.write(&[1, 2, 3, 4]); // window [0,4), read at 0
        r.write(&[5, 6]); // drops 1,2 → window [2,6); read was 0 < tail 2 → read=2
        assert_eq!(r.tail, 2);
        assert_eq!(r.read, 2);
        let mut dst = [0u8; 4];
        let n = r.read(&mut dst);
        assert_eq!(&dst[..n], &[3, 4, 5, 6], "reads resume from the new tail");
    }

    #[test]
    fn ring_seek_clamps_into_the_window() {
        let mut r = Ring::new(8);
        r.write(&[0, 1, 2, 3, 4, 5]); // window [0,6)
        assert_eq!(r.seek(3), 3); // inside → exact
        assert_eq!(r.seek(99), 6); // past live edge → clamped to head
        r.write(&[6, 7, 8, 9]); // window now [2,10)
        assert_eq!(
            r.seek(0),
            2,
            "before the tail → clamped to the oldest retained"
        );
    }

    #[test]
    fn ring_stays_bounded_under_heavy_writes() {
        // Write ~130× the capacity in odd-sized chunks (forcing every wrap case).
        // Memory must never grow and the window must never exceed `cap` — proving
        // the buffer is bounded and can't overflow.
        let mut r = Ring::new(1024);
        let chunk = vec![7u8; 333];
        for _ in 0..400 {
            r.write(&chunk);
            assert!(r.head - r.tail <= r.cap, "window never exceeds capacity");
            assert_eq!(
                r.buf.len() as u64,
                r.cap,
                "the ring never reallocates/grows"
            );
            assert!(
                r.read >= r.tail && r.read <= r.head,
                "cursor stays in the window"
            );
        }
        // reads are bounded to what's retained — never over-read the backing buffer
        let mut dst = vec![0u8; 4096];
        let n = r.read(&mut dst);
        assert!(n as u64 <= r.cap);
    }

    #[test]
    fn write_larger_than_capacity_keeps_only_the_newest() {
        let mut r = Ring::new(4);
        r.write(&[1, 2, 3, 4, 5, 6, 7]); // only the last 4 survive
        let mut dst = [0u8; 8];
        let n = r.read(&mut dst);
        assert_eq!(&dst[..n], &[4, 5, 6, 7]);
    }

    #[test]
    fn timeshift_window_maps_bytes_to_seconds() {
        let ts = Timeshift::new(Duration::from_secs(60));
        ts.set_byte_rate(1000); // 1 KB/s → 1 s per 1000 bytes
        ts.write(&vec![0u8; 5000]); // 5 s of "audio"
        let w = ts.window();
        assert_eq!(w.start, 0.0);
        assert_eq!(w.live, 5.0);
        assert_eq!(w.read, 0.0);
        // seek to 3 s → read cursor at 3000 bytes
        assert_eq!(ts.seek_secs(3.0), 3.0);
        assert_eq!(ts.window().read, 3.0);
        // seeking past the edge clamps to live; before the start clamps to start
        assert_eq!(ts.seek_secs(99.0), 5.0);
        assert_eq!(ts.seek_secs(-5.0), 0.0);
    }

    #[test]
    fn source_reads_buffered_bytes_and_reports_eof() {
        let ts = std::sync::Arc::new(Timeshift::new(Duration::from_secs(1)));
        ts.write(&[10, 20, 30]);
        ts.set_eof();
        let mut src = TimeshiftSource::new(ts);
        let mut buf = [0u8; 8];
        assert_eq!(src.read(&mut buf).unwrap(), 3);
        assert_eq!(&buf[..3], &[10, 20, 30]);
        // drained + EOF → a real end of stream (Ok(0)), not a block
        assert_eq!(src.read(&mut buf).unwrap(), 0);
    }
}
