//! Calendar-day arithmetic shared by [`crate::snapshot`] (formatting/overdue)
//! and [`crate::due_parse`] (resolving typed phrases like "tomorrow" or "dec
//! 25" to a day). No date-library dependency — Howard Hinnant's
//! `civil_from_days`/`days_from_civil`, public domain, see
//! http://howardhinnant.github.io/date_algorithms.html.

pub(crate) const SECONDS_PER_DAY: i64 = 86_400;

pub(crate) const MONTH_ABBREVIATIONS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Day count since the Unix epoch (1970-01-01 = day 0) for a moment
/// expressed in unix seconds.
pub(crate) fn day_number(unix_seconds: i64) -> i64 {
    unix_seconds.div_euclid(SECONDS_PER_DAY)
}

/// `days` -> (year, month, day). See the module doc comment for the
/// algorithm's source.
pub(crate) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    (year, month, day)
}

/// Inverse of [`civil_from_days`]: (year, month, day) -> days since epoch.
/// `month`/`day` are trusted to be in their normal ranges ([1,12]/[1,31]) —
/// every caller in this crate only ever passes values it parsed from
/// [`MONTH_ABBREVIATIONS`] or a weekday table, never arbitrary user ints.
pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = year - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mm = if month > 2 {
        i64::from(month) - 3
    } else {
        i64::from(month) + 9
    };
    let doy = (153 * mm + 2) / 5 + i64::from(day) - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// 0 = Sunday .. 6 = Saturday. 1970-01-01 (day 0) was a Thursday.
pub(crate) fn weekday_from_day_number(days: i64) -> u32 {
    (days + 4).rem_euclid(7) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_epoch_is_1970_01_01() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn civil_from_days_covers_every_month_boundary_in_1970() {
        let expected = [
            (1, 1),
            (1, 2),
            (1, 3),
            (1, 4),
            (1, 5),
            (1, 6),
            (1, 7),
            (1, 8),
            (1, 9),
            (1, 10),
            (1, 11),
            (1, 12),
        ];
        let month_starts_1970 = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
        for (days, (day, month)) in month_starts_1970.iter().zip(expected.iter()) {
            let (year, m, d) = civil_from_days(*days);
            assert_eq!((year, m, d), (1970, *month, *day));
        }
    }

    #[test]
    fn civil_from_days_handles_century_leap_year_rules() {
        // Cross-checked against an independent day-counting method — see the
        // sibling comment this replaced in snapshot.rs for why each of these
        // specifically exercises the algorithm's correction terms.
        let cases: [(i64, (i64, u32, u32)); 6] = [
            (11_016, (2000, 2, 29)), // leap day, divisible by 400
            (11_017, (2000, 3, 1)),  // day after
            (47_481, (2099, 12, 31)),
            (47_482, (2100, 1, 1)),   // divisible by 100, not 400: no leap day
            (-25_509, (1900, 2, 28)), // divisible by 100, not 400: no leap day
            (-25_508, (1900, 3, 1)),
        ];
        for (days, expected) in cases {
            assert_eq!(civil_from_days(days), expected);
        }
    }

    #[test]
    fn civil_from_days_handles_dates_before_the_algorithms_own_epoch_shift() {
        // The algorithm internally shifts to a 0000-03-01 epoch
        // (`z = days + 719_468`) and takes a different branch for
        // `z < 0`, i.e. `days < -719_468`.
        let cases: [(i64, (i64, u32, u32)); 4] = [
            (-719_469, (0, 2, 29)),
            (-719_468, (0, 3, 1)),
            (-800_000, (-221, 9, 4)),
            (-1_000_000, (-768, 2, 4)),
        ];
        for (days, expected) in cases {
            assert_eq!(civil_from_days(days), expected);
        }
    }

    #[test]
    fn days_from_civil_round_trips_through_civil_from_days() {
        for days in [
            0i64, 1, 31, 365, -1, -365, 11_016, 11_017, 47_481, 47_482, -25_509, -25_508, -719_469,
            -719_468, -800_000, -1_000_000, 1_000_000,
        ] {
            let (year, month, day) = civil_from_days(days);
            assert_eq!(days_from_civil(year, month, day), days);
        }
    }

    #[test]
    fn days_from_civil_matches_known_month_starts() {
        let month_starts_1970 = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
        for (month, expected_days) in month_starts_1970.iter().enumerate() {
            assert_eq!(days_from_civil(1970, month as u32 + 1, 1), *expected_days);
        }
    }

    #[test]
    fn weekday_epoch_is_thursday() {
        assert_eq!(weekday_from_day_number(0), 4);
    }

    #[test]
    fn weekday_cycles_correctly_around_the_epoch() {
        // 1969-12-31 (day -1) was a Wednesday.
        assert_eq!(weekday_from_day_number(-1), 3);
        // 1970-01-08 (day 7, one week after the Thursday epoch) was also a
        // Thursday.
        assert_eq!(weekday_from_day_number(7), 4);
    }

    #[test]
    fn weekday_matches_a_known_far_future_date() {
        // 2000-01-01 (day 10_957) was a Saturday.
        assert_eq!(days_from_civil(2000, 1, 1), 10_957);
        assert_eq!(weekday_from_day_number(10_957), 6);
    }

    #[test]
    fn day_number_floors_toward_negative_infinity() {
        assert_eq!(day_number(0), 0);
        assert_eq!(day_number(SECONDS_PER_DAY - 1), 0);
        assert_eq!(day_number(SECONDS_PER_DAY), 1);
        assert_eq!(day_number(-1), -1);
    }
}
