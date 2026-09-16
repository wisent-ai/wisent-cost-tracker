"""One machine's attempt at the first-use journey.

The runtime owns the state file on disk, the queue of events still owed to the
control plane, and the position in the three-screen journey. Everything it
writes goes through :meth:`Runtime._save`, which writes a temporary file and
renames it, so a killed process leaves either the previous state or the new
one.
"""

from __future__ import annotations

import hashlib
import json
import os
import uuid
from typing import Any, Mapping

from ..definition import (
    FIRST_SUCCESS_FACT,
    JOURNEY_ID,
    JOURNEY_VERSION,
    JOURNEY_VERSION_ID,
    PRODUCT_ID,
    SCREEN_ACTIONS,
    SCREEN_ORDER,
    SHIPPED_DEFINITION,
    SOURCE_REVISION,
    now,
    shipped_bundle,
    state_path,
    validate_bundle,
)
from .control_plane import StadoTransport, build_event, flush

#: The shape of the state file this build writes and accepts. A file written
#: by another schema is not read as if it were this one.
STATE_SCHEMA_VERSION = 1

#: The statuses an attempt can be in, from the published contract.
TERMINAL_STATUSES = {"in_progress", "completed", "skipped", "abandoned"}


class Runtime:
    def __init__(self) -> None:
        self.path = state_path()
        self.transport = StadoTransport()
        self.state = self._load()
        self.bundle = self._load_bundle()
        self.subject_hash = hashlib.sha256(
            f"{PRODUCT_ID}:{self.state['installation_id']}".encode("utf-8")
        ).hexdigest()
        self.was_existing = False

    def _fresh(self) -> dict[str, Any]:
        return {
            "schema_version": STATE_SCHEMA_VERSION,
            "installation_id": str(uuid.uuid4()),
            "pending_events": [],
            "evidence": {},
        }

    def _load(self) -> dict[str, Any]:
        try:
            loaded = json.loads(self.path.read_text("utf-8"))
            if (
                isinstance(loaded, dict)
                and loaded.get("schema_version") == STATE_SCHEMA_VERSION
                and isinstance(loaded.get("installation_id"), str)
                and isinstance(loaded.get("pending_events"), list)
                and isinstance(loaded.get("evidence"), dict)
            ):
                return loaded
        except (OSError, ValueError, json.JSONDecodeError):
            pass
        return self._fresh()

    def _save(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        temporary = self.path.with_name(f".{self.path.name}.{uuid.uuid4().hex}.tmp")
        encoded = json.dumps(self.state, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
        with temporary.open("w", encoding="utf-8") as handle:
            os.chmod(temporary, 0o600)
            handle.write(encoded)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, self.path)

    def _load_bundle(self) -> dict[str, Any]:
        """The published definition, from the control plane if it answers.

        Three sources in order of authority: the control plane, the copy this
        machine already validated, and the one packaged with the release. All
        three pass the same check, so a definition that is not the product's
        own is refused whichever of them it came from.
        """
        try:
            response = self.transport.read_bundle()
            bundle = validate_bundle(response.get("bundle", response))
            self.state["bundle"] = bundle
            self._save()
            return bundle
        except (RuntimeError, ValueError):
            try:
                return validate_bundle(self.state.get("bundle"))
            except ValueError:
                return validate_bundle(shipped_bundle())

    def _valid_progress(self) -> bool:
        progress = self.state.get("progress")
        return (
            isinstance(progress, dict)
            and progress.get("product_id") == PRODUCT_ID
            and progress.get("journey_version_id") == JOURNEY_VERSION_ID
            and progress.get("subject_hash") == self.subject_hash
            and progress.get("current_screen_id") in SCREEN_ORDER
            and progress.get("status") in TERMINAL_STATUSES
            and isinstance(progress.get("completed_screen_ids"), list)
        )

    def _new_progress(self) -> dict[str, Any]:
        return {
            "attempt_id": str(uuid.uuid4()),
            "product_id": PRODUCT_ID,
            "journey_version_id": JOURNEY_VERSION_ID,
            "subject_hash": self.subject_hash,
            "scope_kind": "device",
            "current_screen_id": SCREEN_ORDER[0],
            "completed_screen_ids": [],
            "status": "in_progress",
            "evidence_revision": now(),
            "assignment": {"experiment_id": None, "variant_id": "control"},
        }

    @property
    def progress(self) -> dict[str, Any]:
        return self.state["progress"]

    def open(self, start: bool) -> bool:
        """Resume this machine's attempt, or begin one when asked to.

        Returns False for the one case a caller has to tell apart: nothing was
        ever started here and it was not asked to start anything.
        """
        existing = self._valid_progress()
        self.was_existing = existing
        if not existing and not start:
            return False
        if not existing:
            self._begin_attempt()
        else:
            try:
                self.transport.read_state(self.progress)
            except RuntimeError:
                pass
        self.flush_events()
        return True

    def _begin_attempt(self) -> None:
        self.state["progress"] = self._new_progress()
        self.state["evidence"] = {}
        try:
            assignment = self.transport.assign(self.subject_hash)
            selected = assignment.get("assignment", assignment)
            if isinstance(selected, Mapping):
                self.progress["assignment"] = {
                    "experiment_id": selected.get("experiment_id", selected.get("experimentId")),
                    "variant_id": selected.get("variant_id", selected.get("variant", "control")),
                }
        except RuntimeError:
            pass
        self._queue("onboarding_started")
        self._queue("onboarding_step_viewed")
        self._save()

    def _queue(self, name: str, **kwargs: Any) -> None:
        self.state["pending_events"].append(
            build_event(
                name,
                progress=self.progress,
                subject_hash=self.subject_hash,
                **kwargs,
            )
        )

    def flush_events(self) -> None:
        flush(self.state["pending_events"], self.transport, self._save)

    def resume(self) -> None:
        self._record_step_event("onboarding_resumed")

    def expose(self) -> None:
        self._record_step_event("onboarding_step_viewed")

    def _record_step_event(self, name: str) -> None:
        if self.progress["status"] != "in_progress":
            return
        self.progress["evidence_revision"] = now()
        self._queue(name)
        self._save()
        self.flush_events()

    def observe_step(self, screen_id: str, evidence: Mapping[str, Any]) -> bool:
        """Record what happened on one screen, and move on if it counts.

        The screen has to be the one the attempt is on, and the last screen
        additionally has to see the first-success fact, so nobody completes
        the journey by calling this three times with nothing behind it.
        """
        if self.progress["status"] != "in_progress" or self.progress["current_screen_id"] != screen_id:
            return False
        index = SCREEN_ORDER.index(screen_id)
        self.state["evidence"].update(evidence)
        self.progress["evidence_revision"] = now()
        if screen_id not in self.progress["completed_screen_ids"]:
            self.progress["completed_screen_ids"].append(screen_id)
        if index + 1 < len(SCREEN_ORDER):
            self._advance_to(SCREEN_ORDER[index + 1], screen_id, evidence)
        else:
            if self.state["evidence"].get(FIRST_SUCCESS_FACT) is not True:
                return False
            self._complete(screen_id, evidence)
        self._save()
        self.flush_events()
        return True

    def _advance_to(self, next_screen: str, screen_id: str, evidence: Mapping[str, Any]) -> None:
        self._queue(
            "onboarding_step_completed",
            properties=evidence,
            screen_id=screen_id,
            next_screen_id=next_screen,
            reason_code="canonical_progression",
        )
        self.progress["current_screen_id"] = next_screen
        self._queue("onboarding_step_viewed", screen_id=next_screen)

    def _complete(self, screen_id: str, evidence: Mapping[str, Any]) -> None:
        self.progress["status"] = "completed"
        self._queue("onboarding_step_completed", properties=evidence, screen_id=screen_id)
        self._queue("onboarding_first_success_observed", properties=evidence, screen_id=screen_id)
        self._queue("onboarding_completed", properties=evidence, screen_id=screen_id)

    def set_terminal(self, status: str, event: str) -> None:
        if self.progress["status"] == "in_progress":
            self.progress["status"] = status
            self.progress["evidence_revision"] = now()
            self._queue(event)
            self._save()
            self.flush_events()

    def reset(self) -> None:
        if self._valid_progress():
            self._queue("onboarding_reset")
        assignment = self.progress.get("assignment") if self._valid_progress() else None
        self.state["progress"] = self._new_progress()
        if isinstance(assignment, dict):
            self.progress["assignment"] = assignment
        self.state["evidence"] = {}
        self._queue("onboarding_started", properties={"reason_code": "reset"})
        self._save()
        self.flush_events()

    def view(self) -> dict[str, Any]:
        """What the journey looks like right now, for a caller to print."""
        progress = self.progress
        screen = next(
            item
            for item in SHIPPED_DEFINITION["screens"]
            if item["screen_id"] == progress["current_screen_id"]
        )
        return {
            "product_id": PRODUCT_ID,
            "journey_id": JOURNEY_ID,
            "journey_version": JOURNEY_VERSION,
            "journey_version_id": JOURNEY_VERSION_ID,
            "source_revision": SOURCE_REVISION,
            "first_success_fact": FIRST_SUCCESS_FACT,
            "status": progress["status"],
            "attempt_id": progress["attempt_id"],
            "current_screen_id": progress["current_screen_id"],
            "completed_screen_ids": list(progress["completed_screen_ids"]),
            "action": SCREEN_ACTIONS[progress["current_screen_id"]],
            "presentation": screen["presentation"],
            "evidence": dict(self.state["evidence"]),
        }
