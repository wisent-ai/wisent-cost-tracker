# wisent-cost-tracker

Shared per-agent cost tracking + budget enforcement for Wisent services.

Two thin clients (TypeScript + Python) reading the same canonical pricing
table at `pricing/costs.json`. Both clients persist usage records to a
shared Supabase backend (tables `cost_records`, `cost_budgets`) so a single
SQL view (`cost_budget_status`) shows utilization and overrun across
services and languages.

## Layout

```
pricing/costs.json   single source of truth for per-service pricing
src/                 @wisent/cost-tracker — TypeScript client (Node 20+)
py/                  wisent_cost_tracker  — Python 3.11+ client
supabase/            reference migrations (apply via wisent-supabase-* repos)
```

The Python package needs a copy of `pricing/costs.json` inside its package
tree (`py/src/wisent_cost_tracker/pricing/costs.json`) so `pip install
git+...#subdirectory=py` bundles it. Run `npm run sync-pricing` after every
edit to `pricing/costs.json` (or rely on the pre-commit hook below).

## Wiring

### TypeScript

```ts
import { CostTracker, BudgetManager } from '@wisent/cost-tracker';

const tracker = new CostTracker({
  agent_id: process.env.ACTION_LOG_ID,
  sink: 'supabase',
});

tracker.recordCaptcha('capsolver', 'recaptcha_v2');
tracker.recordSms('juicysms', 'reddit');
tracker.recordProxyBytes('oxylabs', 250 * 1024 * 1024);
await tracker.flush();
```

### Python

```python
from wisent_cost_tracker import CostTracker, BudgetManager

tracker = CostTracker(agent_id="trading-agent-X", sink="supabase")
tracker.record_llm("gemini-2.0-flash", input_tokens=8500, output_tokens=1200)
tracker.flush()

budget = BudgetManager(agent_id="trading-agent-X")
if budget.is_over_budget("llm"):
    raise RuntimeError("LLM budget exhausted")
```

## Schema (Supabase)

Apply `supabase/migrations/20260429000001_cost_tracking.sql` to whatever
project hosts the cross-service view (currently content-platform
`yqizdfkfnmhddfemdxtq`). Migration must be PR'd through the dedicated
`wisent-supabase-*` repos per Wisent governance — do not run it directly
against the DB.

## Pricing source

`pricing/costs.json` is human-edited from each provider's public pricing
page snapshots. When a provider's actual billing API returns a different
price (JuicySMS does this), pass `override_usd` to the record call to
preserve accuracy.
