//! Time helpers without pulling in chrono.
//!
//! Produces RFC 3339 UTC strings ("2026-08-04T20:42:00Z") from
//! `SystemTime` only. No allocation beyond the returned `String`.

use std::time::{SystemTime, UNIX_EPOCH};

/// Current wall-clock time as an RFC 3339 UTC string.
pub fn now_rfc3339() -> String {
    rfc3339_from_unix(now_unix())
}

/// Current Unix time in seconds.
pub fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Parse an RFC 3339 UTC string (`YYYY-MM-DDTHH:MM:SSZ`) to Unix seconds.
/// Returns `None` for anything unparseable; callers must tolerate it.
pub fn parse_rfc3339_unix(s: &str) -> Option<u64> {
    let y = s.get(0..4)?.parse::<i64>().ok()?;
    let mo = s.get(5..7)?.parse::<u32>().ok()?;
    let d = s.get(8..10)?.parse::<u32>().ok()?;
    let h: u64 = s.get(11..13)?.parse().ok()?;
    let mi: u64 = s.get(14..16)?.parse().ok()?;
    let se: u64 = s.get(17..19)?.parse().ok()?;
    let days = days_from_civil(y, mo, d);
    Some(days as u64 * 86_400 + h * 3600 + mi * 60 + se)
}

/// Inverse of `civil_from_days`: proleptic Gregorian (y,m,d) → days since epoch.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let m = m as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + (d as i64 - 1);
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Format a Unix timestamp (seconds since epoch, UTC) as RFC 3339.
pub fn rfc3339_from_unix(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hour = rem / 3600;
    let min = (rem % 3600) / 60;
    let sec = rem % 60;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Howard Hinnant's `civil_from_days` — converts days since the Unix epoch
/// (1970-01-01) to a proleptic Gregorian (year, month, day).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m as u32, d as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_epoch_formats_correctly() {
        assert_eq!(rfc3339_from_unix(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn known_timestamp() {
        // 2026-08-04T20:42:00Z
        assert_eq!(rfc3339_from_unix(1_785_876_120), "2026-08-04T20:42:00Z");
    }

    #[test]
    fn handles_month_and_year_boundaries() {
        // 1999-12-31T23:59:59Z
        assert_eq!(rfc3339_from_unix(946_684_799), "1999-12-31T23:59:59Z");
        // 2000-01-01T00:00:00Z (leap-year boundary, Y2K)
        assert_eq!(rfc3339_from_unix(946_684_800), "2000-01-01T00:00:00Z");
    }

    #[test]
    fn is_lexicographically_sortable() {
        // Same-length RFC 3339 strings sort chronologically.
        assert!(rfc3339_from_unix(1_000) < rfc3339_from_unix(2_000));
    }

    #[test]
    fn parse_roundtrips() {
        assert_eq!(
            parse_rfc3339_unix("2026-08-04T20:42:00Z"),
            Some(1_785_876_120)
        );
        assert_eq!(parse_rfc3339_unix("1970-01-01T00:00:00Z"), Some(0));
    }

    #[test]
    fn parse_rejects_garbage() {
        assert_eq!(parse_rfc3339_unix("nonsense"), None);
        assert_eq!(parse_rfc3339_unix(""), None);
    }
}
