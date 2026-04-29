"""CostSink implementations: in-memory, file, Supabase REST."""

import json
from datetime import datetime
from pathlib import Path
from typing import Any, Dict, List, Optional, Protocol

from .types import BudgetPeriod, BudgetStatus, CostRecord


class CostSink(Protocol):
    def write(self, records: List[CostRecord]) -> None: ...
    def read(self, agent_id: str, since: datetime) -> List[CostRecord]: ...
    def read_budgets(self, agent_id: str) -> List[BudgetStatus]: ...
    def write_budget(self, agent_id: str, category: str, allocated_usd: float, period: BudgetPeriod, starts_at: datetime) -> None: ...


class MemorySink:
    def __init__(self) -> None:
        self.records: List[CostRecord] = []

    def write(self, records: List[CostRecord]) -> None:
        self.records.extend(records)

    def read(self, agent_id: str, since: datetime) -> List[CostRecord]:
        return [r for r in self.records if r.agent_id == agent_id]


class FileSink:
    def __init__(self, path: Path | str) -> None:
        self.path = Path(path)

    def write(self, records: List[CostRecord]) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        existing: List[Dict[str, Any]] = []
        if self.path.exists():
            try:
                existing = json.loads(self.path.read_text("utf-8"))
            except Exception:
                existing = []
        existing.extend(r.to_json() for r in records)
        self.path.write_text(json.dumps(existing, indent=2))

    def read(self, agent_id: str, since: datetime) -> List[CostRecord]:
        if not self.path.exists():
            return []
        try:
            raw = json.loads(self.path.read_text("utf-8"))
        except Exception:
            return []
        return [CostRecord(**r) for r in raw if r.get("agent_id") == agent_id]


class SupabaseSink:
    """Supabase REST sink. Uses httpx to keep the client simple."""

    def __init__(
        self,
        url: str,
        key: str,
        table: str = "cost_records",
        budget_table: str = "cost_budgets",
        view_name: str = "cost_budget_status",
    ) -> None:
        self.url = url.rstrip("/")
        self.key = key
        self.table = table
        self.budget_table = budget_table
        self.view_name = view_name
        # Lazy-import httpx so the dep is only required when SupabaseSink is used
        import httpx  # noqa: F401

    def _headers(self, prefer: str = "return=minimal") -> Dict[str, str]:
        return {
            "apikey": self.key,
            "Authorization": f"Bearer {self.key}",
            "Content-Type": "application/json",
            "Prefer": prefer,
        }

    def write(self, records: List[CostRecord]) -> None:
        import httpx
        if not records:
            return
        body = [r.to_json() for r in records]
        r = httpx.post(
            f"{self.url}/rest/v1/{self.table}",
            headers=self._headers(),
            content=json.dumps(body),
            timeout=15,
        )
        if r.status_code >= 400:
            raise RuntimeError(f"SupabaseSink write failed: {r.status_code} {r.text[:200]}")

    def read(self, agent_id: str, since: datetime) -> List[CostRecord]:
        import httpx
        url = (
            f"{self.url}/rest/v1/{self.table}"
            f"?agent_id=eq.{agent_id}"
            f"&created_at=gte.{since.isoformat()}"
            f"&select=*"
        )
        r = httpx.get(url, headers=self._headers("return=representation"), timeout=15)
        if r.status_code >= 400:
            raise RuntimeError(f"SupabaseSink read failed: {r.status_code}")
        return [CostRecord(**row) for row in r.json()]

    def read_budgets(self, agent_id: str) -> List[BudgetStatus]:
        import httpx
        url = f"{self.url}/rest/v1/{self.view_name}?agent_id=eq.{agent_id}&select=*"
        r = httpx.get(url, headers=self._headers("return=representation"), timeout=15)
        if r.status_code >= 400:
            raise RuntimeError(f"SupabaseSink read_budgets failed: {r.status_code}")
        return [
            BudgetStatus(
                category=row["category"],
                allocated_usd=float(row["allocated_usd"]),
                spent_usd=float(row["spent_usd"]),
                remaining_usd=float(row["remaining_usd"]),
                utilization_pct=float(row["utilization_pct"]),
                is_over_budget=bool(row["is_over_budget"]),
                period=row["period"],
                starts_at=row["starts_at"],
            )
            for row in r.json()
        ]

    def write_budget(
        self,
        agent_id: str,
        category: str,
        allocated_usd: float,
        period: BudgetPeriod,
        starts_at: datetime,
    ) -> None:
        import httpx
        url = f"{self.url}/rest/v1/{self.budget_table}?on_conflict=agent_id,category,period,starts_at"
        r = httpx.post(
            url,
            headers=self._headers("resolution=merge-duplicates,return=minimal"),
            content=json.dumps({
                "agent_id": agent_id,
                "category": category,
                "allocated_usd": allocated_usd,
                "period": period,
                "starts_at": starts_at.isoformat(),
            }),
            timeout=15,
        )
        if r.status_code >= 400:
            raise RuntimeError(f"SupabaseSink write_budget failed: {r.status_code} {r.text[:200]}")
