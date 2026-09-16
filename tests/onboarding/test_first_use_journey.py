"""The first-use journey, run for real from the repository root with `pytest`.

Every test here drives the published package interface - the three
observation points, the action dispatch and the command line - against a
state file in this repository's ignored build directory. The control plane is
genuinely unreachable, because no STADO_INTEGRATION_API_URL is set, which is
the case an SDK on a developer's machine is in most of the time: the journey
still has to happen, and the events still have to queue.

Nothing is stubbed. The budget, the tracker and its memory sink are the real
ones, and what the tests read back is the state file the product wrote.
"""

from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path

import pytest

from wisent_cost_tracker import (
    BudgetDecision,
    BudgetManager,
    BudgetStatus,
    CostRecord,
    CostTracker,
    CostTrackerOptions,
)
from wisent_cost_tracker.onboarding import (
    OnboardingError,
    observe_accepted_usage,
    observe_budget_decision,
    observe_budget_status,
    run_onboarding_action,
)
from wisent_cost_tracker.onboarding.definition import SCREEN_ORDER

BUILD_ROOT = Path(__file__).resolve().parents[2] / "build" / "first-use-tests"


@pytest.fixture
def journey_state(request, monkeypatch) -> Path:
    """A state path of this test's own, and no control plane to reach.

    The path is inside the repository's ignored build directory, named after
    the test, so a failing run leaves the state file it failed on and the next
    run does not inherit it.
    """
    BUILD_ROOT.mkdir(parents=True, exist_ok=True)
    path = BUILD_ROOT / f"{request.node.name}.json"
    if path.exists():
        path.unlink()
    monkeypatch.setenv("WISENT_COST_TRACKER_ONBOARDING_STATE_PATH", str(path))
    monkeypatch.delenv("STADO_INTEGRATION_API_URL", raising=False)
    monkeypatch.delenv("WISENT_COST_TRACKER_STADO_INTEGRATION_TOKEN", raising=False)
    return path


def _status(category: str = "all") -> BudgetStatus:
    return BudgetStatus(
        category=category,
        allocated_usd=1.0,
        spent_usd=0.25,
        remaining_usd=0.75,
        utilization_pct=25.0,
        is_over_budget=False,
        period="daily",
        starts_at=datetime.now(timezone.utc).isoformat(),
    )


def _record(service: str = "llm_openai") -> CostRecord:
    return CostRecord(
        service=service,
        usage_type="tokens",
        usage_amount=100,
        cost_usd=0.01,
        resource="test-resource",
        reference_id="test-reference",
        created_at=datetime.now(timezone.utc).isoformat(),
        agent_id="test-agent",
    )


def _decision(statuses: list[BudgetStatus]) -> BudgetDecision:
    return BudgetDecision(
        category="all",
        decision="allow",
        is_over_budget=False,
        remaining_usd=0.75,
        records_considered=1,
        statuses=statuses,
    )


def test_status_before_anything_started(journey_state: Path) -> None:
    result = run_onboarding_action("status")

    assert result["status"] == "not_started"
    assert result["journey_id"] == "first-use"
    assert not journey_state.exists()


def test_three_observations_complete_the_journey(journey_state: Path) -> None:
    statuses = [_status()]
    record = _record()

    assert observe_budget_status(statuses) is True
    after_first = json.loads(journey_state.read_text("utf-8"))
    assert after_first["progress"]["current_screen_id"] == SCREEN_ORDER[1]

    assert observe_accepted_usage([record]) is True
    after_second = json.loads(journey_state.read_text("utf-8"))
    assert after_second["progress"]["current_screen_id"] == SCREEN_ORDER[2]

    assert observe_budget_decision(record, _decision(statuses)) is True
    after_third = json.loads(journey_state.read_text("utf-8"))
    assert after_third["progress"]["status"] == "completed"
    assert after_third["evidence"]["budget_decision_observed"] is True
    assert run_onboarding_action("show")["status"] == "completed"


def test_events_queue_while_the_control_plane_is_unreachable(journey_state: Path) -> None:
    assert observe_budget_status([_status()]) is True

    queued = json.loads(journey_state.read_text("utf-8"))["pending_events"]
    names = [event["event_name"] for event in queued]

    assert names[:2] == ["onboarding_started", "onboarding_step_viewed"]
    assert "onboarding_step_completed" in names
    assert all(event["product_id"] == "wisent-cost-tracker" for event in queued)


def test_decision_without_accepted_usage_is_refused(journey_state: Path) -> None:
    statuses = [_status()]
    assert observe_budget_status(statuses) is True
    assert observe_accepted_usage([_record()]) is True

    other_record = _record(service="llm_anthropic")

    assert observe_budget_decision(other_record, _decision(statuses)) is False
    assert json.loads(journey_state.read_text("utf-8"))["progress"]["status"] == "in_progress"


def test_usage_that_no_sink_would_accept_is_refused(journey_state: Path) -> None:
    unusable = CostRecord(
        service="llm_openai",
        usage_type="tokens",
        usage_amount=float("inf"),
        cost_usd=0.01,
        created_at=datetime.now(timezone.utc).isoformat(),
        agent_id="test-agent",
    )

    assert observe_accepted_usage([unusable]) is False
    assert not journey_state.exists()


def test_unknown_action_is_named(journey_state: Path) -> None:
    with pytest.raises(OnboardingError) as raised:
        run_onboarding_action("teleport")

    assert raised.value.code == "unknown_action"
    assert "teleport" in str(raised.value)


def test_reset_starts_a_new_attempt(journey_state: Path) -> None:
    assert observe_budget_status([_status()]) is True
    first_attempt = json.loads(journey_state.read_text("utf-8"))["progress"]["attempt_id"]

    after_reset = run_onboarding_action("reset")

    assert after_reset["status"] == "in_progress"
    assert after_reset["current_screen_id"] == SCREEN_ORDER[0]
    assert after_reset["attempt_id"] != first_attempt
    assert after_reset["evidence"] == {}


def test_run_action_spends_through_the_real_tracker(journey_state: Path) -> None:
    result = run_onboarding_action("run")

    assert result["usage_accepted"] is True
    assert result["usage"]["service"] == "llm_openai"
    assert result["budget_decision"]["decision"] in {"allow", "deny"}
    assert result["onboarding"]["status"] in {"in_progress", "completed"}


def test_budget_read_records_first_use_through_the_manager(journey_state: Path) -> None:
    """The deferred imports inside the spend package have to resolve.

    BudgetManager and CostTracker import the observation points inside the
    methods that use them, and those imports only run when a status is read or
    a batch is flushed. A broken relative import there is invisible to any
    test that merely imports the package.
    """
    tracker = CostTracker(CostTrackerOptions(agent_id="first-use-test", auto_flush=False))
    manager = BudgetManager("first-use-test", sink=tracker.get_sink())
    manager.set_budget("all", 1.0, "daily", datetime.now(timezone.utc))

    statuses = manager.get_status("all")

    assert statuses
    state = json.loads(journey_state.read_text("utf-8"))
    assert state["progress"]["current_screen_id"] == SCREEN_ORDER[1]
    assert state["evidence"]["budget_status_rows"] == len(statuses)


def test_command_line_reports_status_as_json(journey_state: Path, capsys) -> None:
    from wisent_cost_tracker.onboarding.actions import EXIT_BAD_REQUEST, EXIT_OK, main

    assert main(["status"]) == EXIT_OK
    reported = json.loads(capsys.readouterr().out)
    assert reported["ok"] is True
    assert reported["result"]["status"] == "not_started"

    assert main(["teleport"]) == EXIT_BAD_REQUEST
    refused = json.loads(capsys.readouterr().out)
    assert refused["ok"] is False
    assert refused["error"]["code"] == "unknown_action"
