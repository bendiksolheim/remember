use crate::command::ViewFilter;

/// What crosses the FFI boundary. Plain data: already filtered, already
/// sorted, already formatted. Swift does no computation.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TaskRow {
    pub id: String,
    pub title: String,
    pub notes: String,
    pub done: bool,
    pub due: Option<i64>,
    pub due_label: Option<String>,
    pub overdue: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Snapshot {
    pub rows: Vec<TaskRow>,
    pub view: ViewFilter,
    pub active_count: u32,
    pub can_undo: bool,
    pub can_redo: bool,
    pub revision: u64,
}

const SECONDS_PER_DAY: i64 = 86_400;

const MONTH_ABBREVIATIONS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

fn day_number(unix_seconds: i64) -> i64 {
    unix_seconds.div_euclid(SECONDS_PER_DAY)
}

/// Howard Hinnant's `civil_from_days`: converts a day count since the Unix
/// epoch (1970-01-01) into a (day-of-month, month) pair. Public domain
/// algorithm — see http://howardhinnant.github.io/date_algorithms.html.
/// No date library is in the dependency list, so this is done by hand.
fn month_day_from_day_number(days: i64) -> (u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (day, month)
}

/// `due` and `now` are unix seconds. Classifies by calendar day, not by
/// elapsed hours — a task due later today is "Today", not overdue.
pub fn due_label(due: i64, now: i64) -> String {
    let diff = day_number(due) - day_number(now);
    match diff {
        0 => "Today".to_string(),
        1 => "Tomorrow".to_string(),
        _ => {
            let (day, month) = month_day_from_day_number(day_number(due));
            format!("{day} {}", MONTH_ABBREVIATIONS[(month - 1) as usize])
        }
    }
}

pub(crate) fn is_overdue(due: i64, now: i64) -> bool {
    day_number(due) < day_number(now)
}

pub(crate) fn matches_filter(done: bool, view: ViewFilter) -> bool {
    match view {
        ViewFilter::All => true,
        ViewFilter::Active => !done,
        ViewFilter::Completed => done,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = SECONDS_PER_DAY;

    #[test]
    fn due_label_today() {
        assert_eq!(due_label(1_000, 1_000), "Today");
        // due at second 1_000 of day 0, "now" at the last second of day 0.
        assert_eq!(due_label(1_000, DAY - 1), "Today");
    }

    #[test]
    fn due_label_tomorrow() {
        assert_eq!(due_label(DAY, 0), "Tomorrow");
    }

    #[test]
    fn due_label_overdue_shows_date_not_relative_text() {
        // 1970-01-01 (day 0), viewed from 1970-01-05 (day 4): overdue by 4 days.
        assert_eq!(due_label(0, 4 * DAY), "1 Jan");
    }

    #[test]
    fn due_label_far_future_shows_date() {
        // day 365 = 1971-01-01
        assert_eq!(due_label(365 * DAY, 0), "1 Jan");
    }

    #[test]
    fn due_label_this_week() {
        // day 3 from day 0 = 3 days out
        assert_eq!(due_label(3 * DAY, 0), "4 Jan");
    }

    #[test]
    fn due_label_next_month() {
        // day 40 = 1970-02-10
        assert_eq!(due_label(40 * DAY, 0), "10 Feb");
    }

    #[test]
    fn is_overdue_boundary() {
        assert!(!is_overdue(1_000, 1_000));
        assert!(!is_overdue(DAY, 0));
        assert!(is_overdue(0, DAY));
    }

    #[test]
    fn matches_filter_all_shows_everything() {
        assert!(matches_filter(true, ViewFilter::All));
        assert!(matches_filter(false, ViewFilter::All));
    }

    #[test]
    fn matches_filter_active_hides_done() {
        assert!(matches_filter(false, ViewFilter::Active));
        assert!(!matches_filter(true, ViewFilter::Active));
    }

    #[test]
    fn matches_filter_completed_hides_active() {
        assert!(matches_filter(true, ViewFilter::Completed));
        assert!(!matches_filter(false, ViewFilter::Completed));
    }

    #[test]
    fn month_day_covers_every_month_boundary() {
        // day 0 = 1970-01-01, then step through each month start in 1970.
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
        for (days, expected) in month_starts_1970.iter().zip(expected.iter()) {
            assert_eq!(month_day_from_day_number(*days), *expected);
        }
    }

    #[test]
    fn month_day_handles_century_leap_year_rules() {
        // 1970 alone never exercises the algorithm's century/400-year-cycle
        // correction terms (they're all zero this close to 1970). These
        // cases, cross-checked against an independent day-count, do:
        // - 2000 is a leap year (divisible by 400, despite being a century).
        // - 2100 is NOT a leap year (divisible by 100, but not 400).
        // - 1900 is NOT a leap year either, on the other side of the epoch.
        let cases = [
            (11_016, (29, 2)),  // 2000-02-29: leap day exists
            (11_017, (1, 3)),   // 2000-03-01: day after the leap day
            (47_481, (31, 12)), // 2099-12-31
            (47_482, (1, 1)),   // 2100-01-01: no leap day skipped it
            (-25_509, (28, 2)), // 1900-02-28: last day of February
            (-25_508, (1, 3)),  // 1900-03-01: no leap day here either
        ];
        for (days, expected) in cases {
            assert_eq!(month_day_from_day_number(days), expected);
        }
    }

    #[test]
    fn month_day_handles_ancient_dates_before_the_algorithms_own_epoch_shift() {
        // The algorithm shifts to a 0000-03-01 epoch internally (`z = days +
        // 719_468`) and takes a different branch for z < 0, i.e. days before
        // -719_468. 1900/2000/2100 never go anywhere near this — these
        // cases (cross-checked against an independent day-count) do.
        let cases = [
            (-719_469, (29, 2)), // 0000-02-29: right at the shifted epoch
            (-719_468, (1, 3)),  // 0000-03-01: the shifted epoch itself
            (-800_000, (4, 9)),
            (-1_000_000, (4, 2)),
        ];
        for (days, expected) in cases {
            assert_eq!(month_day_from_day_number(days), expected);
        }
    }
}
