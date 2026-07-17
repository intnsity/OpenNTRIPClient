//! Civil time without a date/time dependency.
//!
//! UTC comes from `SystemTime` plus Howard Hinnant's `civil_from_days`
//! algorithm (exact proleptic Gregorian conversion over a range far wider
//! than we need, in particular past 2038). Local time - used only for daily
//! log filenames and human-facing event timestamps - comes straight from the
//! OS so DST and timezone rules stay the OS's problem.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A UTC instant broken into civil fields. `centis` is hundredths of a second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtcTime {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub min: u8,
    pub sec: u8,
    pub centis: u8,
}

/// A local-timezone wall-clock stamp. No sub-second field: it names log files
/// and prefixes event lines, nothing that needs finer resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalStamp {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub min: u8,
    pub sec: u8,
}

/// Current UTC civil time.
pub fn now_utc() -> UtcTime {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    utc_from_unix(d.as_secs() as i64, (d.subsec_nanos() / 10_000_000) as u8)
}

fn utc_from_unix(secs: i64, centis: u8) -> UtcTime {
    // Euclidean division keeps pre-1970 instants correct (negative seconds).
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    UtcTime {
        year,
        month,
        day,
        hour: (sod / 3_600) as u8,
        min: (sod / 60 % 60) as u8,
        sec: (sod % 60) as u8,
        centis,
    }
}

/// Proleptic Gregorian date from days since 1970-01-01 (Howard Hinnant's
/// `civil_from_days`). Pure integer arithmetic, no tables, exact for every
/// representable day.
fn civil_from_days(days: i64) -> (i32, u8, u8) {
    // Rebase the epoch to 0000-03-01 so each 400-year era starts on March 1
    // and the leap day is the last day of the era's year pattern.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // day of era, [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // March-based month, [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year as i32, month as u8, day as u8)
}

/// Current local wall-clock time from the OS.
#[cfg(windows)]
pub fn now_local() -> LocalStamp {
    /// Layout of the Win32 SYSTEMTIME struct: eight consecutive u16 fields.
    #[repr(C)]
    struct SystemTimeRaw {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetLocalTime(out: *mut SystemTimeRaw);
    }

    let mut st = SystemTimeRaw {
        year: 0,
        month: 0,
        day_of_week: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
        milliseconds: 0,
    };
    // SAFETY: GetLocalTime writes exactly one SYSTEMTIME through the pointer;
    // SystemTimeRaw is repr(C) with the same eight u16 fields. It cannot fail.
    unsafe { GetLocalTime(&mut st) };
    LocalStamp {
        year: i32::from(st.year),
        month: st.month as u8,
        day: st.day as u8,
        hour: st.hour as u8,
        min: st.minute as u8,
        sec: st.second as u8,
    }
}

/// Current local wall-clock time from the OS.
#[cfg(unix)]
pub fn now_local() -> LocalStamp {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let t = secs as libc::time_t;
    // SAFETY: tm is plain-old-data (pointer members on some libcs are allowed
    // to be null); localtime_r only writes it when it returns non-null.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the duration of the call.
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    if ok {
        LocalStamp {
            year: tm.tm_year + 1900,
            month: (tm.tm_mon + 1) as u8,
            day: tm.tm_mday as u8,
            hour: tm.tm_hour as u8,
            min: tm.tm_min as u8,
            sec: tm.tm_sec as u8,
        }
    } else {
        // No timezone database available: fall back to UTC rather than lie.
        let u = now_utc();
        LocalStamp {
            year: u.year,
            month: u.month,
            day: u.day,
            hour: u.hour,
            min: u.min,
            sec: u.sec,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inverse of `civil_from_days`, used to cross-check it property-style.
    fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
        let y = i64::from(year) - i64::from(month <= 2);
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let m = i64::from(month);
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        era * 146_097 + doe - 719_468
    }

    fn utc(secs: i64) -> (i32, u8, u8, u8, u8, u8) {
        let t = utc_from_unix(secs, 0);
        (t.year, t.month, t.day, t.hour, t.min, t.sec)
    }

    #[test]
    fn epoch_is_1970_01_01() {
        assert_eq!(utc(0), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn second_before_epoch() {
        assert_eq!(utc(-1), (1969, 12, 31, 23, 59, 59));
    }

    #[test]
    fn leap_day_2000() {
        // 2000 is a leap year despite being a century (divisible by 400).
        assert_eq!(utc(951_782_400), (2000, 2, 29, 0, 0, 0));
    }

    #[test]
    fn leap_day_2024() {
        assert_eq!(utc(1_709_164_800), (2024, 2, 29, 0, 0, 0));
    }

    #[test]
    fn century_2100_is_not_leap() {
        assert_eq!(utc(4_107_456_000), (2100, 2, 28, 0, 0, 0));
        assert_eq!(utc(4_107_456_000 + 86_400), (2100, 3, 1, 0, 0, 0));
    }

    #[test]
    fn y2038_rollover() {
        // i32 seconds overflow boundary: we must sail straight through it.
        assert_eq!(utc(2_147_483_647), (2038, 1, 19, 3, 14, 7));
        assert_eq!(utc(2_147_483_648), (2038, 1, 19, 3, 14, 8));
    }

    #[test]
    fn u32_seconds_rollover_2106() {
        assert_eq!(utc(4_294_967_296), (2106, 2, 7, 6, 28, 16));
    }

    #[test]
    fn centis_pass_through() {
        assert_eq!(utc_from_unix(0, 99).centis, 99);
    }

    #[test]
    fn civil_days_roundtrip_property() {
        // Fixed-seed LCG over +/- ~2700 years of day numbers.
        let mut x: u64 = 0x00C0_FFEE;
        for _ in 0..1_000 {
            x = x
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let days = (x >> 20) as i64 % 1_000_000 - 500_000;
            let (y, m, d) = civil_from_days(days);
            assert!((1..=12).contains(&m), "month {m} out of range");
            assert!((1..=31).contains(&d), "day {d} out of range");
            assert_eq!(days_from_civil(y, m, d), days, "roundtrip for {y}-{m}-{d}");
        }
    }

    #[test]
    fn now_utc_is_sane() {
        let t = now_utc();
        assert!((2026..=3000).contains(&t.year), "year {}", t.year);
        assert!((1..=12).contains(&t.month));
        assert!((1..=31).contains(&t.day));
        assert!(t.hour < 24 && t.min < 60 && t.sec < 60 && t.centis < 100);
    }

    #[test]
    fn now_local_is_sane_and_near_utc() {
        let l = now_local();
        let u = now_utc();
        assert!((1..=12).contains(&l.month));
        assert!((1..=31).contains(&l.day));
        assert!(l.hour < 24 && l.min < 60 && l.sec < 61);
        // Local civil date can differ from UTC by at most one calendar day.
        let dl = days_from_civil(l.year, l.month, l.day);
        let du = days_from_civil(u.year, u.month, u.day);
        assert!((dl - du).abs() <= 1, "local {dl} vs utc {du}");
    }
}
