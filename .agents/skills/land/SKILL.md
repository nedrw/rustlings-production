---
name: land
description: >-
  Land the current thread's committed changes in the user's own project repository: verify the exact
  commit being landed with the repository's full CI command, push it to the `local` remote so the
  destination checkout updates in place, then verify what landed where. Use only when the user has
  explicitly asked to land or merge the change — the Land Changes button, `/land`, or wording such
  as "land this" or "merge this into main". Not for reviewing a change, preparing or committing
  work, running checks, or installing and editing skills.
disable-model-invocation: true
metadata:
  delta-action: land
---

# Land this project's changes

Landing means: the thread's commits end up on `main` of the project repository the user opened in
Delta, so their own checkout holds them. The request that invoked this skill *is* the decision to
land: do not ask whether to merge again. Keep the safeguards below, and stop only for a genuine
blocker.

## Where landing puts the change

- This repository has no upstream remote (no GitHub/GitLab `origin`, no remote-tracking branches),
  so there is no pull request to open and `.github/workflows/ci.yml` cannot run on a hosted runner.
  The landing target is the `local` remote Delta provides — confirm it with
  `git remote get-url local`; it points at the project repository that holds the user's checkout
  (a `/path/to/project/.git`), expected here to be `/Users/drmin/RustroverProjects/rustlings-production/.git`.
- Pushing to `local` targets the branch checked out there, and the push side is configured with
  `receive.denyCurrentBranch=updateInstead` (`git config --get remote.local.receivepack`), so a clean
  destination updates in place: after the push the files are in the project folder itself. The
  destination's `main` may not exist yet — the first landing creates it.
- `main` is the branch everywhere in this repository, and it is the destination branch.

## Preflight — all of these before pushing

1. `git rev-parse --show-toplevel` fixes the project root; run everything from there.
2. No uncommitted changes to tracked files: `git status --porcelain`. This is the safeguard against
   overwriting unrelated or half-finished work — the destination updates in place, so a dirty tree is
   a stop-and-report, never a `git checkout`/`git reset` away. Untracked files that git ignores
   (`target/`, `app/`, `.kiln/`) are not changes. Files under `.agents/` are the exception: they are
   project configuration and part of what the user asked to land, so commit them first as their own
   short commit (for example `chore(agents): add the land skill`) and continue. Anything else
   untracked or modified: report the paths and stop.
3. `git fetch local` (this only moves `refs/remotes/local/*`) and inspect what would land:
   - if `git rev-parse --verify --quiet refs/remotes/local/main` fails, the destination's `main` is
     unborn: everything on this branch lands;
   - otherwise `git log --oneline refs/remotes/local/main..HEAD` is the change. An empty list means
     there is nothing to land — say so and stop.
4. If `refs/remotes/local/main` exists, it must be an ancestor of `HEAD`
   (`git merge-base --is-ancestor refs/remotes/local/main HEAD`); otherwise go to "A diverged
   destination" below.

## Checks — the repository's own gate, on the exact commit

- The repository's only CI job is `cargo xtask ci` (`.github/workflows/ci.yml` calls it as its single
  run step, and `xtask/src/main.rs` defines it); the authoring checklist in `docs/authoring.md`
  requires it green. It must be green for `git rev-parse HEAD` — the commit being landed, not an
  earlier one, not a pending run.
- Reuse a recorded green run for that exact commit hash when one exists and names the hash and its
  result (a thread note, a parent-thread record, a subagent report). Otherwise run it; a full run
  takes about 12 minutes here.
- Run it with private caches, both because concurrent runs collide and because the container steps
  need a path the container runtime can see:

  ```sh
  CARGO_TARGET_DIR=/private/tmp/kiln-land-target \
  XDG_CACHE_HOME=/private/tmp/xdg-land \
  cargo xtask ci
  ```

  Use `/private/tmp`, not `/tmp`: this machine's container runtime is a podman VM that shares only
  `/Users`, `/private` and `/var/folders`. Never let cargo write `target/` inside the repository.
  Expected last lines: `N/N step(s) hold the contract` and `xtask ci: all checks passed`.
- A failing, partial or unverifiable run is not green: report which check failed and stop without
  pushing.

## Land

```sh
git push local HEAD:main
```

Fast-forward only. Never `--force`, never `--mirror`, never a ref deletion, and never rewrite the
thread's history to make a push fit. On success `refs/remotes/local/main` equals `HEAD`.

## A diverged destination

When the intended result is clear, keep both sides: merge the destination into the thread branch
(`git merge --no-edit refs/remotes/local/main`), re-run the checks on the new `HEAD`, then push
again. When it is not clear — the same lines changed on both sides, or the destination's version
looks like the one to keep — stop and report the conflicting paths and what each side says; do not
guess, and never discard the destination's work.

## Verify what landed

1. `git ls-remote local refs/heads/main` equals `git rev-parse HEAD`.
2. In the destination, with `dest="$(dirname "$(git remote get-url local)")"`:

   ```sh
   git -C "$dest" log -1 --format=%H          # equals HEAD
   git -C "$dest" status --porcelain          # no modified tracked files
   ls "$dest/README.md" "$dest/course" >/dev/null
   ```

   If the branch moved but the files are not there, say so exactly: the branch landed and the
   checkout did not update.
3. Nothing is landed until both hold. If a step failed, report that the change has **not** landed and
   name the blocker.

## Report

The landed commit (short hash and subject), the destination branch and path, the check evidence (the
reused record with its commit hash, or a fresh run's wall time and final lines), and the destination's
state after the push. State only what was verified.
