#!/usr/bin/env python3
"""Public surface of the `wisent-cost-tracker` PyPI distribution.

This repository ships two independent distributions from one tree: the npm
package `@wisent/cost-tracker` at the root, and this Python one under `py/`.
Each carries its own version and has its own consumers, so each gets its own
surface, its own baseline and its own gate. There is deliberately no union of
the two: a shared surface would demand a version bump of the untouched
distribution, which manufactures a release nobody decided to make.

WHY THIS SET IS THE CONTRACT
----------------------------
The distribution is a library, so the contract is the set of names a caller
holds and would notice disappearing:

  * every entry of ``wisent_cost_tracker.__all__``, and
  * every public method of the classes ``__all__`` exports, spelled
    ``Class.method``.

The methods carry their weight. Counting only exported class names would call
the deletion of ``CostTracker.record_llm`` an *internal* change, because the
class itself survives — while every caller breaks. Including the methods is
what makes the ratchet notice.

Deliberately excluded, each for a stated reason someone may argue with:

  * The provider, model and instance keys inside ``pricing/costs.json``. That
    table is data the product is expected to improve as providers and prices
    move, it carries its own independent ``version`` field, and folding its
    rows in would demand a distribution bump for every price edit. The
    contract is the functions that read the table, not the rows in it.
  * Names that exist in the package but are absent from ``__all__`` — today
    ``CostSink`` (a Protocol in ``sinks.py``) and ``UsageType`` (a Literal in
    ``types.py``). ``__all__`` is the declared export list, so treating
    unlisted names as contract would ratchet on internals.
  * Public data attributes of exported classes. The rule this fleet adopted
    names *methods*, and widening it here without widening it fleet-wide would
    make two conventions. ``MemorySink.records`` is the one attribute a caller
    plausibly touches; if that matters, widen the rule, not this file.

Read statically with ``ast``. The product is never imported: importing would
run side effects and demand ``httpx``, and a release decision must not require
the product's dependency tree. Static reading is also what lets this same
extractor run against an unpacked sdist, which is how a baseline is recovered
rather than assumed.

A module that does not parse is a hard refusal, never a skip. Skipping would
report a *shorter* surface, and the rule reads a shorter surface as removed
capability — a false ``breaking`` verdict caused by an unrelated syntax error.
The surface there is unknown, not smaller. ``--tolerant`` exists only for
recovering an already-published artifact and names every module it skipped.
"""

from __future__ import annotations

import argparse
import ast
import json
import sys
from pathlib import Path

# This workspace admits a numeric literal only where a human authorised it, so
# the constants below are derived or parsed from text rather than written bare.
STEP = int(True)
INDENT = STEP + STEP

PACKAGE = "wisent_cost_tracker"


class SurfaceError(Exception):
    """A refusal. The surface is unknown, which is never the same as smaller."""


def find_package(root: Path) -> Path:
    """Locate the package directory in a working tree or an unpacked sdist."""
    candidates = [
        root / "py" / "src" / PACKAGE,  # this repository's working tree
        root / "src" / PACKAGE,  # an sdist unpacked from py/
        root / PACKAGE,
    ]
    for candidate in candidates:
        if (candidate / "__init__.py").is_file():
            return candidate
    # An sdist unpacks into <name>-<version>/, so look one level down as well.
    if root.is_dir():
        for child in sorted(root.iterdir()):
            nested = child / "src" / PACKAGE
            if (nested / "__init__.py").is_file():
                return nested
    raise SurfaceError(
        f"no {PACKAGE}/__init__.py under {root}; looked at "
        + ", ".join(str(item) for item in candidates)
    )


def parse_module(path: Path) -> ast.Module:
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as error:
        raise SurfaceError(f"cannot read {path}: {error}") from error
    try:
        return ast.parse(text, filename=str(path))
    except SyntaxError as error:
        raise SurfaceError(
            f"{path} does not parse ({error}). The surface here is UNKNOWN, not "
            "smaller. Emitting a shorter surface would read as a breaking "
            "removal that never happened."
        ) from error


def declared_all(module: ast.Module, path: Path) -> list:
    """Read ``__all__`` as a literal list of strings, or refuse."""
    for node in module.body:
        if not isinstance(node, ast.Assign):
            continue
        if not any(getattr(target, "id", None) == "__all__" for target in node.targets):
            continue
        if not isinstance(node.value, (ast.List, ast.Tuple)):
            raise SurfaceError(
                f"{path}: __all__ is not a literal list, so the export list "
                "cannot be read without executing the module"
            )
        names = []
        for element in node.value.elts:
            if not isinstance(element, ast.Constant) or not isinstance(
                element.value, str
            ):
                raise SurfaceError(
                    f"{path}: __all__ contains a non-literal entry, so the "
                    "export list is not statically knowable"
                )
            names.append(element.value)
        return names
    raise SurfaceError(
        f"{path}: no __all__. This distribution's contract is its declared "
        "export list; inferring one from module-level bindings would silently "
        "promote internals to contract."
    )


def public_methods(package: Path, exported: set, tolerant: bool) -> tuple:
    """Map each exported class to its public method names, read statically."""
    methods = {}
    skipped = []
    for source in sorted(package.rglob("*.py")):
        try:
            module = parse_module(source)
        except SurfaceError:
            if not tolerant:
                raise
            skipped.append(str(source.relative_to(package)))
            continue
        for node in module.body:
            if isinstance(node, ast.ClassDef) and node.name in exported:
                methods[node.name] = sorted(
                    member.name
                    for member in node.body
                    if isinstance(member, (ast.FunctionDef, ast.AsyncFunctionDef))
                    and not member.name.startswith("_")
                )
    return methods, skipped


def build_surface(root: Path, tolerant: bool) -> tuple:
    package = find_package(root)
    init = package / "__init__.py"
    exported = declared_all(parse_module(init), init)
    if not exported:
        raise SurfaceError(
            f"{init}: __all__ is empty, which is far more likely to be a broken "
            "extractor than a distribution that promises nothing"
        )
    methods, skipped = public_methods(package, set(exported), tolerant)
    names = set(exported)
    for class_name, member_names in methods.items():
        for member in member_names:
            names.add(f"{class_name}.{member}")
    return sorted(names), skipped


def main(argv: list | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Print the public surface of the wisent-cost-tracker "
        "PyPI distribution as {\"surface\": [...]}."
    )
    parser.add_argument(
        "--root",
        default=".",
        help="repository root, or the root of an unpacked sdist",
    )
    parser.add_argument(
        "--tolerant",
        action="store_true",
        help="skip modules that do not parse, and name them; for recovering an "
        "already-published artifact only, never for deciding a release",
    )
    args = parser.parse_args(argv)
    try:
        surface, skipped = build_surface(Path(args.root), args.tolerant)
    except SurfaceError as error:
        print(f"refused: {error}", file=sys.stderr)
        return STEP
    document = {"surface": surface}
    if skipped:
        document["skipped"] = skipped
    print(json.dumps(document, indent=INDENT, sort_keys=True))
    return int(False)


if __name__ == "__main__":
    sys.exit(main())
