use crate::civil;
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
    pub due_state: DueState,
    pub list_id: String,
    /// Denormalized from `lists` so Swift never has to join `list_id`
    /// against the roster itself.
    pub list_name: String,
}

/// A task's urgency relative to "today" — the same calendar-day comparison
/// `is_overdue` used to make alone, now split into three buckets instead of
/// a bool so overdue and due-today can get distinct treatment (e.g. color)
/// without the two ever being representable as simultaneously true. `None`
/// means no due date at all; `due_state` only produces the other three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub enum DueState {
    #[default]
    None,
    Later,
    Today,
    Overdue,
}

/// One entry in the `lists` roster, in sidebar display order. Always at
/// least one row — `Doc` guarantees a list can never be deleted down to
/// zero (see `Doc::apply`'s `DeleteList` handling).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ListRow {
    pub id: String,
    pub name: String,
    pub color: ListColor,
    /// Every task in the list, done ones included -- what a `DeleteList`
    /// on it would delete.
    pub task_count: u32,
}

/// A list's identity color — an opaque palette slot, not a hex value, so
/// the actual color each one maps to is entirely the UI layer's call (and
/// can be re-themed there without touching stored data). Assigned once, at
/// creation, to the lowest slot not already taken by a sibling list (see
/// `Doc::apply_add_list`), and persisted from then on — unlike a computed
/// hash of the list's id, this is real content, synced like any other field,
/// so it can actually guarantee every list in the roster gets a distinct
/// color instead of merely making collisions unlikely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize)]
pub enum ListColor {
    #[default]
    Blue,
    Purple,
    Pink,
    Orange,
    Teal,
    Indigo,
    Mint,
    Yellow,
    Cyan,
}

impl ListColor {
    pub const ALL: [ListColor; 9] = [
        ListColor::Blue,
        ListColor::Purple,
        ListColor::Pink,
        ListColor::Orange,
        ListColor::Teal,
        ListColor::Indigo,
        ListColor::Mint,
        ListColor::Yellow,
        ListColor::Cyan,
    ];

    /// Wraps rather than panics past the end of `ALL`: once there are more
    /// lists than palette slots, every index is already somebody's color,
    /// and collisions from here on are an unavoidable consequence of a
    /// finite palette — not a bug to guard against.
    pub fn from_index(index: u8) -> ListColor {
        Self::ALL[index as usize % Self::ALL.len()]
    }

    pub fn to_index(self) -> u8 {
        match self {
            ListColor::Blue => 0,
            ListColor::Purple => 1,
            ListColor::Pink => 2,
            ListColor::Orange => 3,
            ListColor::Teal => 4,
            ListColor::Indigo => 5,
            ListColor::Mint => 6,
            ListColor::Yellow => 7,
            ListColor::Cyan => 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Snapshot {
    pub rows: Vec<TaskRow>,
    pub view: ViewFilter,
    /// The list `current()` reads, and the one new captures land in — always
    /// a concrete list id.
    pub current_list: String,
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

/// `due`'s urgency relative to `now`, given a due date is actually present
/// (callers map `DueState::None` in themselves when there isn't one — see
/// `Doc::read`). Never returns `None` itself.
pub(crate) fn due_state(due: i64, now: i64, offset_seconds: i32) -> DueState {
    let offset = i64::from(offset_seconds);
    let due_day = civil::day_number(due + offset);
    let now_day = civil::day_number(now + offset);
    match due_day.cmp(&now_day) {
        std::cmp::Ordering::Less => DueState::Overdue,
        std::cmp::Ordering::Equal => DueState::Today,
        std::cmp::Ordering::Greater => DueState::Later,
    }
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
    fn due_state_boundary() {
        assert_eq!(due_state(1_000, 1_000, 0), DueState::Today);
        assert_eq!(due_state(DAY, 0, 0), DueState::Later);
        assert_eq!(due_state(0, DAY, 0), DueState::Overdue);
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
        assert_eq!(due_state(due, now, offset), DueState::Later);
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
        assert_eq!(due_state(due, now, offset), DueState::Overdue);
        assert_eq!(due_state(due, now, 0), DueState::Today);
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
}
