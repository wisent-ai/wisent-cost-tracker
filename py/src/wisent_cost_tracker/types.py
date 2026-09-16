"""Type definitions mirroring @wisent/cost-tracker, and what they answer.

Three functions live beside the dataclasses because they are questions about
a record or a status rather than about any one caller: whether a record is
usable, what identifies it, and what a status looks like as JSON. The
first-use observations and the real-workflow demonstration both ask them.
"""

import hashlib
import json
import math
from dataclasses import dataclass, field, asdict
from datetime import datetime
from typing import Any, Dict, Literal, Optional

UsageType = Literal["solves", "tokens", "bytes", "seconds", "units", "emails"]
BudgetPeriod = Literal["daily", "weekly", "monthly"]


@dataclass
class CostRecord:
    service: str
    usage_type: UsageType
    usage_amount: float
    cost_usd: float
    resource: Optional[str] = None
    reference_id: Optional[str] = None
    metadata: Dict[str, Any] = field(default_factory=dict)
    created_at: Optional[str] = None
    agent_id: Optional[str] = None

    def to_json(self) -> Dict[str, Any]:
        out = asdict(self)
        # Drop nulls so PostgREST doesn't reject NOT NULL columns
        return {k: v for k, v in out.items() if v is not None}


@dataclass
class BudgetStatus:
    category: str
    allocated_usd: float
    spent_usd: float
    remaining_usd: float
    utilization_pct: float
    is_over_budget: bool
    period: BudgetPeriod
    starts_at: str


@dataclass
class BudgetDecision:
    category: str
    decision: Literal["allow", "deny"]
    is_over_budget: bool
    remaining_usd: float
    records_considered: int
    statuses: list[BudgetStatus]

    def to_json(self) -> Dict[str, Any]:
        return asdict(self)


def is_valid_record(record: Any) -> bool:
    """Whether this is a cost record a sink could accept.

    Usage has to be a finite positive number and cost a finite nonnegative
    one, so a NaN or an infinity - which JSON cannot carry and a database
    would refuse - never reaches a sink or counts towards first use.
    """
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


def record_fingerprint(record: CostRecord) -> str:
    """What identifies one accepted record, independently of its metadata."""
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


def status_json(status: BudgetStatus) -> Dict[str, Any]:
    """One budget status as JSON, in the order a reader expects to see it."""
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
