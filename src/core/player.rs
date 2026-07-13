//! Logical player state: status, queue, modes. This is the *intent* layer —
//! the actual audio thread (`crate::audio`) mirrors it and reports progress
//! back. Keeping them separate means the UI stays smooth even if the audio
//! backend stalls.

use std::time::Duration;

use crate::core::model::TrackId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Stopped,
    Playing,
    Paused,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Off,
    One,
    All,
}

/// The active play queue plus listening history (for `Previous` + Recently
/// Played).
#[derive(Debug, Clone, Default)]
pub struct Queue {
    pub items: Vec<TrackId>,
    pub position: usize,
    pub history: Vec<TrackId>,
}

impl Queue {
    pub fn current(&self) -> Option<TrackId> {
        self.items.get(self.position).copied()
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[derive(Debug, Clone)]
pub struct PlayerState {
    pub status: Status,
    pub current: Option<TrackId>,
    pub elapsed: Duration,
    pub duration: Duration,
    pub volume: u8, // 0..=100
    pub speed: f32, // 0.5..=2.0
    pub shuffle: bool,
    pub repeat: Repeat,
    pub queue: Queue,
    /// Latest spectrum frame from the visualizer (filled by the audio thread).
    pub spectrum: Vec<f32>,
    /// Shuffle PRNG state (xorshift64); seeded lazily from the clock.
    rng: u64,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            status: Status::Stopped,
            current: None,
            elapsed: Duration::ZERO,
            duration: Duration::ZERO,
            volume: 72,
            speed: 1.0,
            shuffle: false,
            repeat: Repeat::Off,
            queue: Queue::default(),
            spectrum: Vec::new(),
            rng: 0,
        }
    }
}

impl PlayerState {
    pub fn stop(&mut self) {
        self.status = Status::Stopped;
        self.elapsed = Duration::ZERO;
    }

    pub fn next(&mut self) {
        // walk the queue sequentially, wrapping only under repeat-all. Shuffle
        // pre-reorders the upcoming tracks in `toggle_shuffle`, so the next position
        // is deterministic either way. `None` = nothing to advance to.
        let new_pos = if self.queue.position + 1 < self.queue.len() {
            Some(self.queue.position + 1)
        } else if self.repeat == Repeat::All && !self.queue.is_empty() {
            Some(0)
        } else {
            None
        };
        if let Some(p) = new_pos {
            self.push_history(); // remember where we were (drives Previous)
            self.queue.position = p;
            self.current = self.queue.current();
            self.elapsed = Duration::ZERO;
        }
    }

    /// Back button: replay the most recent still-queued history entry; if the
    /// history is empty (or all its tracks were removed), step back linearly.
    pub fn previous(&mut self) {
        while let Some(prev) = self.queue.history.pop() {
            if let Some(pos) = self.queue.items.iter().position(|&x| x == prev) {
                self.queue.position = pos;
                self.current = self.queue.current();
                self.elapsed = Duration::ZERO;
                return;
            }
        }
        self.queue.position = self.queue.position.saturating_sub(1);
        self.current = self.queue.current();
        self.elapsed = Duration::ZERO;
    }

    /// Record the current track as the most recent history entry (cap 500).
    pub fn push_history(&mut self) {
        if let Some(cur) = self.queue.current() {
            self.queue.history.push(cur);
            if self.queue.history.len() > 500 {
                self.queue.history.remove(0);
            }
        }
    }

    /// Shuffle the upcoming tracks in place (Fisher-Yates over the queue tail after
    /// the current track), so playback then walks the queue sequentially and the
    /// next track is deterministic — the same model the Spotify queue uses
    /// ([`crate::core::shuffle::shuffle_tail`]). The current track and the
    /// already-played prefix stay put.
    fn shuffle_upcoming(&mut self) {
        let start = self.queue.position + 1;
        // split the borrow: the RNG state and the queue items are disjoint fields.
        let Self { rng, queue, .. } = self;
        crate::core::shuffle::shuffle_tail(&mut queue.items, start, |k| xorshift_below(rng, k));
    }

    pub fn toggle_shuffle(&mut self) {
        self.shuffle = !self.shuffle;
        if self.shuffle {
            self.shuffle_upcoming(); // reorder the tail so "next" is deterministic
        }
    }

    pub fn cycle_repeat(&mut self) {
        self.repeat = match self.repeat {
            Repeat::Off => Repeat::One,
            Repeat::One => Repeat::All,
            Repeat::All => Repeat::Off,
        };
    }

    pub fn adjust_volume(&mut self, delta: i8) {
        let v = self.volume as i16 + delta as i16;
        self.volume = v.clamp(0, 100) as u8;
    }

    /// Progress as a 0.0..=1.0 fraction for the progress bar.
    pub fn progress(&self) -> f32 {
        if self.duration.is_zero() {
            0.0
        } else {
            (self.elapsed.as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0)
        }
    }
}

/// A pseudo-random index in `0..n` (xorshift64, lazily seeded from the clock).
/// Free-standing so a shuffle can borrow just the RNG state, not all of `self`.
fn xorshift_below(rng: &mut u64, n: usize) -> usize {
    if n == 0 {
        return 0; // guard the `% n` below against divide-by-zero
    }
    if *rng == 0 {
        *rng = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15)
            | 1;
    }
    let mut x = *rng;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *rng = x;
    (x % n as u64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queued(n: u32) -> PlayerState {
        let mut p = PlayerState::default();
        p.queue.items = (1..=n).map(TrackId::new).collect();
        p.queue.position = 0;
        p.current = p.queue.current();
        p
    }

    #[test]
    fn previous_is_a_back_button() {
        let mut p = queued(5);
        p.next(); // 0 → 1
        p.next(); // 1 → 2
        assert_eq!(p.queue.position, 2);
        p.previous(); // ← 1
        assert_eq!(p.queue.position, 1);
        p.previous(); // ← 0
        assert_eq!(p.queue.position, 0);
    }

    #[test]
    fn cycle_repeat_order() {
        let mut p = queued(1);
        assert_eq!(p.repeat, Repeat::Off);
        for expected in [Repeat::One, Repeat::All, Repeat::Off] {
            p.cycle_repeat();
            assert_eq!(p.repeat, expected);
        }
    }

    #[test]
    fn next_stops_at_end_without_repeat() {
        let mut p = queued(2);
        p.next(); // 0 → 1
        let hist = p.queue.history.len();
        p.next(); // at the end, repeat off → no advance, no spurious history
        assert_eq!(p.queue.position, 1);
        assert_eq!(p.queue.history.len(), hist);
    }

    #[test]
    fn repeat_all_wraps_and_records_history() {
        let mut p = queued(2);
        p.repeat = Repeat::All;
        p.next(); // 0 → 1
        p.next(); // 1 → wrap to 0
        assert_eq!(p.queue.position, 0);
        p.previous(); // ← 1 (the wrapped-from track)
        assert_eq!(p.queue.position, 1);
    }

    #[test]
    fn shuffle_permutes_upcoming_and_covers_queue_once() {
        let mut p = queued(8);
        let current = p.queue.items[p.queue.position];
        // toggling shuffle on pre-reorders the upcoming tail (the current track and
        // played prefix stay put), so a sequential pass still visits every track once.
        p.toggle_shuffle();
        assert!(p.shuffle, "shuffle is on");
        assert_eq!(
            p.queue.items[p.queue.position], current,
            "the current track stays put"
        );
        let mut visited = vec![p.queue.position];
        for _ in 0..7 {
            p.next();
            visited.push(p.queue.position);
        }
        visited.sort_unstable();
        visited.dedup();
        assert_eq!(visited.len(), 8, "a shuffle pass visits every track once");
    }

    #[test]
    fn previous_skips_history_entries_no_longer_queued() {
        let mut p = queued(5);
        p.next(); // 0 → 1 (history: T1)
        p.next(); // 1 → 2 (history: T1, T2)
        // drop T2 (the most recent history entry) from the queue
        p.queue.items.retain(|&id| id != TrackId::new(2));
        p.queue.position = p
            .queue
            .items
            .iter()
            .position(|&x| x == TrackId::new(3))
            .unwrap();
        p.previous(); // T2 gone → fall through to T1
        assert_eq!(p.current, Some(TrackId::new(1)));
    }
}
