import type { BudgetPeriod, BudgetStatus, CostSink } from './types.js';
import { SupabaseSink, type SupabaseSinkOptions } from './sinks.js';

export interface BudgetManagerOptions {
  agent_id: string;
  sink?: CostSink;
  supabase?: SupabaseSinkOptions;  // shortcut: build a SupabaseSink internally
}

/**
 * BudgetManager — read/write budget allocations and check spend status.
 *
 * Uses the Supabase view `cost_budget_status` (see migration) which joins
 * `cost_budgets` with the period-windowed sum of `cost_records.cost_usd`,
 * so consumers don't have to compute spent vs allocated client-side.
 */
export class BudgetManager {
  private sink: CostSink;
  private agent_id: string;

  constructor(opts: BudgetManagerOptions) {
    this.agent_id = opts.agent_id;
    if (opts.sink) this.sink = opts.sink;
    else if (opts.supabase) this.sink = new SupabaseSink(opts.supabase);
    else throw new Error('BudgetManager: provide either opts.sink or opts.supabase');
  }

  async setBudget(category: string, allocated_usd: number, period: BudgetPeriod, starts_at: Date = new Date()): Promise<void> {
    if (!this.sink.writeBudget) throw new Error('BudgetManager: sink does not support writeBudget');
    await this.sink.writeBudget(this.agent_id, category, allocated_usd, period, starts_at);
  }

  async getStatus(category?: string): Promise<BudgetStatus[]> {
    if (!this.sink.readBudgets) throw new Error('BudgetManager: sink does not support readBudgets');
    const all = await this.sink.readBudgets(this.agent_id);
    return category ? all.filter(b => b.category === category) : all;
  }

  async isOverBudget(category: string): Promise<boolean> {
    const rows = await this.getStatus(category);
    return rows.some(b => b.is_over_budget);
  }

  async remaining(category: string): Promise<number> {
    const rows = await this.getStatus(category);
    if (rows.length === 0) return Infinity;
    return rows.reduce((a, b) => a + b.remaining_usd, 0);
  }
}
