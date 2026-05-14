/**
 * Built-in CostSink implementations: in-memory, local-file, Supabase.
 */
import type { CostRecord, CostSink, BudgetStatus, BudgetPeriod } from './types.js';
export declare class MemorySink implements CostSink {
    records: CostRecord[];
    write(records: CostRecord[]): Promise<void>;
    read(): Promise<CostRecord[]>;
}
export declare class FileSink implements CostSink {
    private path;
    constructor(path: string);
    write(records: CostRecord[]): Promise<void>;
    read(): Promise<CostRecord[]>;
}
export interface SupabaseSinkOptions {
    url: string;
    key: string;
    table?: string;
    budgetTable?: string;
    viewName?: string;
}
/** Supabase REST sink. Writes via PostgREST so we don't need the JS client
 *  as a dependency — keeps the package zero-runtime-dep. */
export declare class SupabaseSink implements CostSink {
    private url;
    private key;
    private table;
    private budgetTable;
    private viewName;
    constructor(opts: SupabaseSinkOptions);
    private headers;
    write(records: CostRecord[]): Promise<void>;
    read(agent_id: string, since: Date): Promise<CostRecord[]>;
    readBudgets(agent_id: string): Promise<BudgetStatus[]>;
    writeBudget(agent_id: string, category: string, allocated_usd: number, period: BudgetPeriod, starts_at: Date): Promise<void>;
}
