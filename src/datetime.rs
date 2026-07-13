//! Tiny calendar helpers (unix seconds → civil date) with no external deps.
//! Used by the Forgotten / On This Day discovery lists so we don't pull in a
//! date crate for what amounts to a handful of integer ops.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds in a day.
pub const DAY: u64 = 86_400;

/// Current wall-clock time as unix seconds (0 if the clock predates the epoch).
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Convert unix seconds to a civil `(year, month, day)` in UTC. Howard
/// Hinnant's `civil_from_days` algorithm (proleptic Gregorian; exact).
pub fn ymd_from_unix(secs: u64) -> (i64, u32, u32) {
    let days = (secs / DAY) as i64; // days since 1970-01-01
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // day [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // month [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Inverse of [`ymd_from_unix`]: midnight UTC of a civil date as unix seconds
/// (clamped to 0 for pre-epoch dates). Howard Hinnant's `days_from_civil`.
/// The inverse pair; only exercised by tests today.
#[allow(dead_code)]
pub fn unix_from_ymd(y: i64, m: u32, d: u32) -> u64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400; // [0, 399]
    let (m, d) = (m as i64, d as i64);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    let days = era * 146_097 + doe - 719_468;
    (days * DAY as i64).max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_and_known_dates() {
        assert_eq!(ymd_from_unix(0), (1970, 1, 1));
        // 1700000000 = 2023-11-14 22:13:20 UTC
        assert_eq!(ymd_from_unix(1_700_000_000), (2023, 11, 14));
        // 951782400 = 2000-02-29 00:00:00 UTC (leap day)
        assert_eq!(ymd_from_unix(951_782_400), (2000, 2, 29));
        // one day later is March 1st
        assert_eq!(ymd_from_unix(951_782_400 + DAY), (2000, 3, 1));
    }

    #[test]
    fn ymd_roundtrips() {
        for &(y, m, d) in &[(1970, 1, 1), (2000, 2, 29), (2023, 11, 14), (2026, 6, 7)] {
            let secs = unix_from_ymd(y, m, d);
            assert_eq!(ymd_from_unix(secs), (y, m, d), "{y}-{m}-{d}");
        }
    }
}
