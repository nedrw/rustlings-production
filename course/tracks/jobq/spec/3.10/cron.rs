//! Spec for `jobq/3.10-cron-schedules`: what a schedule is, and the one question it answers.
//!
//! `kiln run` copies this file into the project as `tests/spec_cron.rs`. Pure arithmetic — no clock,
//! no store, no container — because everything asserted here is a property of the expression and of
//! the time zone it is read in, and a test that needed a running system to say "the next 02:30 in New
//! York is 07:30 UTC on a winter Saturday" would be a slower way of saying the same thing.
//!
//! Five decisions about what this file is allowed to assert:
//!
//! * **the refusals are half the grammar.** A five-field cron dialect is defined by what it will not
//!   read as much as by what it will: `* * * * * *`, `60 * * * *`, `*/0 * * * *`, `10-5 * * * *`,
//!   `1,,2 * * * *` and `* * * * MON` are each a shape that looks like a schedule, and the case that
//!   refuses them is the reason an operator finds out at submit time instead of at 03:00.
//! * **the zone is part of the schedule.** The same expression answers two different instants in two
//!   zones, which is what "a schedule carries its time zone" has to mean to a test; and an unknown
//!   zone is refused rather than quietly becoming UTC, because a schedule five hours early is
//!   indistinguishable from a job that does not run.
//! * **the two day fields are a union.** `0 0 1 * 1` fires on the first of the month *or* on Mondays
//!   — asserted from both sides, because the AND reading passes a test that only ever looks at one
//!   of the two fields.
//! * **the DST days are the test, not an edge case.** This file pins both halves of the rule the
//!   module documents: a reading the clock skips does not happen that day (`30 2 * * *` on
//!   2025-03-09 in New York), and a reading the clock repeats happens twice (`30 1 * * *` on
//!   2025-11-02). Every assertion message below says which of the two it is looking at, because
//!   "the run moved" and "the run vanished" are two different bugs.
//! * **the answer is a starting point.** Repeatedly asking from the previous answer walks the
//!   schedule forward: strictly increasing, never the same instant twice, and it has to keep walking
//!   across a transition — a schedule that stalls on the day the clock moves is a schedule that stops
//!   for an hour, or for ever.

use chrono::{DateTime, Utc};
use jobq::engine::Schedule;

/// An instant, written the way this file reads them.
fn at(text: &str) -> DateTime<Utc> {
    text.parse()
        .unwrap_or_else(|error| panic!("`{text}` is an instant: {error}"))
}

/// That expression, read in `zone` — every case needs one and the failure should name the half that
/// went wrong.
fn schedule(expression: &str, zone: &str) -> Schedule {
    Schedule::parse(expression)
        .unwrap_or_else(|error| panic!("`{expression}` is a five-field expression: {error}"))
        .in_zone(zone)
        .unwrap_or_else(|error| panic!("`{zone}` is a time zone this build knows: {error}"))
}

/// A five-field expression reads, and it answers in UTC until it is told otherwise.
#[test]
fn a_five_field_expression_reads_and_answers_in_utc() {
    // 2025-03-03 is a Monday, and 2025-03-07 is the Friday of the same week.
    let weekday_mornings = schedule("*/15 9-17 * * 1-5", "UTC");
    assert_eq!(
        weekday_mornings.next_after(at("2025-03-03T08:59:59Z")),
        Some(at("2025-03-03T09:00:00Z")),
        "the window opens at 09:00 and the minutes are every fifteen: the first reading after \
         08:59:59 is 09:00"
    );
    // Runs of spaces and surrounding whitespace are what a pasted crontab line has; the field count
    // is what is not negotiable.
    assert!(
        Schedule::parse("  0\t9 *  * * ").is_ok(),
        "a crontab line gets pasted with tabs and double spaces in it, and the expression is still \
         five fields"
    );
}

/// An expression that is not five fields is refused, and says how many it found.
#[test]
fn an_expression_that_is_not_five_fields_is_refused() {
    for (bad, what) in [
        ("", "nothing at all"),
        ("   ", "only whitespace"),
        ("* * * *", "four fields"),
        ("0 9 * *", "four fields, with no day field at all"),
        ("* * * * * *", "six fields: the sixth is a second or a year"),
        (
            "0 0 0 9 * *",
            "six fields, spelled like a Quartz expression",
        ),
    ] {
        let error = Schedule::parse(bad).expect_err(&format!(
            "`{bad}` is {what}, and a schedule is exactly five fields: reading it as a schedule \
             would guess at the two the writer meant"
        ));
        assert!(
            error.to_string().contains("field"),
            "the refusal has to say what it counted: {error}"
        );
    }
}

/// A value outside the field's own range is refused.
#[test]
fn a_value_outside_its_field_is_refused() {
    for (bad, what) in [
        ("60 * * * *", "there is no minute 60"),
        ("* 24 * * *", "there is no hour 24"),
        ("* * 0 * *", "there is no day zero of the month"),
        ("* * 32 * *", "February has no 32nd either"),
        ("* * * 0 *", "months are 1 to 12"),
        ("* * * 13 *", "months are 1 to 12"),
        (
            "* * * * 8",
            "days of the week are 0 to 7, with 7 as the second spelling of Sunday",
        ),
        (
            "0-60 * * * *",
            "an upper bound outside the range is the same refusal",
        ),
    ] {
        let error = Schedule::parse(bad).expect_err(&format!(
            "`{bad}` names a value its field does not have ({what}): clamping it would run the job \
             at a time nobody asked for"
        ));
        assert!(
            error.to_string().contains("outside"),
            "the refusal has to quote the value and the range it missed: {error}"
        );
    }
    // `7` is Sunday, which is the whole reason the day-of-week field allows eight values:
    // 2025-03-02 is a Sunday and both spellings name it.
    assert_eq!(
        schedule("0 0 * * 7", "UTC").next_after(at("2025-03-01T00:00:00Z")),
        Some(at("2025-03-02T00:00:00Z")),
        "`7` is Sunday: a schedule written for a crontab that spells it that way means what it says"
    );
}

/// The shapes that look like a schedule and are not one.
#[test]
fn the_shapes_that_look_valid_and_are_not_are_refused() {
    for (bad, what) in [
        ("*/0 * * * *", "a step of zero names nothing"),
        ("10-5 * * * *", "a range written the wrong way round"),
        ("1,,2 * * * *", "an empty item between two commas"),
        ("1- * * * *", "a range with no end"),
        ("-1 * * * *", "a range with no start"),
        ("* * * * MON", "names are not this grammar"),
        ("* * JAN * *", "names are not this grammar"),
        ("1.5 * * * *", "a fraction is not a value"),
        ("1-2-3 * * * *", "two ranges in one item"),
        ("5/ * * * *", "a slash with no step"),
        ("1-5/ * * * *", "a slash with no step"),
        ("* * * * *!", "trailing punctuation"),
    ] {
        let error = Schedule::parse(bad).expect_err(&format!(
            "`{bad}` is not a field ({what}): this grammar is numbers, `*`, `,`, `-` and `/`, and \
             a guess here is a job that runs at a time nobody asked for"
        ));
        assert!(
            !error.to_string().is_empty(),
            "a refusal that says nothing is a refusal nobody can act on"
        );
    }
}

/// An unknown zone is refused rather than quietly becoming UTC.
#[test]
fn an_unknown_zone_is_refused_rather_than_defaulted() {
    let parsed = Schedule::parse("0 9 * * *").expect("this expression reads");
    for unknown in ["Mars/Olympus", "Europe/Berlim", "", "America/New York"] {
        assert!(
            parsed.clone().in_zone(unknown).is_err(),
            "`{unknown}` is not a zone the database has: defaulting to UTC would run the job five \
             hours early and nobody would connect it to the typo"
        );
    }
    assert!(
        parsed.in_zone("America/New_York").is_ok(),
        "and a name the database has is accepted, spelled the way the database spells it"
    );
}

/// The zone is part of the schedule, not of the question.
#[test]
fn the_same_expression_in_two_zones_answers_two_instants() {
    let utc = schedule("0 9 * * *", "UTC");
    let new_york = schedule("0 9 * * *", "America/New_York");
    assert_eq!(
        utc.next_after(at("2025-06-01T00:00:00Z")),
        Some(at("2025-06-01T09:00:00Z")),
        "09:00 in UTC is 09:00 UTC"
    );
    assert_eq!(
        new_york.next_after(at("2025-06-01T00:00:00Z")),
        Some(at("2025-06-01T13:00:00Z")),
        "and 09:00 in New York is 13:00 UTC in June, because the zone is EDT (UTC-4) then: the \
         expression is the same and the instant is not"
    );
}

/// The next run is the next reading the fields name, in the schedule's own zone.
#[test]
fn the_next_run_is_the_next_reading_the_fields_name() {
    let weekday_mornings = schedule("*/15 9-17 * * 1-5", "UTC");
    for (after, expected, what) in [
        (
            "2025-03-03T09:00:00Z",
            "2025-03-03T09:15:00Z",
            "asked from the instant of a run, the answer is the one after it — a schedule that \
             fires on the instant it just fired on is a loop",
        ),
        (
            "2025-03-03T09:07:00Z",
            "2025-03-03T09:15:00Z",
            "a minute that the field does not name is not a run, however close it is to one",
        ),
        (
            "2025-03-07T17:45:00Z",
            "2025-03-10T09:00:00Z",
            "Friday's window closes at 17:45 and the next weekday morning is Monday's: \
             `1-5` is Monday to Friday, and the weekend is not in it",
        ),
        (
            "2025-03-08T12:00:00Z",
            "2025-03-10T09:00:00Z",
            "a whole weekend is skipped in one answer, which is what the day-of-week field is for",
        ),
        (
            "2025-03-03T18:00:00Z",
            "2025-03-04T09:00:00Z",
            "the window is 09:00 to 17:45, so the answer after it closes is tomorrow's opening",
        ),
    ] {
        assert_eq!(
            weekday_mornings.next_after(at(after)),
            Some(at(expected)),
            "{what}"
        );
    }
}

/// The two day fields are a union when both are restricted, and one of them decides when the other
/// is `*`.
#[test]
fn the_two_day_fields_are_an_either_or() {
    // 2025-03-01 is a Saturday, so "the 1st or any Monday" has an answer one day later and a month's
    // worth of Mondays after that.
    let first_or_monday = schedule("0 0 1 * 1", "UTC");
    assert_eq!(
        first_or_monday.next_after(at("2025-03-01T00:00:00Z")),
        Some(at("2025-03-03T00:00:00Z")),
        "`0 0 1 * 1` is the 1st of the month OR Monday, and the 1st has just happened: the next run \
         is the Monday, not the 1st of April. ANDing the two fields is the bug this case exists for — \
         it turns a schedule that runs thirty-odd times a year into one that runs twelve"
    );
    assert_eq!(
        first_or_monday.next_after(at("2025-03-02T00:00:00Z")),
        Some(at("2025-03-03T00:00:00Z")),
        "and it keeps firing weekly while the month is not the 1st"
    );
    assert_eq!(
        schedule("0 0 1 * *", "UTC").next_after(at("2025-03-01T00:00:00Z")),
        Some(at("2025-04-01T00:00:00Z")),
        "with `*` in the weekday field, only the day of the month decides: the next 1st is April's"
    );
    assert_eq!(
        schedule("0 0 * * 1", "UTC").next_after(at("2025-03-01T00:00:00Z")),
        Some(at("2025-03-03T00:00:00Z")),
        "and with `*` in the day-of-month field, only the weekday decides"
    );
}

/// A reading the clock skips does not happen that day.
#[test]
fn a_reading_the_clock_skips_does_not_happen() {
    // New York goes from 02:00 EST to 03:00 EDT on 2025-03-09, so the local reading 02:30 does not
    // exist that day. The day before, 02:30 EST is 07:30 UTC; the day after, 02:30 EDT is 06:30 UTC.
    let nightly = schedule("30 2 * * *", "America/New_York");
    assert_eq!(
        nightly.next_after(at("2025-03-08T04:00:00Z")),
        Some(at("2025-03-08T07:30:00Z")),
        "02:30 on a winter Saturday in New York is 07:30 UTC (EST is UTC-5)"
    );
    assert_eq!(
        nightly.next_after(at("2025-03-09T04:00:00Z")),
        Some(at("2025-03-10T06:30:00Z")),
        "on 2025-03-09 the clock goes from 02:00 to 03:00, so 02:30 local never happens and the run \
         is skipped: the next one is Monday's 02:30 EDT (06:30 UTC). Running the job anyway — at \
         03:00 local, 07:00 UTC — would be inventing a reading the schedule did not name"
    );
    assert_eq!(
        nightly.next_after(at("2025-03-09T06:59:00Z")),
        Some(at("2025-03-10T06:30:00Z")),
        "and there is no run at all in the skipped hour: asked from just before 07:00 UTC (the \
         instant the gap ends), the answer is still tomorrow, not an hour later"
    );
    // The other half of the caller's decision, and the reason the rule is about readings rather than
    // hours: a schedule that names *every* hour is not skipped by the transition, because 03:00
    // local really does happen on the day 02:00 does not.
    let hourly = schedule("0 * * * *", "America/New_York");
    assert_eq!(
        hourly.next_after(at("2025-03-09T06:59:00Z")),
        Some(at("2025-03-09T07:00:00Z")),
        "03:00 local happens on the day the clock jumps, and an hourly schedule runs then: only the \
         readings the clock skipped are skipped"
    );
}

/// A reading the clock repeats happens twice.
#[test]
fn a_reading_the_clock_repeats_happens_twice() {
    // New York goes from 02:00 EDT back to 01:00 EST on 2025-11-02, so 01:30 happens at 05:30 UTC and
    // again at 06:30 UTC — two instants, one wall-clock reading.
    let nightly = schedule("30 1 * * *", "America/New_York");
    let first = at("2025-11-02T05:30:00Z");
    assert_eq!(
        nightly.next_after(at("2025-11-02T00:00:00Z")),
        Some(first),
        "01:30 local on 2025-11-02 is 05:30 UTC with the old offset (EDT, UTC-4)"
    );
    assert_eq!(
        nightly.next_after(first),
        Some(at("2025-11-02T06:30:00Z")),
        "and the same reading happens again at 06:30 UTC, an hour of real time later: both are runs, \
         because the local clock really does read 01:30 twice — skipping the second one would drop a \
         run the schedule named"
    );
    assert_eq!(
        nightly.next_after(at("2025-11-02T06:30:00Z")),
        Some(at("2025-11-03T06:30:00Z")),
        "and then the next day's 01:30 EST, at 06:30 UTC: the transition is over and the schedule is \
         daily again"
    );
}

/// A schedule that can never fire says so instead of searching for ever.
#[test]
fn a_schedule_that_can_never_fire_says_so() {
    let impossible = Schedule::parse("0 0 31 2 *")
        .expect("31 February is a legal expression: it is February that has 28 days");
    assert_eq!(
        impossible.next_after(at("2025-01-01T00:00:00Z")),
        None,
        "no instant in the next five years is the 31st of February, and the answer is `never` rather \
         than a search that never comes back: a schedule that can be written and cannot fire is a \
         misconfiguration, and the process that reads it has to be able to say so"
    );
    // 29 February is the rare one that does exist, which is what makes the horizon a choice: the
    // rarest thing a five-field expression can name comes round every fourth year.
    let leap = Schedule::parse("0 0 29 2 *").expect("29 February is a legal expression");
    assert_eq!(
        leap.next_after(at("2025-03-01T00:00:00Z")),
        Some(at("2028-02-29T00:00:00Z")),
        "29 February 2028 is three years after the query, and answering `None` because a one-year \
         horizon is cheaper would be a lie: the expression names a run and the engine has to find it"
    );
}

/// Every answer is a starting point for the next one, across both DST days.
#[test]
fn every_answer_is_a_starting_point_for_the_next_one() {
    for (expression, from, steps, what) in [
        (
            "30 2 * * *",
            "2025-03-01T00:00:00Z",
            200usize,
            "a daily schedule has to keep walking through the day the clock skips an hour",
        ),
        (
            "30 1 * * *",
            "2025-10-25T00:00:00Z",
            200,
            "and through the day it repeats one: the repeated hour is two runs, and neither stalls \
             the walk",
        ),
    ] {
        let schedule = schedule(expression, "America/New_York");
        let mut cursor = at(from);
        let mut walked = 0;
        while let Some(next) = schedule.next_after(cursor) {
            assert!(
                next > cursor,
                "{what}: `next_after({cursor})` answered {next}, which is not strictly after the \
                 instant it was asked from — a schedule that fires where it just fired is a loop"
            );
            cursor = next;
            walked += 1;
            if walked == steps {
                break;
            }
        }
        assert_eq!(
            walked, steps,
            "{what}: the walk stopped after {walked} run(s), so the schedule has an answer nothing \
             follows — every run it names has to have one after it"
        );
    }
}
