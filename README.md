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
kiln watch    # judge again whenever the project's contents change
kiln plan     # the path ahead; --fast marks the steps a passed probe has already proven
kiln skip     # skip such a step (recorded as `»`, never counted as done)
kiln probe    # re-check one capability with a 3–10 minute exercise
kiln review   # what you have proven per capability, and what is going stale (--due)
kiln site     # render the course as a static site, every authored locale
kiln new-track # lay the mechanical half of a new track (docs/new-track.md)
```

Every step = a short brief (WHY / TASK / DONE WHEN / TRAP / FURTHER) + read-only spec tests + a set of
production gates. The lint ladder that only ever tightens is in; the heavier gates arrived with the
milestones that needed them — mutation testing proving the specs are not vacuous, coverage of error
branches, perf budgets, API baselines, the `unsafe` budget, and the `sim`/`soak`/`fuzz` trio
(plan.md §10, §11).

**No repeated teaching across projects.** Every learnable thing is a *capability* with a depth
(`aware → apply → design → operate`). A step declares what it teaches and at which depth; the runner
and `kiln lint-caps` reject any step that re-teaches a capability at a depth it already reached. So a
later project never re-explains graceful shutdown — it makes you apply it under a harder failure mode,
with stricter gates and fewer hints.

| track | project | difficulty | steps |
| --- | --- | --- | --- |
| T1 | `mailer` — subscription microservice (inspired by *Zero To Production in Rust*) **delivered** | intro → mid | 22 |
| T2 | `relay` — webhook delivery service **delivered** | mid | 24 |
| T3 | `jobq` — job queue engine (library + worker + CLI) **delivered** | mid-high | 27 |
| T4 | `lakelet` — telemetry ingest and query **authored** | high | 24 |
| T5 | `meshkv` — distributed KV / scheduler (optional) **authored** | highest | 22 |

## Read next

- **[docs/plan.md](docs/plan.md)** — mechanisms, decisions (and rejected alternatives), runner design,
  gate catalogue, environment constraints, milestones, risks.
- **[docs/curriculum.md](docs/curriculum.md)** — capability ledger, track dependency graph, all 119
  planned steps, and worked examples of one capability across tracks (what is new, what is
  deliberately not repeated).
- **[docs/authoring.md](docs/authoring.md)** — step templates, metadata rules, i18n, scaffold patches,
  checklists. `tools/rebuild-patches.py` is the mechanical half of it: a track's scaffold and
  solution patches are *derived* from the trees they define, and this tool lays the chain, formats
  the trees and regenerates the patches (or, with `--check`, proves that the committed patches are
  what the trees produce — which is a step of `cargo xtask ci`).
- **[docs/t1-mailer.md](docs/t1-mailer.md)** — T1 `mailer` as delivered: the 22 steps, the gate
  ladder, the project's final shape, the test fixtures, the patterns T2–T5 copy, and the measured
  numbers. [docs/t2-relay.md](docs/t2-relay.md), [docs/t3-jobq.md](docs/t3-jobq.md),
  [docs/t4-lakelet.md](docs/t4-lakelet.md) and [docs/t5-meshkv.md](docs/t5-meshkv.md) record T2–T5 in
  the same shape.
- **[docs/handoff-m0.md](docs/handoff-m0.md)** — the compressed M0 brief: scope, acceptance, verified
  environment facts, conventions, and where everything else lives. Read this one first;
  [docs/handoff-m0-log.md](docs/handoff-m0-log.md) is the build log behind it (per-segment progress,
  measured facts, traps). [docs/handoff-m1.md](docs/handoff-m1.md),
  [docs/handoff-m2.md](docs/handoff-m2.md) and [docs/handoff-m3.md](docs/handoff-m3.md) are the same
  for M1 (the runtime half of de-duplication), M2 (the slow gates and T3) and M3 (T4 and the
  `sim`/`soak`/`fuzz` gates), each with its own `-log.md`.
- **[docs/drafts/](docs/drafts/)** — measured designs for steps whose decisions were expensive to
  re-derive.

Planning docs are written in Chinese (they are maintainer-facing); course content is English with a
locale fallback and translation-drift detection, so other languages can be added without code changes.

## Status

**M0 through M3 are closed.** The metadata layer, the capability lint, the gates
and the runner are in. M1 added the runtime half of the de-duplication mechanism (`kiln plan --fast`
/ `skip` / `probe` / `review --due`, one capability record per learner per capability) and authored
T2 `relay` end to end; M2 authored T3 `jobq` and brought the slow gates (`mutants`, `coverage`,
`bench-budget`, `semver`, `unsafe-budget`, `conformance`); M3 authored T4 `lakelet` end to end — 24
steps — added the `slow:sim`/`slow:soak`/`slow:fuzz` gates and the container and profiling
infrastructure, and filled the probe pool to 65 exercises behind the spaced-repetition loop. M3's
acceptance ran green on this tree: the offline fast path 24/24, `kiln verify lakelet --slow --json`
`{passed: 24, failed: 0, clean: true}`, all six acceptance drills, and `cargo xtask ci` exit 0 —
see [docs/handoff-m3.md](docs/handoff-m3.md) §4 for the numbers and
[docs/t4-lakelet.md](docs/t4-lakelet.md) for what T4 delivers.

**M5 is delivered.** The course reads in Chinese (119/119 bodies, `kiln i18n status` — structure 0
errors, drift 0 warnings, 133 glossary terms), `kiln new-track` plus [docs/new-track.md](docs/new-track.md)
were verified the way the plan asks — a three-step toy track built with the guide reaches
`3/3 step(s) hold the contract` — `kiln site` publishes the whole course (251 pages in two locales,
every link checked by `site --check`, which `cargo xtask ci` runs), and
[docs/maintenance.md](docs/maintenance.md) is the annual pass. What changed, what it cost and what
the acceptance run found: [docs/handoff-m5-log.md](docs/handoff-m5-log.md).

**What the numbers below measure.** The M1 numbers were measured on this machine on the M1 tree
(`bf9ac5f`) with a clean build cache; the T1-only numbers are kept from the M0 tree (`62e7c5c`,
re-verified at `dd47c11`) and say so; the current-tree numbers are from this README's own pass over
the M3 tree. The link counts come from the repository-wide report, which counts the repository's own
pages and names the directories it skipped (`app/<track>` is a learner's working copy, judged by its
step's own `links` gate).

- `cargo xtask ci` — **761 s** on the M1 tree: fmt, `rustfmt --check` on the course's own files,
  clippy `-D warnings`, 202 + 15 + 1 tests, spec integrity 46 pinned, `kiln lint-caps`
  `97 capabilities · 134 slots · 118 ledger positions · 46 authored steps`, `kiln links` offline,
  `kiln probe --check` 19/19, and `kiln selftest` — last lines `22/22 step(s) hold the contract` and
  `xtask ci: all checks passed`. (Measured with libtest serialized to dodge the fixture collision
  `bf9ac5f` fixes; that workaround is no longer needed. The unit tests read 208 then; the current
  tree reads 335 — the growth came with the slow gates, the probe pool and the state-file and
  patch-engine fixes, not with a change to what is judged.)
- On the current tree (M4, T5 authored end to end): `kiln lint-caps` — `97 capabilities · 136
  slots · 119 ledger positions · 119 authored steps` (all five tracks' bodies), no issues;
  `kiln links` — 139 pages, 557 links, 330 external targets, 0 findings; `kiln probe --check` —
  83/83 (the M4 batch took the pool from 65 to 83); `cargo test -p kiln` — 379 + 21 + 1 + 1 tests (the last file pins step titles to their
  bodies);
  the offline fast path — 24/24 with `CARGO_NET_OFFLINE=true` and a private
  `XDG_CACHE_HOME`; `kiln verify lakelet --slow --json` —
  `{blocked: 0, clean: true, failed: 0, not_authored: 0, passed: 24, steps: 24}` (625 s).
- `kiln selftest --track relay` — **476 s**, `24/24 step(s) hold the contract`: every T2 step's spec
  fails on its scaffold, passes on the reference solution, and every must gate is green.
- `kiln verify relay` — **155 s**, `24/24 judged step(s) pass · 0 failed · 0 not prepared · 0 without
  content`; `kiln caps --track relay` shows the six exit capabilities verified.
- `kiln links` (offline, the repository's own markdown) — 57 pages, 370 links, 228 external targets,
  0 findings; a learner's project under `app/` is skipped and named, so the number is the same on a
  fresh clone as on a machine mid-course. `kiln links --external` — **115 s** (M0 tree),
  `112 answered · 7 unclear · 0 gone` (the seven `?` are hosts that were unreachable from that
  network, not 404s).
- M0 tree, for reference: `kiln verify --json mailer` — **181 s**, `"passed": 22, "failed": 0`;
  `cargo xtask ci` — **579 s**.
- `kiln --lang ja list` — exit 0, prints `content locale: ja — 0/97 step bodies, falling back through
  ja -> en`.
- The three container steps (1.9 Postgres, 1.19 mailpit, 1.21 the image itself) run under local
  podman. `kiln images` says which images to pull first; `kiln doctor` warns when the machine needs
  `KILN_REGISTRY_MIRROR`.

```sh
cargo xtask ci           # the only entry point CI calls: fmt, `rustfmt --check` on course/,
                         # clippy -D warnings, tests, spec integrity, lint-caps, the offline link
                         # check, `kiln probe --check`, `kiln selftest`
cargo run -p kiln --     # continue at the first unfinished step
cargo run -p kiln -- list
cargo run -p kiln -- run mailer/1.22-runbook    # scaffold + spec tests (specs are synced into app/)
cargo run -p kiln -- check mailer/1.22-runbook  # the step's must gates
cargo run -p kiln -- hint 1.1
cargo run -p kiln -- selftest                   # scaffold-fails / solution-passes, in a scratch copy
cargo run -p kiln -- selftest --track relay 2.1 # the same check for another track, one step at a time
cargo run -p kiln -- verify --json              # every authored step, with honesty markers
cargo run -p kiln -- plan --fast                # what a passed probe would let you skip, and why
cargo run -p kiln -- probe                      # the review exercises; `probe --check` validates them
cargo run -p kiln -- review --due               # capabilities whose last proof is over 45 days old
cargo run -p kiln -- images                     # what the container steps declare, and what is local
cargo run -p kiln -- links                      # the repository's own links, offline (learner projects aside)
cargo run -p kiln -- spec restore               # put the course's spec files back
cargo run -p kiln -- lint-caps                  # 97 capabilities, 136 slots, 119 steps, all authored
cargo run -p kiln -- doctor                     # toolchain, runtime, cargo tools, registry mirror
cargo run -p kiln -- i18n status
```

Also in: the course metadata under `course/` (`course.toml`, `capabilities.toml`, the step tables in
`tracks/<track>/track.toml`); the capability rules R0–R6 as `kiln lint-caps`; twenty implemented
gates — the eleven fast ones (`spec-integrity` / `caps` / `fmt` / `clippy@N` / `no-todo` /
`docs:public` / `exit-code` / `i18n` / `images` / `container-repro` / `links`) and the nine that M2
and M3 added (`mutants` / `coverage` / `bench-budget` / `semver` / `unsafe-budget` / `conformance` /
`sim` / `soak` / `fuzz`) — plus `timeout`, which is the runner's own deadline rather than a per-step
gate (plan.md §6), and `deny`, which the catalogue holds until `cargo-deny` joins the toolchain
baseline; `list` / `caps` / `run` / `check` / `hint` / `reset` /
`solution` / `spec restore` / `spec manifest` / `selftest` / `verify` / `watch` / `doctor` / `images` /
`links` / `i18n status` / `plan` / `skip` / `probe` / `review`; and the locale fallback chain.
`tools/lint-caps-md.py` is the prototype the
Rust lint was ported from; it still cross-checks the ledger against the curriculum's own tables,
which is why it is kept:

```sh
python3 tools/lint-caps-md.py   # 97 capabilities, 119 steps, 136 (capability, depth) slots, 0 issues
```

What is not in yet: hosting for the generated site (`kiln site` writes files; no host is chosen),
and one full `cargo xtask ci` run over the tree that carries `kiln site` and `kiln new-track` — the
milestone work was verified with the content tier plus `cargo test -p kiln` (see
[docs/handoff-m5-log.md](docs/handoff-m5-log.md) §19).
