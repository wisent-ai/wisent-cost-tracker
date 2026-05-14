import type { BudgetPeriod, BudgetStatus, CostSink } from './types.js';
import { type SupabaseSinkOptions } from './sinks.js';
export interface BudgetManagerOptions {
    agent_id: string;
    sink?: CostSink;
    supabase?: SupabaseSinkOptions;
}
/**
 * BudgetManager — read/write budget allocations and check spend status.
 *
 * Uses the Supabase view `cost_budget_status` (see migration) which joins
 * `cost_budgets` with the period-windowed sum of `cost_records.cost_usd`,
 * so consumers don't have to compute spent vs allocated client-side.
 */
export declare class BudgetManager {
    private sink;
    private agent_id;
    constructor(opts: BudgetManagerOptions);
    setBudget(category: string, allocated_usd: number, period: BudgetPeriod, starts_at?: Date): Promise<void>;
    getStatus(category?: string): Promise<BudgetStatus[]>;
    isOverBudget(category: string): Promise<boolean>;
    remaining(category: string): Promise<number>;
}
