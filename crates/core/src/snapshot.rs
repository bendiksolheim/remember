use crate::civil;
use crate::command::{ListFilter, ViewFilter};

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
    pub list_id: String,
    /// Denormalized from `lists` so a cross-list ("All") view can render
    /// which list a row belongs to without Swift ever joining `list_id`
    /// against the roster itself.
    pub list_name: String,
}

/// One entry in the `lists` roster, in sidebar display order. Always at
/// least one row — `Doc` guarantees a list can never be deleted down to
/// zero (see `Doc::apply`'s `DeleteList` handling).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ListRow {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Snapshot {
    pub rows: Vec<TaskRow>,
    pub view: ViewFilter,
    pub current_list: ListFilter,
    /// The concrete list a new capture would land in right now — mirrors
    /// `AppState::capture_list_id`. `Doc::read` doesn't track this itself
    /// (it's `App`-level state, sticky across "All" views), so it fills in
    /// an empty placeholder here; `App::current()` overwrites it with the
    /// real value before returning.
    pub capture_list_id: String,
    pub lists: Vec<ListRow>,
    pub active_count: u32,
    pub can_undo: bool,
    pub can_redo: bool,
    pub revision: u64,
}

/// Result of detecting a due-date phrase in freshly typed text (see
/// `crate::due_parse` for the detection itself). Also FFI-boundary plain
/// data: `stripped_title` already has the matched phrase — and whatever
/// trailing whitespace it leaves behind — removed, so the caller can use it
/// as-is; `label` is the same "Today"/"Tomorrow"/"25 Dec" text `due_label`
/// produces, for a live badge; `due` is the instant to store.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct DueDetection {
    pub stripped_title: String,
    pub label: String,
    pub due: i64,
}

/// `due` and `now` are unix seconds; `offset_seconds` is the caller's local
/// UTC offset (see `Doc::set_local_offset_seconds`). Both are shifted by it
/// before comparing calendar days, so "Today"/"Tomorrow" and the cutoff
/// below match the device's actual local date rather than UTC's — a task
/// due at 11pm local time in UTC-8 must not read as "Tomorrow" just because
/// it's already past midnight in UTC.
pub fn due_label(due: i64, now: i64, offset_seconds: i32) -> String {
    let offset = i64::from(offset_seconds);
    let due_day = civil::day_number(due + offset);
    let now_day = civil::day_number(now + offset);
    match due_day - now_day {
        0 => "Today".to_string(),
        1 => "Tomorrow".to_string(),
        _ => {
            let (_, month, day) = civil::civil_from_days(due_day);
            format!("{day} {}", civil::MONTH_ABBREVIATIONS[(month - 1) as usize])
        }
    }
}

pub(crate) fn is_overdue(due: i64, now: i64, offset_seconds: i32) -> bool {
    let offset = i64::from(offset_seconds);
    civil::day_number(due + offset) < civil::day_number(now + offset)
}

pub(crate) fn matches_filter(done: bool, view: ViewFilter) -> bool {
    match view {
        ViewFilter::All => true,
        ViewFilter::Active => !done,
        ViewFilter::Completed => done,
    }
}

pub(crate) fn matches_list_filter(list_id: &str, filter: &ListFilter) -> bool {
    match filter {
        ListFilter::All => true,
        ListFilter::List(id) => id == list_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = civil::SECONDS_PER_DAY;

    #[test]
    fn due_label_today() {
        assert_eq!(due_label(1_000, 1_000, 0), "Today");
        // due at second 1_000 of day 0, "now" at the last second of day 0.
        assert_eq!(due_label(1_000, DAY - 1, 0), "Today");
    }

    #[test]
    fn due_label_tomorrow() {
        assert_eq!(due_label(DAY, 0, 0), "Tomorrow");
    }

    #[test]
    fn due_label_overdue_shows_date_not_relative_text() {
        // 1970-01-01 (day 0), viewed from 1970-01-05 (day 4): overdue by 4 days.
        assert_eq!(due_label(0, 4 * DAY, 0), "1 Jan");
    }

    #[test]
    fn due_label_far_future_shows_date() {
        // day 365 = 1971-01-01
        assert_eq!(due_label(365 * DAY, 0, 0), "1 Jan");
    }

    #[test]
    fn due_label_this_week() {
        // day 3 from day 0 = 3 days out
        assert_eq!(due_label(3 * DAY, 0, 0), "4 Jan");
    }

    #[test]
    fn due_label_next_month() {
        // day 40 = 1970-02-10
        assert_eq!(due_label(40 * DAY, 0, 0), "10 Feb");
    }

    #[test]
    fn is_overdue_boundary() {
        assert!(!is_overdue(1_000, 1_000, 0));
        assert!(!is_overdue(DAY, 0, 0));
        assert!(is_overdue(0, DAY, 0));
    }

    #[test]
    fn offset_shifts_the_day_boundary_for_both_due_and_now() {
        // UTC-8: "now" is 1970-01-01 20:00 local (8pm) = 1970-01-02 04:00
        // UTC (100_800s) — already the next UTC day. "due" is local
        // midnight the following day = 1970-01-02 08:00 UTC (115_200s).
        // Raw UTC day math (offset 0) puts both on UTC day 1, so it would
        // wrongly call this "Today" — the bug this offset fixes.
        let offset = -8 * 3_600;
        let now = 100_800;
        let due = 115_200;
        assert_eq!(due_label(due, now, offset), "Tomorrow");
        assert!(!is_overdue(due, now, offset));
        // Without the offset, the same instants read as "Today" instead —
        // documents the bug this shift fixes, not just the fix itself.
        assert_eq!(due_label(due, now, 0), "Today");

        // UTC+12: "now" is 1970-01-02 01:00 local = 1970-01-01 13:00 UTC
        // (46_800s) — local day 1. "due" is 1970-01-01 00:00 UTC (0s),
        // which is still local day 0 at +12h — a day already past. Raw UTC
        // day math puts both on UTC day 0 and would wrongly call this not
        // overdue.
        let offset = 12 * 3_600;
        let now = 46_800;
        let due = 0;
        assert!(is_overdue(due, now, offset));
        assert!(!is_overdue(due, now, 0));
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
    fn matches_list_filter_all_shows_every_list() {
        assert!(matches_list_filter("a", &ListFilter::All));
        assert!(matches_list_filter("b", &ListFilter::All));
    }

    #[test]
    fn matches_list_filter_list_shows_only_that_list() {
        let filter = ListFilter::List("a".to_string());
        assert!(matches_list_filter("a", &filter));
        assert!(!matches_list_filter("b", &filter));
    }
}
