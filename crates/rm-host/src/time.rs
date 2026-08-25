//! Days, in and out.
//!
//! A decision log is read and written at the granularity of days: somebody
//! knows a choice was made in March, not that it was made at 14:32:07. Both
//! directions here deal in `YYYY-MM-DD` and neither carries a clock time, which
//! keeps the store from implying a precision nobody has.
//!
//! Written out rather than pulled in. This workspace has chosen the small thing
//! over the dependency four times -- exact search over an approximate index, a
//! hand-written argument parser over `syn`, ports over an HTTP client, a linear
//! scan over an ANN crate -- and a date library for two functions is the same
//! trade. The conversions are Howard Hinnant's `civil_from_days` and
//! `days_from_civil`, exact for every day in the proleptic Gregorian calendar
//! and what the date libraries run anyway.

use rm_engine::Timestamp;

const MS_PER_DAY: i64 = 86_400_000;

/// A millisecond timestamp as `YYYY-MM-DD`, in UTC.
pub fn format_day(ms: Timestamp) -> String {
    // Floor division, so a timestamp before the epoch lands on the day it
    // belongs to rather than the one after.
    let (y, m, d) = civil_from_days(ms.div_euclid(MS_PER_DAY));
    format!("{y:04}-{m:02}-{d:02}")
}

/// `YYYY-MM-DD` as the first millisecond of that day, in UTC.
///
/// Refuses anything else, and says what it wanted. Falling back to the clock on
/// a date it could not read would record the decision under today's date, which
/// is the one thing the caller was trying to avoid by passing a date at all.
pub fn parse_day(text: &str) -> Result<Timestamp, String> {
    let bad = || {
        format!("{text:?} is not a date -- write it as YYYY-MM-DD, like \"2026-03-14\". Days are the granularity: a decision is made on a day, not at a time.")
    };
    let parts: Vec<&str> = text.split('-').collect();
    let [y, m, d] = parts[..] else {
        return Err(bad());
    };
    if y.len() != 4 || m.len() != 2 || d.len() != 2 {
        return Err(bad());
    }
    let (y, m, d) = (
        y.parse::<i64>().map_err(|_| bad())?,
        m.parse::<u32>().map_err(|_| bad())?,
        d.parse::<u32>().map_err(|_| bad())?,
    );
    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
        // Named apart from a malformed string: the shape was right and the day
        // does not exist, which is a different mistake and usually one digit.
        return Err(format!(
            "there is no {y:04}-{m:02}-{d:02}. Check the day and the month."
        ));
    }
    Ok(days_from_civil(y, m, d) * MS_PER_DAY)
}

/// `YYYY-MM-DD` as the *last* millisecond of that day, in UTC.
///
/// What a date means when it is used as a point in time, rather than as the
/// moment something began.
///
/// `--at` writes the start of a day, because a decision made on the 14th held
/// from the 14th. A question asked *about* the 14th means something else: "what
/// was true then" and "what did we know by then" both want the day to be over.
/// Read at the start instead, and a decision recorded at nine in the morning is
/// invisible to a query naming its own day -- which is the first thing anybody
/// tries.
pub fn parse_day_end(text: &str) -> Result<Timestamp, String> {
    parse_day(text).map(|start| start + MS_PER_DAY - 1)
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        // The 400-year rule, not just the 4-year one: 1900 was not a leap year
        // and 2000 was.
        2 if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (y + i64::from(m <= 2), m as u32, d as u32)
}

fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from(if m > 2 { m - 3 } else { m + 9 });
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The two clocks a read is answered under: what held at `valid`, as known by
/// `tx`.
///
/// One value rather than two parameters because both are `Timestamp` and they
/// pass through three layers -- `decisions` to `chain` to `edges_into` -- where
/// swapping them compiles and returns a plausible wrong answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct At {
    /// When in the world. Filters on `Version::valid.from`.
    pub valid: Timestamp,
    /// When the store learned it. Filters on `provenance.observed_at`.
    pub tx: Timestamp,
}

impl At {
    /// Everything the store holds.
    ///
    /// Deliberately not a `Default` impl. `Engine::edges_from` makes the
    /// argument: an edge read without a `tx_t` is a claim about now that
    /// quietly stops being reproducible -- so every call site names what it is
    /// asking rather than inheriting it.
    ///
    /// `MAX` rather than the current time, because that is what the decision
    /// reads did before they took an `At`. `now` would silently drop a decision
    /// recorded with a future `--at`.
    pub fn latest() -> Self {
        At {
            valid: Timestamp::MAX,
            tx: Timestamp::MAX,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Known dates, including the ones the arithmetic gets wrong if it is
    /// wrong: an epoch boundary, a leap day, a century that is not a leap year,
    /// one that is, and a date before the epoch.
    #[test]
    fn days_convert_both_ways() {
        for (ms, day) in [
            (0i64, "1970-01-01"),
            (86_399_999, "1970-01-01"),
            (86_400_000, "1970-01-02"),
            (951_782_400_000, "2000-02-29"),
            (4_107_542_400_000, "2100-03-01"),
            (1_787_532_411_419, "2026-08-24"),
            (-1, "1969-12-31"),
            (-86_400_000, "1969-12-31"),
        ] {
            assert_eq!(format_day(ms), day, "formatting {ms}");
        }
        for day in [
            "1970-01-01",
            "2000-02-29",
            "2026-08-24",
            "2100-03-01",
            "1969-12-31",
            "1899-12-31",
        ] {
            let ms = parse_day(day).unwrap_or_else(|e| panic!("{day}: {e}"));
            assert_eq!(format_day(ms), day, "round trip for {day}");
        }
    }

    /// A date used as a point in time covers the whole day it names.
    #[test]
    fn a_day_as_a_point_in_time_is_its_last_millisecond() {
        let start = parse_day("2026-03-14").unwrap();
        let end = parse_day_end("2026-03-14").unwrap();
        assert_eq!(
            end - start,
            86_399_999,
            "the whole day, less one millisecond"
        );
        assert_eq!(
            format_day(end),
            "2026-03-14",
            "and still that day, not the next"
        );
        // The case this exists for: something recorded during the day is
        // visible to a query naming that day.
        let nine_am = start + 9 * 3_600_000;
        assert!(nine_am <= end);
        assert!(nine_am > start);
        // A bad date is refused here too, not only by `parse_day`.
        assert!(parse_day_end("2026-02-30").is_err());
    }

    #[test]
    fn a_date_that_is_not_one_is_refused_and_says_what_it_wanted() {
        for bad in [
            "",
            "today",
            "2026",
            "2026-3-14",
            "26-03-14",
            "2026/03/14",
            "2026-03-14T09:00:00Z",
            "x-y-z",
        ] {
            let e = parse_day(bad).unwrap_err();
            assert!(e.contains("YYYY-MM-DD"), "for {bad:?}: {e}");
        }
    }

    /// A day that does not exist is a different mistake from a malformed
    /// string, and usually one digit rather than the whole format.
    #[test]
    fn a_day_that_does_not_exist_is_named_as_such() {
        for bad in [
            "2026-02-30",
            "2026-13-01",
            "2026-00-10",
            "2100-02-29",
            "2026-04-31",
        ] {
            let e = parse_day(bad).unwrap_err();
            assert!(e.contains("there is no"), "for {bad:?}: {e}");
        }
        assert!(parse_day("2000-02-29").is_ok());
        assert!(parse_day("2024-02-29").is_ok());
    }

    #[test]
    fn latest_is_the_end_of_both_axes_and_not_the_current_time() {
        let at = At::latest();
        assert_eq!(at.valid, Timestamp::MAX);
        assert_eq!(at.tx, Timestamp::MAX);
    }
}
