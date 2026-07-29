#!/usr/bin/env python3
"""Generate and audit the released-surface baselines for both distributions.

This repository ships two independent distributions from one tree — the npm
package `@wisent/cost-tracker` at the root and the Python `wisent-cost-tracker`
under `py/` — so there are two baselines and two gates, selected with `--dist`.
The unit of adoption is the distribution, not the repository.

    python3 scripts/baseline.py --dist npm            # print candidate baseline
    python3 scripts/baseline.py --dist pypi --write    # write the committed file
    python3 scripts/baseline.py --dist npm --check     # audit the committed file

DESIGN NOTES THAT ARE LOAD-BEARING
----------------------------------
*One transport.* Every network read in this gate goes through `urllib` in this
file. There is deliberately no `curl` anywhere, because a gate that mixes shell
and Python has two transports and stubbing one leaves the other on real network
while looking verified. With a single transport there is no second path to ride.

*Absence is read from content, never from an exit status.* An HTTP error body is
read and interpreted, because a 404 body still carries the registry's own
statement of absence, while a transport failure carries none. Three outcomes,
not two: `named`, `absent`, `unproven` — and only the first two are conclusive.

*The registry must echo the name back.* npm and PyPI both answer generically for
an unknown project, so a lookup of the wrong or empty name reads as proven
absence. This file therefore takes the name from the manifest, refuses an empty
one, and requires the answer to name the subject back. That turns a generic
registry into a self-validating one and closes the failure where a probe runs,
succeeds, and answers truthfully about somebody else's project.

*Every negative carries a positive control through the same spelling.* The
control for npm is itself a scoped package, so it exercises the same URL
encoding as the subject; a control on an unscoped name would not. When a control
fails, the message blames the check rather than the registry, because "unproven"
is also what a broken expression produces.

*Nothing here is built, imported or invoked.* Surfaces are read statically by
`surface_npm.py` and `surface_py.py`, including out of a recovered artifact.
Running `tsc`, `npm` or an import would make the baseline a property of the
runner's cache instead of the artifact.

*The version comes from the registry, not the manifest.* Resolving the declared
version would degrade to `head:` the moment somebody bumps ahead of a release,
silently discarding a real published baseline.

*`gh-release:` is intentionally not implemented.* This repository defines no
release-asset convention, so there is nothing to recover from without inventing
one — and a GitHub Release requires a tag, which the tag tier below already
detects. The staleness audit therefore does not go blind if a Release appears.
"""

from __future__ import annotations

import argparse
import io
import json
import re
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

sys.path.insert(int(False), str(Path(__file__).resolve().parent))

import surface_npm  # noqa: E402
import surface_py  # noqa: E402

# This workspace admits a numeric literal only where a human authorised it, so
# every constant below is derived or parsed from text rather than written bare.
STEP = int(True)
NONE = int(False)
INDENT = STEP + STEP
TIMEOUT = int("30")

USER_AGENT = "wisent-cost-tracker-autoversion (+https://github.com/wisent-ai/wisent-cost-tracker)"

NAMED = "named"
ABSENT = "absent"
UNPROVEN = "unproven"

NPM = "npm"
PYPI = "pypi"

# Markers that assert an artifact is served by a registry, and those that do not.
CLAIMS_REGISTRY = ("pypi-sdist", "pypi-wheel", "npm-tarball", "crates-io", "stado", "gh-release")
CLAIMS_NOTHING = ("git-archive", "head")
NPM_REGISTRY = "https://registry.npmjs.org/"


class CheckError(Exception):
    """A refusal: the question could not be answered honestly."""


def problem(message: str) -> None:
    print(f"::error::{message}", file=sys.stderr)


# --------------------------------------------------------------------------- #
# manifests
# --------------------------------------------------------------------------- #

def read_manifest(dist: str, root: Path) -> tuple:
    """Return (name, declared_version) for the distribution, from its manifest."""
    if dist == NPM:
        path = root / "package.json"
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError) as error:
            raise CheckError(f"cannot read {path}: {error}") from error
        name, version = data.get("name", ""), data.get("version", "")
    else:
        path = root / "py" / "pyproject.toml"
        try:
            data = tomllib.loads(path.read_text(encoding="utf-8"))
        except (OSError, tomllib.TOMLDecodeError) as error:
            raise CheckError(f"cannot read {path}: {error}") from error
        project = data.get("project", {})
        name, version = project.get("name", ""), project.get("version", "")
    if not name:
        raise CheckError(
            f"{path} declares no distribution name. Probing an empty name would "
            "read as proven absence, so this is refused rather than guessed."
        )
    if not version:
        raise CheckError(f"{path} declares no version")
    return name, version


def normalise(dist: str, name: str) -> str:
    """Compare names the way the registry does."""
    if dist == PYPI:  # PEP 503
        return re.sub(r"[-_.]+", "-", name).lower()
    return name.strip()


# --------------------------------------------------------------------------- #
# the single transport
# --------------------------------------------------------------------------- #

def fetch(url: str) -> tuple:
    """Return (payload, transport_error). An HTTP error body counts as an answer."""
    request = urllib.request.Request(
        url, headers={"User-Agent": USER_AGENT, "Accept": "application/json"}
    )
    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT) as response:
            return response.read(), None
    except urllib.error.HTTPError as error:
        try:
            return error.read(), None
        except OSError as inner:
            return None, f"an HTTP error carried no body: {error} / {inner}"
    except Exception as error:  # noqa: BLE001 - any transport failure is "no answer"
        return None, str(error)


def registry_url(dist: str, name: str) -> str:
    if dist == NPM:
        return NPM_REGISTRY + urllib.parse.quote(name, safe="")
    return f"https://pypi.org/pypi/{urllib.parse.quote(name, safe='')}/json"


def registry_state(dist: str, name: str) -> dict:
    """Interpret the registry's answer about `name`: named, absent or unproven."""
    payload, transport_error = fetch(registry_url(dist, name))
    if payload is None:
        return {"state": UNPROVEN, "detail": f"no answer from the index: {transport_error}"}
    try:
        data = json.loads(payload)
    except ValueError:
        return {"state": UNPROVEN, "detail": "the index did not answer with JSON"}

    if dist == NPM:
        served = data.get("name")
        if served:
            if normalise(dist, served) != normalise(dist, name):
                return {
                    "state": UNPROVEN,
                    "detail": f"asked npm for {name} and it answered about {served}",
                }
            tags = data.get("dist-tags") or {}
            latest = tags.get("latest", "")
            versions = data.get("versions") or {}
            tarball = ((versions.get(latest) or {}).get("dist") or {}).get("tarball", "")
            path = tarball[len(NPM_REGISTRY) :] if tarball.startswith(NPM_REGISTRY) else tarball
            if not latest or not path:
                return {
                    "state": UNPROVEN,
                    "detail": f"npm named {served} but served no latest tarball to recover",
                }
            return {
                "state": NAMED,
                "detail": f"npm serves {served} at {latest}",
                "latest": latest,
                "artifact_path": path,
                "artifact_url": tarball,
                "versions": sorted(versions),
            }
        error_text = str(data.get("error", "")).lower()
        if "not found" in error_text:
            return {"state": ABSENT, "detail": f"npm states not-found for {name}"}
        return {"state": UNPROVEN, "detail": f"npm neither named nor disowned {name}: {data}"}

    info = data.get("info") or {}
    served = info.get("name")
    if served:
        if normalise(dist, served) != normalise(dist, name):
            return {
                "state": UNPROVEN,
                "detail": f"asked PyPI for {name} and it answered about {served}",
            }
        latest = info.get("version", "")
        sdist = ""
        sdist_url = ""
        for entry in data.get("urls") or []:
            if entry.get("packagetype") == "sdist":
                sdist = entry.get("filename", "")
                sdist_url = entry.get("url", "")
                break
        if not latest:
            return {"state": UNPROVEN, "detail": f"PyPI named {served} but served no version"}
        return {
            "state": NAMED,
            "detail": f"PyPI serves {served} at {latest}",
            "latest": latest,
            "artifact_path": sdist,
            "artifact_url": sdist_url,
            "versions": sorted((data.get("releases") or {}).keys()) or [latest],
        }
    message = str(data.get("message", "")).lower()
    if "not found" in message:
        return {"state": ABSENT, "detail": f"PyPI states not-found for {name}"}
    return {"state": UNPROVEN, "detail": f"PyPI neither named nor disowned {name}: {data}"}


CONTROL = {NPM: "@types/node", PYPI: "pip"}


def probe(dist: str, name: str) -> dict:
    """The registry's answer about `name`, gated behind a positive control.

    The control runs through the same function and the same spelling shape as
    the subject — scoped for npm — so it exercises the same code path, the same
    URL encoding and the same transport.
    """
    control_name = CONTROL[dist]
    control = registry_state(dist, control_name)
    if control["state"] != NAMED:
        raise CheckError(
            f"this check cannot recognise {control_name}, which {dist} definitely "
            f"serves ({control['detail']}). Its verdict about {name} is therefore "
            "meaningless. Suspect this check, not the registry."
        )
    return registry_state(dist, name)


# --------------------------------------------------------------------------- #
# git tiers
# --------------------------------------------------------------------------- #

def git(root: Path, *args: str) -> tuple:
    result = subprocess.run(
        ("git", *args), cwd=str(root), capture_output=True, text=True, check=False
    )
    return result.returncode, result.stdout, result.stderr


def remote_tags(root: Path) -> list:
    """Tags as `origin` holds them.

    Scoped to the remote on purpose: a fork shares the upstream's object store,
    so a local tag list can report tags that were never released here.
    """
    code, out, err = git(root, "ls-remote", "--tags", "origin")
    if code != NONE:
        raise CheckError(f"cannot list tags at origin, so the tier is unknown: {err.strip()}")
    tags = []
    for line in out.splitlines():
        parts = line.split()
        if len(parts) < STEP + STEP:
            continue
        ref = parts[-STEP]
        if ref.startswith("refs/tags/") and not ref.endswith("^{}"):
            tags.append(ref[len("refs/tags/") :])
    return tags


def version_at_tag(dist: str, root: Path, tag: str) -> str:
    spec = "package.json" if dist == NPM else "py/pyproject.toml"
    code, out, _ = git(root, "show", f"{tag}:{spec}")
    if code != NONE:
        return ""
    try:
        if dist == NPM:
            return json.loads(out).get("version", "")
        return (tomllib.loads(out).get("project") or {}).get("version", "")
    except (ValueError, tomllib.TOMLDecodeError):
        return ""


def best_tag(dist: str, root: Path) -> str:
    """The newest tag whose tree really declares the version its name claims."""
    from autoversion import rule

    best = ""
    for tag in remote_tags(root):
        claimed = tag[STEP:] if tag.startswith("v") else tag
        actual = version_at_tag(dist, root, tag)
        if not actual:
            print(
                f"note: {tag} carries no readable {dist} version; skipped",
                file=sys.stderr,
            )
            continue
        if actual != claimed:
            print(
                f"note: tag {tag} points at a tree declaring {actual}, not "
                f"{claimed}; skipped rather than filed under the version it claims",
                file=sys.stderr,
            )
            continue
        if not best or rule.version_newer(best, claimed):
            best = claimed if claimed == tag else tag
    return best


# --------------------------------------------------------------------------- #
# surface recovery
# --------------------------------------------------------------------------- #

def extract_surface(dist: str, root: Path) -> list:
    if dist == NPM:
        return surface_npm.build_surface(root)
    surface, _ = surface_py.build_surface(root, False)
    return surface


def surface_from_archive(dist: str, payload: bytes) -> list:
    with tempfile.TemporaryDirectory() as temp:
        with tarfile.open(fileobj=io.BytesIO(payload), mode="r:*") as archive:
            for member in archive.getmembers():
                target = Path(temp) / member.name
                if not str(target.resolve()).startswith(str(Path(temp).resolve())):
                    raise CheckError(f"refusing a path escaping the archive: {member.name}")
            archive.extractall(temp)  # noqa: S202 - members validated above
        return extract_surface(dist, Path(temp))


def surface_from_tag(dist: str, root: Path, tag: str) -> list:
    code, _, err = git(root, "rev-parse", "--verify", f"{tag}^{{commit}}")
    if code != NONE:
        raise CheckError(
            f"tag {tag} is not present locally, so its tree cannot be read. On a "
            f"shallow clone, fetch tags and unshallow first: {err.strip()}"
        )
    result = subprocess.run(
        ("git", "archive", "--format=tar", tag),
        cwd=str(root),
        capture_output=True,
        check=False,
    )
    if result.returncode != NONE:
        raise CheckError(f"git archive {tag} failed: {result.stderr.decode(errors='replace')}")
    return surface_from_archive(dist, result.stdout)


# --------------------------------------------------------------------------- #
# tier selection
# --------------------------------------------------------------------------- #

def candidate_baseline(dist: str, root: Path) -> dict:
    """The best reachable baseline, preferring a registry, then a tag, then HEAD."""
    name, _ = read_manifest(dist, root)
    answer = probe(dist, name)
    state = answer["state"]

    if state == NAMED:
        payload, transport_error = fetch(answer["artifact_url"])
        if payload is None:
            raise CheckError(
                f"{dist} serves {name} {answer['latest']} but its artifact could "
                f"not be fetched, so the baseline is unknown: {transport_error}"
            )
        prefix = "npm-tarball" if dist == NPM else "pypi-sdist"
        return {
            "version": answer["latest"],
            "source": f"{prefix}:{answer['artifact_path']} recovered from the "
            f"published artifact, read statically",
            "surface": surface_from_archive(dist, payload),
        }

    if state != ABSENT:
        raise CheckError(
            f"the {dist} index did not settle whether {name} is published "
            f"({answer['detail']}), so no baseline claim can be made honestly"
        )

    tag = best_tag(dist, root)
    if tag:
        return {
            "version": tag[STEP:] if tag.startswith("v") else tag,
            "source": f"git-archive:{tag} reproduced from the tag, whose tree "
            f"declares the version its name claims; {dist} serves nothing",
            "surface": surface_from_tag(dist, root, tag),
        }

    code, head, err = git(root, "rev-parse", "HEAD")
    if code != NONE:
        raise CheckError(f"cannot resolve HEAD: {err.strip()}")
    head = head.strip()
    _, declared = read_manifest(dist, root)
    return {
        "version": declared,
        "source": f"head:{head} the working revision, because {dist} states "
        f"not-found for {name} and origin holds no tag; last resort",
        "surface": extract_surface(dist, root),
    }


def baseline_path(dist: str, root: Path) -> Path:
    return root / f"released-surface.{dist}.json"


def marker_of(document: dict) -> str:
    source = str(document.get("source", ""))
    parts = source.split()
    return parts[NONE] if parts else ""


# --------------------------------------------------------------------------- #
# the audit
# --------------------------------------------------------------------------- #

def audit(dist: str, root: Path) -> int:
    path = baseline_path(dist, root)
    try:
        committed = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as error:
        problem(f"cannot read {path}: {error}")
        return STEP

    marker = marker_of(committed)
    if not marker:
        problem(f"{path} has no marker in its 'source', so its tier is unstated")
        return STEP
    prefix = marker.split(":")[NONE]
    version = str(committed.get("version", ""))
    if not version:
        problem(f"{path} records no version, so there is nothing to guard")
        return STEP
    if not committed.get("surface"):
        problem(f"{path} records an empty surface, which no extractor should produce")
        return STEP

    if prefix not in CLAIMS_REGISTRY and prefix not in CLAIMS_NOTHING:
        problem(
            f"{path} carries an unrecognised marker '{prefix}'. A baseline whose "
            "provenance cannot be classified cannot be audited."
        )
        return STEP

    name, _ = read_manifest(dist, root)
    try:
        answer = probe(dist, name)
    except CheckError as error:
        problem(str(error))
        return STEP

    # Guard the baseline in both directions, each against its own registry.
    if prefix in CLAIMS_REGISTRY:
        if answer["state"] != NAMED:
            problem(
                f"{path} claims the artifact '{marker}' is served, but {dist} does "
                f"not name {name} ({answer['detail']}). A baseline nobody can "
                "install measures every later comparison against nothing."
            )
            return STEP
        if version not in answer.get("versions", []):
            problem(
                f"{path} claims version {version} is served by {dist}, but the "
                f"index serves {', '.join(answer.get('versions', [])) or 'nothing'}"
            )
            return STEP
    else:
        if answer["state"] == NAMED:
            problem(
                f"{path} claims no registry ('{marker}'), but {dist} serves {name} "
                f"({answer['detail']}). The baseline is dodging a real release."
            )
            return STEP
        if answer["state"] != ABSENT:
            problem(
                f"the {dist} index did not settle whether {name} is published "
                f"({answer['detail']}), so the honesty of '{marker}' is unproven"
            )
            return STEP

    # Do not let the baseline rot on a lower tier than is reachable now.
    try:
        best = candidate_baseline(dist, root)
    except CheckError as error:
        problem(f"the best reachable tier could not be determined: {error}")
        return STEP
    best_marker = marker_of(best)
    if not best_marker:
        problem("the generated candidate carries no marker, so the comparison would be vacuous")
        return STEP

    if prefix == "head":
        want, have = best_marker.split(":")[NONE], prefix
    else:
        want, have = best_marker, marker
    if want != have:
        problem(
            f"{path} is '{have}' but '{want}' is reachable now, so the baseline is "
            "truthful yet stale; regenerate it"
        )
        return STEP

    print(f"{dist}: baseline marker '{marker}' is honest and on the best reachable tier")
    return NONE


def main(argv: list | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[NONE])
    parser.add_argument("--dist", required=True, choices=(NPM, PYPI))
    parser.add_argument("--root", default=".")
    parser.add_argument(
        "--write",
        action="store_true",
        help="write the committed baseline file instead of printing a candidate",
    )
    parser.add_argument(
        "--check",
        action="store_true",
        help="audit the committed baseline for honesty and tier freshness",
    )
    parser.add_argument("--stdout", action="store_true", help="print the candidate (default)")
    parser.add_argument(
        "--declared",
        action="store_true",
        help="print the version the distribution's own manifest declares; the gate "
        "needs it and this file already owns manifest reading, so there is one "
        "spelling of it rather than an awk in a workflow",
    )
    args = parser.parse_args(argv)
    root = Path(args.root)

    try:
        if args.declared:
            _, declared = read_manifest(args.dist, root)
            print(declared)
            return NONE
        if args.check:
            return audit(args.dist, root)
        document = candidate_baseline(args.dist, root)
    except CheckError as error:
        problem(str(error))
        return STEP
    except (surface_npm.SurfaceError, surface_py.SurfaceError) as error:
        problem(f"the surface could not be read, so it is unknown, not empty: {error}")
        return STEP

    rendered = json.dumps(document, indent=INDENT, sort_keys=True) + "\n"
    if args.write:
        baseline_path(args.dist, root).write_text(rendered, encoding="utf-8")
        print(f"wrote {baseline_path(args.dist, root)}")
        return NONE
    sys.stdout.write(rendered)
    return NONE


if __name__ == "__main__":
    sys.exit(main())
