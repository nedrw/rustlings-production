# rustlings-production

A step-by-step course for building **production-grade Rust services** — rustlings-style verification
(one task, one command, pass/fail), but the training ground is five real projects that grow from
simple to complex instead of isolated single-file exercises.

```sh
kiln          # continue at the first unfinished step
kiln run      # run this step's spec tests
kiln check    # run this step's production gates only (the lint ladder, docs, contracts)
kiln hint     # one nudge, and where to read the rest
kiln reset    # take this step's scaffold back out, keeping your code
kiln solution # apply the reference solution (recorded, not prevented)
kiln verify   # full check, CI mode (--json, --slow)
kiln watch    # re-run on save
```

Every step = a short brief (WHY / TASK / DONE WHEN / TRAP / FURTHER) + read-only spec tests + a set of
production gates: a lint ladder that only ever tightens, `cargo-mutants` proving the specs are not
vacuous, public-API baselines, perf budgets, dependency and `unsafe` budgets, graceful shutdown and
failure drills.

**No repeated teaching across projects.** Every learnable thing is a *capability* with a depth
(`aware → apply → design → operate`). A step declares what it teaches and at which depth; the runner
and `kiln lint-caps` reject any step that re-teaches a capability at a depth it already reached. So a
later project never re-explains graceful shutdown — it makes you apply it under a harder failure mode,
with stricter gates and fewer hints.

| track | project | difficulty | steps |
| --- | --- | --- | --- |
| T1 | `mailer` — subscription microservice (inspired by *Zero To Production in Rust*) | intro → mid | 22 |
| T2 | `relay` — webhook delivery service | mid | 24 |
| T3 | `jobq` — job queue engine (library + worker + CLI) | mid-high | 26 |
| T4 | `lakelet` — telemetry ingest and query | high | 24 |
| T5 | `meskv` — distributed KV / scheduler (optional) | highest | 22 |

## Read next

- **[docs/plan.md](docs/plan.md)** — mechanisms, decisions (and rejected alternatives), runner design,
  gate catalogue, environment constraints, milestones, risks.
- **[docs/curriculum.md](docs/curriculum.md)** — capability ledger, track dependency graph, all 118
  steps, and worked examples of one capability across tracks (what is new, what is deliberately not
  repeated).
- **[docs/authoring.md](docs/authoring.md)** — step templates, metadata rules, i18n, scaffold patches,
  checklists.
- **[docs/handoff-m0.md](docs/handoff-m0.md)** — the compressed M0 brief: scope, acceptance, verified
  environment facts, what M0 deliberately does not do. Read this one first;
  [docs/handoff-m0-log.md](docs/handoff-m0-log.md) is the build log behind it (per-segment progress,
  measured facts, traps).
- **[docs/drafts/](docs/drafts/)** — measured designs for steps whose decisions were expensive to
  re-derive.

Planning docs are written in Chinese (they are maintainer-facing); course content is English with a
locale fallback and translation-drift detection, so other languages can be added without code changes.

## Status

**M0 is closed.** The metadata layer, the capability lint, the twelve gates and the runner are in, and
T1 `mailer` is authored end to end: 22 steps, each with a brief, read-only spec tests, a scaffold, a
reference solution and gate wiring. T2–T5 are metadata and step tables only. M1 is next (T2 plus the
capability-dedup runtime); see [docs/plan.md](docs/plan.md) §11.

The numbers below were measured on this machine at commit `c08f33a`, on the frozen tree:

- `cargo xtask ci` — **486 s**, last line `22/22 step(s) hold the contract` (fmt, clippy
  `-D warnings`, workspace tests 179+15+1, spec integrity 22 pinned, `kiln lint-caps`
  `97 capabilities · 134 slots · 118 ledger positions · 22 authored steps`).
- `kiln verify --json mailer` — **205 s**, `"passed": 22, "failed": 0, `blocked`: 0,
  "not_authored": 0`.
- `kiln links` (offline, whole repository) — 29 pages, 167 links, 0 findings;
  `kiln links --external` — 115 s, `112 answered · 7 unclear · 0 gone` (the seven `?` are hosts that
  were unreachable from that network, not 404s).
- `kiln --lang ja list` — exit 0, prints `content locale: ja — 0/22 step bodies, falling back through
  ja -> en`.
- The three container steps (1.9 Postgres, 1.19 mailpit, 1.21 the image itself) run under local
  podman. `kiln images` says which images to pull first; `kiln doctor` warns when the machine needs
  `KILN_REGISTRY_MIRROR`.

```sh
cargo xtask ci           # the only entry point CI calls: fmt, clippy -D warnings, tests,
                         # spec integrity, lint-caps, and `kiln selftest` (the anti-rot check)
cargo run -p kiln --     # continue at the first unfinished step
cargo run -p kiln -- list
cargo run -p kiln -- run mailer/1.22-runbook    # scaffold + spec tests (specs are synced into app/)
cargo run -p kiln -- check mailer/1.22-runbook  # the step's must gates
cargo run -p kiln -- hint 1.1
cargo run -p kiln -- selftest                   # scaffold-fails / solution-passes, in a scratch copy
cargo run -p kiln -- verify --json              # every authored step, with honesty markers
cargo run -p kiln -- images                     # what the container steps declare, and what is local
cargo run -p kiln -- links                      # every relative link in the repository, offline
cargo run -p kiln -- spec restore               # put the course's spec files back
cargo run -p kiln -- lint-caps                  # 97 capabilities, 134 slots, 118 planned, 22 authored
cargo run -p kiln -- doctor                     # toolchain, runtime, cargo tools, registry mirror
cargo run -p kiln -- i18n status
```

Also in: the course metadata under `course/` (`course.toml`, `capabilities.toml`, all 22 T1 steps in
`tracks/mailer/track.toml`); the capability rules R0–R6 as `kiln lint-caps`; the gates
`spec-integrity` / `caps` / `fmt` / `clippy@N` / `no-todo` / `docs:public` / `exit-code` / `i18n` /
`images` / `container-repro` / `links` (`timeout` is deliberately the runner's own deadline rather
than a per-step gate, see plan.md §6); `list` / `caps` / `run` / `check` / `hint` / `reset` /
`solution` / `spec restore` / `spec manifest` / `selftest` / `verify` / `watch` / `doctor` / `images` /
`links` / `i18n status`; and the locale fallback chain. `tools/lint-caps-md.py` is the prototype the
Rust lint was ported from; it still cross-checks the ledger against the curriculum's own tables,
which is why it is kept:

```sh
python3 tools/lint-caps-md.py   # 97 capabilities, 118 steps, 134 (capability, depth) slots, 0 issues
```

What is not in yet: T2–T5 content; the slow-path gates (`mutants`, `coverage`, `bench`, `loom`,
`soak`, `sim`); capability dedup at runtime (`kiln plan --fast`, `kiln probe`, `review --due`, spaced
repetition); translations (the fallback chain and the drift detector are ready, no locale has
translated bodies); the `new-track` generator; a documentation site. See
[docs/plan.md](docs/plan.md) §11.
