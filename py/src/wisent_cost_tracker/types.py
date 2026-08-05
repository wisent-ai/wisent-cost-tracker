"""Type definitions mirroring @wisent/cost-tracker."""

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
