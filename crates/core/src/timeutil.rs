//! Calendar helpers without a date-time dependency. Timestamps are Unix seconds (UTC).

/// Days since 1970-01-01 for a proleptic Gregorian date.
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn unix_from_parts(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> i64 {
    days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s
}

pub fn parts_from_unix(t: i64) -> (i64, i64, i64, i64, i64, i64) {
    let days = t.div_euclid(86_400);
    let rem = t.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    (y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// `YYYY-MM-DD HH:MM` of a Unix timestamp (UTC).
pub fn format_minutes(t: i64) -> String {
    let (y, m, d, h, mi, _) = parts_from_unix(t);
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}")
}

/// Decodes an MS-DOS date/time pair into a "naive" timestamp (the digits as if they were UTC).
pub fn dos_to_naive(date: u16, time: u16) -> Option<i64> {
    let d = (date & 0x1f) as i64;
    let mo = ((date >> 5) & 0x0f) as i64;
    let y = ((date >> 9) & 0x7f) as i64 + 1980;
    let s = ((time & 0x1f) * 2) as i64;
    let mi = ((time >> 5) & 0x3f) as i64;
    let h = ((time >> 11) & 0x1f) as i64;
    if d == 0 || !(1..=12).contains(&mo) || d > 31 || h > 23 || mi > 59 || s > 59 {
        return None;
    }
    Some(unix_from_parts(y, mo, d, h, mi, s))
}

pub fn naive_to_dos(naive: i64) -> (u16, u16) {
    let (y, mo, d, h, mi, s) = parts_from_unix(naive);
    if y < 1980 {
        return (0x21, 0); // 1980-01-01
    }
    if y > 2107 {
        return ((127 << 9) | (12 << 5) | 31, (23 << 11) | (59 << 5) | 29);
    }
    let date = (((y - 1980) as u16) << 9) | ((mo as u16) << 5) | d as u16;
    let time = ((h as u16) << 11) | ((mi as u16) << 5) | (s / 2) as u16;
    (date, time)
}

/// Reinterprets a naive "local wall clock" timestamp as UTC using the machine's time zone.
pub fn local_to_utc(naive: i64) -> i64 {
    convert_tz(naive, true)
}

/// Converts a UTC timestamp to the naive local wall clock of this machine.
pub fn utc_to_local(utc: i64) -> i64 {
    convert_tz(utc, false)
}

#[cfg(windows)]
fn convert_tz(t: i64, local_to_utc: bool) -> i64 {
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::Time::{SystemTimeToTzSpecificLocalTime, TzSpecificLocalTimeToSystemTime};

    let (y, mo, d, h, mi, s) = parts_from_unix(t);
    if !(1601..=30_000).contains(&y) {
        return t;
    }
    let st = SYSTEMTIME {
        wYear: y as u16,
        wMonth: mo as u16,
        wDayOfWeek: 0,
        wDay: d as u16,
        wHour: h as u16,
        wMinute: mi as u16,
        wSecond: s as u16,
        wMilliseconds: 0,
    };
    let mut out = st;
    // SAFETY: both pointers refer to live SYSTEMTIME values; a null time-zone pointer means "current zone".
    let ok = unsafe {
        if local_to_utc {
            TzSpecificLocalTimeToSystemTime(std::ptr::null(), &st, &mut out)
        } else {
            SystemTimeToTzSpecificLocalTime(std::ptr::null(), &st, &mut out)
        }
    };
    if ok == 0 {
        return t;
    }
    unix_from_parts(
        out.wYear as i64,
        out.wMonth as i64,
        out.wDay as i64,
        out.wHour as i64,
        out.wMinute as i64,
        out.wSecond as i64,
    )
}

#[cfg(not(windows))]
fn convert_tz(t: i64, _local_to_utc: bool) -> i64 {
    t
}

pub fn unix_to_system_time(t: i64) -> std::time::SystemTime {
    if t >= 0 {
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(t as u64)
    } else {
        std::time::UNIX_EPOCH - std::time::Duration::from_secs(t.unsigned_abs())
    }
}

pub fn system_time_to_unix(t: std::time::SystemTime) -> i64 {
    match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_roundtrip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(unix_from_parts(2000, 2, 29, 12, 30, 15), 951_827_415);
        for t in [-86_400, 0, 951_827_415, 1_700_000_000, 4_102_444_800] {
            let (y, mo, d, h, mi, s) = parts_from_unix(t);
            assert_eq!(unix_from_parts(y, mo, d, h, mi, s), t);
        }
        assert_eq!(format_minutes(951_827_415), "2000-02-29 12:30");
    }

    #[test]
    fn dos_roundtrip() {
        let t = unix_from_parts(2024, 7, 9, 18, 45, 30);
        let (d, tm) = naive_to_dos(t);
        assert_eq!(dos_to_naive(d, tm), Some(t));
        assert_eq!(dos_to_naive(0, 0), None);
    }

    #[test]
    fn timezone_conversion_is_invertible() {
        let t = unix_from_parts(2024, 1, 15, 10, 0, 0);
        assert_eq!(utc_to_local(local_to_utc(t)), t);
    }
}
