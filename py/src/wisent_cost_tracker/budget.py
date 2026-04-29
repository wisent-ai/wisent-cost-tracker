"""BudgetManager — Python mirror of @wisent/cost-tracker BudgetManager."""

from datetime import datetime
from typing import List, Optional

from .sinks import CostSink, SupabaseSink
from .types import BudgetPeriod, BudgetStatus


class BudgetManager:
    def __init__(
        self,
        agent_id: str,
        sink: Optional[CostSink] = None,
        supabase_url: Optional[str] = None,
        supabase_key: Optional[str] = None,
    ) -> None:
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
        rows = self.sink.read_budgets(self.agent_id)  # type: ignore[attr-defined]
        return [r for r in rows if (category is None or r.category == category)]

    def is_over_budget(self, category: str) -> bool:
        return any(b.is_over_budget for b in self.get_status(category))

    def remaining(self, category: str) -> float:
        rows = self.get_status(category)
        if not rows:
            return float("inf")
        return sum(b.remaining_usd for b in rows)
