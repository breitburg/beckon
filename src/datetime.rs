// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 breitburg

//! Minimal UTC date/time helpers shared by the calendar and mail toolsets.
//!
//! All conversions are exact integer math (Howard Hinnant's civil-from-days
//! algorithms) — no `chrono`/`time` dependency, no floating point, no
//! leap-second handling (Unix time ignores them). Everything is UTC.

use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// (year, month, day) for `z` days since 1970-01-01 (proleptic Gregorian).
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Days since 1970-01-01 for a civil (year, month, day).
pub(crate) fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mi = m as i64;
    let doy = (153 * (if mi > 2 { mi - 3 } else { mi + 9 }) + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `secs` (UTC) → EDS `make-time` string `YYYYMMDDTHHMMSSZ`.
pub(crate) fn epoch_to_make_time(secs: i64) -> String {
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}{m:02}{d:02}T{h:02}{mi:02}{s:02}Z")
}

/// `secs` (UTC) → `YYYYMMDD` for all-day `VALUE=DATE` values.
pub(crate) fn epoch_to_date(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}{m:02}{d:02}")
}

/// `secs` (UTC) → human-readable `YYYY-MM-DD HH:MM` for display.
pub(crate) fn format_epoch(secs: i64) -> String {
    let rem = secs.rem_euclid(86_400);
    let (h, mi) = (rem / 3600, (rem % 3600) / 60);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}")
}

/// Parse an ISO-8601-ish string into (UTC epoch seconds, had_time_component).
/// Accepts `YYYY-MM-DD` and `YYYY-MM-DD[T| ]HH:MM[:SS][Z]`; naive input is UTC.
pub(crate) fn parse_iso(input: &str) -> Result<(i64, bool), String> {
    let s = input.trim().trim_end_matches('Z');
    let (date, time) = match s.split_once(['T', ' ']) {
        Some((d, t)) => (d, Some(t)),
        None => (s, None),
    };
    let parts: Vec<&str> = date.split('-').collect();
    if parts.len() != 3 {
        return Err(format!("invalid date \"{input}\" (expected YYYY-MM-DD)"));
    }
    let y: i64 = parts[0]
        .parse()
        .map_err(|_| format!("invalid year in \"{input}\""))?;
    let mo: u32 = parts[1]
        .parse()
        .map_err(|_| format!("invalid month in \"{input}\""))?;
    let d: u32 = parts[2]
        .parse()
        .map_err(|_| format!("invalid day in \"{input}\""))?;
    let (mut h, mut mi, mut se) = (0i64, 0i64, 0i64);
    let has_time = time.is_some();
    if let Some(t) = time {
        let tp: Vec<&str> = t.split(':').collect();
        h = tp.first().and_then(|v| v.parse().ok()).unwrap_or(0);
        mi = tp.get(1).and_then(|v| v.parse().ok()).unwrap_or(0);
        se = tp.get(2).and_then(|v| v.parse().ok()).unwrap_or(0);
    }
    Ok((days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se, has_time))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates_round_trip() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        for &(y, m, d) in &[(1970, 1, 1), (2000, 2, 29), (2026, 6, 13), (2027, 1, 1)] {
            assert_eq!(civil_from_days(days_from_civil(y, m, d)), (y, m, d));
        }
    }

    #[test]
    fn make_time_formats_utc() {
        let (secs, _) = parse_iso("2026-06-13T14:30:00Z").unwrap();
        assert_eq!(epoch_to_make_time(secs), "20260613T143000Z");
        assert_eq!(epoch_to_date(secs), "20260613");
        assert_eq!(format_epoch(secs), "2026-06-13 14:30");
    }

    #[test]
    fn parse_iso_date_and_time() {
        let (_, has_time) = parse_iso("2026-06-13").unwrap();
        assert!(!has_time);
        let (secs, has_time) = parse_iso("2026-06-13T09:05").unwrap();
        assert!(has_time);
        assert_eq!(secs - days_from_civil(2026, 6, 13) * 86_400, 9 * 3600 + 5 * 60);
        assert!(parse_iso("not-a-date").is_err());
    }
}
