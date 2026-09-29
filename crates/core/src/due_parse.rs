//! Detects a trailing due-date phrase in freshly typed text — "buy milk
//! tomorrow", "call John dec 25" — with no date-library dependency (see
//! `crate::civil` for the underlying day math). Pure and stateless: given
//! "today" as a day number, this never touches the clock itself, so every
//! case is deterministic and directly testable.
//!
//! Deliberately narrow for v1, per the grilled design: only a *trailing*
//! phrase is ever recognized (never mid-sentence, to avoid false positives
//! like a title genuinely about "Friday"), and only "today", "tomorrow",
//! weekday names, and "<month-abbrev> <day>" are supported — no numeric
//! dates (locale-ambiguous), no relative offsets like "in 3 days".

use crate::civil;

/// A recognized trailing phrase: `match_start` is the byte offset (into the
/// text `detect` was given) where the phrase begins, `day_number` is the
/// resolved target day (days since the Unix epoch).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Detection {
    pub match_start: usize,
    pub day_number: i64,
}

/// (full name, 3-letter abbreviation, weekday index per
/// `civil::weekday_from_day_number` — 0 = Sunday).
const WEEKDAYS: [(&str, &str, u32); 7] = [
    ("sunday", "sun", 0),
    ("monday", "mon", 1),
    ("tuesday", "tue", 2),
    ("wednesday", "wed", 3),
    ("thursday", "thu", 4),
    ("friday", "fri", 5),
    ("saturday", "sat", 6),
];

/// Finds a trailing due-date phrase in `text`, resolved against `today`
/// (the caller's local day number — see `civil::day_number`). `None` if the
/// trailing word(s) don't match any recognized phrase.
pub(crate) fn detect(text: &str, today: i64) -> Option<Detection> {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return None;
    }
    detect_month_day(trimmed, today).or_else(|| detect_single_word(trimmed, today))
}

/// Byte offset where the last whitespace-delimited token of `s` begins. `0`
/// if `s` has no whitespace at all (the whole string is one token). `s`
/// must be non-empty with no trailing whitespace, which `detect` already
/// guarantees via `trim_end`.
fn last_token_start(s: &str) -> usize {
    s.rfind(char::is_whitespace).map_or(0, |i| i + 1)
}

fn detect_single_word(trimmed: &str, today: i64) -> Option<Detection> {
    let start = last_token_start(trimmed);
    let word = trimmed[start..].to_ascii_lowercase();
    let day_number = match word.as_str() {
        "today" => today,
        "tomorrow" => today + 1,
        _ => resolve_weekday(&word, today)?,
    };
    Some(Detection {
        match_start: start,
        day_number,
    })
}

/// Next occurrence of the named weekday, always strictly after `today` —
/// per the grilled design, typing a weekday's own name on that same day
/// means next week, never "right now".
fn resolve_weekday(word: &str, today: i64) -> Option<i64> {
    let (_, _, target) = WEEKDAYS
        .iter()
        .find(|(full, abbrev, _)| word == *full || word == *abbrev)?;
    let current = civil::weekday_from_day_number(today);
    let delta = (target + 7 - current) % 7;
    let delta = if delta == 0 { 7 } else { delta };
    Some(today + i64::from(delta))
}

/// "<month-abbrev> <day>", e.g. "dec 25". A date already passed this year
/// rolls to next year — a due date is always ahead of you, same rule as
/// weekday names above.
fn detect_month_day(trimmed: &str, today: i64) -> Option<Detection> {
    let day_start = last_token_start(trimmed);
    if day_start == 0 {
        return None; // only one token total — no room for a preceding month
    }
    let day: u32 = trimmed[day_start..].parse().ok()?;
    if !(1..=31).contains(&day) {
        return None;
    }

    // `day_start - 1` is the separating whitespace byte; look at the token
    // immediately before it.
    let before_day = trimmed[..day_start - 1].trim_end();
    let month_start = last_token_start(before_day);
    let month_word = before_day[month_start..].to_ascii_lowercase();
    let month = civil::MONTH_ABBREVIATIONS
        .iter()
        .position(|abbrev| abbrev.to_ascii_lowercase() == month_word)? as u32
        + 1;

    let (year, _, _) = civil::civil_from_days(today);
    let candidate = civil::days_from_civil(year, month, day);
    let day_number = if candidate < today {
        civil::days_from_civil(year + 1, month, day)
    } else {
        candidate
    };
    Some(Detection {
        match_start: month_start,
        day_number,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// 1970-01-01 was a Thursday — used throughout as a `today` with a
    /// known weekday.
    const THURSDAY: i64 = 0;

    #[test]
    fn detects_today() {
        let d = detect("Buy milk today", THURSDAY).unwrap();
        assert_eq!(d.day_number, THURSDAY);
        assert_eq!(&"Buy milk today"[d.match_start..], "today");
    }

    #[test]
    fn detects_tomorrow_case_insensitively() {
        let d = detect("Buy milk TOMORROW", THURSDAY).unwrap();
        assert_eq!(d.day_number, THURSDAY + 1);
        assert_eq!(&"Buy milk TOMORROW"[d.match_start..], "TOMORROW");
    }

    #[test]
    fn today_and_tomorrow_as_the_whole_input() {
        assert_eq!(detect("today", THURSDAY).unwrap().day_number, THURSDAY);
        assert_eq!(
            detect("tomorrow", THURSDAY).unwrap().day_number,
            THURSDAY + 1
        );
    }

    #[test]
    fn weekday_full_name_resolves_to_next_occurrence() {
        // Today is Thursday; "monday" is 4 days out.
        let d = detect("Team sync monday", THURSDAY).unwrap();
        assert_eq!(d.day_number, THURSDAY + 4);
    }

    #[test]
    fn weekday_abbreviation_matches_too() {
        let d = detect("Team sync mon", THURSDAY).unwrap();
        assert_eq!(d.day_number, THURSDAY + 4);
    }

    #[test]
    fn weekday_named_after_today_skips_today_by_a_full_week() {
        // Today (day 0) is a Thursday; typing "thursday" must not mean
        // today.
        let d = detect("Call back thursday", THURSDAY).unwrap();
        assert_eq!(d.day_number, THURSDAY + 7);
    }

    #[test]
    fn weekday_resolution_covers_every_day_of_the_week() {
        for (name, expected_delta) in [
            ("sunday", 3),
            ("monday", 4),
            ("tuesday", 5),
            ("wednesday", 6),
            ("thursday", 7),
            ("friday", 1),
            ("saturday", 2),
        ] {
            let d = detect(name, THURSDAY).unwrap();
            assert_eq!(
                d.day_number,
                THURSDAY + expected_delta,
                "weekday {name} resolved wrong"
            );
        }
    }

    #[test]
    fn month_day_resolves_this_year_when_still_ahead() {
        // Today is 1970-01-01 (day 0); "dec 25" is later this year.
        let d = detect("Buy gifts dec 25", THURSDAY).unwrap();
        assert_eq!(civil::civil_from_days(d.day_number), (1970, 12, 25));
        assert_eq!(&"Buy gifts dec 25"[d.match_start..], "dec 25");
    }

    #[test]
    fn month_day_is_case_insensitive() {
        let d = detect("Buy gifts DEC 25", THURSDAY).unwrap();
        assert_eq!(civil::civil_from_days(d.day_number), (1970, 12, 25));
    }

    #[test]
    fn month_day_already_passed_this_year_rolls_to_next_year() {
        // "today" is 1970-11-15; "jan 5" has already gone by this year.
        let today = civil::days_from_civil(1970, 11, 15);
        let d = detect("Renew passport jan 5", today).unwrap();
        assert_eq!(civil::civil_from_days(d.day_number), (1971, 1, 5));
    }

    #[test]
    fn month_day_exactly_today_does_not_roll_over() {
        let today = civil::days_from_civil(1970, 12, 25);
        let d = detect("Buy gifts dec 25", today).unwrap();
        assert_eq!(d.day_number, today);
    }

    #[test]
    fn month_day_as_the_whole_input_with_no_other_words() {
        let d = detect("dec 25", THURSDAY).unwrap();
        assert_eq!(civil::civil_from_days(d.day_number), (1970, 12, 25));
        assert_eq!(d.match_start, 0);
    }

    #[test]
    fn matching_is_trailing_only_not_anywhere_in_the_text() {
        // "friday" is present but not trailing — must not match.
        assert!(detect("Friday's meeting notes", THURSDAY).is_none());
        assert!(detect("dec 25 is the deadline", THURSDAY).is_none());
    }

    #[test]
    fn no_match_on_plain_text() {
        assert!(detect("Buy milk", THURSDAY).is_none());
    }

    #[test]
    fn no_match_on_empty_or_whitespace_only_input() {
        assert!(detect("", THURSDAY).is_none());
        assert!(detect("   ", THURSDAY).is_none());
    }

    #[test]
    fn no_match_on_out_of_range_day_number() {
        assert!(detect("Meet Bob dec 32", THURSDAY).is_none());
        assert!(detect("Meet Bob dec 0", THURSDAY).is_none());
    }

    #[test]
    fn no_match_when_preceding_word_is_not_a_month() {
        assert!(detect("Meet Bob at 25", THURSDAY).is_none());
    }

    #[test]
    fn no_match_on_a_bare_trailing_number_with_no_earlier_words() {
        assert!(detect("25", THURSDAY).is_none());
    }

    #[test]
    fn trailing_whitespace_is_ignored_and_match_start_still_refers_to_the_original_text() {
        let text = "Buy milk tomorrow   ";
        let d = detect(text, THURSDAY).unwrap();
        assert_eq!(text[d.match_start..].trim_end(), "tomorrow");
    }
}
