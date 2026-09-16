"""What an operator can ask of the first-use journey, and the command line.

Six actions: show it, report its status without starting anything, skip it,
abandon it, reset it, or run the real workflow it describes. `run` is the one
that spends: it records usage through the real tracker and takes a real budget
decision, which is also how first use is demonstrated end to end.
"""

from __future__ import annotations

import json
import sys
import uuid
from datetime import datetime, timezone
from typing import Any, Sequence

from ..types import status_json
from .definition import (
    FIRST_SUCCESS_FACT,
    JOURNEY_ID,
    JOURNEY_VERSION,
    JOURNEY_VERSION_ID,
    PRODUCT_ID,
    SOURCE_REVISION,
    OnboardingError,
)
from .engine import Runtime

ACTIONS = ("show", "status", "skip", "abandon", "reset", "run")

#: What the command returns. A bad request and an operation that failed while
#: doing what it was asked are told apart by the status, not by the message.
EXIT_OK = 0
EXIT_OPERATION_FAILED = 1
EXIT_BAD_REQUEST = 2

#: The example the `run` action spends: a one dollar daily budget and a
#: hundred-token call priced at a cent, small enough to repeat for free and
#: large enough for the budget decision to be a real one.
EXAMPLE_BUDGET_USD = 1.0
EXAMPLE_USAGE_TOKENS = 100
EXAMPLE_COST_USD = 0.01


def run_onboarding_action(action: str = "show") -> dict[str, Any]:
    if action not in ACTIONS:
        raise OnboardingError("unknown_action", f"unknown onboarding action {action!r}")
    if action == "run":
        return _run_real_workflow()
    runtime = Runtime()
    if not runtime.open(start=action != "status"):
        return _not_started()
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


def _not_started() -> dict[str, Any]:
    """The answer to `status` on a machine that never started the journey."""
    return {
        "product_id": PRODUCT_ID,
        "journey_id": JOURNEY_ID,
        "journey_version": JOURNEY_VERSION,
        "journey_version_id": JOURNEY_VERSION_ID,
        "source_revision": SOURCE_REVISION,
        "first_success_fact": FIRST_SUCCESS_FACT,
        "status": "not_started",
    }


def _run_real_workflow() -> dict[str, Any]:
    """Do the work the journey is about, and report what it cost.

    A daily budget is set, one usage record is written through the tracker and
    flushed to its sink, and a decision is taken against the budget that now
    knows about it. The journey advances because those things happened, not
    because this function was called.
    """
    from ..spend.budget import BudgetManager
    from ..spend.tracker import CostTracker, CostTrackerOptions

    tracker = CostTracker(CostTrackerOptions(agent_id="first-use", auto_flush=False))
    manager = BudgetManager("first-use", sink=tracker.get_sink())
    manager.set_budget("all", EXAMPLE_BUDGET_USD, "daily", datetime.now(timezone.utc))
    baseline = manager.get_status("all")
    record = tracker.record(
        service="llm_openai",
        resource="first-use-example",
        usage_type="tokens",
        usage_amount=EXAMPLE_USAGE_TOKENS,
        cost_usd=EXAMPLE_COST_USD,
        reference_id=f"first-use-{uuid.uuid4()}",
        metadata={"journey_id": JOURNEY_ID},
    )
    tracker.flush()
    decision = manager.decide("all")
    runtime = Runtime()
    runtime.open(start=False)
    return {
        "usage_accepted": True,
        "usage": record.to_json(),
        "baseline_budget_status": [status_json(status) for status in baseline],
        "budget_decision": decision.to_json(),
        "onboarding": runtime.view(),
    }


def main(argv: Sequence[str] | None = None) -> int:
    """The wisent-cost-tracker-onboarding command.

    One JSON object on stdout either way: ok with the result, or ok false with
    a code and a message.
    """
    arguments = list(argv if argv is not None else sys.argv[1:])
    action = arguments[0] if arguments else "show"
    try:
        result = run_onboarding_action(action)
    except OnboardingError as error:
        print(json.dumps({"ok": False, "error": {"code": error.code, "message": str(error)}}))
        return EXIT_BAD_REQUEST
    except (OSError, RuntimeError, TypeError, ValueError) as error:
        print(json.dumps({"ok": False, "error": {"code": "operation_failed", "message": str(error)}}))
        return EXIT_OPERATION_FAILED
    print(json.dumps({"ok": True, "result": result}, sort_keys=True, separators=(",", ":")))
    return EXIT_OK
