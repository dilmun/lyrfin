//! Library & listening insights, computed on demand from the in-memory library.
//!
//! Everything here is derived from data we already track (durations, play
//! counts, ratings, genres, years), so it needs no extra persistence. A
//! timestamped play-history log (for heatmaps / streaks) is a separate, later
//! step — see the roadmap M7.

use std::collections::HashMap;
use std::time::Duration;

use crate::core::model::Track;

/// A snapshot of aggregate stats over a set of tracks.
#[derive(Debug, Default, Clone)]
pub struct Stats {
    pub tracks: usize,
    pub artists: usize,
    pub albums: usize,
    pub total_time: Duration,
    /// Approx. time listened = Σ duration × play_count.
    pub played_time: Duration,
    pub total_plays: u64,
    pub favorites: usize,
    pub rated: usize,
    pub avg_rating: f32,
    pub lossless: usize,
    /// "Artist — Title", play_count — most played first.
    pub top_tracks: Vec<(String, u32)>,
    /// artist, summed play_count.
    pub top_artists: Vec<(String, u32)>,
    /// "Artist — Album", summed play_count.
    pub top_albums: Vec<(String, u32)>,
    /// genre, track count.
    pub top_genres: Vec<(String, usize)>,
    /// decade (e.g. 1990), track count — chronological.
    pub decades: Vec<(u16, usize)>,
}

/// Pick the top `n` of a (key, weight) map, weight-descending then key-asc for
/// stable ties.
fn top_n<W: Copy + Ord>(map: HashMap<String, W>, n: usize) -> Vec<(String, W)> {
    let mut v: Vec<(String, W)> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v.truncate(n);
    v
}

impl Stats {
    pub fn compute<'a>(tracks: impl Iterator<Item = &'a Track>) -> Stats {
        let mut s = Stats::default();
        let mut artists = std::collections::HashSet::new();
        let mut albums = std::collections::HashSet::new();
        let mut artist_plays: HashMap<String, u32> = HashMap::new();
        let mut album_plays: HashMap<String, u32> = HashMap::new();
        let mut genres: HashMap<String, usize> = HashMap::new();
        let mut decades: HashMap<u16, usize> = HashMap::new();
        let mut rating_sum: u64 = 0;
        let mut track_list: Vec<(String, u32)> = Vec::new();

        for t in tracks {
            s.tracks += 1;
            s.total_time += t.duration();
            s.played_time += t.duration() * t.play_count;
            s.total_plays += t.play_count as u64;
            if t.favorite {
                s.favorites += 1;
            }
            if t.rating > 0 {
                s.rated += 1;
                rating_sum += t.rating as u64;
            }
            if t.audio.map(|a| a.codec.is_lossless()).unwrap_or(false) {
                s.lossless += 1;
            }

            let artist = if t.album_artist.trim().is_empty() {
                t.artist.trim()
            } else {
                t.album_artist.trim()
            };
            if !artist.is_empty() {
                artists.insert(artist.to_lowercase());
                *artist_plays.entry(artist.to_string()).or_default() += t.play_count;
            }
            if !t.album.trim().is_empty() {
                let key = format!(
                    "{}\u{0}{}",
                    artist.to_lowercase(),
                    t.album.trim().to_lowercase()
                );
                albums.insert(key);
                let label = format!("{} — {}", artist, t.album.trim());
                *album_plays.entry(label).or_default() += t.play_count;
            }
            if let Some(g) = t.genre.as_deref().map(str::trim).filter(|g| !g.is_empty()) {
                *genres.entry(g.to_string()).or_default() += 1;
            }
            if let Some(y) = t.year.filter(|y| *y > 0) {
                *decades.entry(y / 10 * 10).or_default() += 1;
            }
            if t.play_count > 0 {
                let who = if t.artist.trim().is_empty() {
                    "—"
                } else {
                    t.artist.trim()
                };
                track_list.push((format!("{} — {}", who, t.title), t.play_count));
            }
        }

        s.artists = artists.len();
        s.albums = albums.len();
        s.avg_rating = if s.rated > 0 {
            rating_sum as f32 / s.rated as f32
        } else {
            0.0
        };
        track_list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        track_list.truncate(5);
        s.top_tracks = track_list;
        s.top_artists = top_n(
            artist_plays.into_iter().filter(|(_, p)| *p > 0).collect(),
            5,
        );
        s.top_albums = top_n(album_plays.into_iter().filter(|(_, p)| *p > 0).collect(), 5);
        s.top_genres = top_n(genres, 5);
        let mut dv: Vec<(u16, usize)> = decades.into_iter().collect();
        dv.sort_by_key(|(d, _)| *d);
        s.decades = dv;
        s
    }
}

const DAY: u64 = 86_400;

/// Aggregates over the timestamped listening history (`play_history`).
#[derive(Debug, Default, Clone)]
pub struct History {
    pub total: usize,
    /// Plays per weekday, Mon..Sun (UTC).
    pub by_weekday: [usize; 7],
    /// Plays per hour of day 0..23 (UTC).
    pub by_hour: [usize; 24],
    pub days_active: usize,
    pub current_streak: u32,
    pub longest_streak: u32,
    pub plays_7d: usize,
    pub plays_30d: usize,
}

impl History {
    pub fn compute(plays: &[u64], now: u64) -> History {
        let mut h = History {
            total: plays.len(),
            ..Default::default()
        };
        let mut days = std::collections::BTreeSet::new();
        for &ts in plays {
            let day = ts / DAY;
            // 1970-01-01 (day 0) was a Thursday → +3 puts Monday at 0
            h.by_weekday[((day + 3) % 7) as usize] += 1;
            h.by_hour[((ts % DAY) / 3600).min(23) as usize] += 1;
            days.insert(day);
            if ts + 7 * DAY >= now {
                h.plays_7d += 1;
            }
            if ts + 30 * DAY >= now {
                h.plays_30d += 1;
            }
        }
        h.days_active = days.len();

        // longest run of consecutive days anywhere in the history
        let mut prev: Option<u64> = None;
        let mut run = 0u32;
        for &d in &days {
            // checked_sub guards against a corrupt history.json injecting day 0
            run = if prev == d.checked_sub(1) { run + 1 } else { 1 };
            h.longest_streak = h.longest_streak.max(run);
            prev = Some(d);
        }
        // current streak: consecutive days ending today (or yesterday, grace)
        let today = now / DAY;
        let mut anchor = if days.contains(&today) {
            Some(today)
        } else if days.contains(&today.wrapping_sub(1)) {
            Some(today - 1)
        } else {
            None
        };
        while let Some(d) = anchor {
            h.current_streak += 1;
            anchor = if d > 0 && days.contains(&(d - 1)) {
                Some(d - 1)
            } else {
                None
            };
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{Track, TrackId};
    use std::path::PathBuf;

    const DAY: u64 = 86_400;

    #[test]
    fn history_streaks_and_buckets() {
        let now = 100 * DAY + 12 * 3600; // day 100, noon UTC
        // plays today, yesterday, day-before → current streak 3; plus an old day
        let plays = vec![
            100 * DAY + 9 * 3600,
            99 * DAY + 20 * 3600,
            98 * DAY + 8 * 3600,
            50 * DAY, // isolated old day
        ];
        let h = History::compute(&plays, now);
        assert_eq!(h.total, 4);
        assert_eq!(h.days_active, 4);
        assert_eq!(h.current_streak, 3, "today+yesterday+day-before");
        assert_eq!(h.longest_streak, 3);
        assert_eq!(h.plays_7d, 3, "the day-50 play is outside 7d");
        assert_eq!(h.by_hour[9], 1);
        assert_eq!(h.by_hour[8], 1);
    }

    #[test]
    fn history_current_streak_breaks_with_gap() {
        let now = 100 * DAY;
        // most recent play was 5 days ago → no current streak
        let h = History::compute(&[95 * DAY, 94 * DAY], now);
        assert_eq!(h.current_streak, 0);
        assert_eq!(h.longest_streak, 2);
    }

    fn track(title: &str, artist: &str, album: &str, plays: u32, year: u16) -> Track {
        Track {
            id: TrackId::new(1),
            path: PathBuf::new(),
            title: title.into(),
            artist: artist.into(),
            album: album.into(),
            album_artist: artist.into(),
            album_id: None,
            artist_id: None,
            track_no: 0,
            disc_no: 0,
            track_total: 0,
            disc_total: 0,
            duration_ms: 200_000,
            year: Some(year),
            genre: Some("Rock".into()),
            composer: String::new(),
            comment: String::new(),
            audio: None,
            rating: 4,
            favorite: plays > 5,
            play_count: plays,
            added_at: 0,
            last_played: 0,
        }
    }

    #[test]
    fn computes_totals_and_tops() {
        let tracks = [
            track("A", "Adele", "30", 10, 2021),
            track("B", "Adele", "30", 3, 2021),
            track("C", "Muse", "Drones", 8, 2015),
        ];
        let s = Stats::compute(tracks.iter());
        assert_eq!(s.tracks, 3);
        assert_eq!(s.artists, 2); // Adele, Muse
        assert_eq!(s.albums, 2); // 30, Drones
        assert_eq!(s.total_plays, 21);
        // top artist by summed plays = Adele (13) over Muse (8)
        assert_eq!(s.top_artists[0], ("Adele".to_string(), 13));
        // top track = "Adele — A" (10 plays)
        assert_eq!(s.top_tracks[0].1, 10);
        // decades: 2010 (1) then 2020 (2), chronological
        assert_eq!(s.decades, vec![(2010, 1), (2020, 2)]);
        assert!((s.avg_rating - 4.0).abs() < 0.01);
    }

    #[test]
    fn empty_library_is_safe() {
        let s = Stats::compute([].iter());
        assert_eq!(s.tracks, 0);
        assert_eq!(s.avg_rating, 0.0);
        assert!(s.top_tracks.is_empty());
    }
}
