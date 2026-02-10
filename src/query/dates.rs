use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, TimeZone, Utc};

#[derive(Debug, Clone, PartialEq)]
pub struct DateRange {
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
}

impl DateRange {
    #[cfg(test)]
    pub fn contains(&self, dt: &DateTime<Utc>) -> bool {
        if let Some(start) = &self.start {
            if dt < start {
                return false;
            }
        }
        if let Some(end) = &self.end {
            if dt >= end {
                return false;
            }
        }
        true
    }

    #[cfg(test)]
    pub fn open() -> Self {
        DateRange {
            start: None,
            end: None,
        }
    }
}

/// Parse a relative date term into a DateRange, using `now` as reference.
/// The `tz` offset determines what "today" means in the user's local time.
pub fn parse_relative(term: &str, now: DateTime<Utc>, tz: &FixedOffset) -> Option<DateRange> {
    let today_start = start_of_day(now, tz);
    let tomorrow_start = today_start + Duration::days(1);

    match term {
        "today" => Some(DateRange {
            start: Some(today_start),
            end: Some(tomorrow_start),
        }),
        "yesterday" => Some(DateRange {
            start: Some(today_start - Duration::days(1)),
            end: Some(today_start),
        }),
        "this-week" => {
            let week_start = start_of_week(now, tz);
            let week_end = week_start + Duration::days(7);
            Some(DateRange {
                start: Some(week_start),
                end: Some(week_end),
            })
        }
        "last-week" => {
            let this_week_start = start_of_week(now, tz);
            let last_week_start = this_week_start - Duration::days(7);
            Some(DateRange {
                start: Some(last_week_start),
                end: Some(this_week_start),
            })
        }
        "this-month" => {
            let month_start = start_of_month(now, tz);
            let next_month_start = next_month(month_start, tz);
            Some(DateRange {
                start: Some(month_start),
                end: Some(next_month_start),
            })
        }
        "last-month" => {
            let this_month_start = start_of_month(now, tz);
            let last_month_start = prev_month(this_month_start, tz);
            Some(DateRange {
                start: Some(last_month_start),
                end: Some(this_month_start),
            })
        }
        _ => None,
    }
}

/// Parse an absolute date string (ISO 8601 date or datetime).
/// Date-only input is interpreted as midnight in the user's local timezone.
pub fn parse_absolute(s: &str, tz: &FixedOffset) -> Option<DateTime<Utc>> {
    // Try full datetime first
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    // Try date-only — interpret as local midnight
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let naive_midnight = date.and_hms_opt(0, 0, 0)?;
        let local_dt = tz.from_local_datetime(&naive_midnight).single()?;
        return Some(local_dt.with_timezone(&Utc));
    }
    None
}

/// Parse a duration shorthand (e.g., "3d", "2w", "1m") into a point in time
/// by subtracting the duration from `now`. Returns start-of-day for the result
/// in the user's local timezone.
pub fn parse_duration(s: &str, now: DateTime<Utc>, tz: &FixedOffset) -> Option<DateTime<Utc>> {
    if s.len() < 2 {
        return None;
    }

    let (digits, unit) = s.split_at(s.len() - 1);
    let n: u32 = digits.parse().ok()?;
    if n == 0 {
        return None;
    }

    let result = match unit {
        "d" => now - Duration::days(n as i64),
        "w" => now - Duration::weeks(n as i64),
        "m" => subtract_months(now, n, tz)?,
        _ => return None,
    };

    Some(start_of_day(result, tz))
}

/// Subtract N months from a datetime, clamping the day to the last valid day
/// of the target month (e.g., March 31 minus 1 month = Feb 28).
/// Returns start-of-day in the user's local timezone.
fn subtract_months(dt: DateTime<Utc>, months: u32, tz: &FixedOffset) -> Option<DateTime<Utc>> {
    let local = dt.with_timezone(tz);
    let total_months = local.year() * 12 + local.month() as i32 - 1 - months as i32;
    let target_year = total_months.div_euclid(12);
    let target_month = (total_months.rem_euclid(12) + 1) as u32;

    let max_day = days_in_month(target_year, target_month);
    let target_day = local.day().min(max_day);

    let naive = NaiveDate::from_ymd_opt(target_year, target_month, target_day)?
        .and_hms_opt(0, 0, 0)?;
    let local_dt = tz.from_local_datetime(&naive).single()?;
    Some(local_dt.with_timezone(&Utc))
}

/// Returns the number of days in a given month.
fn days_in_month(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// Build a DateRange from --from and --to arguments plus optional relative terms.
/// The `tz` offset determines how local dates/times are interpreted.
pub fn build_date_range(
    from: Option<&str>,
    to: Option<&str>,
    relative: Option<&str>,
    now: DateTime<Utc>,
    tz: &FixedOffset,
) -> Option<DateRange> {
    if let Some(term) = relative {
        return parse_relative(term, now, tz);
    }

    let start = from.and_then(|s| parse_absolute(s, tz).or_else(|| parse_duration(s, now, tz)));
    let end = to.and_then(|s| parse_absolute(s, tz).or_else(|| parse_duration(s, now, tz)));

    if start.is_none() && end.is_none() {
        return None;
    }

    Some(DateRange { start, end })
}

/// Compute the start of day (midnight) in the user's local timezone,
/// returned as a UTC DateTime.
fn start_of_day(dt: DateTime<Utc>, tz: &FixedOffset) -> DateTime<Utc> {
    let local = dt.with_timezone(tz);
    let naive_midnight = NaiveDate::from_ymd_opt(local.year(), local.month(), local.day())
        .and_then(|d| d.and_hms_opt(0, 0, 0));
    match naive_midnight {
        Some(nm) => tz
            .from_local_datetime(&nm)
            .single()
            .map(|ldt| ldt.with_timezone(&Utc))
            .unwrap_or(dt),
        None => dt,
    }
}

/// Compute the start of the week (Monday midnight) in the user's local timezone.
fn start_of_week(dt: DateTime<Utc>, tz: &FixedOffset) -> DateTime<Utc> {
    let local = dt.with_timezone(tz);
    let days_since_monday = local.weekday().num_days_from_monday();
    let monday = local - Duration::days(days_since_monday as i64);
    let naive_midnight = NaiveDate::from_ymd_opt(monday.year(), monday.month(), monday.day())
        .and_then(|d| d.and_hms_opt(0, 0, 0));
    match naive_midnight {
        Some(nm) => tz
            .from_local_datetime(&nm)
            .single()
            .map(|ldt| ldt.with_timezone(&Utc))
            .unwrap_or(dt),
        None => dt,
    }
}

/// Compute the start of the month (1st at midnight) in the user's local timezone.
fn start_of_month(dt: DateTime<Utc>, tz: &FixedOffset) -> DateTime<Utc> {
    let local = dt.with_timezone(tz);
    let naive = NaiveDate::from_ymd_opt(local.year(), local.month(), 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0));
    match naive {
        Some(nm) => tz
            .from_local_datetime(&nm)
            .single()
            .map(|ldt| ldt.with_timezone(&Utc))
            .unwrap_or(dt),
        None => dt,
    }
}

/// Compute the start of the next month in the user's local timezone.
fn next_month(dt: DateTime<Utc>, tz: &FixedOffset) -> DateTime<Utc> {
    let local = dt.with_timezone(tz);
    let (year, month) = if local.month() == 12 {
        (local.year() + 1, 1)
    } else {
        (local.year(), local.month() + 1)
    };
    let naive = NaiveDate::from_ymd_opt(year, month, 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0));
    match naive {
        Some(nm) => tz
            .from_local_datetime(&nm)
            .single()
            .map(|ldt| ldt.with_timezone(&Utc))
            .unwrap_or(dt),
        None => dt,
    }
}

/// Compute the start of the previous month in the user's local timezone.
fn prev_month(dt: DateTime<Utc>, tz: &FixedOffset) -> DateTime<Utc> {
    let local = dt.with_timezone(tz);
    let (year, month) = if local.month() == 1 {
        (local.year() - 1, 12)
    } else {
        (local.year(), local.month() - 1)
    };
    let naive = NaiveDate::from_ymd_opt(year, month, 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0));
    match naive {
        Some(nm) => tz
            .from_local_datetime(&nm)
            .single()
            .map(|ldt| ldt.with_timezone(&Utc))
            .unwrap_or(dt),
        None => dt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    fn utc_tz() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn fixed_now() -> DateTime<Utc> {
        // Wednesday, Jan 22, 2026 at noon UTC
        Utc.with_ymd_and_hms(2026, 1, 22, 12, 0, 0).unwrap()
    }

    #[test]
    fn test_parse_today() {
        let range = parse_relative("today", fixed_now(), &utc_tz()).unwrap();
        assert_eq!(
            range.start,
            Some(Utc.with_ymd_and_hms(2026, 1, 22, 0, 0, 0).unwrap())
        );
        assert_eq!(
            range.end,
            Some(Utc.with_ymd_and_hms(2026, 1, 23, 0, 0, 0).unwrap())
        );
    }

    #[test]
    fn test_parse_yesterday() {
        let range = parse_relative("yesterday", fixed_now(), &utc_tz()).unwrap();
        assert_eq!(
            range.start,
            Some(Utc.with_ymd_and_hms(2026, 1, 21, 0, 0, 0).unwrap())
        );
        assert_eq!(
            range.end,
            Some(Utc.with_ymd_and_hms(2026, 1, 22, 0, 0, 0).unwrap())
        );
    }

    #[test]
    fn test_parse_this_week() {
        let range = parse_relative("this-week", fixed_now(), &utc_tz()).unwrap();
        // Jan 22 is a Thursday, so week starts Monday Jan 19
        assert_eq!(
            range.start,
            Some(Utc.with_ymd_and_hms(2026, 1, 19, 0, 0, 0).unwrap())
        );
        assert_eq!(
            range.end,
            Some(Utc.with_ymd_and_hms(2026, 1, 26, 0, 0, 0).unwrap())
        );
    }

    #[test]
    fn test_parse_last_week() {
        let range = parse_relative("last-week", fixed_now(), &utc_tz()).unwrap();
        assert_eq!(
            range.start,
            Some(Utc.with_ymd_and_hms(2026, 1, 12, 0, 0, 0).unwrap())
        );
        assert_eq!(
            range.end,
            Some(Utc.with_ymd_and_hms(2026, 1, 19, 0, 0, 0).unwrap())
        );
    }

    #[test]
    fn test_parse_this_month() {
        let range = parse_relative("this-month", fixed_now(), &utc_tz()).unwrap();
        assert_eq!(
            range.start,
            Some(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap())
        );
        assert_eq!(
            range.end,
            Some(Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap())
        );
    }

    #[test]
    fn test_parse_last_month() {
        let range = parse_relative("last-month", fixed_now(), &utc_tz()).unwrap();
        assert_eq!(
            range.start,
            Some(Utc.with_ymd_and_hms(2025, 12, 1, 0, 0, 0).unwrap())
        );
        assert_eq!(
            range.end,
            Some(Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap())
        );
    }

    #[test]
    fn test_parse_absolute_date() {
        let dt = parse_absolute("2026-01-15", &utc_tz()).unwrap();
        assert_eq!(dt.year(), 2026);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 15);
    }

    #[test]
    fn test_parse_absolute_datetime() {
        let dt = parse_absolute("2026-01-15T10:30:00Z", &utc_tz()).unwrap();
        assert_eq!(dt.hour(), 10);
    }

    #[test]
    fn test_parse_absolute_invalid() {
        assert!(parse_absolute("not-a-date", &utc_tz()).is_none());
    }

    #[test]
    fn test_date_range_contains() {
        let range = DateRange {
            start: Some(Utc.with_ymd_and_hms(2026, 1, 20, 0, 0, 0).unwrap()),
            end: Some(Utc.with_ymd_and_hms(2026, 1, 25, 0, 0, 0).unwrap()),
        };
        let inside = Utc.with_ymd_and_hms(2026, 1, 22, 12, 0, 0).unwrap();
        let before = Utc.with_ymd_and_hms(2026, 1, 19, 12, 0, 0).unwrap();
        let after = Utc.with_ymd_and_hms(2026, 1, 26, 12, 0, 0).unwrap();

        assert!(range.contains(&inside));
        assert!(!range.contains(&before));
        assert!(!range.contains(&after));
    }

    #[test]
    fn test_date_range_open() {
        let range = DateRange::open();
        let any_time = Utc.with_ymd_and_hms(2020, 6, 15, 0, 0, 0).unwrap();
        assert!(range.contains(&any_time));
    }

    #[test]
    fn test_build_date_range_relative() {
        let range = build_date_range(None, None, Some("today"), fixed_now(), &utc_tz()).unwrap();
        assert!(range.start.is_some());
        assert!(range.end.is_some());
    }

    #[test]
    fn test_build_date_range_absolute() {
        let range =
            build_date_range(Some("2026-01-01"), Some("2026-01-31"), None, fixed_now(), &utc_tz()).unwrap();
        assert_eq!(range.start.unwrap().day(), 1);
        assert_eq!(range.end.unwrap().day(), 31);
    }

    #[test]
    fn test_build_date_range_none() {
        let range = build_date_range(None, None, None, fixed_now(), &utc_tz());
        assert!(range.is_none());
    }

    // === Duration parsing tests ===

    #[test]
    fn test_parse_duration_days() {
        // 3d from Jan 22 2026 noon → Jan 19 2026 00:00 UTC
        let result = parse_duration("3d", fixed_now(), &utc_tz()).unwrap();
        assert_eq!(
            result,
            Utc.with_ymd_and_hms(2026, 1, 19, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn test_parse_duration_weeks() {
        // 2w from Jan 22 2026 noon → Jan 8 2026 00:00 UTC
        let result = parse_duration("2w", fixed_now(), &utc_tz()).unwrap();
        assert_eq!(
            result,
            Utc.with_ymd_and_hms(2026, 1, 8, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn test_parse_duration_months() {
        // 1m from Jan 22 2026 noon → Dec 22 2025 00:00 UTC
        let result = parse_duration("1m", fixed_now(), &utc_tz()).unwrap();
        assert_eq!(
            result,
            Utc.with_ymd_and_hms(2025, 12, 22, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn test_parse_duration_months_clamp() {
        // 1m from March 31 → Feb 28 (non-leap year 2026)
        let march_31 = Utc.with_ymd_and_hms(2026, 3, 31, 12, 0, 0).unwrap();
        let result = parse_duration("1m", march_31, &utc_tz()).unwrap();
        assert_eq!(
            result,
            Utc.with_ymd_and_hms(2026, 2, 28, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn test_parse_duration_months_multiple() {
        // 3m from Jan 22 2026 → Oct 22 2025
        let result = parse_duration("3m", fixed_now(), &utc_tz()).unwrap();
        assert_eq!(
            result,
            Utc.with_ymd_and_hms(2025, 10, 22, 0, 0, 0).unwrap()
        );
    }

    #[test]
    fn test_parse_duration_invalid() {
        let now = fixed_now();
        let tz = utc_tz();
        assert!(parse_duration("abc", now, &tz).is_none());
        assert!(parse_duration("3x", now, &tz).is_none());
        assert!(parse_duration("", now, &tz).is_none());
        assert!(parse_duration("d", now, &tz).is_none());
        assert!(parse_duration("0d", now, &tz).is_none());
    }

    #[test]
    fn test_build_date_range_with_duration() {
        // --from 2w should produce start = Jan 8 2026 00:00 UTC
        let range = build_date_range(Some("2w"), None, None, fixed_now(), &utc_tz()).unwrap();
        assert_eq!(
            range.start,
            Some(Utc.with_ymd_and_hms(2026, 1, 8, 0, 0, 0).unwrap())
        );
        assert!(range.end.is_none());
    }

    #[test]
    fn test_build_date_range_mixed() {
        // --from 4w --to 2w
        let range = build_date_range(Some("4w"), Some("2w"), None, fixed_now(), &utc_tz()).unwrap();
        assert_eq!(
            range.start,
            Some(Utc.with_ymd_and_hms(2025, 12, 25, 0, 0, 0).unwrap())
        );
        assert_eq!(
            range.end,
            Some(Utc.with_ymd_and_hms(2026, 1, 8, 0, 0, 0).unwrap())
        );
    }

    // === Timezone-aware date range tests ===

    #[test]
    fn today_at_utc_2am_with_utc_minus_5_is_previous_day() {
        // At UTC 2am on Jan 22, local time in UTC-5 is Jan 21 at 9pm
        // So "today" in UTC-5 should be Jan 21 local = Jan 21 05:00 UTC to Jan 22 05:00 UTC
        let utc_minus_5 = FixedOffset::west_opt(5 * 3600).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 1, 22, 2, 0, 0).unwrap();
        let range = parse_relative("today", now, &utc_minus_5).unwrap();
        // Local midnight Jan 21 = UTC Jan 21 05:00
        assert_eq!(
            range.start,
            Some(Utc.with_ymd_and_hms(2026, 1, 21, 5, 0, 0).unwrap())
        );
        // Local midnight Jan 22 = UTC Jan 22 05:00
        assert_eq!(
            range.end,
            Some(Utc.with_ymd_and_hms(2026, 1, 22, 5, 0, 0).unwrap())
        );
    }

    #[test]
    fn today_at_utc_8pm_with_utc_plus_9_is_next_day() {
        // At UTC 8pm on Jan 22, local time in UTC+9 is Jan 23 at 5am
        // So "today" in UTC+9 should be Jan 23 local = Jan 22 15:00 UTC to Jan 23 15:00 UTC
        let utc_plus_9 = FixedOffset::east_opt(9 * 3600).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 1, 22, 20, 0, 0).unwrap();
        let range = parse_relative("today", now, &utc_plus_9).unwrap();
        // Local midnight Jan 23 = UTC Jan 22 15:00
        assert_eq!(
            range.start,
            Some(Utc.with_ymd_and_hms(2026, 1, 22, 15, 0, 0).unwrap())
        );
        // Local midnight Jan 24 = UTC Jan 23 15:00
        assert_eq!(
            range.end,
            Some(Utc.with_ymd_and_hms(2026, 1, 23, 15, 0, 0).unwrap())
        );
    }

    #[test]
    fn date_only_absolute_with_offset_gives_local_midnight_in_utc() {
        // "2026-01-15" in UTC-5 should be Jan 15 00:00 local = Jan 15 05:00 UTC
        let utc_minus_5 = FixedOffset::west_opt(5 * 3600).unwrap();
        let dt = parse_absolute("2026-01-15", &utc_minus_5).unwrap();
        assert_eq!(dt, Utc.with_ymd_and_hms(2026, 1, 15, 5, 0, 0).unwrap());
    }

    #[test]
    fn full_datetime_absolute_ignores_tz_parameter() {
        // Full RFC 3339 datetime should be parsed as-is regardless of tz param
        let utc_minus_5 = FixedOffset::west_opt(5 * 3600).unwrap();
        let dt = parse_absolute("2026-01-15T10:30:00Z", &utc_minus_5).unwrap();
        assert_eq!(dt.hour(), 10);
    }
}
