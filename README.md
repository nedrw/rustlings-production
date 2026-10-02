# rustlings-production

A step-by-step course for building **production-grade Rust services** — rustlings-style verification (one task, one
command, pass/fail), but the training ground is five real projects that grow from simple to complex instead of
isolated single-file exercises.

```sh
kiln          # continue at the first unfinished step
kiln run      # run this step's spec tests
kiln check    # run this step's production gates only (lint ladder, docs, mutants, deny…)
kiln watch    # re-run on save
kiln verify   # full check, CI mode (--json, --slow)
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

Planning stage — no runner code yet. `tools/lint-caps-md.py` already mechanically checks the capability ledger in
`docs/curriculum.md` against the step tables (the prototype of `kiln lint-caps`):

```sh
python3 tools/lint-caps-md.py   # 97 capabilities, 118 steps, 134 (capability, depth) slots, 0 issues
```

Next: **M0** — `kiln` skeleton plus all 22 `mailer` steps as real content (see [docs/plan.md](docs/plan.md) §11).

