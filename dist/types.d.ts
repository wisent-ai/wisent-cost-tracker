export type UsageType = 'solves' | 'tokens' | 'bytes' | 'seconds' | 'units' | 'emails';
export interface CostRecord {
    service: string;
    resource?: string;
    usage_type: UsageType;
    usage_amount: number;
    cost_usd: number;
    reference_id?: string;
    metadata?: Record<string, unknown>;
    created_at?: string;
}
export interface BudgetSnapshot {
    cost_usd: number;
    service_costs: Record<string, number>;
    records: CostRecord[];
}
export type BudgetPeriod = 'daily' | 'weekly' | 'monthly';
export interface BudgetStatus {
    category: string;
    allocated_usd: number;
    spent_usd: number;
    remaining_usd: number;
    utilization_pct: number;
    is_over_budget: boolean;
    period: BudgetPeriod;
    starts_at: string;
}
export interface CostSink {
    /** Persist a batch of records. Idempotent: rerunning with the same
     *  reference_id should be safe (sink may dedupe by reference_id). */
    write(records: CostRecord[]): Promise<void>;
    /** Read records for an agent in a window. Used by BudgetManager. */
    read?(agent_id: string, since: Date): Promise<CostRecord[]>;
    /** Read budget rows for an agent. */
    readBudgets?(agent_id: string): Promise<BudgetStatus[]>;
    /** Upsert a budget row. */
    writeBudget?(agent_id: string, category: string, allocated_usd: number, period: BudgetPeriod, starts_at: Date): Promise<void>;
}
