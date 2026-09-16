"""What wisent-cost-tracker published as its first-use journey.

This module is the product's side of the contract: the identity of the
journey, the definition as it was published, the check that anything claiming
to be that definition really is it, and where this machine keeps its state.
Nothing here talks to a network or holds an attempt - the control plane is in
engine/control_plane.py and the attempt is in engine/runtime.py.

The definition string below is byte-for-byte the one in
wisent-supabase-echo/supabase/seeds/onboarding_09_e.sql, because its digest is
part of what every reader validates. Reformat it and every machine's stored
bundle stops matching.
"""

from __future__ import annotations

import hashlib
import json
import os
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Mapping

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
SHIPPED_DEFINITION: dict[str, Any] = json.loads(CANONICAL_DEFINITION)

SCREEN_ORDER = ("review-budget", "record-usage", "make-budget-decision")
SCREEN_ACTIONS = {
    "review-budget": "budget_status",
    "record-usage": "record_and_flush_usage",
    "make-budget-decision": "decide_budget",
}


class OnboardingError(RuntimeError):
    """Stable machine-readable first-use error."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


def now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def state_path() -> Path:
    configured = os.environ.get("WISENT_COST_TRACKER_ONBOARDING_STATE_PATH")
    if configured:
        return Path(configured)
    root = Path(os.environ.get("XDG_STATE_HOME", Path.home() / ".local" / "state"))
    return root / PRODUCT_ID / "onboarding.json"


def shipped_bundle() -> dict[str, Any]:
    """The definition packaged with this release, in envelope form.

    Used when the control plane cannot be reached and this machine has no
    validated copy of its own. It passes the same check as a bundle that
    arrived over the network.
    """
    return {
        "journey_version_id": JOURNEY_VERSION_ID,
        "definition": SHIPPED_DEFINITION,
        "canonical_definition": CANONICAL_DEFINITION,
        "content_sha256": hashlib.sha256(CANONICAL_DEFINITION.encode("utf-8")).hexdigest(),
        "source_revision": SOURCE_REVISION,
    }


def validate_bundle(value: Any) -> dict[str, Any]:
    if not isinstance(value, Mapping):
        raise ValueError("bundle envelope must be an object")
    bundle = dict(value)
    canonical = bundle.get("canonical_definition")
    definition = bundle.get("definition")
    if (
        bundle.get("journey_version_id") != JOURNEY_VERSION_ID
        or bundle.get("source_revision") != SOURCE_REVISION
        or canonical != CANONICAL_DEFINITION
        or definition != SHIPPED_DEFINITION
        or bundle.get("content_sha256") != hashlib.sha256(CANONICAL_DEFINITION.encode("utf-8")).hexdigest()
    ):
        raise ValueError("bundle does not match the product-owned canonical definition")
    return bundle

