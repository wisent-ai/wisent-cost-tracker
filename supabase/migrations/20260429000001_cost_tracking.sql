-- Cost tracking schema for wisent-cost-tracker.
-- This file is canonical; copy it into the appropriate
-- wisent-supabase-* repo (content-platform or wisent-app) as a new
-- timestamped migration before running it. Per Wisent governance, do NOT
-- run this directly against the database from this repo.

create table if not exists cost_records (
  id uuid primary key default gen_random_uuid(),
  agent_id text not null,
  service text not null,
  resource text,
  usage_type text not null check (usage_type in ('solves','tokens','bytes','seconds','units','emails')),
  usage_amount numeric not null,
  cost_usd numeric not null default 0,
  reference_id text,
  metadata jsonb not null default '{}'::jsonb,
  created_at timestamptz not null default now()
);

create index if not exists cost_records_agent_id_idx on cost_records (agent_id, created_at desc);
create index if not exists cost_records_service_idx on cost_records (service, created_at desc);
create index if not exists cost_records_reference_idx on cost_records (reference_id);

-- Budgets are scoped to (agent_id, category, period, starts_at). The unique
-- constraint enables on_conflict upserts from the client without requiring
-- a separate "current period" lookup.
create table if not exists cost_budgets (
  id uuid primary key default gen_random_uuid(),
  agent_id text not null,
  category text not null,
  allocated_usd numeric not null,
  period text not null check (period in ('daily','weekly','monthly')),
  starts_at timestamptz not null,
  metadata jsonb not null default '{}'::jsonb,
  created_at timestamptz not null default now(),
  constraint cost_budgets_unique unique (agent_id, category, period, starts_at)
);

create index if not exists cost_budgets_agent_id_idx on cost_budgets (agent_id);

-- Live status: utilization, remaining, overrun. Joins records by agent + the
-- period window starting at starts_at. Categories are arbitrary client-side
-- strings ('captcha', 'proxy', 'llm', 'compute', etc.); records are bucketed
-- by their `service` prefix to that category via the case below.
create or replace view cost_budget_status as
select
  b.id,
  b.agent_id,
  b.category,
  b.period,
  b.starts_at,
  b.allocated_usd,
  coalesce(sum(r.cost_usd), 0)::numeric as spent_usd,
  (b.allocated_usd - coalesce(sum(r.cost_usd), 0))::numeric as remaining_usd,
  case when b.allocated_usd > 0
    then (coalesce(sum(r.cost_usd), 0) / b.allocated_usd * 100)::numeric
    else 0
  end as utilization_pct,
  (coalesce(sum(r.cost_usd), 0) > b.allocated_usd) as is_over_budget
from cost_budgets b
left join cost_records r
  on  r.agent_id = b.agent_id
  and r.created_at >= b.starts_at
  and r.created_at < b.starts_at +
        case b.period
          when 'daily' then interval '1 day'
          when 'weekly' then interval '7 days'
          when 'monthly' then interval '30 days'
        end
  and (
    (b.category = 'captcha' and r.service like 'captcha\_%' escape '\') or
    (b.category = 'sms' and r.service like 'sms\_%' escape '\') or
    (b.category = 'proxy' and r.service like 'proxy\_%' escape '\') or
    (b.category = 'llm' and r.service like 'llm\_%' escape '\') or
    (b.category = 'compute' and r.service like 'compute\_%' escape '\') or
    (b.category = 'email' and r.service like 'email\_%' escape '\') or
    (b.category = 'all')
  )
group by b.id;

-- Optional RLS scaffolding. Disabled by default — enable if you want
-- multi-tenant isolation by agent_id. Service-role key bypasses RLS.
-- alter table cost_records enable row level security;
-- alter table cost_budgets enable row level security;
