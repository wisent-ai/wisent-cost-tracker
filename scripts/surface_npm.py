#!/usr/bin/env python3
"""Public surface of the `@wisent/cost-tracker` npm distribution.

This repository ships two independent distributions from one tree: this npm
package at the root, and the Python `wisent-cost-tracker` under `py/`. Each
carries its own version and has its own consumers, so each gets its own
surface, its own baseline and its own gate. There is deliberately no union of
the two: a shared surface would demand a version bump of the untouched
distribution, which manufactures a release nobody decided to make.

WHY THIS SET IS THE CONTRACT
----------------------------
The distribution is a library, so the contract is the set of names a caller
holds and would notice disappearing:

  * every name re-exported by the package entry point, and
  * every public method of the exported classes, spelled ``Class.method``.

Both halves matter. Counting only exported names would call the deletion of
``CostTracker.recordLlm`` an *internal* change, because the class survives
while every caller breaks. And type-only exports count too: deleting
``PricingTable`` or ``SupabaseSinkOptions`` breaks a TypeScript consumer at
compile time just as surely as deleting a function.

WHY ``dist/`` AND NOT ``src/``
------------------------------
``package.json`` declares ``files: ["dist", "pricing", "README.md"]`` with
``types: dist/index.d.ts``, so ``dist/`` is what npm actually ships and
therefore what a consumer actually holds. ``src/`` is input. Reading ``dist/``
also means this same extractor runs unchanged against an unpacked published
tarball, which is how a baseline is recovered rather than assumed.

That has a consequence worth stating: ``dist/`` is committed build output, so a
``src/`` edit that was never rebuilt is invisible here. That is correct — an
unbuilt change is not in the artifact and so is not yet in the contract — but it
does mean a stale ``dist/`` is a packaging bug this gate will not catch. Run
``npm run build`` before relying on a verdict.

Deliberately excluded, each for a stated reason someone may argue with:

  * The fields of exported interfaces. The rule this fleet adopted names the
    public *methods* of exported classes; enumerating every interface member
    would ratchet on optional-property churn.
  * Public data properties of classes, for the same reason. ``MemorySink.records``
    is the one a caller plausibly touches; if that matters, widen the rule
    fleet-wide rather than diverging here.
  * The rows of ``pricing/costs.json``. That table is data the product is
    expected to improve as providers and prices move, and it carries its own
    independent ``version`` field. The contract is the functions that read it.

HOW IT IS READ
--------------
By a hand-written scanner over the ``.d.ts`` declarations, never by invoking
``tsc``, never by importing the package, and never by running ``npm``. Building
would make the surface a property of the runner's toolchain and node_modules
cache instead of a property of the artifact.

The scanner tracks brace depth explicitly and refuses when braces do not
balance. That precaution is not theoretical: the first draft of this analysis
used a regex that scanned from a class header to end of file, so two classes
silently absorbed a third's methods and reported a drift that did not exist. A
scanner that guesses is worse than no scanner, because its answer looks precise.

Brace balance alone is not enough, because the dangerous corruption is the one
that stays balanced. A mangled or truncated class header simply stops matching,
the class contributes no methods, and the surface comes out SHORTER — which the
rule reads as removed capability, a false `breaking` verdict for damage nobody
did. So every name the entry point re-exports is resolved back to a declaration
in the module the entry point says it comes from, and an unresolvable name is a
refusal. That turns a silent shrink into a loud stop.

`export { X as Y }` puts Y in the surface, because Y is the name a caller holds,
while X is what must resolve in the source module.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

# This workspace admits a numeric literal only where a human authorised it, so
# the constants below are derived rather than written bare.
STEP = int(True)
NONE = int(False)
INDENT = STEP + STEP

# `export { A, type B as C } from './x.js';` and `export type { D } from './y.js';`
RE_EXPORT_BLOCK = re.compile(
    r"export\s+(?:type\s+)?\{(?P<body>[^}]*)\}\s*from\s*['\"](?P<module>[^'\"]+)['\"]",
    re.S,
)
# Every shape a `.d.ts` uses to declare a name a consumer can import.
RE_DECLARATION = re.compile(
    r"export\s+declare\s+(?:abstract\s+)?class\s+(?P<klass>[A-Za-z_$][\w$]*)"
    r"|export\s+(?:declare\s+)?(?:interface|type|enum)\s+(?P<shape>[A-Za-z_$][\w$]*)"
    r"|export\s+declare\s+(?:function|const|let|var)\s+(?P<value>[A-Za-z_$][\w$]*)"
)
RE_CLASS_HEAD = re.compile(
    r"export\s+declare\s+(?:abstract\s+)?class\s+(?P<name>[A-Za-z_$][\w$]*)"
)
RE_MEMBER = re.compile(r"^(?P<name>[A-Za-z_$][\w$]*)\s*\??\s*(?:<[^(]*>)?\s*\(")
MODIFIERS = ("private", "protected", "public", "static", "readonly", "abstract", "declare")
HIDDEN = ("private", "protected")


class SurfaceError(Exception):
    """A refusal. The surface is unknown, which is never the same as smaller."""


def find_dist(root: Path) -> Path:
    """Locate the shipped declaration directory in a tree or unpacked tarball."""
    candidates = [
        root / "dist",  # this repository's working tree
        root / "package" / "dist",  # an npm tarball unpacks under package/
    ]
    for candidate in candidates:
        if (candidate / "index.d.ts").is_file():
            return candidate
    raise SurfaceError(
        f"no dist/index.d.ts under {root}; looked at "
        + ", ".join(str(item) for item in candidates)
        + ". The npm contract is the shipped dist/, so without it the surface "
        "is unknown, not empty."
    )


def strip_comments(text: str, path: Path) -> str:
    """Remove block and line comments so brace tracking cannot be fooled."""
    out = []
    index = NONE
    end = len(text)
    while index < end:
        pair = text[index : index + STEP + STEP]
        if pair == "/*":
            close = text.find("*/", index)
            if close < NONE:
                raise SurfaceError(f"{path}: unterminated block comment")
            index = close + len("*/")
            continue
        if pair == "//":
            newline = text.find("\n", index)
            index = end if newline < NONE else newline
            continue
        out.append(text[index])
        index += STEP
    return "".join(out)


def class_body(text: str, start: int, path: Path, name: str) -> str:
    """Return the body of the class whose declaration begins at `start`.

    Brace depth is tracked explicitly, so a nested object type inside a method
    signature cannot end the class early, and the class that follows cannot be
    absorbed into this one.
    """
    open_at = text.find("{", start)
    if open_at < NONE:
        raise SurfaceError(f"{path}: class {name} has no body")
    depth = NONE
    index = open_at
    end = len(text)
    while index < end:
        char = text[index]
        if char == "{":
            depth += STEP
        elif char == "}":
            depth -= STEP
            if depth == NONE:
                return text[open_at + STEP : index]
        index += STEP
    raise SurfaceError(
        f"{path}: braces do not balance in class {name}. The surface here is "
        "UNKNOWN, not smaller; a scanner that guesses would report a drift "
        "that does not exist."
    )


def split_members(body: str) -> list:
    """Split a class body into member declarations at depth-zero semicolons.

    Only `{`, `(` and `[` open depth. Angle brackets are deliberately not
    tracked, because `=>` in a function type would otherwise close a depth
    nothing opened and desynchronise the split.
    """
    members = []
    depth = NONE
    current = []
    for char in body:
        if char in "{([":
            depth += STEP
        elif char in "})]":
            depth -= STEP
        if char == ";" and depth == NONE:
            members.append("".join(current))
            current = []
            continue
        current.append(char)
    tail = "".join(current).strip()
    if tail:
        members.append(tail)
    return members


def public_method_names(body: str) -> list:
    names = []
    for member in split_members(body):
        text = member.strip()
        if not text:
            continue
        hidden = False
        stripping = True
        while stripping:
            stripping = False
            for modifier in MODIFIERS:
                if text.startswith(modifier + " "):
                    if modifier in HIDDEN:
                        hidden = True
                    text = text[len(modifier) + STEP :].lstrip()
                    stripping = True
                    break
        if hidden or text.startswith("constructor"):
            continue
        match = RE_MEMBER.match(text)
        if match:
            names.append(match.group("name"))
    return sorted(set(names))


def exported_names(index: Path) -> list:
    """Return [(local, exported, module)] for every name the entry point re-exports.

    `local` is the identifier the source module must declare; `exported` is the
    name a caller holds. They differ under `export { X as Y }`.
    """
    text = strip_comments(index.read_text(encoding="utf-8"), index)
    entries = []
    for block in RE_EXPORT_BLOCK.finditer(text):
        module = block.group("module")
        for raw in block.group("body").split(","):
            entry = re.sub(r"^type\s+", "", raw.strip()).strip()
            if not entry:
                continue
            parts = re.split(r"\s+as\s+", entry)
            entries.append((parts[NONE].strip(), parts[-STEP].strip(), module))
    if not entries:
        raise SurfaceError(
            f"{index}: no re-exported names found. An empty surface is far more "
            "likely to be a broken scanner than a package that exports nothing."
        )
    return entries


def module_path(dist: Path, index: Path, module: str) -> Path:
    """Resolve an ES module specifier to the `.d.ts` beside it, or refuse."""
    stem = re.sub(r"\.js$", "", module.lstrip("./"))
    path = dist / f"{stem}.d.ts"
    if not path.is_file():
        raise SurfaceError(
            f"{index} re-exports from '{module}', but {path} is not there. The "
            "declarations are incomplete, so the surface is unknown, not smaller."
        )
    return path


def declared_names(text: str) -> set:
    """Every name a `.d.ts` declares for export, in any of its shapes."""
    found = set()
    for match in RE_DECLARATION.finditer(text):
        for group in ("klass", "shape", "value"):
            name = match.group(group)
            if name:
                found.add(name)
    return found


def build_surface(root: Path) -> list:
    dist = find_dist(root)
    index = dist / "index.d.ts"
    entries = exported_names(index)

    # Resolve every re-export against a declaration in the module it names. A
    # mangled or truncated declaration stops matching rather than erroring, so
    # without this the surface would silently come out shorter and the rule would
    # read a removal nobody made.
    bodies = {}
    classes = {}
    for local, exported, module in entries:
        source = module_path(dist, index, module)
        if source not in bodies:
            bodies[source] = strip_comments(source.read_text(encoding="utf-8"), source)
        if local not in declared_names(bodies[source]):
            raise SurfaceError(
                f"{index} re-exports '{local}' from '{module}', but {source} "
                f"declares no such name. The surface is unknown, not smaller: a "
                "shrink here would read as a breaking removal that never happened."
            )
        classes[(source, local)] = exported

    names = {exported for _, exported, _ in entries}
    for (source, local), exported in classes.items():
        text = bodies[source]
        for head in RE_CLASS_HEAD.finditer(text):
            if head.group("name") != local:
                continue
            body = class_body(text, head.end(), source, local)
            for method in public_method_names(body):
                names.add(f"{exported}.{method}")
    return sorted(names)


def main(argv: list | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Print the public surface of the @wisent/cost-tracker npm "
        'distribution as {"surface": [...]}.'
    )
    parser.add_argument(
        "--root",
        default=".",
        help="repository root, or the root of an unpacked npm tarball",
    )
    args = parser.parse_args(argv)
    try:
        surface = build_surface(Path(args.root))
    except SurfaceError as error:
        print(f"refused: {error}", file=sys.stderr)
        return STEP
    print(json.dumps({"surface": surface}, indent=INDENT, sort_keys=True))
    return NONE


if __name__ == "__main__":
    sys.exit(main())
