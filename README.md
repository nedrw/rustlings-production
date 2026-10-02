# rustlings-production

A step-by-step course for building **production-grade Rust services** — rustlings-style verification (one task, one
command, pass/fail), but the training ground is five real projects that grow from simple to complex instead of
isolated single-file exercises.

```sh
kiln          # continue at the first unfinished step
kiln run      # run this step's spec tests
kiln check    # run this step's production gates only (lint ladder, docs, mutants, deny…)
kiln hint     # one nudge, and where to read the rest
kiln reset    # take this step's scaffold back out, keeping your code
kiln solution # apply the reference solution (recorded, not prevented)
kiln verify   # full check, CI mode (--json, --slow)
kiln watch    # re-run on save
```

Every step = a short brief (WHY / TASK / DONE WHEN / TRAP / FURTHER) + read-only spec tests + a set of production
gates: a lint ladder that only ever tightens, `cargo-mutants` proving the specs are not vacuous, public-API
baselines, perf budgets, dependency and `unsafe` budgets, graceful shutdown and failure drills.

**No repeated teaching across projects.** Every learnable thing is a *capability* with a depth
(`aware → apply → design → operate`). A step declares what it teaches and at which depth; the runner and
`kiln lint-caps` reject any step that re-teaches a capability at a depth it already reached. So a later project
never re-explains graceful shutdown — it makes you apply it under a harder failure mode, with stricter gates and
fewer hints.

| track | project | difficulty | steps |
| --- | --- | --- | --- |
| T1 | `mailer` — subscription microservice (topic inspired by *Zero To Production in Rust*) | intro → mid | 22 |
| T2 | `relay` — webhook delivery service | mid | 24 |
| T3 | `jobq` — job queue engine (library + worker + CLI) | mid-high | 26 |
| T4 | `lakelet` — telemetry ingest and query | high | 24 |
| T5 | `meshkv` — distributed KV / scheduler (optional) | highest | 22 |

## Read next

- **[docs/plan.md](docs/plan.md)** — mechanisms, decisions (and rejected alternatives), runner design, gate
  catalogue, environment constraints, milestones, risks.
- **[docs/curriculum.md](docs/curriculum.md)** — capability ledger, track dependency graph, all 118 steps, and
  worked examples of one capability across tracks (what is new, what is deliberately not repeated).
- **[docs/authoring.md](docs/authoring.md)** — step templates, metadata rules, i18n, scaffold patches, checklists.
- **[docs/handoff-m0.md](docs/handoff-m0.md)** — the compressed build brief: M0 scope, build order, verified
  environment facts, what M0 deliberately does not do. Read this one first when starting implementation.

Planning docs are written in Chinese (they are maintainer-facing); course content is English with a locale
fallback and translation-drift detection, so other languages can be added without code changes.

## Status

M0 in progress. The metadata layer, the capability lint, the gates and the runner are in, and T1 steps 1.1–1.3
are authored end to end (body, spec, scaffold, reference solution, gates), so the step format is proven
before it is repeated 19 more times.

```sh
cargo xtask ci           # the only entry point CI calls: fmt, clippy -D warnings, tests,
                         # spec integrity, lint-caps, and `kiln selftest` (the course's anti-rot check)
cargo run -p kiln --     # continue at the first unfinished step (T1 content for 1.4+ is next)
cargo run -p kiln -- list
cargo run -p kiln -- run mailer/1.1-workspace   # scaffold + spec tests (specs are synced into app/)
cargo run -p kiln -- check mailer/1.1-workspace # the step's must gates
cargo run -p kiln -- hint 1.1
cargo run -p kiln -- selftest                   # scaffold-fails / solution-passes, in a scratch copy
cargo run -p kiln -- verify --json              # every authored step, with honesty markers
cargo run -p kiln -- spec restore               # put the course's spec files back
cargo run -p kiln -- lint-caps                  # 97 capabilities, 134 slots, 118 planned steps, 22 authored
cargo run -p kiln -- doctor                     # toolchain, container runtime, cargo tools, registry mirror
cargo run -p kiln -- i18n status
```

What works today: the course metadata under `course/` (`course.toml`, `capabilities.toml`, all 22 T1 steps in
`tracks/mailer/track.toml`), the capability rules R0–R6 as `kiln lint-caps`, six working gates
(`spec-integrity` / `caps` / `fmt` / `clippy@N` / `no-todo` / `i18n`), the scaffolds, reference solutions and
read-only specs of T1 steps 1.1–1.3, `list` / `caps` / `run` / `check` / `hint` / `reset` / `solution` /
`spec restore` / `spec manifest` / `selftest` / `verify` / `doctor` / `i18n status`, and the locale
fallback chain. The prototype of the lint is still `tools/lint-caps-md.py` (it reads the curriculum
document directly):

```sh
python3 tools/lint-caps-md.py   # 97 capabilities, 118 steps, 134 (capability, depth) slots, 0 issues
```

What is not there yet: step bodies, spec tests, scaffolds and reference solutions for T1 1.4–1.22 and
T2–T5; `kiln watch`; the remaining gates (`docs:public`, `exit-code`, `images`, `container-repro`, `links`,
and the slow ones); translations. See [docs/plan.md](docs/plan.md) §11.

