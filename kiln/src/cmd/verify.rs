//! `kiln verify [track]` — judge every authored step, and say what the result is worth.
//!
//! `kiln run` answers "does this step work?"; `verify` answers it for the whole track at once, which
//! is what CI and a returning learner both need (plan §10). Two things make it more than a loop over
//! `run`:
//!
//! * it is a **report**, not a mutation. It never scaffolds and never creates a project; it syncs a
//!   step's spec files only into a project that already exists, so running it cannot change what the
//!   learner has. A step whose project is not there yet is `blocked`, with the command that starts
//!   it;
//! * it carries the **honesty markers** from `.kiln/state.toml` — was the reference solution
//!   applied, was a spec file edited — so a green report can be read for what it is (plan §3 D2).
//!
//! `--json` prints the same thing as one machine-readable document.

use serde_json::{Value, json};

use crate::cmd::Session;
use crate::error::{EXIT_ERROR, EXIT_FAILED, EXIT_NOT_AUTHORED, EXIT_OK, Result};
use crate::gates::{GateResult, Tier};
use crate::info::{Step, Track};
use crate::project;
use crate::spec;

/// One step's verdict.
struct Verdict {
    id: String,
    number: String,
    title: String,
    /// `passed`, `failed`, `blocked` or `not-authored`.
    status: &'static str,
    /// Why, in one line.
    detail: String,
    /// Spec run, when it was judged.
    spec: Option<(bool, String)>,
    /// Gate results, in declaration order.
    gates: Vec<GateResult>,
    /// What to type next, when the step could not be judged.
    fix: Option<String>,
}

impl Verdict {
    fn passed(&self) -> bool {
        self.status == "passed"
    }
}

/// Judge every authored step of the requested tracks.
pub fn run(
    session: &mut Session,
    track: Option<&str>,
    as_json: bool,
    run_slow: bool,
) -> Result<u8> {
    let tracks: Vec<&Track> = match track {
        Some(id) => vec![session.course().track(id)?],
        None => session.course().tracks().iter().collect(),
    };

    let mut verdicts = Vec::new();
    for track in tracks {
        for step in &track.step {
            verdicts.push(judge(session, track, step, run_slow)?);
        }
        if !as_json {
            print_track(track, &verdicts);
        }
    }

    let failed = verdicts.iter().filter(|v| v.status == "failed").count();
    let blocked = verdicts.iter().filter(|v| v.status == "blocked").count();
    let missing = verdicts
        .iter()
        .filter(|v| v.status == "not-authored")
        .count();
    let judged = verdicts.len() - missing - blocked;
    let green = verdicts.iter().filter(|v| v.passed()).count();

    if as_json {
        println!("{}", report_json(session, track, &verdicts));
    } else {
        println!();
        println!(
            "{green}/{judged} judged step(s) pass · {failed} failed · {blocked} not prepared · \
             {missing} without content"
        );
        if failed == 0 && blocked == 0 && missing == 0 {
            println!("✓ every authored step of this track passes");
        } else if failed > 0 {
            println!("✗ the steps above that failed are the ones to fix");
        } else {
            println!("nothing failed yet — but not everything was judged either");
        }
    }

    // Exit codes are part of the contract (cmd/mod.rs): 1 a failure, 2 nothing judgeable at all,
    // 3 content that is not written yet, and only a fully green track exits 0.
    if failed > 0 {
        return Ok(EXIT_FAILED);
    }
    if blocked > 0 && judged == 0 {
        return Ok(EXIT_ERROR);
    }
    if blocked > 0 || missing > 0 {
        return Ok(EXIT_NOT_AUTHORED);
    }
    Ok(EXIT_OK)
}

/// Judge one step without changing anything the learner wrote.
fn judge(session: &Session, track: &Track, step: &Step, run_slow: bool) -> Result<Verdict> {
    let mut verdict = Verdict {
        id: step.qualified_id(),
        number: step.number().to_owned(),
        title: step.title.clone(),
        status: "not-authored",
        detail: String::new(),
        spec: None,
        gates: Vec::new(),
        fix: None,
    };

    let missing = project::missing_content(session.course(), track, step);
    if !missing.is_empty() {
        verdict.detail = missing.join("; ");
        return Ok(verdict);
    }

    let project_dir = track.project_dir(session.course().root());
    if !project_dir.is_dir() {
        verdict.status = "blocked";
        verdict.detail = format!("{} does not exist yet", track.project);
        verdict.fix = Some(format!("kiln run {}", step.qualified_id()));
        return Ok(verdict);
    }

    // Sync the specs into a project that exists, so a step the learner solved is judged against the
    // course's test rather than against a file that was never copied there.
    spec::sync(session.course().root(), track, step, &project_dir, false)?;
    verdict.gates = session.gates(step, run_slow)?;
    let gates_green = verdict
        .gates
        .iter()
        .all(|result| result.spec.tier != Tier::Must || !result.breaks_the_step());
    let cannot_judge = verdict.gates.iter().any(|result| {
        result.spec.tier == Tier::Must && result.outcome == crate::gates::Outcome::Blocked
    });

    match session.spec(step) {
        Ok(ran) => {
            verdict.spec = Some((ran.success(), summary(&ran)));
            if ran.success() && gates_green {
                verdict.status = "passed";
                verdict.detail = format!(
                    "spec ✓ · {}/{} must gates",
                    verdict
                        .gates
                        .iter()
                        .filter(|result| result.spec.tier == Tier::Must
                            && result.outcome == crate::gates::Outcome::Passed)
                        .count(),
                    verdict
                        .gates
                        .iter()
                        .filter(|result| result.spec.tier == Tier::Must)
                        .count()
                );
            } else {
                verdict.status = "failed";
                verdict.detail = if ran.success() {
                    "the specs pass but a must gate does not".to_owned()
                } else {
                    format!("spec ✗ {}", summary(&ran))
                };
            }
        }
        Err(error) => {
            verdict.status = "blocked";
            verdict.detail = error.to_string();
        }
    }
    if verdict.status == "failed" && cannot_judge {
        verdict.detail.push_str(" (a gate could not run at all)");
    }
    Ok(verdict)
}

fn summary(ran: &crate::process::Ran) -> String {
    if ran.timed_out {
        return format!("timed out after {:?}", ran.elapsed);
    }
    match ran.code {
        Some(0) => "exit code 0".to_owned(),
        Some(code) => {
            let first = if ran.stderr.trim().is_empty() {
                &ran.stdout
            } else {
                &ran.stderr
            };
            let line = first
                .lines()
                .find(|line| !line.trim().is_empty())
                .map_or("no output", str::trim);
            format!("exit code {code}: {line}")
        }
        None => "killed by a signal".to_owned(),
    }
}

fn print_track(track: &Track, verdicts: &[Verdict]) {
    println!();
    println!("{} {} — {}", track.label(), track.id, track.title);
    for verdict in verdicts {
        let mark = match verdict.status {
            "passed" => "✓",
            "failed" => "✗",
            _ => "·",
        };
        println!(
            "  {mark} {:<20} {:<13} {}",
            verdict.number, verdict.status, verdict.detail
        );
        for gate in &verdict.gates {
            if gate.breaks_the_step() || gate.outcome == crate::gates::Outcome::Pending {
                println!("      {}", gate.line());
            }
        }
        if let Some(fix) = &verdict.fix {
            println!("      start it with `{fix}`");
        }
    }
}

/// The whole report as one JSON document.
fn report_json(session: &Session, track: Option<&str>, verdicts: &[Verdict]) -> Value {
    let steps: Vec<Value> = verdicts.iter().map(|v| step_json(session, v)).collect();
    let count = |status: &str| steps.iter().filter(|s| s["status"] == status).count();
    json!({
        "format_version": 1,
        "track": track,
        "steps": steps,
        "totals": {
            "steps": steps.len(),
            "passed": count("passed"),
            "failed": count("failed"),
            "blocked": count("blocked"),
            "not_authored": count("not-authored"),
            "clean": count("failed") == 0 && count("blocked") == 0 && count("not-authored") == 0,
        }
    })
}

fn step_json(session: &Session, verdict: &Verdict) -> Value {
    let recorded = session.state().steps.get(&verdict.id);
    let honesty = json!({
        "solution_applied": recorded.is_some_and(|state| state.solution_applied),
        "spec_modified": recorded.is_some_and(|state| state.spec_modified),
        "own_work": !recorded.is_some_and(|state| state.solution_applied),
    });
    let spec = match &verdict.spec {
        Some((passed, detail)) => {
            json!({"status": if *passed { "passed" } else { "failed" }, "detail": detail})
        }
        None => Value::Null,
    };
    let gates: Vec<Value> = verdict
        .gates
        .iter()
        .map(|gate| {
            json!({
                "gate": gate.spec.to_string(),
                "tier": match gate.spec.tier {
                    Tier::Must => "must",
                    Tier::Should => "should",
                },
                "slow": gate.spec.slow,
                "outcome": gate.outcome.as_str(),
                "detail": gate.detail,
            })
        })
        .collect();
    json!({
        "id": verdict.id,
        "number": verdict.number,
        "title": verdict.title,
        "status": verdict.status,
        "detail": verdict.detail,
        "fix": verdict.fix,
        "spec": spec,
        "gates": gates,
        "honesty": honesty,
    })
}
