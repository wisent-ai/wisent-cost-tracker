"""BudgetManager — Python mirror of @wisent/cost-tracker BudgetManager."""

from datetime import datetime, timezone
import math
from typing import List, Optional

from .sinks import CostSink, SupabaseSink
from .types import BudgetDecision, BudgetPeriod, BudgetStatus, CostRecord


class BudgetManager:
    def __init__(
        self,
        agent_id: str,
        sink: Optional[CostSink] = None,
        supabase_url: Optional[str] = None,
        supabase_key: Optional[str] = None,
    ) -> None:
        if not isinstance(agent_id, str) or not agent_id.strip():
            raise ValueError("BudgetManager: agent_id must be a non-empty string")
        self.agent_id = agent_id
        if sink is not None:
            self.sink = sink
        elif supabase_url and supabase_key:
            self.sink = SupabaseSink(supabase_url, supabase_key)
        else:
            raise ValueError("BudgetManager: provide either sink= or supabase_url + supabase_key")

    def set_budget(
        self,
        category: str,
        allocated_usd: float,
        period: BudgetPeriod,
        starts_at: Optional[datetime] = None,
    ) -> None:
        if not isinstance(category, str) or not category:
            raise ValueError("BudgetManager: category must be a non-empty string")
        if type(allocated_usd) not in (int, float) or not math.isfinite(allocated_usd) or allocated_usd < 0:
            raise ValueError("BudgetManager: allocated_usd must be a finite non-negative number")
        if period not in {"daily", "weekly", "monthly"}:
            raise ValueError(f"BudgetManager: unsupported period {period!r}")
        if not hasattr(self.sink, "write_budget"):
            raise RuntimeError("BudgetManager: sink does not support write_budget")
        self.sink.write_budget(  # type: ignore[attr-defined]
            self.agent_id,
            category,
            allocated_usd,
            period,
            starts_at or datetime.utcnow(),
        )

    def get_status(self, category: Optional[str] = None) -> List[BudgetStatus]:
        if not hasattr(self.sink, "read_budgets"):
            raise RuntimeError("BudgetManager: sink does not support read_budgets")
        rows = [
            row for row in self.sink.read_budgets(self.agent_id)  # type: ignore[attr-defined]
            if category is None or row.category == category
        ]
        if rows:
            try:
                from .onboarding import observe_budget_status

                observe_budget_status(rows)
            except (OSError, TypeError, ValueError):
                # First-use telemetry must never alter a real status read.
                pass
        return rows

    def is_over_budget(self, category: str) -> bool:
        return any(b.is_over_budget for b in self.get_status(category))

    def remaining(self, category: str) -> float:
        rows = self.get_status(category)
        if not rows:
            return float("inf")
        return sum(b.remaining_usd for b in rows)

    def decide(self, category: str) -> BudgetDecision:
        """Return a budget decision only after the sink accepted matching usage."""
        if not isinstance(category, str) or not category:
            raise ValueError("BudgetManager: category must be a non-empty string")
        statuses = self.get_status(category)
        if not statuses:
            raise RuntimeError("BudgetManager: no budget is configured for this category")
        if not hasattr(self.sink, "read"):
            raise RuntimeError("BudgetManager: sink cannot confirm accepted usage")
        starts_at = min(_parse_timestamp(status.starts_at) for status in statuses)
        records = self.sink.read(self.agent_id, starts_at)  # type: ignore[attr-defined]
        matching = [
            record for record in records
            if _valid_accepted_record(record, self.agent_id)
            and _matches_category(record, category)
        ]
        if not matching:
            raise RuntimeError("BudgetManager: no accepted usage exists for this budget decision")
        is_over_budget = any(status.is_over_budget for status in statuses)
        decision = BudgetDecision(
            category=category,
            decision="deny" if is_over_budget else "allow",
            is_over_budget=is_over_budget,
            remaining_usd=sum(status.remaining_usd for status in statuses),
            records_considered=len(matching),
            statuses=statuses,
        )
        try:
            from .onboarding import observe_budget_decision

            for record in reversed(matching):
                if observe_budget_decision(record, decision):
                    break
        except (OSError, TypeError, ValueError):
            # First-use telemetry must never alter a real budget decision.
            pass
        return decision


def _parse_timestamp(value: str) -> datetime:
    parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return parsed


def _matches_category(record: CostRecord, category: str) -> bool:
    return category == "all" or record.service.startswith(f"{category}_")


def _valid_accepted_record(record: CostRecord, agent_id: str) -> bool:
    return (
        isinstance(record, CostRecord)
        and record.agent_id == agent_id
        and isinstance(record.service, str)
        and bool(record.service)
        and isinstance(record.usage_amount, (int, float))
        and math.isfinite(record.usage_amount)
        and type(record.usage_amount) in (int, float)
        and isinstance(record.cost_usd, (int, float))
        and math.isfinite(record.cost_usd)
        and type(record.cost_usd) in (int, float)
        and isinstance(record.created_at, str)
        and bool(record.created_at)
    )
