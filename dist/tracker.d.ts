import type { CostRecord, CostSink } from './types.js';
import { type SupabaseSinkOptions } from './sinks.js';
export interface CostTrackerOptions {
    agent_id: string;
    reference_id?: string;
    sink?: 'memory' | 'file' | 'supabase';
    filePath?: string;
    supabase?: SupabaseSinkOptions;
    /** Auto-flush hooks: register beforeExit/SIGINT/SIGTERM listeners that
     *  flush pending records before the process exits. Default: true. */
    autoFlush?: boolean;
}
export declare class CostTracker {
    private sink;
    private buffer;
    private agent_id;
    private reference_id?;
    private flushed;
    constructor(opts: CostTrackerOptions);
    private installExitHooks;
    /** Append a raw record. Computes cost from pricing if cost_usd absent. */
    record(rec: Omit<CostRecord, 'cost_usd'> & {
        cost_usd?: number;
    }): CostRecord;
    recordCaptcha(service: string, taskType: string, override?: number): CostRecord;
    recordSms(provider: string, platform: string, override?: number): CostRecord;
    recordProxyBytes(provider: string, bytes: number, isMobile?: boolean): CostRecord;
    recordLlm(model: string, inputTokens: number, outputTokens: number, override?: number): CostRecord;
    recordCompute(instanceType: string, seconds: number, override?: number): CostRecord;
    recordEmail(provider: string, count?: number, override?: number): CostRecord;
    /** Total $ across all buffered records. */
    total(): number;
    /** Per-service cost rollup compatible with weles' service_costs JSONB. */
    snapshot(): {
        cost_usd: number;
        service_costs: Record<string, number>;
        records: CostRecord[];
    };
    /** Flush buffered records to the sink. Idempotent — safe to call multiple
     *  times; second call is a no-op. */
    flush(): Promise<void>;
    /** Underlying sink, for advanced usage (e.g. read-back during the same run). */
    getSink(): CostSink;
}
