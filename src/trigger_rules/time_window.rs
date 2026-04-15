use chrono::{DateTime, Datelike, NaiveTime, Timelike, Utc, Weekday};

use crate::config::TimeWindow;
use crate::models::trigger_attempt::TriggerAttemptStatus;

use super::EvalOutcome;

pub fn evaluate(windows: &[TimeWindow]) -> EvalOutcome {
    evaluate_at(windows, Utc::now())
}

pub fn evaluate_at(windows: &[TimeWindow], now: DateTime<Utc>) -> EvalOutcome {
    let current_time =
        NaiveTime::from_hms_opt(now.hour(), now.minute(), now.second()).unwrap_or(NaiveTime::MIN);
    let current_day = weekday_abbrev(now.weekday());

    for window in windows {
        let Ok(start) = NaiveTime::parse_from_str(&window.start_time, "%H:%M") else {
            continue;
        };
        let Ok(end) = NaiveTime::parse_from_str(&window.end_time, "%H:%M") else {
            continue;
        };

        let day_matches = window.days.is_empty()
            || window
                .days
                .iter()
                .any(|d| d.eq_ignore_ascii_case(current_day));

        let time_matches = current_time >= start && current_time < end;

        if day_matches && time_matches {
            return EvalOutcome::Allow;
        }
    }

    EvalOutcome::Reject {
        status: TriggerAttemptStatus::ScheduleSkipped,
        reason: format!(
            "no time window matches current time {} {}",
            current_day,
            current_time.format("%H:%M")
        ),
    }
}

fn weekday_abbrev(wd: Weekday) -> &'static str {
    match wd {
        Weekday::Mon => "Mon",
        Weekday::Tue => "Tue",
        Weekday::Wed => "Wed",
        Weekday::Thu => "Thu",
        Weekday::Fri => "Fri",
        Weekday::Sat => "Sat",
        Weekday::Sun => "Sun",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn window(days: &[&str], start: &str, end: &str) -> TimeWindow {
        TimeWindow {
            days: days.iter().map(|s| s.to_string()).collect(),
            start_time: start.to_string(),
            end_time: end.to_string(),
        }
    }

    fn utc(year: i32, month: u32, day: u32, hour: u32, min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, min, 0)
            .unwrap()
    }

    // 2026-04-13 is a Monday
    // 2026-04-18 is a Saturday

    #[test]
    fn within_window_allows() {
        let now = utc(2026, 4, 13, 10, 0); // Monday 10:00
        let result = evaluate_at(
            &[window(
                &["Mon", "Tue", "Wed", "Thu", "Fri"],
                "09:00",
                "17:00",
            )],
            now,
        );
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[test]
    fn outside_window_rejects() {
        let now = utc(2026, 4, 13, 20, 0); // Monday 20:00
        let result = evaluate_at(
            &[window(
                &["Mon", "Tue", "Wed", "Thu", "Fri"],
                "09:00",
                "17:00",
            )],
            now,
        );
        assert!(matches!(result, EvalOutcome::Reject { .. }));
    }

    #[test]
    fn wrong_day_rejects() {
        let now = utc(2026, 4, 18, 10, 0); // Saturday 10:00
        let result = evaluate_at(
            &[window(
                &["Mon", "Tue", "Wed", "Thu", "Fri"],
                "09:00",
                "17:00",
            )],
            now,
        );
        assert!(matches!(result, EvalOutcome::Reject { .. }));
    }

    #[test]
    fn empty_days_matches_all() {
        let now = utc(2026, 4, 14, 12, 0); // Tuesday 12:00
        let result = evaluate_at(&[window(&[], "09:00", "17:00")], now);
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[test]
    fn multiple_windows_or_logic() {
        let now = utc(2026, 4, 18, 11, 0); // Saturday 11:00
        let windows = vec![
            window(&["Mon", "Tue", "Wed", "Thu", "Fri"], "09:00", "17:00"),
            window(&["Sat", "Sun"], "10:00", "14:00"),
        ];
        let result = evaluate_at(&windows, now);
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[test]
    fn edge_exact_start_time_allows() {
        let now = utc(2026, 4, 13, 9, 0); // Monday 09:00
        let result = evaluate_at(&[window(&["Mon"], "09:00", "17:00")], now);
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[test]
    fn edge_exact_end_time_rejects() {
        let now = utc(2026, 4, 13, 17, 0); // Monday 17:00 — exclusive end
        let result = evaluate_at(&[window(&["Mon"], "09:00", "17:00")], now);
        assert!(matches!(result, EvalOutcome::Reject { .. }));
    }

    #[test]
    fn case_insensitive_day_matching() {
        let now = utc(2026, 4, 13, 10, 0); // Monday 10:00
        let result = evaluate_at(&[window(&["mon"], "09:00", "17:00")], now);
        assert!(matches!(result, EvalOutcome::Allow));
    }

    #[test]
    fn no_windows_rejects() {
        // Empty windows list — no window can match, so reject.
        // Caller is responsible for skipping evaluation when list is empty.
        let now = utc(2026, 4, 13, 10, 0);
        let result = evaluate_at(&[], now);
        assert!(matches!(result, EvalOutcome::Reject { .. }));
    }
}
