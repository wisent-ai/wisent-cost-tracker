export type UsageType = 'solves' | 'tokens' | 'bytes' | 'seconds' | 'units' | 'emails';

export interface CostRecord {
  service: string;          // 'capsolver', 'gemini-2.0-flash', 'proxy_oxylabs'
  resource?: string;        // 'recaptcha_v2', 'gpt-4o', 'mobile-residential'
  usage_type: UsageType;
  usage_amount: number;
  cost_usd: number;
  reference_id?: string;    // e.g. ACTION_LOG_ID, trade_id
  metadata?: Record<string, unknown>;
  // ISO timestamp. CostTracker.record sets it when the caller leaves it out;
  // a record written to a sink directly keeps it absent in FileSink and
  // MemorySink, and the Supabase table fills it with its column default now().
  created_at?: string;
  // The agent the spend is attributed to. CostTracker.record sets the
  // tracker's own agent_id on every record it buffers and returns;
  // SupabaseSink.read selects records by it, and the cost_budget_status view
  // counts a record against the budgets of the agent it names. Ownership is
  // readonly; the other fields intentionally remain editable before flush.
  readonly agent_id?: string;
}

/** What CostTracker.record takes: a record without agent_id, which the
 *  tracker sets itself. */
export type RecordInput = Omit<CostRecord, 'agent_id'>;

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
