<!-- wisent-banner:start -->
<p align="center">
  <img src="assets/readme-banner.webp" alt="wisent-cost-tracker by Wisent" width="100%">
</p>
<!-- wisent-banner:end -->

<!-- wisent-readme-signals:start -->
[![Source](https://img.shields.io/badge/GitHub-Source-181717?logo=github)](https://github.com/wisent-ai/wisent-cost-tracker) [![Issues](https://img.shields.io/badge/GitHub-Issues-181717?logo=github)](https://github.com/wisent-ai/wisent-cost-tracker/issues) [![Wisent](https://img.shields.io/badge/Wisent-Website-0B0B0B)](https://wisent.com) [![Discord](https://img.shields.io/badge/Discord-Join-5865F2?logo=discord&logoColor=white)](https://discord.gg/qRjpkthq54) [![LinkedIn](https://img.shields.io/badge/LinkedIn-Follow-0A66C2?logo=linkedin&logoColor=white)](https://www.linkedin.com/company/wisent-ai/) [![X](https://img.shields.io/badge/X-Follow-000000?logo=x&logoColor=white)](https://x.com/wisentai) [![Enterprise](https://img.shields.io/badge/Enterprise-Book%20a%20call-0B0B0B?logo=calendly)](https://calendly.com/lbartoszcze)
<!-- wisent-readme-signals:end -->

# wisent-cost-tracker

Know What Every Agent Costs Before the Invoice Does.

An agent that runs all night is either the cheapest colleague you have or a bill
nobody approved, and today you find out at the end of the month. Wisent Cost
Tracker records what each agent spends while it spends it, from TypeScript and
from Python, against one canonical pricing table both clients read. The budget
manager returns spend status; the calling application uses that status to refuse
further work. Usage lands as records you can total per agent, per service, per day.

Per-Agent Spend, Enforced.

Two thin clients (TypeScript + Python) reading the same canonical pricing
table at `pricing/costs.json`. Both clients persist usage records to a
shared Supabase backend (tables `cost_records`, `cost_budgets`) so a single
SQL view (`cost_budget_status`) shows utilization and overrun across
services and languages.

## Layout

```
pricing/costs.json   single source of truth for per-service pricing
src/                 @wisent/cost-tracker — TypeScript client
py/                  wisent_cost_tracker — Python API and Rust extension sources
supabase/            reference migrations (apply via wisent-supabase-* repos)
```

The Python package needs a copy of `pricing/costs.json` inside its package
tree (`py/src/wisent_cost_tracker/pricing/costs.json`). Run
`npm run sync-pricing` after every edit to the canonical table. The
`sync-pricing` script copies the file; setuptools includes that copy rather
than generating it.

## Wiring

### TypeScript

```ts
import { CostTracker, BudgetManager } from '@wisent/cost-tracker';

export async function trackUsage(
  agentId: string,
  supabase: { url: string; key: string },
  service: string,
  usageAmount: number,
  billedUsd: number,
) {
  const tracker = new CostTracker({ agent_id: agentId, sink: 'supabase', supabase });
  const budget = new BudgetManager({ agent_id: agentId, sink: tracker.getSink() });
  tracker.record({
    service,
    usage_type: 'units',
    usage_amount: usageAmount,
    cost_usd: billedUsd,
  });
  await tracker.flush();
  const statuses = await budget.getStatus('all');
  return {
    spend: tracker.snapshot(),
    canStartNextOperation: statuses.length > 0 && !statuses.some(status => status.is_over_budget),
  };
}
```

Pass the endpoint and credential supplied to the application into `supabase`.
The tracker does not resolve them from `sink: 'supabase'` alone. Credentials
must not be placed in command-line arguments or committed examples.

### Python

```python
from wisent_cost_tracker import CostTracker, BudgetManager

def track_usage(agent_id, supabase_url, supabase_key, service, usage_amount, billed_usd):
    tracker = CostTracker(
        agent_id=agent_id,
        sink="supabase",
        supabase_url=supabase_url,
        supabase_key=supabase_key,
    )
    budget = BudgetManager(agent_id=agent_id, sink=tracker.get_sink())
    tracker.record(
        service=service,
        usage_type="units",
        usage_amount=usage_amount,
        cost_usd=billed_usd,
    )
    tracker.flush()
    statuses = budget.get_status("all")
    return {
        "spend": tracker.snapshot(),
        "can_start_next_operation": bool(statuses) and not any(
            status.is_over_budget for status in statuses
        ),
    }
```

The examples always record an amount already obtained from billing, even if the
budget is exhausted. Their returned flag is for the application's next paid
operation. Check the budget before starting that operation, not before recording
an expense that already happened. This check reads current spend; it does not
reserve funds. Concurrent callers can consume the same remaining budget, and
recording usage does not itself prevent a provider from charging.

### Native Python modules

`py/pyproject.toml` declares the Python requirement and two PyO3 extensions:
`wisent_cost_tracker.spend` from `py/native/`, and
`wisent_cost_tracker.onboarding.engine.control_plane` from `py/control-plane/`.
The public Python imports remain available; this is not a standalone Rust CLI.
Pricing, record types and parts of onboarding still have Python source, so the
presence of the native modules does not mean the whole package has moved to Rust.

Building the package requires Rust, Cargo and the Python build requirements
declared in that manifest. From the repository root:

```sh
npm run sync-pricing
pip install ./py
```

This command builds and installs the extensions. A source checkout without built
extensions is not an installed Python package.

### Persistence and errors

`record` appends usage to the tracker. `flush` publishes records to the selected
sink. An acknowledged record is not sent again by a later flush on the same
tracker. Totals and snapshots continue to include acknowledged records.
A failed write remains pending. A lost acknowledgement can still leave a record
on the server: this is not an exactly-once delivery guarantee.

The clients do not round recorded costs, totals or service subtotals to a fixed
number of decimal places. This does not turn floating-point values into exact
decimal money; the supplied numeric representation and backend schema still
determine precision.

The native HTTP transport waits for its operation to return rather than assigning
a client request deadline. A stuck connection can therefore hold up an explicit
flush or process shutdown. The automatic exit hooks report flush failures on
stderr; call `flush` explicitly when the caller must handle a failed write.

The native transport reports the operation, method, URL and cause for a failed
request. A non-success HTTP response also includes its status and full response
body. Invalid JSON reports the response status and body. Supabase reads follow
server pagination rather than treating one server-sized page as the full result.
An empty page before the reported total is an error, not a successful partial read.

Configuration refusals include:

| Interface | Refusal | Required correction |
|---|---|---|
| TypeScript tracker | `CostTracker: sink=supabase requires opts.supabase` | Supply `supabase: { url, key }`. |
| TypeScript tracker | `CostTracker: sink=file requires opts.filePath` | Supply the destination file path. |
| Python tracker | `CostTracker: sink='supabase' requires supabase_url + supabase_key` | Supply both connection parameters. |
| Python tracker | `CostTracker: sink='file' requires file_path` | Supply the destination file path. |
| Python budget manager | `BudgetManager: provide either sink= or supabase_url + supabase_key` | Reuse the tracker's sink or provide the connection parameters. |

In the Python API, a concurrent flush raises
`CostTracker: a flush is already in progress; pending records were not resent`.
Let the active call finish before flushing again. TypeScript callers instead
share the active write and then publish any additional records they requested.

`is_over_budget` / `isOverBudget` returns false when no budget row exists;
`remaining` returns infinity in that case. Applications requiring a configured
budget must check that `get_status` / `getStatus` returned rows, as above.


## Schema (Supabase)

The schema is declared in
`supabase/migrations/20260429000001_cost_tracking.sql`.
Provision it through the dedicated `wisent-supabase-*` repository for the
application's database; do not apply this reference migration directly to an
operator database.

## Pricing source

`pricing/costs.json` is human-edited from each provider's public pricing
page snapshots. When the provider returns an actual billed amount, pass it as
the helper's override argument or as `cost_usd` to `record`. TypeScript helpers
take a positional `override`; Python helpers that accept one name it `override`,
not `override_usd`.
