"""Durable machine first-use adapter for wisent-cost-tracker."""

from __future__ import annotations

import hashlib
import json
import math
import os
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Mapping, Sequence

from .types import BudgetDecision, BudgetStatus, CostRecord

PRODUCT_ID = "wisent-cost-tracker"
CLIENT_ID = PRODUCT_ID
JOURNEY_ID = "first-use"
JOURNEY_VERSION = "2026-08-04.1"
JOURNEY_VERSION_ID = "12000000-0000-4000-8000-000000000009"
SOURCE_REVISION = "wisent-cost-tracker-first-use-2026-08-04"
FIRST_SUCCESS_FACT = "budget_decision_observed"

CANONICAL_EVENTS = frozenset(
    {
        "onboarding_started",
        "onboarding_resumed",
        "onboarding_step_viewed",
        "onboarding_step_completed",
        "onboarding_step_skipped",
        "onboarding_abandoned",
        "onboarding_reset",
        "onboarding_first_success_observed",
        "onboarding_completed",
    }
)

# This is byte-for-byte the canonical definition in
# wisent-supabase-echo/supabase/seeds/onboarding_09_e.sql.
CANONICAL_DEFINITION = r'''{"analytics_contract":{"completion_event":"onboarding_completed","contract_version":"1","exposure_event":"onboarding_step_viewed","first_success_event":"onboarding_first_success_observed","primary_action_event":"onboarding_step_completed","surface":"sdk_first_use"},"entry_screen_id":"review-budget","experiment_contract":null,"first_success_fact":"budget_decision_observed","journey_id":"first-use","journey_version":"2026-08-04.1","product_id":"wisent-cost-tracker","published_at":"2026-08-04T00:00:00Z","schema_version":1,"screens":[{"actions":["budget_status"],"body_key":"wisent-cost-tracker.onboarding.review-budget.body","completion_evidence":null,"entry_conditions":null,"fallback_screen_id":null,"presentation":{"body":"Read the current budget status rows before recording usage so the decision has an observable baseline.","renderer":"machine_discovery","title":"Review the current budget"},"required":true,"screen_id":"review-budget","screen_kind":"machine_discovery","title_key":"wisent-cost-tracker.onboarding.review-budget.title","transitions":[{"next_screen_id":"record-usage","priority":10,"reason_code":"canonical_progression"}]},{"actions":["record_and_flush_usage"],"body_key":"wisent-cost-tracker.onboarding.record-usage.body","completion_evidence":null,"entry_conditions":null,"fallback_screen_id":null,"presentation":{"body":"Submit and flush one validated nonempty usage batch through the configured sink, and wait until the sink write resolves.","renderer":"machine_action","title":"Record real usage"},"required":true,"screen_id":"record-usage","screen_kind":"machine_action","title_key":"wisent-cost-tracker.onboarding.record-usage.title","transitions":[{"next_screen_id":"make-budget-decision","priority":10,"reason_code":"canonical_progression"}]},{"actions":["decide_budget"],"body_key":"wisent-cost-tracker.onboarding.make-budget-decision.body","completion_evidence":{"fact":"budget_decision_observed","kind":"fact","operator":"eq","value":true},"entry_conditions":null,"fallback_screen_id":null,"presentation":{"body":"Ask the same tracker instance for isOverBudget and remaining after it observed the accepted usage batch.","renderer":"machine_result","title":"Make a budget decision"},"required":true,"screen_id":"make-budget-decision","screen_kind":"machine_result","title_key":"wisent-cost-tracker.onboarding.make-budget-decision.title","transitions":[]}],"source_revision":"wisent-cost-tracker-first-use-2026-08-04"}'''
FALLBACK_DEFINITION: dict[str, Any] = json.loads(CANONICAL_DEFINITION)

_SCREEN_ORDER = ("review-budget", "record-usage", "make-budget-decision")
_SCREEN_ACTIONS = {
    "review-budget": "budget_status",
    "record-usage": "record_and_flush_usage",
    "make-budget-decision": "decide_budget",
}


class OnboardingError(RuntimeError):
    """Stable machine-readable first-use error."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def _now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def _state_path() -> Path:
    configured = os.environ.get("WISENT_COST_TRACKER_ONBOARDING_STATE_PATH")
    if configured:
        return Path(configured)
    root = Path(os.environ.get("XDG_STATE_HOME", Path.home() / ".local" / "state"))
    return root / PRODUCT_ID / "onboarding.json"


def _fallback_bundle() -> dict[str, Any]:
    return {
        "journey_version_id": JOURNEY_VERSION_ID,
        "definition": FALLBACK_DEFINITION,
        "canonical_definition": CANONICAL_DEFINITION,
        "content_sha256": hashlib.sha256(CANONICAL_DEFINITION.encode("utf-8")).hexdigest(),
        "source_revision": SOURCE_REVISION,
    }


def _validate_bundle(value: Any) -> dict[str, Any]:
    if not isinstance(value, Mapping):
        raise ValueError("bundle envelope must be an object")
    bundle = dict(value)
    canonical = bundle.get("canonical_definition")
    definition = bundle.get("definition")
    if (
        bundle.get("journey_version_id") != JOURNEY_VERSION_ID
        or bundle.get("source_revision") != SOURCE_REVISION
        or canonical != CANONICAL_DEFINITION
        or definition != FALLBACK_DEFINITION
        or bundle.get("content_sha256") != hashlib.sha256(CANONICAL_DEFINITION.encode("utf-8")).hexdigest()
    ):
        raise ValueError("bundle does not match the product-owned canonical definition")
    return bundle


class _StadoTransport:
    def __init__(self) -> None:
        self.base_url = os.environ.get("STADO_INTEGRATION_API_URL", "").rstrip("/")
        self.token = os.environ.get("WISENT_COST_TRACKER_STADO_INTEGRATION_TOKEN", "")
        self.timeout = float(os.environ.get("STADO_ONBOARDING_TIMEOUT_SECONDS", "2"))
        self.available = True

    def post(self, operation: str, body: Mapping[str, Any]) -> dict[str, Any]:
        if not self.available or not self.base_url or not self.token:
            raise RuntimeError("onboarding control plane is unavailable")
        parsed = urllib.parse.urlparse(self.base_url)
        if parsed.scheme != "https" or not parsed.netloc or parsed.username or parsed.password:
            self.available = False
            raise RuntimeError("onboarding control plane URL is invalid")
        request = urllib.request.Request(
            f"{self.base_url}/api/integration/onboarding/{operation}",
            data=json.dumps({"client_id": CLIENT_ID, **body}, separators=(",", ":")).encode("utf-8"),
            headers={
                "Authorization": f"Bearer {self.token}",
                "Content-Type": "application/json",
                "X-Onboarding-Client": CLIENT_ID,
            },
            method="POST",
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                envelope = json.loads(response.read().decode("utf-8"))
            if not isinstance(envelope, dict):
                raise ValueError("invalid response")
            if envelope.get("ok") is True and "result" in envelope:
                result = envelope["result"]
                return dict(result) if isinstance(result, Mapping) else {}
            return envelope
        except (OSError, urllib.error.URLError, ValueError, json.JSONDecodeError) as error:
            self.available = False
            raise RuntimeError(f"Stado {operation} unavailable") from error

    def read_bundle(self) -> dict[str, Any]:
        return self.post(
            "bundle.read",
            {
                "product_id": PRODUCT_ID,
                "journey_id": JOURNEY_ID,
                "journey_version": JOURNEY_VERSION,
                "if_none_match": None,
            },
        )

    def assign(self, subject_hash: str) -> dict[str, Any]:
        return self.post(
            "experiments.assign",
            {
                "product_id": PRODUCT_ID,
                "journey_id": JOURNEY_ID,
                "journey_version": JOURNEY_VERSION,
                "subject_hash": subject_hash,
                "scope_kind": "device",
                "surface": "sdk_first_use",
            },
        )

    def read_state(self, progress: Mapping[str, Any]) -> dict[str, Any]:
        return self.post(
            "state.read",
            {
                "product_id": PRODUCT_ID,
                "journey_version_id": JOURNEY_VERSION_ID,
                "attempt_id": progress["attempt_id"],
                "subject_hash": progress["subject_hash"],
                "scope_kind": "device",
            },
        )

    def collect(self, event: Mapping[str, Any]) -> dict[str, Any]:
        return self.post("events.collect", event)


class _Runtime:
    def __init__(self) -> None:
        self.path = _state_path()
        self.transport = _StadoTransport()
        self.state = self._load()
        self.bundle = self._load_bundle()
        self.subject_hash = hashlib.sha256(
            f"{PRODUCT_ID}:{self.state['installation_id']}".encode("utf-8")
        ).hexdigest()
        self.was_existing = False

    def _fresh(self) -> dict[str, Any]:
        return {
            "schema_version": 1,
            "installation_id": str(uuid.uuid4()),
            "pending_events": [],
            "evidence": {},
        }

    def _load(self) -> dict[str, Any]:
        try:
            loaded = json.loads(self.path.read_text("utf-8"))
            if (
                isinstance(loaded, dict)
                and loaded.get("schema_version") == 1
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
        try:
            response = self.transport.read_bundle()
            bundle = _validate_bundle(response.get("bundle", response))
            self.state["bundle"] = bundle
            self._save()
            return bundle
        except (RuntimeError, ValueError):
            try:
                return _validate_bundle(self.state.get("bundle"))
            except ValueError:
                return _validate_bundle(_fallback_bundle())

    def _valid_progress(self) -> bool:
        progress = self.state.get("progress")
        return (
            isinstance(progress, dict)
            and progress.get("product_id") == PRODUCT_ID
            and progress.get("journey_version_id") == JOURNEY_VERSION_ID
            and progress.get("subject_hash") == self.subject_hash
            and progress.get("current_screen_id") in _SCREEN_ORDER
            and progress.get("status") in {"in_progress", "completed", "skipped", "abandoned"}
            and isinstance(progress.get("completed_screen_ids"), list)
        )

    def _new_progress(self) -> dict[str, Any]:
        revision = _now()
        return {
            "attempt_id": str(uuid.uuid4()),
            "product_id": PRODUCT_ID,
            "journey_version_id": JOURNEY_VERSION_ID,
            "subject_hash": self.subject_hash,
            "scope_kind": "device",
            "current_screen_id": "review-budget",
            "completed_screen_ids": [],
            "status": "in_progress",
            "evidence_revision": revision,
            "assignment": {"experiment_id": None, "variant_id": "control"},
        }

    @property
    def progress(self) -> dict[str, Any]:
        return self.state["progress"]

    def open(self, start: bool) -> bool:
        existing = self._valid_progress()
        self.was_existing = existing
        if not existing and not start:
            return False
        if not existing:
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
        else:
            try:
                self.transport.read_state(self.progress)
            except RuntimeError:
                pass
        self.flush_events()
        return True

    def _event(
        self,
        name: str,
        properties: Mapping[str, Any] | None = None,
        screen_id: str | None = None,
        next_screen_id: str | None = None,
        reason_code: str | None = None,
    ) -> dict[str, Any]:
        if name not in CANONICAL_EVENTS:
            raise ValueError(f"unsupported onboarding event {name}")
        assignment = self.progress.get("assignment", {})
        return {
            "event_id": str(uuid.uuid4()),
            "event_name": name,
            "attempt_id": self.progress["attempt_id"],
            "product_id": PRODUCT_ID,
            "journey_id": JOURNEY_ID,
            "journey_version": JOURNEY_VERSION,
            "journey_version_id": JOURNEY_VERSION_ID,
            "subject_hash": self.subject_hash,
            "scope_kind": "device",
            "screen_id": screen_id or self.progress["current_screen_id"],
            "occurred_at": _now(),
            "evidence_revision": self.progress["evidence_revision"],
            "experiment_id": assignment.get("experiment_id"),
            "variant_id": assignment.get("variant_id", "control"),
            "selected_next_screen_id": next_screen_id,
            "reason_code": reason_code,
            "properties": dict(properties or {}),
            "answers": [],
        }

    def _queue(self, name: str, **kwargs: Any) -> None:
        self.state["pending_events"].append(self._event(name, **kwargs))

    def flush_events(self) -> None:
        while self.state["pending_events"]:
            try:
                self.transport.collect(self.state["pending_events"][0])
            except RuntimeError:
                return
            self.state["pending_events"].pop(0)
            self._save()

    def resume(self) -> None:
        if self.progress["status"] == "in_progress":
            self.progress["evidence_revision"] = _now()
            self._queue("onboarding_resumed")
            self._save()
            self.flush_events()

    def expose(self) -> None:
        if self.progress["status"] == "in_progress":
            self.progress["evidence_revision"] = _now()
            self._queue("onboarding_step_viewed")
            self._save()
            self.flush_events()

    def observe_step(self, screen_id: str, evidence: Mapping[str, Any]) -> bool:
        if self.progress["status"] != "in_progress" or self.progress["current_screen_id"] != screen_id:
            return False
        index = _SCREEN_ORDER.index(screen_id)
        self.state["evidence"].update(evidence)
        self.progress["evidence_revision"] = _now()
        if screen_id not in self.progress["completed_screen_ids"]:
            self.progress["completed_screen_ids"].append(screen_id)
        if index + 1 < len(_SCREEN_ORDER):
            next_screen = _SCREEN_ORDER[index + 1]
            self._queue(
                "onboarding_step_completed",
                properties=evidence,
                screen_id=screen_id,
                next_screen_id=next_screen,
                reason_code="canonical_progression",
            )
            self.progress["current_screen_id"] = next_screen
            self._queue("onboarding_step_viewed", screen_id=next_screen)
        else:
            if self.state["evidence"].get(FIRST_SUCCESS_FACT) is not True:
                return False
            self.progress["status"] = "completed"
            self._queue("onboarding_step_completed", properties=evidence, screen_id=screen_id)
            self._queue("onboarding_first_success_observed", properties=evidence, screen_id=screen_id)
            self._queue("onboarding_completed", properties=evidence, screen_id=screen_id)
        self._save()
        self.flush_events()
        return True

    def set_terminal(self, status: str, event: str) -> None:
        if self.progress["status"] == "in_progress":
            self.progress["status"] = status
            self.progress["evidence_revision"] = _now()
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
        progress = self.progress
        screen = next(item for item in FALLBACK_DEFINITION["screens"] if item["screen_id"] == progress["current_screen_id"])
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
            "action": _SCREEN_ACTIONS[progress["current_screen_id"]],
            "presentation": screen["presentation"],
            "evidence": dict(self.state["evidence"]),
        }


def _status_json(status: BudgetStatus) -> dict[str, Any]:
    return {
        "category": status.category,
        "allocated_usd": status.allocated_usd,
        "spent_usd": status.spent_usd,
        "remaining_usd": status.remaining_usd,
        "utilization_pct": status.utilization_pct,
        "is_over_budget": status.is_over_budget,
        "period": status.period,
        "starts_at": status.starts_at,
    }


def _valid_record(record: Any) -> bool:
    return (
        isinstance(record, CostRecord)
        and isinstance(record.agent_id, str)
        and bool(record.agent_id)
        and isinstance(record.service, str)
        and bool(record.service)
        and record.usage_type in {"solves", "tokens", "bytes", "seconds", "units", "emails"}
        and type(record.usage_amount) in (int, float)
        and math.isfinite(record.usage_amount)
        and record.usage_amount > 0
        and type(record.cost_usd) in (int, float)
        and math.isfinite(record.cost_usd)
        and record.cost_usd >= 0
        and isinstance(record.created_at, str)
        and bool(record.created_at)
    )

def _record_fingerprint(record: CostRecord) -> str:
    material = {
        "agent_id": record.agent_id,
        "service": record.service,
        "resource": record.resource,
        "usage_type": record.usage_type,
        "usage_amount": record.usage_amount,
        "cost_usd": record.cost_usd,
        "reference_id": record.reference_id,
        "created_at": record.created_at,
    }
    canonical = json.dumps(material, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


def observe_budget_status(statuses: Sequence[BudgetStatus]) -> bool:
    """Advance only after real budget status rows were returned."""
    if not statuses or any(not isinstance(status, BudgetStatus) for status in statuses):
        return False
    try:
        runtime = _Runtime()
        runtime.open(start=True)
        return runtime.observe_step(
            "review-budget",
            {"budget_status_rows": len(statuses), "categories": sorted({status.category for status in statuses})},
        )
    except (OSError, RuntimeError, TypeError, ValueError):
        return False


def observe_accepted_usage(records: Sequence[CostRecord]) -> bool:
    """Advance only after a sink write accepted a nonempty validated batch."""
    if not records or any(not _valid_record(record) for record in records):
        return False
    try:
        runtime = _Runtime()
        runtime.open(start=True)
        amount = sum(record.usage_amount for record in records)
        cost = sum(record.cost_usd for record in records)
        return runtime.observe_step(
            "record-usage",
            {
                "accepted_record_count": len(records),
                "usage_amount": amount,
                "cost_usd": cost,
                "services": sorted({record.service for record in records}),
                "accepted_record_fingerprints": [_record_fingerprint(record) for record in records],
            },
        )
    except (OSError, RuntimeError, TypeError, ValueError):
        return False


def observe_budget_decision(record: CostRecord, decision: BudgetDecision) -> bool:
    """Complete only for a decision derived from accepted, matching usage."""
    if (
        not _valid_record(record)
        or not isinstance(decision, BudgetDecision)
        or decision.decision not in {"allow", "deny"}
        or decision.records_considered < 1
        or not decision.statuses
        or record.agent_id is None
        or not (decision.category == "all" or record.service.startswith(f"{decision.category}_"))
    ):
        return False
    try:
        runtime = _Runtime()
        runtime.open(start=True)
        accepted = runtime.state["evidence"].get("accepted_record_fingerprints", [])
        if _record_fingerprint(record) not in accepted:
            return False
        evidence = {
            FIRST_SUCCESS_FACT: True,
            "category": decision.category,
            "decision": decision.decision,
            "is_over_budget": decision.is_over_budget,
            "remaining_usd": decision.remaining_usd,
            "records_considered": decision.records_considered,
            "service": record.service,
        }
        return runtime.observe_step("make-budget-decision", evidence)
    except (OSError, RuntimeError, TypeError, ValueError):
        return False


def run_onboarding_action(action: str = "show") -> dict[str, Any]:
    allowed = {"show", "status", "skip", "abandon", "reset", "run"}
    if action not in allowed:
        raise OnboardingError("unknown_action", f"unknown onboarding action {action!r}")
    if action == "run":
        return _run_real_workflow()
    runtime = _Runtime()
    if not runtime.open(start=action != "status"):
        return {
            "product_id": PRODUCT_ID,
            "journey_id": JOURNEY_ID,
            "journey_version": JOURNEY_VERSION,
            "journey_version_id": JOURNEY_VERSION_ID,
            "source_revision": SOURCE_REVISION,
            "first_success_fact": FIRST_SUCCESS_FACT,
            "status": "not_started",
        }
    if action == "reset":
        runtime.reset()
    elif action == "skip":
        runtime.set_terminal("skipped", "onboarding_step_skipped")
    elif action == "abandon":
        runtime.set_terminal("abandoned", "onboarding_abandoned")
    elif action == "show" and runtime.was_existing:
        runtime.resume()
        runtime.expose()
    return runtime.view()


def _run_real_workflow() -> dict[str, Any]:
    from .budget import BudgetManager
    from .tracker import CostTracker, CostTrackerOptions

    tracker = CostTracker(CostTrackerOptions(agent_id="first-use", auto_flush=False))
    manager = BudgetManager("first-use", sink=tracker.get_sink())
    starts_at = datetime.now(timezone.utc)
    manager.set_budget("all", 1.0, "daily", starts_at)
    baseline = manager.get_status("all")
    record = tracker.record(
        service="llm_openai",
        resource="first-use-example",
        usage_type="tokens",
        usage_amount=100,
        cost_usd=0.01,
        reference_id=f"first-use-{uuid.uuid4()}",
        metadata={"journey_id": JOURNEY_ID},
    )
    tracker.flush()
    decision = manager.decide("all")
    runtime = _Runtime()
    runtime.open(start=False)
    return {
        "usage_accepted": True,
        "usage": record.to_json(),
        "baseline_budget_status": [_status_json(status) for status in baseline],
        "budget_decision": decision.to_json(),
        "onboarding": runtime.view(),
    }


def main(argv: Sequence[str] | None = None) -> int:
    arguments = list(argv if argv is not None else sys.argv[1:])
    action = arguments[0] if arguments else "show"
    try:
        result = run_onboarding_action(action)
    except OnboardingError as error:
        print(json.dumps({"ok": False, "error": {"code": error.code, "message": str(error)}}))
        return 2
    except (OSError, RuntimeError, TypeError, ValueError) as error:
        print(json.dumps({"ok": False, "error": {"code": "operation_failed", "message": str(error)}}))
        return 1
    print(json.dumps({"ok": True, "result": result}, sort_keys=True, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
