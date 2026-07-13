//! Fisher-Yates tail shuffle shared by the local and Spotify play queues, so both
//! shuffle the *upcoming* tracks identically — the already-played prefix and the
//! current track stay put, and playback then walks the queue sequentially. Keeping
//! this in one place is what makes "shuffle" behave the same across sources (and
//! leaves each queue's next track deterministic, so the "▶ Next:" hint can show it).

/// Shuffle `items[start..]` in place with an unbiased Fisher-Yates pass, leaving
/// `items[..start]` untouched. `rand_below(k)` must return a uniformly random index
/// in `0..k`; each source supplies its own RNG (the local xorshift, Spotify's OS
/// entropy). A `start` at or past the end is a no-op.
pub fn shuffle_tail<T>(items: &mut [T], start: usize, mut rand_below: impl FnMut(usize) -> usize) {
    let n = items.len();
    if start >= n {
        return;
    }
    // classic Fisher-Yates over [start, n): for each slot from the end down to
    // start+1, swap it with a uniformly-chosen slot in [start, i].
    for i in (start + 1..n).rev() {
        let j = start + rand_below(i - start + 1);
        items.swap(i, j);
    }
}

#[cfg(test)]
mod tests {
    use super::shuffle_tail;

    #[test]
    fn prefix_is_untouched_and_tail_is_a_permutation() {
        let mut v: Vec<u32> = (0..10).collect();
        // a fixed pseudo-random source keeps the test deterministic
        let mut state = 0x1234_5678u64;
        let mut rng = |k: usize| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % k as u64) as usize
        };
        shuffle_tail(&mut v, 3, &mut rng);
        assert_eq!(&v[..3], &[0, 1, 2], "the played+current prefix stays put");
        let mut tail = v[3..].to_vec();
        tail.sort_unstable();
        assert_eq!(
            tail,
            (3..10).collect::<Vec<_>>(),
            "the tail is a permutation"
        );
    }

    #[test]
    fn start_at_or_past_end_is_a_noop() {
        let mut v = vec![1, 2, 3];
        shuffle_tail(&mut v, 3, |_| 0);
        assert_eq!(v, vec![1, 2, 3], "nothing upcoming → unchanged");
        shuffle_tail(&mut v, 9, |_| 0);
        assert_eq!(v, vec![1, 2, 3], "out-of-range start → unchanged");
    }
}
