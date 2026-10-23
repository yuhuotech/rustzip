//! Civil date <-> Unix time without a date library (all values UTC).
//! Algorithms from Howard Hinnant's "chrono-Compatible Low-Level Date Algorithms".

pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

pub fn civil_to_unix(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64) -> i64 {
    days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + s
}

/// Returns `(year, month, day, hour, minute, second)`.
pub fn unix_to_civil(t: i64) -> (i64, i64, i64, i64, i64, i64) {
    let days = t.div_euclid(86_400);
    let rem = t.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    (y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_and_known_dates() {
        assert_eq!(unix_to_civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(unix_to_civil(1_700_000_000), (2023, 11, 14, 22, 13, 20));
        assert_eq!(civil_to_unix(2023, 11, 14, 22, 13, 20), 1_700_000_000);
    }

    #[test]
    fn leap_days_round_trip() {
        let t = civil_to_unix(2024, 2, 29, 12, 0, 0);
        assert_eq!(unix_to_civil(t), (2024, 2, 29, 12, 0, 0));
        let t = civil_to_unix(1980, 1, 1, 0, 0, 0);
        assert_eq!(unix_to_civil(t), (1980, 1, 1, 0, 0, 0));
    }

    #[test]
    fn pre_epoch_dates() {
        assert_eq!(unix_to_civil(-1), (1969, 12, 31, 23, 59, 59));
    }
}
