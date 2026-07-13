//! ReplayGain: read a file's normalization tags and turn them into a linear
//! playback multiplier. The audio engine reads this once per track (via
//! `app::effects`) and folds the factor into the output volume scalar; no audio
//! is touched here, only tag parsing + gain math.

use std::path::Path;

use lofty::prelude::*;

/// ReplayGain values read from a file's tags (gains in dB, peaks as a 0..~1
/// linear sample amplitude). All optional — most files have none.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ReplayGain {
    pub track_gain_db: Option<f32>,
    pub album_gain_db: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_peak: Option<f32>,
}

impl ReplayGain {
    /// Linear playback multiplier for `mode` (1 = track, 2 = album; else 1.0),
    /// with `preamp_db` added. Limited by the peak so `gain * peak <= 1.0` (no
    /// clipping), and capped for safety. Returns 1.0 when the needed tag is
    /// missing — i.e. normalization is a no-op rather than a surprise.
    pub fn gain_factor(&self, mode: u8, preamp_db: f32) -> f32 {
        let (gain_db, peak) = match mode {
            1 => (self.track_gain_db, self.track_peak),
            2 => (
                self.album_gain_db.or(self.track_gain_db),
                self.album_peak.or(self.track_peak),
            ),
            _ => return 1.0,
        };
        let Some(g) = gain_db else { return 1.0 };
        let mut factor = 10f32.powf((g + preamp_db) / 20.0);
        if let Some(pk) = peak
            && pk > 0.0
            && factor * pk > 1.0
        {
            factor = 1.0 / pk; // clamp to the loudest sample → no clipping
        }
        factor.clamp(0.0, 4.0)
    }
}

/// Parse a ReplayGain dB string like `"-6.48 dB"` or `"+3.2"` → `-6.48`/`3.2`.
fn parse_db(s: &str) -> Option<f32> {
    s.split_whitespace().next()?.parse::<f32>().ok()
}

/// Read ReplayGain tags from `path` (best-effort; missing → `None` fields).
pub fn read_replaygain(path: &Path) -> ReplayGain {
    let Ok(tagged) = lofty::read_from_path(path) else {
        return ReplayGain::default();
    };
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return ReplayGain::default();
    };
    let db = |k: ItemKey| tag.get_string(k).and_then(parse_db);
    let pk = |k: ItemKey| {
        tag.get_string(k)
            .and_then(|s| s.split_whitespace().next()?.parse::<f32>().ok())
    };
    ReplayGain {
        track_gain_db: db(ItemKey::ReplayGainTrackGain),
        album_gain_db: db(ItemKey::ReplayGainAlbumGain),
        track_peak: pk(ItemKey::ReplayGainTrackPeak),
        album_peak: pk(ItemKey::ReplayGainAlbumPeak),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaygain_db_parsing() {
        assert_eq!(parse_db("-6.48 dB"), Some(-6.48));
        assert_eq!(parse_db("+3.2"), Some(3.2));
        assert_eq!(parse_db("0"), Some(0.0));
        assert_eq!(parse_db("loud"), None);
    }

    #[test]
    fn replaygain_gain_factor() {
        let rg = ReplayGain {
            track_gain_db: Some(-6.0),
            album_gain_db: None,
            track_peak: None,
            album_peak: None,
        };
        // off → unity
        assert_eq!(rg.gain_factor(0, 0.0), 1.0);
        // -6 dB ≈ 0.501x
        assert!((rg.gain_factor(1, 0.0) - 0.5012).abs() < 0.001);
        // album mode falls back to track gain when album is absent
        assert!((rg.gain_factor(2, 0.0) - 0.5012).abs() < 0.001);
        // missing gain → unity
        assert_eq!(ReplayGain::default().gain_factor(1, 0.0), 1.0);
    }

    #[test]
    fn replaygain_peak_limits_to_avoid_clipping() {
        let rg = ReplayGain {
            track_gain_db: Some(6.0), // +6 dB ≈ 2.0x
            album_gain_db: None,
            track_peak: Some(0.8),
            album_peak: None,
        };
        // 2.0 * 0.8 = 1.6 would clip → clamp to 1/0.8 = 1.25
        assert!((rg.gain_factor(1, 0.0) - 1.25).abs() < 0.001);
    }
}
