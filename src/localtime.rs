//! Timestamps are UTC on ingest (every provider normalises); this module is
//! the single place they are turned into the system-local wall clock for
//! display and for day-based filtering. The helpers are generic over
//! [`TimeZone`] so tests can pin an offset; call sites use the plain
//! wrappers, which follow the system zone (`TZ`, then `/etc/localtime`).

use std::fmt::Display;

use chrono::{DateTime, Local, NaiveDate, TimeZone, Utc};

pub fn local_date_in<Tz: TimeZone>(ts: DateTime<Utc>, tz: &Tz) -> NaiveDate {
    ts.with_timezone(tz).date_naive()
}

pub fn fmt_date_in<Tz: TimeZone>(ts: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    ts.with_timezone(tz).format("%Y-%m-%d").to_string()
}

pub fn fmt_datetime_in<Tz: TimeZone>(ts: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    ts.with_timezone(tz).format("%Y-%m-%d %H:%M").to_string()
}

pub fn fmt_time_in<Tz: TimeZone>(ts: DateTime<Utc>, tz: &Tz) -> String
where
    Tz::Offset: Display,
{
    ts.with_timezone(tz).format("%H:%M").to_string()
}

pub fn fmt_date(ts: DateTime<Utc>) -> String {
    fmt_date_in(ts, &Local)
}

pub fn fmt_datetime(ts: DateTime<Utc>) -> String {
    fmt_datetime_in(ts, &Local)
}

pub fn fmt_time(ts: DateTime<Utc>) -> String {
    fmt_time_in(ts, &Local)
}

/// Today's date in the system zone.
pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;

    fn utc(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    fn east10() -> FixedOffset {
        FixedOffset::east_opt(10 * 3600).unwrap()
    }

    fn west5() -> FixedOffset {
        FixedOffset::west_opt(5 * 3600).unwrap()
    }

    #[test]
    fn date_crosses_midnight_east_but_not_west() {
        let ts = utc("2026-10-28T22:00:00Z");
        assert_eq!(fmt_date_in(ts, &east10()), "2026-10-29");
        assert_eq!(fmt_date_in(ts, &west5()), "2026-10-28");
        assert_eq!(
            local_date_in(ts, &east10()),
            NaiveDate::from_ymd_opt(2026, 10, 29).unwrap()
        );
    }

    #[test]
    fn datetime_and_time_use_the_zone() {
        let ts = utc("2026-10-28T22:05:00Z");
        assert_eq!(fmt_datetime_in(ts, &east10()), "2026-10-29 08:05");
        assert_eq!(fmt_datetime_in(ts, &west5()), "2026-10-28 17:05");
        assert_eq!(fmt_time_in(ts, &east10()), "08:05");
    }
}
