#!/usr/bin/env python3
"""Rebuild a track's scaffold/solution patches from the trees they define.

The committed patches are *derived* artifacts: step N's scaffold is the diff between step N−1's
full tree and step N's stub tree, and its solution is the diff between the stub tree and the full
tree. The trees themselves are not committed — they are what the patch chain lays. That makes
"change the reference code of an early step" the most fragile edit in the course: every later
patch carries the earlier tree as context, and a hand-edited patch corrupts its hunk headers long
before anything complains (three measured incidents, handoff-m4-log 段 12).

This tool makes the derivation mechanical, so the fragile edit becomes:

    1. lay the chain with kiln itself (`prepare` + `solution` per step, no compilation anywhere);
    2. snapshot the stub and the full tree of every step;
    3. apply an optional fixes module to every snapshot;
    4. `rustfmt` every snapshot (a patch delivered unformatted fails the `fmt` gate first);
    5. regenerate every patch as `diff -u`, keeping each committed patch's own section labels and
       section order (`--- /dev/null` is what tells kiln a file is *created*; `--- a/path` reads as
       a modification of an existing file — losing the label breaks the chain two steps later);
    6. refuse to write a scaffold that removes lines (a scaffold may only create or add,
       authoring.md §5), then write the patches back.

`--check` stops after the comparison and exits non-zero on any drift: with no fixes, the
regenerated patches must be byte-identical to the committed ones, which is what keeps this tool
honest about its own invariants (run it before every use).

Fixes module (optional, `--fixes path.py`): a Python file defining

    def apply_fixes(tree: Path, step: str, stub: bool) -> None:

called once per snapshot, *before* formatting and diffing. `stub` is True for stub trees. Do the
edits with plain reads/writes; make them idempotent, because the tool may be re-run. A worked
example lives in the M4 log (handoff-m4-log.md 段 12): the nine review fixes over meshkv's
twenty-one step trees.

Usage:
    tools/rebuild-patches.py --track meshkv --check          # verify the committed patches
    tools/rebuild-patches.py --track meshkv --fixes f.py     # apply fixes, rewrite the patches
    tools/rebuild-patches.py --track meshkv --steps 5.4-snapshots,5.5-membership
"""

import argparse
import filecmp
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

KILN = os.environ.get("KILN_BIN", "kiln")


def fail(message: str) -> None:
    print(f"rebuild-patches: {message}", file=sys.stderr)
    sys.exit(2)


def run(*args: str, cwd: Path | None = None, check: bool = True) -> subprocess.CompletedProcess:
    result = subprocess.run(args, cwd=cwd, capture_output=True, text=True)
    if check and result.returncode != 0:
        fail(f"`{' '.join(args)}` failed ({result.returncode}):\n{result.stdout}\n{result.stderr}")
    return result


def steps_of(track_dir: Path) -> list[dict]:
    """The steps of a track, in order, with their scaffold and solution paths."""
    text = (track_dir / "track.toml").read_text()
    steps = []
    for chunk in text.split("\n[[step]]\n")[1:]:
        step = {
            "id": re.search(r'id = "([^"]+)"', chunk).group(1),
            "scaffold": None,
            "solution": None,
        }
        if match := re.search(r'scaffold = "([^"]+)"', chunk):
            step["scaffold"] = match.group(1)
        if match := re.search(r'solution = "([^"]+)"', chunk):
            step["solution"] = match.group(1)
        steps.append(step)
    if not steps:
        fail(f"{track_dir}/track.toml declares no steps")
    return steps


def section_labels(patch_text: str) -> list[tuple[str, str, str]]:
    """Each patch section's `(old label, new label, path)`, in the order it is written."""
    found = []
    lines = patch_text.split("\n")
    for index, line in enumerate(lines):
        if not line.startswith("--- ") or index + 1 >= len(lines):
            continue
        if not lines[index + 1].startswith("+++ "):
            continue
        old, new = line[4:].strip(), lines[index + 1][4:].strip()
        path = new if new != "/dev/null" else old
        found.append((old, new, path.removeprefix("a/").removeprefix("b/")))
    return found


GIT_HEADERS = ("index ", "new file mode", "old mode", "deleted file mode",
               "similarity index", "dissimilarity index", "rename from", "rename to",
               "old mode", "copy from", "copy to")


def normalize_git(text: str) -> str:
    """Drop the git extended headers a comparison cannot reproduce.

    `kiln`'s patch applier ignores file modes (t4-lakelet.md §6), so a laid tree holds every file
    as 100644 while the committed patch may record `new file mode 100755`; and the abbreviated
    blob hashes depend on the object database that produced them. Neither is part of what this
    check is about — the hunks, the labels and the section order are.
    """
    kept = [line for line in text.splitlines() if not line.startswith(GIT_HEADERS)]
    return "".join(line + "\n" for line in kept)


class Differ:
    """Produces one patch's sections in the format the committed patch was written in.

    Two formats live in this repository: the older tracks were authored with `git diff` (extended
    headers, and a differ whose hunk boundaries can differ from GNU diff's when a blank line
    borders an insertion), and meshkv was authored with plain `diff -u`. Regenerating with the
    other one produces hunks that differ *only* in where an ambiguous blank line went — the
    classic false alarm — so the format is read off the committed patch itself.
    """

    def __init__(self, committed: str, from_tree: Path, to_tree: Path, work: Path):
        self.git = "diff --git " in committed
        self.from_tree, self.to_tree, self.work = from_tree, to_tree, work
        self.repo = None
        if self.git:
            self.repo = self._git_repo()

    def _git_repo(self) -> tuple[str, str]:
        repo = self.work / "gitdiff"
        if repo.exists():
            shutil.rmtree(repo)
        repo.mkdir(parents=True)
        run("git", "init", "-q", cwd=repo)
        config = ["-c", "user.email=tool@local", "-c", "user.name=tool"]
        for index, tree in enumerate((self.from_tree, self.to_tree)):
            if index:
                # Overlay is not enough: a file the stub has and the full tree lacks (a `.sqlx`
                # entry the solution deletes) would survive the copy, and the deletion would never
                # reach the diff. The second commit must be the full tree *exactly*.
                for entry in repo.iterdir():
                    if entry.name != ".git":
                        shutil.rmtree(entry) if entry.is_dir() else entry.unlink()
            for path in tree.rglob("*"):
                if path.is_file():
                    target = repo / path.relative_to(tree)
                    target.parent.mkdir(parents=True, exist_ok=True)
                    shutil.copy2(path, target)
            # -f: a laid tree may carry a `.gitignore` of its own (mailer's ignores `.sqlx`), and a
            # file git skips here is a file the diff then cannot see — measured as a phantom
            # drift on mailer/1.12.
            run("git", *config, "add", "-A", "-f", cwd=repo)
            run("git", *config, "commit", "-qm", "tree", cwd=repo)
        before = run("git", "rev-parse", "HEAD~1", cwd=repo).stdout.strip()
        after = run("git", "rev-parse", "HEAD", cwd=repo).stdout.strip()
        return before, after

    def section(self, old: str, new: str, path: str) -> str:
        left, right = self.from_tree / path, self.to_tree / path
        if left.is_file() and right.is_file() and filecmp.cmp(left, right, shallow=False):
            return ""
        if self.git:
            out = run("git", "diff", self.repo[0], self.repo[1], "--", path,
                      cwd=self.work / "gitdiff").stdout
            # git writes `diff --git a/<p> b/<p>` for one-sided files and keeps the real path on
            # both sides; the committed label (`/dev/null`) belongs where git put the a-side.
            left = old if old != "/dev/null" else f"a/{path}"
            right = new if new != "/dev/null" else f"b/{path}"
            lines = out.splitlines()
            if lines and lines[0].startswith("diff --git "):
                lines[0] = f"diff --git {left} {right}"
            return normalize_git("\n".join(lines) + "\n")
        result = subprocess.run(
            ["diff", "-u", "--label", old, "--label", new,
             str(left) if left.is_file() else "/dev/null",
             str(right) if right.is_file() else "/dev/null"],
            capture_output=True, text=True,
        )
        return result.stdout

    def close(self) -> None:
        if self.repo is not None:
            shutil.rmtree(self.work / "gitdiff")


def regenerate(committed: str, from_tree: Path, to_tree: Path, work: Path) -> str:
    differ = Differ(committed, from_tree, to_tree, work)
    try:
        return "".join(
            differ.section(old, new, path)
            for old, new, path in section_labels(committed)
        )
    finally:
        differ.close()


def semantics(patch_text: str) -> list[tuple]:
    """What a patch *does*, as `(old label, new label, added lines, removed lines)` per section.

    The comparison is semantic on purpose. Two provably-equivalent patches of the same pair can
    differ in hunk boundaries and context width — the older tracks were authored with context
    settings that varied — and a byte comparison would report that as drift while the patch, the
    trees and the learner's experience are all identical. What cannot vary is the order of the
    sections and, within one, the sequence of lines the patch adds and removes.
    """
    sections: list[tuple] = []
    lines = patch_text.split("\n")
    index = 0
    while index < len(lines):
        if lines[index].startswith("--- ") and index + 1 < len(lines) \
                and lines[index + 1].startswith("+++ "):
            old, new = lines[index][4:].strip(), lines[index + 1][4:].strip()
            added: list[str] = []
            removed: list[str] = []
            for line in lines[index + 2:]:
                if line.startswith("@@"):
                    continue
                if line.startswith(("diff --git", "--- ")) or not line:
                    break
                if line.startswith("+"):
                    added.append(line[1:])
                elif line.startswith("-"):
                    removed.append(line[1:])
            sections.append((old, new, tuple(added), tuple(removed)))
            index += 2
        else:
            index += 1
    return sections


def rustfmt_tree(tree: Path, config: Path) -> None:
    sources = sorted(
        path for pattern in ("src/**/*.rs", "tests/**/*.rs")
        for path in tree.glob(pattern) if path.is_file()
    )
    if sources:
        run("rustfmt", "--edition", "2024", "--config-path", str(config), *map(str, sources))


def copy_tree(source: Path, target: Path) -> None:
    shutil.copytree(source, target, ignore=shutil.ignore_patterns("target", ".kiln", "Cargo.lock"))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--course", default=Path(__file__).resolve().parent.parent,
                        help="repository root (default: the checkout this tool lives in)")
    parser.add_argument("--track", required=True, help="track id, e.g. meshkv")
    parser.add_argument("--steps", help="comma-separated step ids; default: every authored step")
    parser.add_argument("--fixes", help="Python module with apply_fixes(tree, step, stub)")
    parser.add_argument("--check", action="store_true",
                        help="verify the committed patches and exit; write nothing")
    parser.add_argument("--work", help="scratch directory (default: a fresh one under /private/tmp)")
    args = parser.parse_args()

    course = Path(args.course).resolve()
    track_dir = course / "course" / "tracks" / args.track
    if not track_dir.is_dir():
        fail(f"no track at {track_dir}")
    steps = steps_of(track_dir)
    if args.steps:
        wanted = {step.strip() for step in args.steps.split(",")}
        steps = [step for step in steps if step["id"] in wanted]
        missing = wanted - {step["id"] for step in steps}
        if missing:
            fail(f"unknown step(s): {', '.join(sorted(missing))}")
    fixes = None
    if args.fixes:
        import importlib.util
        spec = importlib.util.spec_from_file_location("fixes", args.fixes)
        fixes = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(fixes)
        if not hasattr(fixes, "apply_fixes"):
            fail(f"{args.fixes} defines no apply_fixes(tree, step, stub)")

    # The scratch is a repo-root look-alike: `course/` inside it, and kiln runs with it as its
    # working directory, so the laid projects land in `<scratch>/app/<track>`.
    work = Path(args.work) if args.work else Path(tempfile.mkdtemp(prefix=f"rebuild-{args.track}-"))
    work.mkdir(parents=True, exist_ok=True)
    scratch = work / "course"
    if scratch.exists():
        fail(f"{scratch} already exists — pass a fresh --work")
    shutil.copytree(course / "course", scratch, ignore=shutil.ignore_patterns(".kiln"))
    for name in ("rust-toolchain.toml", "rustfmt.toml"):
        if (course / name).is_file():
            shutil.copy2(course / name, work / name)
    print(f"scratch course: {scratch}")

    kiln = shutil.which(KILN) or fail(f"no `{KILN}` on PATH — set KILN_BIN")
    trees = work / "trees"

    # 0. Lay the `requires_track` chain first (plan §3 D6): meshkv's project does not build without
    # jobq's tree beside it, and `prepare` refuses without it — correctly, so the tool obeys.
    required = (track_dir / "track.toml").read_text()
    if match := re.search(r'requires_track = "([^"]+)"', required):
        for prerequisite in steps_of(course / "course" / "tracks" / match.group(1)):
            run(kiln, "prepare", f"{match.group(1)}/{prerequisite['id']}", cwd=scratch)
            run(kiln, "solution", f"{match.group(1)}/{prerequisite['id']}", cwd=scratch)
        print(f"laid the {match.group(1)} prerequisite chain")

    # 1+2. Lay the chain; snapshot the stub and the full tree of every step.
    previous_full = trees / "base"
    previous_full.mkdir(parents=True)
    snapshots: dict[str, tuple[Path, Path]] = {}
    for step in steps:
        step_id = f"{args.track}/{step['id']}"
        run(kiln, "prepare", step_id, cwd=scratch)
        stub = trees / f"{step['id']}-stub"
        copy_tree(work / "app" / args.track, stub)
        run(kiln, "solution", step_id, cwd=scratch)
        full = trees / f"{step['id']}-full"
        copy_tree(work / "app" / args.track, full)
        snapshots[step["id"]] = (stub, full)
        print(f"laid {step['id']}: stub + full")

    # 3. The fixes, before anything is formatted or diffed.
    if fixes is not None:
        for step in steps:
            stub, full = snapshots[step["id"]]
            fixes.apply_fixes(stub, step["id"], stub=True)
            fixes.apply_fixes(full, step["id"], stub=False)
        print("fixes applied")

    # 4. Formatting, before any diff (a delivered tree that fails `fmt` fails its first gate).
    for stub, full in snapshots.values():
        rustfmt_tree(stub, course / "rustfmt.toml")
        rustfmt_tree(full, course / "rustfmt.toml")

    # 5+6. Regenerate, check, and either compare or write back.
    drifted = []
    for step in steps:
        stub, full = snapshots[step["id"]]
        for kind, key, from_tree, to_tree in (
            ("scaffold", "scaffold", previous_full, stub),
            ("solutions", "solution", stub, full),
        ):
            if step[key] in (None, "skeleton"):
                continue
            declared = step["scaffold"] if kind == "scaffold" else step["solution"]
            relative = f"course/tracks/{args.track}/{declared}"
            committed = (course / relative).read_text()
            rebuilt = regenerate(committed, from_tree, to_tree, work)
            if kind == "scaffold" and re.search(r"^-(?!--)", rebuilt, re.MULTILINE):
                fail(f"{relative}: the rebuilt scaffold removes lines, which a scaffold may not do "
                     "(authoring.md §5) — the fix belongs in the solution of the step that owns them")
            if semantics(rebuilt) == semantics(committed):
                print(f"identical  {relative}")
            elif args.check:
                drifted.append(relative)
            else:
                (course / relative).write_text(rebuilt)
                print(f"rewrote    {relative}")
        previous_full = full

    if args.check:
        if drifted:
            fail(f"{len(drifted)} patch(es) drift from the trees they define:\n  " +
                 "\n  ".join(drifted))
        print(f"all committed patches of {args.track} match the trees they define")


if __name__ == "__main__":
    main()
