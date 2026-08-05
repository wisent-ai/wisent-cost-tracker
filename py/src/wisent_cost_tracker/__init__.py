"""Per-agent cost tracking + budget enforcement.

Public API mirrors @wisent/cost-tracker (npm).
"""

from .tracker import CostTracker, CostTrackerOptions
from .budget import BudgetManager
from .pricing import (
    PRICES,
    captcha_price,
    sms_price,
    proxy_cost_for_bytes,
    llm_cost,
    compute_cost,
    load_pricing,
)
from .sinks import MemorySink, FileSink, SupabaseSink
from .types import BudgetDecision, BudgetPeriod, BudgetStatus, CostRecord
from .onboarding import (
    FIRST_SUCCESS_FACT,
    JOURNEY_VERSION,
    JOURNEY_VERSION_ID,
    run_onboarding_action,
)

__all__ = [
    "CostTracker",
    "CostTrackerOptions",
    "BudgetManager",
    "PRICES",
    "captcha_price",
    "sms_price",
    "proxy_cost_for_bytes",
    "llm_cost",
    "compute_cost",
    "load_pricing",
    "MemorySink",
    "FileSink",
    "SupabaseSink",
    "CostRecord",
    "BudgetStatus",
    "BudgetPeriod",
    "BudgetDecision",
    "FIRST_SUCCESS_FACT",
    "JOURNEY_VERSION",
    "JOURNEY_VERSION_ID",
    "run_onboarding_action",
]
