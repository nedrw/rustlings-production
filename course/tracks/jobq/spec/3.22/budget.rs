//! Spec for `jobq/3.22-bench-budget`: a budget built out of counts, and the guard over their shape.
//!
//! `kiln run` copies this file into the project as `tests/spec_budget.rs`; it is the course's file,
//! not the learner's (plan §3 D2). Four decisions, and the first two are the ones a review argues
//! about:
//!
//! * **a count is not a clock.** `Work` counts cells and statements, and the case that matters is
//!   the pair: a drain that examines every row twice has twice the cells and the same statements,
//!   which is what localizes a defect that a single total would only report as "something changed";
//! * **the guard is a shape, not a number.** `scale_to` compares the work at twice the size with the
//!   work at one size, and the case below runs *both* drains — the one that remembers where it
//!   stopped and the one that starts over — to show what the guard is for. On a ten-row queue they
//!   are 10 cells and 55;
//! * **a comparison with no denominator is not a ratio.** `None` rather than `0.0`, because a zero
//!   would compare with a bound like any other number and report a pass for a drain that measured
//!   nothing;
//! * **the names and the bound are content.** The lines this file checks are the lines a harness
//!   prints and the metrics the course's baseline is written in
//!   (`course/tracks/jobq/budget/3.22-bench-budget.toml`), and `docs/budget.md` states them, because
//!   a budget whose names live in three places is a budget that drifts.

use std::path::PathBuf;

use chrono::{DateTime, Duration, Utc};
use jobq::engine::{JobId, METRICS, Pull, Row, SCALE_MAX, Work, report};

/// The page this step delivers, relative to the project.
const PAGE: &str = "docs/budget.md";
/// The size the budget's counts are taken at, and the size the guard compares with.
const ROWS: usize = 256;
/// One row in four of the workload is not due yet.
const NOT_DUE_EVERY: usize = 4;
/// The fixed instant every case asks about.
const NOW_SECONDS: i64 = 1_700_000_000;

/// The instant every case compares with.
fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(NOW_SECONDS, 0).expect("a fixed instant these cases state themselves")
}

/// One batch: rows in claim order, every fourth of them due later rather than now.
fn batch(rows: usize) -> Vec<Row> {
    (0..rows)
        .map(|index| {
            let waiting = index % NOT_DUE_EVERY == NOT_DUE_EVERY - 1;
            let run_at = if waiting {
                now() + Duration::minutes(5)
            } else {
                now()
            };
            Row::new(JobId::new(), run_at)
        })
        .collect()
}

/// Drain a batch the way the step's module does, and answer what it cost.
fn drain(rows: usize) -> Work {
    let mut work = Work::new();
    let mut pull = Pull::of(batch(rows));
    while pull.next(now(), &mut work).is_some() {}
    work
}

/// The drain this step's guard exists for: every pull starts at the front of the batch and looks for
/// the next row it has not handed out yet.
///
/// Written out rather than described, because the guard is a claim about this shape: the two drains
/// agree on every row they hand out and on nothing about what it cost.
fn rescanning(rows: &[Row]) -> Work {
    let mut work = Work::new();
    let mut taken: Vec<JobId> = Vec::new();
    loop {
        let mut found = None;
        for row in rows {
            work.examine();
            if row.is_due(now()) && !taken.contains(&row.id()) {
                found = Some(row.id());
                break;
            }
        }
        let Some(id) = found else {
            return work;
        };
        work.statement();
        taken.push(id);
    }
}

/// The page, read from the project it is part of.
fn page() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(PAGE);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!("{PAGE} is the page this step delivers, and it cannot be read: {error}")
    })
}

#[test]
fn the_meter_counts_cells_and_statements_apart() {
    let mut work = Work::new();
    assert_eq!(work.cells(), 0);
    assert_eq!(work.statements(), 0);
    assert_eq!(work.total(), 0);
    assert_eq!(
        work,
        Work::default(),
        "a meter that has counted nothing is the empty one"
    );

    work.examine();
    work.examine();
    work.statement();
    assert_eq!(work.cells(), 2, "two rows looked at");
    assert_eq!(work.statements(), 1, "one row handed out");
    assert_eq!(
        work.total(),
        3,
        "the whole cost is both units: a drain that examines every row twice and writes once per
         batch is a different defect from one that writes twice, and the pair is what says which"
    );
}

#[test]
fn a_comparison_with_no_denominator_is_not_a_ratio() {
    let nothing = Work::new();
    let something = drain(8);
    assert_eq!(
        nothing.scale_to(&something),
        None,
        "a ratio with no denominator is not a number: `0.0` would compare with a bound like any
         other ratio and report a pass for a drain that measured nothing at all"
    );

    let ratio = something
        .scale_to(&drain(16))
        .expect("this drain examined rows");
    assert!(
        ratio > 1.0 && ratio < 10.0,
        "twice the queue is more work and not an order of magnitude more: {ratio}"
    );
    assert!(
        something.scale_to(&something) == Some(1.0),
        "the same input costs the same work"
    );
}

#[test]
fn a_row_is_due_at_the_instant_it_may_run() {
    let id = JobId::new();
    let row = Row::new(id, now());
    assert_eq!(row.id(), id);
    assert_eq!(row.run_at(), now());
    assert!(
        row.is_due(now()),
        "a row scheduled for this instant is due at this instant: `<` instead of `<=` is a worker
         that sits idle for the second the timer meant to spend running"
    );
    assert!(row.is_due(now() + Duration::seconds(1)));
    assert!(!row.is_due(now() - Duration::seconds(1)));
}

#[test]
fn the_pull_hands_out_the_due_rows_in_the_order_it_was_given() {
    let due = [JobId::new(), JobId::new(), JobId::new()];
    let waiting = JobId::new();
    let rows = vec![
        Row::new(due[0], now()),
        Row::new(waiting, now() + Duration::minutes(5)),
        Row::new(due[1], now()),
        Row::new(due[2], now()),
    ];

    let mut work = Work::new();
    let mut pull = Pull::of(rows);
    assert_eq!(pull.remaining(), 4);
    for expected in due {
        assert_eq!(
            pull.next(now(), &mut work),
            Some(expected),
            "the order the store ranked the batch in is the order a worker is handed it, and a pull
             that re-decided it would be a second copy of the policy"
        );
    }
    assert_eq!(
        pull.next(now(), &mut work),
        None,
        "the row that is not due yet is not handed out: it belongs to the next question, not to an
         answer about this instant"
    );
    assert_eq!(pull.remaining(), 0);
    assert_eq!(work.statements(), 3);
    assert_eq!(
        work.cells(),
        4,
        "the cost of the batch is what the loop walked over, not what it kept"
    );
}

#[test]
fn the_pull_remembers_where_it_stopped() {
    // The counts the course's baseline is written against: `claim_statements` and `scan_cells` for a
    // 256-row queue with one row in four not due. Both are the size of the batch, and the guard below
    // is what says they stay that way.
    for rows in [16, 64, 256, 1_024] {
        let work = drain(rows);
        assert_eq!(
            work.cells(),
            rows,
            "a drain of {rows} rows looks at each of them once: the pull keeps its place, and
             starting over is the quadratic drain the next case measures"
        );
        assert_eq!(
            work.statements(),
            rows - rows / NOT_DUE_EVERY,
            "the rows that are due are the ones handed out, and the ones that are not stay in the
             queue for the next pull"
        );
    }
    assert_eq!(drain(ROWS).statements(), 192);
    assert_eq!(drain(ROWS).cells(), 256);
}

#[test]
fn a_drained_pull_costs_nothing_to_ask_again() {
    let mut work = Work::new();
    let mut pull = Pull::of(batch(8));
    while pull.next(now(), &mut work).is_some() {}
    let spent = work.cells();
    assert_eq!(pull.remaining(), 0);
    assert_eq!(
        pull.next(now(), &mut work),
        None,
        "nothing left in the batch is due"
    );
    assert_eq!(
        work.cells(),
        spent,
        "asking a drained pull again costs nothing: the caller that drains in a loop cannot pay for
         the batch twice by asking once too often, which is what a cursor reset at the end would do"
    );
    assert_eq!(work.statements(), 6);
}

#[test]
fn the_guard_is_the_shape_of_the_growth() {
    let honest = drain(ROWS)
        .scale_to(&drain(2 * ROWS))
        .expect("rows were examined");
    assert!(
        honest <= SCALE_MAX,
        "twice the queue costs about twice the work and not more: {honest} against a bound of \
         {SCALE_MAX}"
    );
    assert!(
        honest < 2.1,
        "a drain that keeps its place is linearithmic at worst: {honest}"
    );

    // And the drain the guard exists for: it hands out exactly the same rows and costs the square.
    let naive = rescanning(&batch(ROWS))
        .scale_to(&rescanning(&batch(2 * ROWS)))
        .expect("rows were examined");
    assert!(
        naive > SCALE_MAX,
        "starting over on every pull is the defect this step's guard is written against, and it is \
         invisible on a short queue: twice the input costs {naive} times the work"
    );
    assert_eq!(
        rescanning(&batch(16)).statements(),
        drain(16).statements(),
        "the quadratic drain is not wrong, it is expensive — which is why a count of what it did
         cannot tell the two apart and the ratio is needed"
    );
    assert!(
        rescanning(&batch(16)).cells() > drain(16).cells(),
        "and on a sixteen-row queue the difference is already visible in the counts"
    );
}

#[test]
fn the_budget_is_written_in_names_a_gate_can_read() {
    let small = drain(ROWS);
    let large = drain(2 * ROWS);
    assert_eq!(
        report(&small, &large),
        [
            "jobq-budget claim_statements=192".to_owned(),
            "jobq-budget scan_cells=256".to_owned(),
            "jobq-budget scale_ratio_max=2.000".to_owned(),
        ],
        "the harness prints these lines and `kiln`'s bench-budget gate parses them: one metric per
         line, `name=value`, and the guard with room for a fraction"
    );
    assert_eq!(
        METRICS,
        ["claim_statements", "scan_cells", "scale_ratio_max"],
        "the names a page, a harness and the course's baseline are written in, in one list"
    );
    let honest = small.scale_to(&large).expect("rows were examined");
    assert!(
        SCALE_MAX > honest,
        "the reference drain passes the bound the course ships: {honest} against {SCALE_MAX}"
    );
    assert!(
        SCALE_MAX - honest < 0.5,
        "and the bound is a tenth over the two a linear drain costs rather than a target with room
         to hide a change of shape: {honest} against {SCALE_MAX}"
    );

    let nothing = report(&Work::new(), &large);
    assert_eq!(
        nothing.len(),
        2,
        "a comparison with no denominator prints no guard line at all: a `0` would be a pass for a
         drain that measured nothing, and the gate treats a missing metric as `blocked` instead"
    );
}

#[test]
fn the_page_states_the_budget_the_gate_compares() {
    let page = page();
    let low = page.to_ascii_lowercase();
    for placeholder in ["todo", "tbd", "fixme"] {
        assert!(
            !low.contains(placeholder),
            "{PAGE} still says `{placeholder}`: a budget page with a blank in it is a budget nobody
             has written down"
        );
    }
    for metric in METRICS {
        assert!(
            page.contains(metric),
            "{PAGE} does not state `{metric}`, which is a number the gate compares — a metric a page
             does not explain is a number a reviewer has to guess at"
        );
    }
    assert!(
        page.contains(&SCALE_MAX.to_string()),
        "{PAGE} does not state the guard's bound ({SCALE_MAX}), so the page and the baseline the gate
         reads are two different budgets"
    );
    assert!(
        page.contains("count") || page.contains("worked") || page.contains("cells"),
        "{PAGE} does not say what the metrics count: a budget of wall-clock time is a budget a
         loaded machine fails, and the page is where that decision is written down"
    );
}
