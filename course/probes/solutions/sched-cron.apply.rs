//! probe = "sched/cron@apply"
//! title = "Next run, and the two day fields that are a union"
//! hint = "A field is a table of what it allows, `0 0 1 * 0` is the 1st OR Sunday, and a missed window is the caller's policy — the schedule only answers `next_after`, `matches` and `catch_up`."
//!
//! Step `jobq/3.10-cron-schedules` gives the engine a way to say "run this every night at 02:00"
//! and, more importantly, to say where that decision stops: a schedule is *arithmetic*, and
//! everything about what to do with a missed run is the caller's policy.
//!
//! Three things about cron are traps, and each one has a rule that can be written down:
//!
//! 1. **the two day fields are not an AND.** `0 0 1 * 0` is "the first of the month" *or*
//!    "Sunday", because that is what the classic implementations do and what a schedule written by
//!    somebody else already means. The rule keys on whether a field was `*`: with both fields
//!    restricted the job fires on either, with one of them `*` only the other one decides. An
//!    engine that ANDs them silently changes when a customer's nightly job runs — twice a month
//!    instead of thirty times;
//! 2. **a minute that never happens.** [`Zone`] answers the offset in effect at an instant, so a
//!    zone whose offset moves forward has a wall-clock hour that does not exist: `0 2 * * *` fires
//!    on that day not at all, and the honest answer to `next_after` is the next day. An offset
//!    that moves backwards makes a wall-clock hour happen *twice*, and then the same schedule
//!    really does have two instants that match;
//! 3. **the outage.** A schedule that owes a run per day and has not run for three days owes three
//!    runs — or it owes one and a count of what it dropped, which is [`catch_up`]'s policy. Both
//!    are defensible; what is not defensible is a worker that fires 4 320 times because nobody
//!    wrote the policy down.
//!
//! ## What you write
//!
//! * [`Schedule::parse`] — five fields, each a comma-separated list of `*`, `N`, `N-M`, `*/S` or
//!   `N-M/S`;
//! * [`Schedule::matches`] — is this instant one the schedule names, in the caller's zone?
//! * [`Schedule::next_after`] — the first instant strictly after a reading, or `None` when the
//!   schedule can never fire again (there is no 30 February);
//! * [`Schedule::catch_up`] — what a gap owes: the runs to make, oldest first, and how many were
//!   dropped rather than swallowed.
//!
//! [`Schedule`]'s representation, [`Zone`], [`Fixed`], [`CatchUp`], [`Error`] and the civil-time
//! helpers ([`wall_fields`], [`civil_to_minutes`]) are given: turning a minute count into a
//! year-month-day is arithmetic from a calendar, not from this course, and the tables a field
//! parses into are the shape the rule needs.
//!
//! ## What the tests ask
//!
//! Every error a parser has to catch, with the field it names. A field is a list of ranges of steps,
//! and `7` is Sunday. `* * * * *` matches everywhere and `0 9 * * *` fires once a day, at exactly
//! 09:00 and not one minute later. The offset is the caller's, so 09:00 local is 07:00 UTC at +02:00
//! and 14:00 UTC at -05:00. Then the union rule from both sides, a schedule for a date that does not
//! exist, an hour that does not exist, an hour that happens twice, and a three-day outage.
//!
//! ## Where this is honest, and where it is not
//!
//! One file, no dependencies, **no `chrono` and no `chrono-tz`**: the time zone here is a closed
//! form (`offset_minutes(instant)`), which is exactly what a real timezone database *is* — a
//! function from an instant to an offset — so a transition is modelled by the caller and the two
//! DST cases below are the real ones rather than approximations. What is missing is the database
//! itself: the real step reads `chrono-tz`, so its zone is a name rather than a closure, and a
//! probe that shipped a tz database would not be one file.
//!
//! Also missing on purpose: nothing here wakes a worker (`sched/delayed-timers@apply`), nothing
//! here decides *what* a run is (the engine's scheduler does), and `catch_up` enumerates at most
//! [`HORIZON_MINUTES`] of a gap — a caller passing a longer outage gets the horizon's worth and
//! should be asking a different question.

/// The values a field may name, per position, as the error messages and the tables use them.
pub const MINUTE: (u32, u32) = (0, 59);
/// Hours of the day.
pub const HOUR: (u32, u32) = (0, 23);
/// Days of the month. Index 0 of the table is never set: there is no day zero.
pub const DAY: (u32, u32) = (1, 31);
/// Months of the year.
pub const MONTH: (u32, u32) = (1, 12);
/// Days of the week, Sunday as 0. `7` is accepted as a second spelling of Sunday.
pub const WEEKDAY: (u32, u32) = (0, 7);

/// How far [`Schedule::next_after`] and [`Schedule::catch_up`] are willing to look, in minutes.
///
/// Bounded on purpose: `0 0 30 2 *` never fires, and a search that does not stop is a worker that
/// does not come back. A year is longer than any interval a cron expression can name.
pub const HORIZON_MINUTES: i64 = 366 * 24 * 60;

/// A time zone, as a schedule sees it: the offset that is in effect at an instant.
///
/// This is what a timezone database is, minus the database: the offset is a function of the
/// instant, so a transition is a step in that function and both sides of it can be tested without
/// shipping a tz database or depending on the one this machine happens to have.
pub trait Zone {
    /// Minutes east of UTC that a clock in this zone reads at `utc_minutes`.
    fn offset_minutes(&self, utc_minutes: i64) -> i64;
}

/// A zone that never changes. What most deployments actually run, and the ones where DST is a
/// non-event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fixed {
    /// Minutes east of UTC.
    pub offset_minutes: i64,
}

impl Zone for Fixed {
    fn offset_minutes(&self, _utc_minutes: i64) -> i64 {
        self.offset_minutes
    }
}

/// Why an expression was refused.
///
/// The field is named in every variant, because "invalid cron expression" is the error message that
/// makes somebody read the source of the program that printed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not five fields.
    Fields {
        /// How many were found.
        found: usize,
    },
    /// A number outside what this field allows.
    Range {
        /// Which field.
        field: &'static str,
        /// The number that was out of range.
        value: u32,
        /// Lowest value the field allows.
        from: u32,
        /// Highest value the field allows.
        to: u32,
    },
    /// `5-2`: a range written the wrong way round, which is a typo rather than an empty set.
    Reversed {
        /// Which field.
        field: &'static str,
        /// The start of the range.
        from: u32,
        /// The end of the range.
        to: u32,
    },
    /// `*/0`: a step of zero names nothing and is how an infinite loop is written by accident.
    Step {
        /// Which field.
        field: &'static str,
    },
    /// An item that is none of `*`, `N`, `N-M`, `*/S`, `N-M/S`.
    Syntax {
        /// Which field.
        field: &'static str,
        /// The text that could not be read.
        item: String,
    },
}

/// A parsed schedule: for each field, the values it allows.
///
/// Tables rather than the original text, because the question being asked of a schedule is
/// "does this field allow 3?" and a parser that keeps the text has to answer it again on every
/// minute of every search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    /// Minutes of the hour that are allowed.
    minute: [bool; 60],
    /// Hours of the day that are allowed.
    hour: [bool; 24],
    /// Days of the month that are allowed; index 0 is unused.
    day: [bool; 32],
    /// Months that are allowed; index 0 is unused.
    month: [bool; 13],
    /// Weekdays that are allowed, Sunday at index 0.
    weekday: [bool; 7],
    /// Whether the day-of-month field was written `*`.
    day_any: bool,
    /// Whether the day-of-week field was written `*`.
    weekday_any: bool,
}

/// What a gap between the last run and now owes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatchUp {
    /// The runs to make, oldest first, at most as many as the caller asked for.
    pub runs: Vec<i64>,
    /// Matching instants that did not fit and will not be run. Reported rather than swallowed: a
    /// dropped run is a decision an operator reads.
    pub dropped: u64,
}

impl Schedule {
    /// Parse a five-field expression: minute, hour, day of month, month, day of week.
    ///
    /// # Errors
    ///
    /// One of the five [`Error`]s, naming the field.
    pub fn parse(expression: &str) -> Result<Self, Error> {
        let fields: Vec<&str> = expression.split_whitespace().collect();
        if fields.len() != 5 {
            return Err(Error::Fields {
                found: fields.len(),
            });
        }
        let mut schedule = Self {
            minute: [false; 60],
            hour: [false; 24],
            day: [false; 32],
            month: [false; 13],
            weekday: [false; 7],
            day_any: false,
            weekday_any: false,
        };
        schedule.day_any = fill(&mut schedule.minute, "minute", fields[0], MINUTE)?;
        fill(&mut schedule.hour, "hour", fields[1], HOUR)?;
        schedule.day_any = fill(&mut schedule.day, "day", fields[2], DAY)?;
        fill(&mut schedule.month, "month", fields[3], MONTH)?;
        schedule.weekday_any = fill(&mut schedule.weekday, "weekday", fields[4], WEEKDAY)?;
        Ok(schedule)
    }

    /// Does this schedule name `utc_minutes`, read in the zone this caller lives in?
    ///
    /// The day fields are cron's, not everybody's intuition: with both of them restricted the
    /// instant matches on either, and with one of them `*` the other one decides alone.
    #[must_use]
    pub fn matches(&self, utc_minutes: i64, zone: &dyn Zone) -> bool {
        let local = utc_minutes.saturating_add(zone.offset_minutes(utc_minutes));
        let (weekday, day, month, hour, minute) = wall_fields(local);
        if !self.minute[minute as usize] || !self.hour[hour as usize] || !self.month[month as usize]
        {
            return false;
        }
        match (self.day_any, self.weekday_any) {
            (true, true) => true,
            (true, false) => self.weekday[weekday as usize],
            (false, true) => self.day[day as usize],
            (false, false) => self.day[day as usize] || self.weekday[weekday as usize],
        }
    }

    /// The first instant strictly after `after_utc_minutes` that this schedule names.
    ///
    /// Strictly after, because a schedule that fires on the instant it just fired is a loop. `None`
    /// when nothing matches inside the horizon: a schedule for the 30th of February is legal to
    /// write and does not fire.
    #[must_use]
    pub fn next_after(&self, after_utc_minutes: i64, zone: &dyn Zone) -> Option<i64> {
        let mut candidate = after_utc_minutes.saturating_add(1);
        for _ in 0..HORIZON_MINUTES {
            if self.matches(candidate, zone) {
                return Some(candidate);
            }
            candidate = candidate.saturating_add(1);
        }
        None
    }

    /// What the gap between `last_utc_minutes` (inclusive, so passing the last run does not repeat
    /// it) and `now_utc_minutes` owes, keeping at most `max_runs` of it.
    ///
    /// The policy written here, and the reason it is written down: **the most recent runs are the
    /// ones that happen.** A job that has to catch up on a month of a daily schedule is usually a
    /// job that wants one fresh run, and the thirty that did not happen are an incident to report,
    /// not thirty stale reports to send. [`CatchUp::dropped`] is that incident, and it is a number
    /// rather than a log line because the operator needs to chart it.
    #[must_use]
    pub fn catch_up(
        &self,
        last_utc_minutes: i64,
        now_utc_minutes: i64,
        zone: &dyn Zone,
        max_runs: usize,
    ) -> CatchUp {
        let mut runs = Vec::new();
        let mut at = last_utc_minutes.saturating_add(1);
        while at <= now_utc_minutes && at - last_utc_minutes <= HORIZON_MINUTES {
            if self.matches(at, zone) {
                runs.push(at);
            }
            at += 1;
        }
        let dropped = runs.len().saturating_sub(max_runs) as u64;
        if runs.len() > max_runs {
            runs.drain(..runs.len() - max_runs);
        }
        CatchUp { runs, dropped }
    }
}

/// Fill one field's table from its text, and answer whether the field was exactly `*`.
fn fill(
    allowed: &mut [bool],
    field: &'static str,
    spec: &str,
    bounds: (u32, u32),
) -> Result<bool, Error> {
    let (low, high) = bounds;
    let width = allowed.len() as u32;
    // A number this field may name, with `7` folded onto Sunday for the weekday field.
    let value = |text: &str| -> Result<u32, Error> {
        let Ok(number) = text.trim().parse::<u32>() else {
            return Err(Error::Syntax {
                field,
                item: text.to_owned(),
            });
        };
        if number < low || number > high {
            return Err(Error::Range {
                field,
                value: number,
                from: low,
                to: high,
            });
        }
        Ok(number)
    };

    let any = spec == "*";
    for item in spec.split(',') {
        let (body, step) = match item.split_once('/') {
            Some((body, step)) => {
                let Ok(step) = step.parse::<u32>() else {
                    return Err(Error::Syntax {
                        field,
                        item: item.to_owned(),
                    });
                };
                if step == 0 {
                    return Err(Error::Step { field });
                }
                (body, step)
            }
            None => (item, 1),
        };
        let (start, end) = match body {
            "*" => (low, high),
            _ => match body.split_once('-') {
                Some((from, to)) => {
                    let (from, to) = (value(from)?, value(to)?);
                    if from > to {
                        return Err(Error::Reversed { field, from, to });
                    }
                    (from, to)
                }
                None => {
                    let single = value(body)?;
                    (single, single)
                }
            },
        };
        // The weekday field's `7` is a second spelling of Sunday, so the value is folded onto the
        // table here rather than in `value`: `0-7` still has to name all seven days.
        let mut at = start;
        while at <= end {
            allowed[(at % width) as usize] = true;
            at += step;
        }
        // `*/S` and `N-M/S` only: `N/S` is not a shape this parser accepts, and a number with a
        // step that names one value would look like it did something.
        if !any && step > 1 && body != "*" && !body.contains('-') && start == end {
            return Err(Error::Syntax {
                field,
                item: item.to_owned(),
            });
        }
    }
    Ok(any)
}

/// The wall-clock fields of a local minute: `(weekday, day, month, hour, minute)`, Sunday as 0.
///
/// Given rather than written: turning a count of minutes into a date is calendar arithmetic from a
/// different century, and a probe that spent its budget on it would not be about cron.
#[must_use]
pub fn wall_fields(local_minutes: i64) -> (u32, u32, u32, u32, u32) {
    let days = local_minutes.div_euclid(1_440);
    let minute_of_day = local_minutes.rem_euclid(1_440);
    let (_, month, day) = civil_from_days(days);
    // 1970-01-01 was a Thursday, which is 4 when Sunday is 0.
    let weekday = (days + 4).rem_euclid(7) as u32;
    (
        weekday,
        day,
        month,
        (minute_of_day / 60) as u32,
        (minute_of_day % 60) as u32,
    )
}

/// Minutes since the Unix epoch for a civil instant, in UTC. The inverse of [`wall_fields`]'s date
/// half, for the tests that want to write `1970-01-05 02:00` instead of a number.
#[must_use]
pub fn civil_to_minutes(year: i64, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
    days_from_civil(year, month, day) * 1_440 + i64::from(hour) * 60 + i64::from(minute)
}

/// Days since 1970-01-01 for a civil date.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_prime = i64::from(if month > 2 { month - 3 } else { month + 9 });
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The civil date for a count of days since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shift = days + 719_468;
    let era = shift.div_euclid(146_097);
    let day_of_era = shift.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zone that steps once: `before` minutes east of UTC until `at`, `after` from there on. This
    /// is what a DST transition is, and it is the whole of what a schedule needs to know about one.
    #[derive(Debug)]
    struct Step {
        at: i64,
        before: i64,
        after: i64,
    }

    impl Zone for Step {
        fn offset_minutes(&self, utc_minutes: i64) -> i64 {
            if utc_minutes < self.at {
                self.before
            } else {
                self.after
            }
        }
    }

    const UTC: Fixed = Fixed { offset_minutes: 0 };

    #[test]
    fn the_parser_names_the_field_it_refused() {
        assert_eq!(Schedule::parse("* * * *"), Err(Error::Fields { found: 4 }));
        assert_eq!(
            Schedule::parse("* * * * * *"),
            Err(Error::Fields { found: 6 })
        );
        assert_eq!(
            Schedule::parse("60 * * * *"),
            Err(Error::Range {
                field: "minute",
                value: 60,
                from: 0,
                to: 59
            })
        );
        assert_eq!(
            Schedule::parse("* 24 * * *"),
            Err(Error::Range {
                field: "hour",
                value: 24,
                from: 0,
                to: 23
            })
        );
        assert_eq!(
            Schedule::parse("* * 0 * *"),
            Err(Error::Range {
                field: "day",
                value: 0,
                from: 1,
                to: 31
            })
        );
        assert_eq!(
            Schedule::parse("* * * 13 *"),
            Err(Error::Range {
                field: "month",
                value: 13,
                from: 1,
                to: 12
            })
        );
        assert_eq!(
            Schedule::parse("* * * * 8"),
            Err(Error::Range {
                field: "weekday",
                value: 8,
                from: 0,
                to: 7
            })
        );
        assert_eq!(
            Schedule::parse("* 5-2 * * *"),
            Err(Error::Reversed {
                field: "hour",
                from: 5,
                to: 2
            })
        );
        assert_eq!(
            Schedule::parse("*/0 * * * *"),
            Err(Error::Step { field: "minute" })
        );
        assert_eq!(
            Schedule::parse("x * * * *"),
            Err(Error::Syntax {
                field: "minute",
                item: "x".to_owned()
            })
        );
        assert_eq!(
            Schedule::parse("1--2 * * * *"),
            Err(Error::Syntax {
                field: "minute",
                item: "-2".to_owned()
            })
        );
        assert_eq!(
            Schedule::parse("0 0 * * fri"),
            Err(Error::Syntax {
                field: "weekday",
                item: "fri".to_owned()
            }),
            "names are a different grammar; this engine speaks numbers only"
        );
        assert_eq!(
            Schedule::parse("5/10 * * * *"),
            Err(Error::Syntax {
                field: "minute",
                item: "5/10".to_owned()
            }),
            "`N/S` names one value with a step, which a reader would be wrong to expect to work"
        );
        assert!(
            Schedule::parse("* * * * *").is_ok(),
            "the plain one has to parse"
        );
    }

    #[test]
    fn a_field_is_a_list_of_ranges_of_steps() {
        let quarter = Schedule::parse("*/15 * * * *").expect("a valid expression");
        let matches = |schedule: &Schedule, minute: u32| {
            schedule.matches(civil_to_minutes(1970, 1, 1, 0, minute), &UTC)
        };
        for minute in [0, 15, 30, 45] {
            assert!(matches(&quarter, minute), "{minute} is a quarter hour");
        }
        for minute in [1, 14, 16, 44, 46, 59] {
            assert!(!matches(&quarter, minute), "{minute} is not");
        }

        let business = Schedule::parse("0 9-17 * * 1-5").expect("a valid expression");
        let weekday =
            |hour: u32, day: u32| business.matches(civil_to_minutes(1970, 1, day, hour, 0), &UTC);
        // 1970-01-05 was a Monday, so 9 through 15 January are Monday to Thursday and the 16th is
        // a Friday; the 17th is a Saturday.
        assert!(weekday(9, 5), "Monday at 09:00");
        assert!(weekday(17, 9), "Friday at 17:00");
        assert!(!weekday(8, 5), "one hour before the range");
        assert!(!weekday(18, 5), "one hour after it");
        assert!(!weekday(12, 17), "Saturday is not in 1-5");

        let list = Schedule::parse("1,2,3 * * * *").expect("a valid expression");
        assert!(list.matches(civil_to_minutes(1970, 1, 1, 0, 3), &UTC));
        assert!(!list.matches(civil_to_minutes(1970, 1, 1, 0, 4), &UTC));

        // `7` is Sunday, the same day as `0`.
        let sunday = Schedule::parse("0 0 * * 7").expect("a valid expression");
        assert!(sunday.matches(civil_to_minutes(1970, 1, 4, 0, 0), &UTC));
        assert!(!sunday.matches(civil_to_minutes(1970, 1, 5, 0, 0), &UTC));
    }

    #[test]
    fn a_daily_schedule_fires_on_its_minute_and_asks_for_the_next_one() {
        let nightly = Schedule::parse("0 9 * * *").expect("a valid expression");
        let nine = civil_to_minutes(1970, 1, 5, 9, 0);

        assert!(nightly.matches(nine, &UTC), "exactly 09:00 is the run");
        assert!(
            !nightly.matches(nine + 1, &UTC),
            "09:01 is a different schedule's business, and the point of a cron field is that it is \
             not a window"
        );
        assert!(!nightly.matches(nine - 1, &UTC));
        assert_eq!(
            nightly.next_after(nine, &UTC),
            Some(civil_to_minutes(1970, 1, 6, 9, 0)),
            "the next run in the same minute is tomorrow"
        );
        assert_eq!(
            nightly.next_after(nine - 60, &UTC),
            Some(nine),
            "and from a minute before, it is this one"
        );
        assert_eq!(
            nightly.next_after(nine - 1, &UTC),
            Some(nine),
            "strictly after, so the instant itself still counts"
        );

        // `* * * * *` has a run every minute, and the next one is a minute away.
        let every = Schedule::parse("* * * * *").expect("a valid expression");
        assert!(every.matches(nine, &UTC));
        assert!(every.matches(nine + 1, &UTC));
        assert_eq!(every.next_after(nine, &UTC), Some(nine + 1));
    }

    #[test]
    fn the_offset_is_the_callers_and_the_wall_time_is_what_the_fields_see() {
        let nightly = Schedule::parse("0 9 * * *").expect("a valid expression");
        let nine = civil_to_minutes(1970, 1, 5, 9, 0);

        let ahead = Fixed {
            offset_minutes: 120,
        };
        assert!(
            nightly.matches(nine - 120, &ahead),
            "09:00 at +02:00 is 07:00 UTC"
        );
        assert!(!nightly.matches(nine - 120, &UTC));
        assert_eq!(nightly.next_after(nine - 121, &ahead), Some(nine - 120));

        let behind = Fixed {
            offset_minutes: -300,
        };
        assert!(
            nightly.matches(nine + 300, &behind),
            "09:00 at -05:00 is 14:00 UTC"
        );
        assert_eq!(nightly.next_after(nine + 299, &behind), Some(nine + 300));

        // The fields are wall-clock fields, so one UTC instant is a different local day either
        // side of midnight — which is the customer's problem, and only the caller knows which zone
        // the customer is in. 1970-01-04 was a Sunday.
        let sunday_night = civil_to_minutes(1970, 1, 4, 23, 30);
        let weekly = Schedule::parse("30 23 * * 0").expect("a valid expression");
        assert!(
            weekly.matches(sunday_night, &UTC),
            "Sunday 23:30 UTC is the run"
        );
        assert!(
            !weekly.matches(sunday_night, &Fixed { offset_minutes: 60 }),
            "at +01:00 that instant is Monday 00:30: the wrong day and the wrong hour"
        );
        assert!(
            !weekly.matches(
                sunday_night,
                &Fixed {
                    offset_minutes: -60
                }
            ),
            "at -01:00 it is Sunday 22:30: the right day and the wrong hour"
        );
    }

    #[test]
    fn the_two_day_fields_are_a_union_only_when_both_are_restricted() {
        let first = civil_to_minutes(1970, 1, 1, 0, 0); // Thursday the 1st
        let sunday = civil_to_minutes(1970, 1, 4, 0, 0);
        let saturday = civil_to_minutes(1970, 1, 3, 0, 0);

        let both = Schedule::parse("0 0 1 * 0").expect("a valid expression");
        assert!(both.matches(first, &UTC), "the first of the month");
        assert!(both.matches(sunday, &UTC), "and every Sunday");
        assert!(
            !both.matches(saturday, &UTC),
            "but not because it is a Saturday"
        );

        let day_only = Schedule::parse("0 0 1 * *").expect("a valid expression");
        assert!(day_only.matches(first, &UTC));
        assert!(
            !day_only.matches(sunday, &UTC),
            "with the weekday `*`, only the day of the month decides — the `*` is not a value that \
             happens to match"
        );

        let weekday_only = Schedule::parse("0 0 * * 0").expect("a valid expression");
        assert!(weekday_only.matches(sunday, &UTC));
        assert!(!weekday_only.matches(first, &UTC));

        // Both `*`: every day, which is the case that would be an OR with nothing on one side.
        let every_day = Schedule::parse("0 0 * * *").expect("a valid expression");
        assert!(every_day.matches(first, &UTC));
        assert!(every_day.matches(sunday, &UTC));

        // The month and the time still AND: a union of the day fields does not make the expression
        // one big OR.
        let march_only = Schedule::parse("0 0 1 3 0").expect("a valid expression");
        assert!(march_only.matches(civil_to_minutes(1970, 3, 1, 0, 0), &UTC));
        assert!(march_only.matches(civil_to_minutes(1970, 3, 8, 0, 0), &UTC));
        assert!(
            !march_only.matches(civil_to_minutes(1970, 4, 1, 0, 0), &UTC),
            "the 1st of April is a Wednesday in a month that is not 3"
        );
    }

    #[test]
    fn a_date_that_does_not_exist_never_fires() {
        let impossible = Schedule::parse("0 0 30 2 *").expect("a legal expression");
        assert_eq!(
            impossible.next_after(civil_to_minutes(1970, 1, 1, 0, 0), &UTC),
            None,
            "there is no 30 February, and the search has to end rather than run forever"
        );
        // The 29th of February does exist, in some years, and the horizon is what makes it
        // findable: searched from 1970 it is two years away and the answer is `None`, which is why
        // the horizon is a year rather than a day.
        let leap = Schedule::parse("0 0 29 2 *").expect("a legal expression");
        assert_eq!(
            leap.next_after(civil_to_minutes(1970, 1, 1, 0, 0), &UTC),
            None,
            "the next 29 February is more than a year away"
        );
        let found = leap
            .next_after(civil_to_minutes(1972, 1, 1, 0, 0), &UTC)
            .expect("1972 is a leap year");
        let (_, day, month, hour, minute) = wall_fields(found);
        assert_eq!((day, month, hour, minute), (29, 2, 0, 0));
    }

    #[test]
    fn an_hour_that_does_not_exist_is_skipped_and_says_so_by_moving_on() {
        // 1970-01-05 02:00 UTC is when the clock jumps forward: at and after it, local time is an
        // hour ahead, so the local hour 02:00 that day never happens.
        let at = civil_to_minutes(1970, 1, 5, 2, 0);
        let spring = Step {
            at,
            before: 0,
            after: 60,
        };
        let nightly = Schedule::parse("0 2 * * *").expect("a valid expression");

        assert!(
            !nightly.matches(at - 1, &spring),
            "01:59 local is not 02:00"
        );
        assert!(
            !nightly.matches(at, &spring),
            "the instant the clock jumped, local time is 03:00: the 02:00 run does not exist today"
        );
        assert!(
            !nightly.matches(at + 59, &spring),
            "still inside the missing hour"
        );

        let found = nightly
            .next_after(at - 1, &spring)
            .expect("the schedule fires again tomorrow");
        assert_eq!(
            found,
            civil_to_minutes(1970, 1, 6, 1, 0),
            "tomorrow's 02:00 local is 01:00 UTC, because the offset has changed"
        );
        let (_, day, month, hour, minute) = wall_fields(found + 60);
        assert_eq!(
            (day, month, hour, minute),
            (6, 1, 2, 0),
            "and it is 02:00 where the job runs"
        );

        // The caller's other half of the decision: a job that must run *every* hour is not skipped
        // by the transition — only the runs whose wall time did not happen are.
        let hourly = Schedule::parse("0 * * * *").expect("a valid expression");
        assert_eq!(
            hourly.next_after(at - 1, &spring),
            Some(at),
            "03:00 local happens after the jump, and it is the next hour boundary"
        );
    }

    #[test]
    fn an_hour_that_happens_twice_is_two_runs_unless_the_caller_says_otherwise() {
        // The same instant, the other direction: local time goes from 02:00 back to 01:00, so the
        // wall hour 02:00 occurs once with the old offset and once with the new one.
        let at = civil_to_minutes(1970, 1, 5, 2, 0);
        let fall = Step {
            at,
            before: 60,
            after: 0,
        };
        let nightly = Schedule::parse("0 2 * * *").expect("a valid expression");

        assert!(
            nightly.matches(at - 60, &fall),
            "02:00 local, before the clock was turned back"
        );
        assert!(
            nightly.matches(at, &fall),
            "and 02:00 local again, an hour of real time later"
        );

        let caught = nightly.catch_up(at - 61, at, &fall, 4);
        assert_eq!(
            caught,
            CatchUp {
                runs: vec![at - 60, at],
                dropped: 0
            },
            "a repeated hour is two instants that match, and a schedule that only runs one of \
             them is a schedule that skipped a run"
        );

        // The other defensible policy is the caller's: one run per wall time. The schedule cannot
        // know which one was meant, which is why `max_runs` is a parameter.
        let once = nightly.catch_up(at - 61, at, &fall, 1);
        assert_eq!(
            once,
            CatchUp {
                runs: vec![at],
                dropped: 1
            }
        );
    }

    #[test]
    fn a_gap_owes_the_runs_that_are_missing_and_says_what_it_dropped() {
        let nightly = Schedule::parse("0 0 * * *").expect("a valid expression");
        let start = civil_to_minutes(1970, 1, 1, 0, 0);

        // Three days of not running: three runs are owed, oldest first.
        let three = nightly.catch_up(start, start + 3 * 1_440, &UTC, 10);
        assert_eq!(
            three.runs,
            vec![start + 1_440, start + 2 * 1_440, start + 3 * 1_440]
        );
        assert_eq!(three.dropped, 0, "nothing was dropped that fitted");
        assert!(
            !three.runs.contains(&start),
            "the instant passed in as the last run is not run again"
        );

        // The same gap with a ceiling of one: the most recent run happens and the rest are a
        // number an operator can chart.
        let one = nightly.catch_up(start, start + 3 * 1_440, &UTC, 1);
        assert_eq!(
            one,
            CatchUp {
                runs: vec![start + 3 * 1_440],
                dropped: 2
            },
            "an engine that ran the *oldest* three days of reports on the fourth is sending stale \
             data, which is the other policy and not this one"
        );

        // Nothing owed: the gap is inside one interval.
        assert_eq!(
            nightly.catch_up(start, start + 100, &UTC, 5),
            CatchUp {
                runs: Vec::new(),
                dropped: 0
            }
        );

        // A ceiling of zero is a policy too: run nothing, report everything.
        assert_eq!(
            nightly.catch_up(start, start + 2 * 1_440, &UTC, 0),
            CatchUp {
                runs: Vec::new(),
                dropped: 2
            }
        );
    }
}
