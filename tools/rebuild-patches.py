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

`--check` stops after the comparison and exits non-zero on any drift. The comparison is *semantic*:
per file, the two labels and the exact sequence of lines the patch adds and removes. Hunk headers,
hunk boundaries and context width are the differ's freedom, not content — two provably equivalent
patches of the same pair can differ in all three while giving the learner the same tree. (The
tool's own `--self-test` pins all of this; run it before trusting a change here.)

What the check **cannot** see: a patch that was wrong when it was committed. The trees are laid *by*
the patches, so a section that was dropped at authoring time is missing from the trees too, and the
only step where the omission shows is the *next* step's scaffold (the file appears there as
something the previous step should have written). A track's last step therefore has no such witness
— the omission is caught by that step's spec, or by nobody.

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

    The bytes a regeneration produces therefore depend on the local `git` and `diff` (BSD and GNU
    disagree about hunk boundaries around blank lines). The *check* does not: it compares content,
    so it is the same answer on every machine. The *write* path may shift a hunk boundary on a
    different platform — harmless, and the next `--check` is happy with it.
    """

    def __init__(self, committed: str, from_tree: Path, to_tree: Path, work: Path):
        self.git = "diff --git " in committed
        self.from_tree, self.to_tree, self.work = from_tree, to_tree, work
        self.repo = None
        # The committed text of every section, kept for the files the trees do not carry.
        self.keep: dict[str, str] = {}
        for index, line in enumerate(committed.split("\n")):
            if line.startswith("--- ") and index + 1 < len(committed.split("\n")) \
                    and committed.split("\n")[index + 1].startswith("+++ "):
                old_label = line[4:].strip()
                new_label = committed.split("\n")[index + 1][4:].strip()
                path = (new_label if new_label != "/dev/null" else old_label)
                path = path.removeprefix("a/").removeprefix("b/")
                start = committed.index(line)
                end = committed.find("\ndiff --git ", start)
                self.keep[path] = committed[start:end if end != -1 else len(committed)]
        if self.git:
            self.repo = self._git_repo()

    def _git_repo(self) -> tuple[str, str]:
        repo = self.work / "gitdiff"
        if repo.exists():
            shutil.rmtree(repo)
        repo.mkdir(parents=True)
        run("git", "init", "-q", cwd=repo)
        config = [
            "-c", "user.email=tool@local",
            "-c", "user.name=tool",
            # The user's own configuration must not decide what this tool writes: a colour, an
            # external diff driver or a prefix setting would all show up as drift.
            "-c", "color.ui=false",
            "-c", "diff.noprefix=false",
            "-c", "diff.external=",
            "-c", "core.quotepath=false",
        ]
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
        if not left.is_file() and not right.is_file():
            # A file neither tree holds: the committed section describes it, and dropping the
            # section would silently delete it from the patch (measured on mailer/1.21's
            # `Cargo.lock`, which a build generates and the laid trees do not carry). Keep it.
            return self.keep.get(path, "")
        if left.is_file() and right.is_file() and filecmp.cmp(left, right, shallow=False):
            return ""
        if self.git:
            out = run("git", "--no-pager", "diff", "--no-color", "--no-ext-diff",
                      self.repo[0], self.repo[1], "--", path,
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


def tree_files(tree: Path) -> set[str]:
    """Every file in a tree, relative to it."""
    return {
        str(path.relative_to(tree))
        for path in tree.rglob("*")
        if path.is_file()
    }


def course_specs(track_dir: Path) -> set[str]:
    """The project-relative paths of the course's own spec files.

    `kiln prepare` copies each step's spec into the project as `tests/spec_<name>.rs`; those files
    are course content, not the patch's business. A patch that "covered" them would be wrong, and
    a check that expected them would report every scaffold as drifting.
    """
    text = (track_dir / "track.toml").read_text()
    specs: set[str] = set()
    for match in re.finditer(r'spec = \[([^\]]*)\]', text):
        for entry in re.findall(r'"([^"]+)"', match.group(1)):
            name = entry.rsplit("/", 1)[-1]
            if name.endswith(".rs"):
                name = name[:-3]
            specs.add(f"tests/spec_{name}.rs")
    return specs


def changed_files(from_tree: Path, to_tree: Path) -> list[str]:
    """The files that differ between two trees: added, removed, or edited.

    This is the list a patch *has* to cover. Without it the check only ever regenerates the
    sections the committed patch already lists, so a patch that silently omits a file's changes
    passes — the exact defect the tool exists to catch (measured: deleting one section from
    mailer/1.22's solution left `--check` green).
    """
    before, after = tree_files(from_tree), tree_files(to_tree)
    changed = []
    for path in sorted(before | after):
        left, right = from_tree / path, to_tree / path
        if not left.is_file() or not right.is_file():
            changed.append(path)
        elif not filecmp.cmp(left, right, shallow=False):
            changed.append(path)
    return changed


def regenerate(
    committed: str,
    from_tree: Path,
    to_tree: Path,
    work: Path,
    ignore: set[str] | None = None,
) -> str:
    """The patch that takes `from_tree` to `to_tree`, in the committed patch's own format.

    The sections are the committed ones *plus* every file the two trees disagree about: a file the
    trees changed and the patch does not mention is a file the patch would drop, and the check has
    to see it. A committed section for a file the trees do not differ about is kept as it is — the
    trees may simply not carry that file (a generated one), and dropping the section would lose it.
    """
    differ = Differ(committed, from_tree, to_tree, work)
    try:
        text = ""
        covered = {path for _, _, path in section_labels(committed)}
        for old, new, path in section_labels(committed):
            text += differ.section(old, new, path)
        for path in changed_files(from_tree, to_tree):
            if path in covered or (ignore and path in ignore):
                continue
            left = "a/" + path if (from_tree / path).is_file() else "/dev/null"
            right = "b/" + path if (to_tree / path).is_file() else "/dev/null"
            text += differ.section(left, right, path)
        return text
    finally:
        differ.close()


def semantics(patch_text: str) -> dict[str, tuple]:
    """What a patch *does*, per file: `(old label, new label, added lines, removed lines)`.

    The comparison is semantic on purpose. Two provably-equivalent patches of the same pair can
    differ in hunk boundaries, context width and the order of their sections — the older tracks
    were authored with context settings that varied, and the applier does not care about order —
    so a byte comparison would report that as drift while the patch, the trees and the learner's
    experience are all identical.

    What *is* compared, per file: the two labels, and the exact sequence of lines the patch adds
    and removes. The hunk headers are not: the applier anchors on content when the numbers do not
    fit, and a hunk count that is wrong is rejected loudly by the applier itself.
    """
    sections: dict[str, tuple] = {}
    lines = patch_text.split("\n")
    index = 0
    while index < len(lines):
        if lines[index].startswith("--- ") and index + 1 < len(lines) \
                and lines[index + 1].startswith("+++ "):
            old, new = lines[index][4:].strip(), lines[index + 1][4:].strip()
            path = (new if new != "/dev/null" else old).removeprefix("a/").removeprefix("b/")
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
            sections[path] = (old, new, tuple(added), tuple(removed))
            index += 2
        else:
            index += 1
    return sections


def rustfmt_tree(tree: Path, config: Path) -> None:
    """Format every Rust file the tree holds.

    Not only `src/` and `tests/`: a track's tree also carries `xtask/src/main.rs`, and the `fmt`
    gate runs `cargo fmt --all --check` over the whole project, so a scaffold that creates an
    unformatted file outside `src/` fails its first gate.
    """
    sources = sorted(path for path in tree.rglob("*.rs") if path.is_file())
    if sources:
        run("rustfmt", "--edition", "2024", "--config-path", str(config), *map(str, sources))


def copy_tree(source: Path, target: Path) -> None:
    shutil.copytree(source, target, ignore=shutil.ignore_patterns("target", ".kiln", "Cargo.lock"))


def self_test() -> int:
    """The tool's own checks, on synthetic trees.

    A maintenance tool that nobody can break on purpose is a maintenance tool whose guarantees are
    beliefs. These four cases are the guarantees this tool claims:

    1. a patch that omits a file the trees changed is *not* equal to what the trees produce;
    2. a patch whose section names a file neither tree holds keeps that section;
    3. a difference in hunk boundaries or context width is not a difference in content;
    4. a difference in the lines a patch adds or removes is a difference.
    """
    root = Path(tempfile.mkdtemp(prefix="rebuild-selftest-"))
    try:
        before, after = root / "before", root / "after"
        for tree in (before, after):
            (tree / "src").mkdir(parents=True)
        (before / "src" / "a.rs").write_text("fn a() {}\n")
        (after / "src" / "a.rs").write_text("fn a() {}\n")
        (after / "src" / "b.rs").write_text("fn b() {}\n")
        work = root / "work"
        work.mkdir()

        committed = (
            "--- /dev/null\n"
            "+++ b/src/b.rs\n"
            "@@ -0,0 +1 @@\n"
            "+fn b() {}\n"
        )
        # 1. The patch above does not mention `src/a.rs`, which the two trees also agree on, so it
        #    is complete. Now let the trees disagree about a file the patch does not mention.
        (after / "src" / "a.rs").write_text("fn a() { let _ = 1; }\n")
        rebuilt = regenerate(committed, before, after, work)
        if semantics(rebuilt) == semantics(committed):
            print("self-test FAILED: a file the trees changed and the patch omits was not detected")
            return 1
        print("✓ a patch that omits a changed file is detected")

        # 2. A section for a file neither tree holds keeps its text.
        ghost = (
            "--- /dev/null\n"
            "+++ b/Cargo.lock\n"
            "@@ -0,0 +1 @@\n"
            "+# generated\n"
        )
        kept = regenerate(ghost, before, after, work)
        if "Cargo.lock" not in kept:
            print("self-test FAILED: the section for a file neither tree holds was dropped")
            return 1
        print("✓ a section for a file the trees do not carry is kept")

        # 3+4. Hunk boundaries and context width are not content; the added lines are.
        narrow = "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-fn a() {}\n+fn a() { let _ = 1; }\n"
        # The same change, written with more context around it and a wider hunk header.
        wide = (
            "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,5 +1,5 @@\n"
            " fn before() {}\n"
            "-fn a() {}\n"
            "+fn a() { let _ = 1; }\n"
            " fn after() {}\n"
        )
        if semantics(narrow) != semantics(wide):
            print("self-test FAILED: a context-width difference was read as a content difference")
            return 1
        changed = "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-fn a() {}\n+fn a() { let _ = 2; }\n"
        if semantics(narrow) == semantics(changed):
            print("self-test FAILED: a changed line was not read as a content difference")
            return 1
        print("✓ hunk boundaries are not content; the added lines are")
        print("self-test: all checks passed")
        return 0
    finally:
        shutil.rmtree(root, ignore_errors=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--course", default=Path(__file__).resolve().parent.parent,
                        help="repository root (default: the checkout this tool lives in)")
    parser.add_argument("--track", help="track id, e.g. meshkv (not needed with --self-test)")
    parser.add_argument("--steps", help="comma-separated step ids; default: every authored step")
    parser.add_argument("--fixes", help="Python module with apply_fixes(tree, step, stub)")
    parser.add_argument("--check", action="store_true",
                        help="verify the committed patches and exit; write nothing")
    parser.add_argument("--work", help="scratch directory (default: a fresh one under /private/tmp)")
    parser.add_argument("--keep", action="store_true",
                        help="keep the scratch directory (it is removed otherwise)")
    parser.add_argument("--self-test", action="store_true",
                        help="run the tool's own checks on synthetic trees and exit")
    args = parser.parse_args()

    if args.self_test:
        raise SystemExit(self_test())

    if not args.track:
        fail("no --track given (or use --self-test)")
    course = Path(args.course).resolve()
    track_dir = course / "course" / "tracks" / args.track
    if not track_dir.is_dir():
        fail(f"no track at {track_dir}")
    steps = steps_of(track_dir)
    # `--steps` selects what is *written*, not what is laid: a step's scaffold is a diff against
    # the previous step's full tree, so the steps before it have to be laid either way. (The first
    # version filtered the chain itself and could not produce a correct scaffold at all.)
    selected = {step["id"] for step in steps}
    if args.steps:
        wanted = {step.strip() for step in args.steps.split(",")}
        unknown = wanted - selected
        if unknown:
            fail(f"unknown step(s): {', '.join(sorted(unknown))}")
        selected = wanted
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
    temporary = args.work is None
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
    specs = course_specs(track_dir)
    for step in steps:
        stub, full = snapshots[step["id"]]
        for kind, key, from_tree, to_tree in (
            ("scaffold", "scaffold", previous_full, stub),
            ("solutions", "solution", stub, full),
        ):
            declared = step["scaffold"] if kind == "scaffold" else step["solution"]
            if declared in (None, "skeleton") or step["id"] not in selected:
                continue
            relative = f"course/tracks/{args.track}/{declared}"
            committed = (course / relative).read_text()
            rebuilt = regenerate(committed, from_tree, to_tree, work, specs)
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
    if temporary and not args.keep:
        # The scratch is a copy of the course and a tree per step: it is worth keeping while a
        # run is being looked at, and worth nothing afterwards. A --work directory the caller
        # named is theirs and is left alone.
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
