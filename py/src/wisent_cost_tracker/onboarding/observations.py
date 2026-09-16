"""The three points where real work advances the first-use journey.

Each function is called from the code that did the work - a budget read, an
accepted sink write, a budget decision - and each one checks the work before
it counts it. Nothing here starts a journey on its own claim: the last screen
only completes for a decision derived from usage this machine already had
accepted, matched by fingerprint.

A failure to record first use is never allowed to break the operation that
triggered it, which is why each function ends in False rather than an
exception.
"""

from __future__ import annotations

from typing import Sequence

from ..types import (
    BudgetDecision,
    BudgetStatus,
    CostRecord,
    is_valid_record,
    record_fingerprint,
)
from .definition import FIRST_SUCCESS_FACT
from .engine import Runtime


def observe_budget_status(statuses: Sequence[BudgetStatus]) -> bool:
    """Advance only after real budget status rows were returned."""
    if not statuses or any(not isinstance(status, BudgetStatus) for status in statuses):
        return False
    try:
        runtime = Runtime()
        runtime.open(start=True)
        return runtime.observe_step(
            "review-budget",
            {
                "budget_status_rows": len(statuses),
                "categories": sorted({status.category for status in statuses}),
            },
        )
    except (OSError, RuntimeError, TypeError, ValueError):
        return False


def observe_accepted_usage(records: Sequence[CostRecord]) -> bool:
    """Advance only after a sink write accepted a nonempty validated batch."""
    if not records or any(not is_valid_record(record) for record in records):
        return False
    try:
        runtime = Runtime()
        runtime.open(start=True)
        return runtime.observe_step(
            "record-usage",
            {
                "accepted_record_count": len(records),
                "usage_amount": sum(record.usage_amount for record in records),
                "cost_usd": sum(record.cost_usd for record in records),
                "services": sorted({record.service for record in records}),
                "accepted_record_fingerprints": [record_fingerprint(record) for record in records],
            },
        )
    except (OSError, RuntimeError, TypeError, ValueError):
        return False


def observe_budget_decision(record: CostRecord, decision: BudgetDecision) -> bool:
    """Complete only for a decision derived from accepted, matching usage."""
    if not _decision_matches_record(record, decision):
        return False
    try:
        runtime = Runtime()
        runtime.open(start=True)
        accepted = runtime.state["evidence"].get("accepted_record_fingerprints", [])
        if record_fingerprint(record) not in accepted:
            return False
        return runtime.observe_step(
            "make-budget-decision",
            {
                FIRST_SUCCESS_FACT: True,
                "category": decision.category,
                "decision": decision.decision,
                "is_over_budget": decision.is_over_budget,
                "remaining_usd": decision.remaining_usd,
                "records_considered": decision.records_considered,
                "service": record.service,
            },
        )
    except (OSError, RuntimeError, TypeError, ValueError):
        return False


def _decision_matches_record(record: CostRecord, decision: BudgetDecision) -> bool:
    """Whether this decision is one this record could have taken part in.

    A category decision only counts for a service inside that category, and a
    decision that considered no records or returned no statuses is not a
    budget decision at all.
    """
    return (
        is_valid_record(record)
        and isinstance(decision, BudgetDecision)
        and decision.decision in {"allow", "deny"}
        and decision.records_considered >= 1
        and bool(decision.statuses)
        and record.agent_id is not None
        and (decision.category == "all" or record.service.startswith(f"{decision.category}_"))
    )
