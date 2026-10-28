"""Historical record — the fixes module that made T6-T9's stub bodies consume their inputs.

Already baked into the committed patches (verified by `rebuild-patches.py --check` for all four
tracks); kept as the worked `--fixes` example for the next track, alongside the M4 one in
handoff-m4-log.md 段 12. Re-running it with `--track cache|broker|rpc|platform` must be a no-op.

Make every stub body consume its inputs, so a learner's starting tree builds warning-free.

House style (relay's skeleton, handoff-m1): a stub names what a real body will use —

    let _ = (pool, key);
    todo!("step relay/2.1-project-skeleton")

T6-T9's scaffolds forgot it, so every novice's first `cargo build` printed unused-variable and
dead-field warnings. This module inserts the line in stub trees and removes it from full trees (the
solution replaces the stub body, so the line must not survive there). It runs on *every* snapshot,
because rebuild-patches lays the chain before the fixes: a line left in step N's full tree would
otherwise reach step N+1's scaffold and never be removed by anyone.
"""

from pathlib import Path

import re

# (file, function) -> struct fields the function reads. The parameters come from the signature.
FIELDS = {
    ("src/lib.rs", "get"): ["ttl"],
    ("src/lib.rs", "append"): ["next_unkeyed"],
    ("src/lib.rs", "wire"): [],
    ("src/lib.rs", "audit"): [],
    ("src/invalidation.rs", "write"): [],
    ("src/single_flight.rs", "begin"): [],
    ("src/expiry.rs", "ttl_for"): [],
    ("src/versioned.rs", "get"): ["ttl"],
    ("src/two_level.rs", "get"): ["local", "shared", "shared_ttl"],
    ("src/effectiveness.rs", "ratio"): [],
    ("src/effectiveness.rs", "verdict"): [],
    ("src/breaker.rs", "route"): ["threshold", "cooldown", "opened_at", "probe_handed_out"],
    ("src/breaker.rs", "record"): [],
    ("src/consumer.rs", "commit"): [],
    ("src/assignment.rs", "assign"): [],
    ("src/lag.rs", "lag"): [],
    ("src/replay.rs", "next_record"): ["topic", "partition"],
    ("src/ordering.rs", "plan"): [],
    ("src/exactly_once.rs", "process"): [],
    ("src/schema.rs", "compat"): [],
    ("src/alert.rs", "evaluate"): [],
    ("src/deadline.rs", "call"): [],
    ("src/status.rs", "code_for"): [],
    ("src/status.rs", "retryable"): [],
    ("src/stream.rs", "send"): ["items", "window"],
    ("src/multiplex.rs", "pump"): [],
    ("src/rollout.rs", "may_terminate"): [],
    ("src/budget.rs", "allowed"): [],
    ("src/secrets.rs", "resolve"): [],
    ("src/secrets.rs", "leaks"): [],
    ("src/oidc.rs", "validate"): [],
    ("src/autoscaling.rs", "desired"): [],
    ("src/autoscaling.rs", "observe"): ["current", "down_after", "low_windows"],
    ("src/drill.rs", "drill"): [],
}

FN = re.compile(r"(?:pub(?:\([^)]*\))?\s+)?(?:const\s+)?(?:async\s+)?fn\s+(\w+)")


def split_top(text: str) -> list[str]:
    """Split on commas that are not inside `<>`, `()`, `[]` or a string."""
    parts, depth, current, in_string = [], 0, "", False
    for ch in text:
        if in_string:
            current += ch
            if ch == '"':
                in_string = False
            continue
        if ch == '"':
            in_string = True
        elif ch in "<([":
            depth += 1
        elif ch in ">)]":
            depth -= 1
        elif ch == "," and depth == 0:
            parts.append(current)
            current = ""
            continue
        current += ch
    if current.strip():
        parts.append(current)
    return parts


def params_of(text: str, open_paren: int) -> tuple[list[str], int]:
    """Parameter names of the signature whose `(` is at `open_paren`."""
    depth, i = 0, open_paren
    while i < len(text):
        if text[i] == "(":
            depth += 1
        elif text[i] == ")":
            depth -= 1
            if depth == 0:
                break
        i += 1
    names = []
    for part in split_top(text[open_paren + 1 : i]):
        head = part.split(":", 1)[0].strip()
        if head in ("self", "&self", "&mut self", "mut self"):
            continue
        head = head.removeprefix("mut ").strip()
        if not re.fullmatch(r"[A-Za-z_]\w*", head) or head.startswith("_"):
            continue
        names.append(head)
    return names, i + 1


def body_of(text: str, after_paren: int) -> tuple[int, int] | None:
    """Span of the function body that starts after `after_paren`."""
    i = after_paren
    while i < len(text) and text[i] not in "{;":
        i += 1
    if i >= len(text) or text[i] == ";":
        return None
    depth, start = 0, i
    while i < len(text):
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                return start, i + 1
        i += 1
    return None


def consume_parts(names: list[str], fields: list[str]) -> list[str]:
    return [f"&self.{field}" for field in fields] + names


def apply_fixes(tree: Path, step: str, stub: bool) -> None:
    by_file: dict[str, list[tuple[str, list[str]]]] = {}
    for (rel, fn), fields in FIELDS.items():
        by_file.setdefault(rel, []).append((fn, fields))

    for rel, sites in by_file.items():
        path = tree / rel
        if not path.exists():
            continue
        text = path.read_text()
        for fn, fields in sites:
            for match in FN.finditer(text):
                if match.group(1) != fn:
                    continue
                open_paren = text.index("(", match.end())
                names, after = params_of(text, open_paren)
                span = body_of(text, after)
                if span is None:
                    continue
                start, end = span
                body = text[start:end]
                parts = consume_parts(names, fields)
                if not parts:
                    break
                unindented = "let _ = (" + ", ".join(parts) + ");"
                if len(parts) == 1:
                    unindented = f"let _ = {parts[0]};"
                lines = body.splitlines()
                if stub and body[1:].strip().startswith("todo!(") and unindented not in body:
                    indent = re.search(r"\n([ \t]*)\S", body).group(1)
                    lines.insert(1, indent + unindented)
                    text = text[:start] + "\n".join(lines) + text[end:]
                elif not stub and any(line.strip() == unindented for line in lines):
                    lines = [line for line in lines if line.strip() != unindented]
                    text = text[:start] + "\n".join(lines) + text[end:]
                break
        path.write_text(text)

    # The rpc skeleton's `BTreeMap` import belongs to `wire`'s solution: the stub does not use it,
    # so only the full trees carry it (the learner adds the import when the body needs it).
    if step.startswith("8.") and (step != "8.1-field-numbers" or not stub):
        lib = tree / "src/lib.rs"
        if lib.exists():
            text = lib.read_text()
            if "use std::collections::BTreeMap;" not in text:
                text = text.replace(
                    "// kiln:scaffold:end\n",
                    "// kiln:scaffold:end\n\nuse std::collections::BTreeMap;\n",
                    1,
                )
                lib.write_text(text)
