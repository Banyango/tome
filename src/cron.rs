//! Standard 5-field cron expressions (`minute hour day-of-month month
//! day-of-week`), evaluated in local time.
//!
//! Fields take `*`, numbers, `a-b` ranges, `/step`s and `,` lists; months
//! and weekdays also take names (`jan`, `mon`). Weekday 0 and 7 are Sunday.
//! As in cron, when both day fields are restricted a day matching either
//! one matches.

use chrono::{Datelike, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike};

#[derive(Debug, Clone)]
pub struct Cron {
    pub source: String,
    minutes: u64,
    hours: u64,
    days: u64,
    months: u64,
    weekdays: u64,
    days_any: bool,
    weekdays_any: bool,
}

const MONTHS: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const WEEKDAYS: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

impl Cron {
    pub fn parse(expr: &str) -> Result<Cron, String> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(format!(
                "invalid cron expression `{expr}`: expected 5 fields (minute hour day-of-month month day-of-week), found {}",
                fields.len()
            ));
        }
        let err = |e: String| format!("invalid cron expression `{expr}`: {e}");
        let minutes = field(fields[0], "minute", 0, 59, &[]).map_err(err)?;
        let hours = field(fields[1], "hour", 0, 23, &[]).map_err(err)?;
        let days = field(fields[2], "day-of-month", 1, 31, &[]).map_err(err)?;
        let months = field(fields[3], "month", 1, 12, &MONTHS).map_err(err)?;
        let mut weekdays = field(fields[4], "day-of-week", 0, 7, &WEEKDAYS).map_err(err)?;
        if weekdays & (1 << 7) != 0 {
            weekdays |= 1;
        }
        Ok(Cron {
            source: expr.to_string(),
            minutes,
            hours,
            days,
            months,
            weekdays,
            days_any: fields[2] == "*",
            weekdays_any: fields[4] == "*",
        })
    }

    fn day_matches(&self, d: NaiveDate) -> bool {
        let dom = self.days & (1 << d.day()) != 0;
        let dow = self.weekdays & (1 << d.weekday().num_days_from_sunday()) != 0;
        match (self.days_any, self.weekdays_any) {
            (false, false) => dom || dow,
            _ => dom && dow,
        }
    }

    /// The first scheduled local time strictly after `after`.
    pub fn next_after(&self, after: NaiveDateTime) -> Option<NaiveDateTime> {
        let mut t = after.with_second(0)?.with_nanosecond(0)? + Duration::minutes(1);
        // Five years covers every satisfiable expression (Feb 29 included).
        let limit = t + Duration::days(366 * 5);
        while t < limit {
            if self.months & (1 << t.month()) == 0 {
                let (y, m) = if t.month() == 12 {
                    (t.year() + 1, 1)
                } else {
                    (t.year(), t.month() + 1)
                };
                t = NaiveDate::from_ymd_opt(y, m, 1)?.and_hms_opt(0, 0, 0)?;
                continue;
            }
            if !self.day_matches(t.date()) {
                t = (t.date() + Duration::days(1)).and_hms_opt(0, 0, 0)?;
                continue;
            }
            if self.hours & (1 << t.hour()) == 0 {
                t = t.with_minute(0)? + Duration::hours(1);
                continue;
            }
            if self.minutes & (1 << t.minute()) == 0 {
                t += Duration::minutes(1);
                continue;
            }
            return Some(t);
        }
        None
    }

    /// The next scheduled time after `after`, as a real local instant.
    /// Times that fall in a DST gap don't exist and are skipped.
    pub fn next_local(&self, after: chrono::DateTime<Local>) -> Option<chrono::DateTime<Local>> {
        let mut t = after.naive_local();
        for _ in 0..8 {
            t = self.next_after(t)?;
            if let Some(local) = Local.from_local_datetime(&t).earliest() {
                if local > after {
                    return Some(local);
                }
            }
        }
        None
    }
}

fn field(spec: &str, what: &str, min: u32, max: u32, names: &[&str]) -> Result<u64, String> {
    let value = |s: &str| -> Result<u32, String> {
        let lower = s.to_ascii_lowercase();
        if let Some(i) = names.iter().position(|n| *n == lower) {
            // Months are 1-based, weekdays 0-based.
            return Ok(i as u32 + min);
        }
        let n: u32 = s
            .parse()
            .map_err(|_| format!("`{s}` is not a valid {what}"))?;
        if n < min || n > max {
            return Err(format!("{what} `{n}` is out of range ({min}-{max})"));
        }
        Ok(n)
    };
    let mut bits = 0u64;
    for part in spec.split(',') {
        if part.is_empty() {
            return Err(format!("empty item in {what} field `{spec}`"));
        }
        let (range, step) = match part.split_once('/') {
            Some((r, s)) => {
                let step: u32 = s
                    .parse()
                    .map_err(|_| format!("`{s}` is not a valid step in {what} field"))?;
                if step == 0 {
                    return Err(format!("step 0 in {what} field"));
                }
                (r, step)
            }
            None => (part, 1),
        };
        let (lo, hi) = if range == "*" {
            (min, max)
        } else if let Some((a, b)) = range.split_once('-') {
            let (a, b) = (value(a)?, value(b)?);
            if a > b {
                return Err(format!("{what} range `{range}` runs backwards"));
            }
            (a, b)
        } else {
            let a = value(range)?;
            // `5/15` runs from 5 to the end.
            (a, if part.contains('/') { max } else { a })
        };
        let mut n = lo;
        while n <= hi {
            bits |= 1 << n;
            n += step;
        }
    }
    Ok(bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    fn next(expr: &str, from: &str) -> String {
        Cron::parse(expr)
            .unwrap()
            .next_after(at(from))
            .unwrap()
            .format("%Y-%m-%d %H:%M")
            .to_string()
    }

    #[test]
    fn schedules() {
        assert_eq!(next("* * * * *", "2026-01-01 10:00"), "2026-01-01 10:01");
        assert_eq!(
            next("0 9 * * 1-5", "2026-09-25 09:00"),
            "2026-09-28 09:00",
            "friday to monday"
        );
        assert_eq!(next("*/15 * * * *", "2026-01-01 10:07"), "2026-01-01 10:15");
        assert_eq!(next("30 2 1 * *", "2026-01-15 00:00"), "2026-02-01 02:30");
        assert_eq!(next("0 0 29 feb *", "2026-03-01 00:00"), "2028-02-29 00:00");
        assert_eq!(
            next("0 12 * jan-mar sun", "2026-03-30 00:00"),
            "2027-01-03 12:00"
        );
        assert_eq!(
            next("0 0 * * 7", "2026-09-27 00:00"),
            "2026-10-04 00:00",
            "7 is sunday"
        );
        // Both day fields restricted: either matches.
        assert_eq!(next("0 0 13 * fri", "2026-09-01 00:00"), "2026-09-04 00:00");
        assert_eq!(next("5/20 * * * *", "2026-01-01 10:46"), "2026-01-01 11:05");
    }

    #[test]
    fn errors() {
        for bad in [
            "* * * *",
            "60 * * * *",
            "* 24 * * *",
            "* * 0 * *",
            "* * * 13 *",
            "*/0 * * * *",
            "5-1 * * * *",
            "x * * * *",
            "1,,2 * * * *",
        ] {
            assert!(Cron::parse(bad).is_err(), "{bad}");
        }
        assert!(Cron::parse("0 0 31 2 *")
            .unwrap()
            .next_after(at("2026-01-01 00:00"))
            .is_none());
    }
}
