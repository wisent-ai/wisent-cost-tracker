import type { CostRecord, CostSink, UsageType } from '../types.js';
import { MemorySink, FileSink, SupabaseSink, type SupabaseSinkOptions } from './sinks.js';
import { captchaPrice, smsPrice, proxyCostForBytes, llmCost, computeCost, PRICES } from '../pricing.js';

export interface CostTrackerOptions {
  agent_id: string;
  reference_id?: string;             // e.g. ACTION_LOG_ID for weles
  sink?: 'memory' | 'file' | 'supabase';
  filePath?: string;                 // when sink === 'file'
  supabase?: SupabaseSinkOptions;    // when sink === 'supabase'
  /** Auto-flush hooks: register beforeExit/SIGINT/SIGTERM listeners that
   *  flush pending records before the process exits. Default: true. */
  autoFlush?: boolean;
}

export class CostTracker {
  private sink: CostSink;
  private buffer: CostRecord[] = [];
  private agent_id: string;
  private reference_id?: string;
  private accepted = 0;
  private flushing: Promise<void> | undefined;

  constructor(opts: CostTrackerOptions) {
    this.agent_id = opts.agent_id;
    this.reference_id = opts.reference_id;
    if (opts.sink === 'supabase') {
      if (!opts.supabase) throw new Error('CostTracker: sink=supabase requires opts.supabase');
      this.sink = new SupabaseSink(opts.supabase);
    } else if (opts.sink === 'file') {
      if (!opts.filePath) throw new Error('CostTracker: sink=file requires opts.filePath');
      this.sink = new FileSink(opts.filePath);
    } else {
      this.sink = new MemorySink();
    }
    if (opts.autoFlush !== false) this.installExitHooks();
  }

  // A signal exits only after the sink has answered the flush, however long
  // that takes; a failed flush is reported on stderr, never swallowed.
  private installExitHooks(): void {
    const flush = () => this.flush().catch((error) => {
      console.error(`CostTracker: flushing spend records failed: ${error instanceof Error ? error.message : String(error)}`);
    });
    process.on('beforeExit', () => { void flush(); });
    process.on('SIGINT', () => { void flush().finally(() => process.exit(130)); });
    process.on('SIGTERM', () => { void flush().finally(() => process.exit(143)); });
  }

  /** Append a raw record with an explicitly supplied, finite amount. */
  record(rec: CostRecord): CostRecord {
    const cost_usd = rec.cost_usd;
    if (typeof cost_usd !== 'number' || !Number.isFinite(cost_usd)) {
      throw new TypeError('CostTracker.record: cost_usd must be an explicit finite number; use a pricing helper when the amount is unknown');
    }
    const full: CostRecord = {
      service: rec.service,
      resource: rec.resource,
      usage_type: rec.usage_type,
      usage_amount: rec.usage_amount,
      cost_usd,
      reference_id: rec.reference_id ?? this.reference_id,
      metadata: rec.metadata ?? {},
      created_at: rec.created_at ?? new Date().toISOString(),
    };
    this.buffer.push(full);
    return full;
  }

  recordCaptcha(service: string, taskType: string, override?: number): CostRecord {
    return this.record({
      service: `captcha_${service}`,
      resource: taskType,
      usage_type: 'solves',
      usage_amount: 1,
      cost_usd: override ?? captchaPrice(service, taskType),
    });
  }

  recordSms(provider: string, platform: string, override?: number): CostRecord {
    return this.record({
      service: `sms_${provider}`,
      resource: platform,
      usage_type: 'units',
      usage_amount: 1,
      cost_usd: override ?? smsPrice(provider, platform),
    });
  }

  recordProxyBytes(provider: string, bytes: number, isMobile = false): CostRecord {
    const key = isMobile && provider === 'oxylabs' ? 'oxylabs_mobile' : provider;
    return this.record({
      service: `proxy_${key}`,
      resource: isMobile ? 'mobile' : 'residential',
      usage_type: 'bytes',
      usage_amount: bytes,
      cost_usd: proxyCostForBytes(provider, bytes, isMobile),
    });
  }

  recordLlm(model: string, inputTokens: number, outputTokens: number, override?: number): CostRecord {
    return this.record({
      service: `llm_${normalizeLlmService(model)}`,
      resource: model,
      usage_type: 'tokens',
      usage_amount: inputTokens + outputTokens,
      cost_usd: override ?? llmCost(model, inputTokens, outputTokens),
      metadata: { input_tokens: inputTokens, output_tokens: outputTokens },
    });
  }

  recordCompute(instanceType: string, seconds: number, override?: number): CostRecord {
    return this.record({
      service: `compute_${instanceType.split('_')[0] ?? 'other'}`,
      resource: instanceType,
      usage_type: 'seconds',
      usage_amount: seconds,
      cost_usd: override ?? computeCost(instanceType, seconds),
    });
  }

  recordEmail(provider: string, count = 1, override?: number): CostRecord {
    const unit = (PRICES.email as Record<string, number>)[provider] ?? PRICES.email.default;
    return this.record({
      service: `email_${provider}`,
      resource: provider,
      usage_type: 'emails',
      usage_amount: count,
      cost_usd: override ?? unit * count,
    });
  }

  /** Total $ across all buffered records. */
  total(): number { return this.buffer.reduce((a, r) => a + r.cost_usd, 0); }

  /** Per-service cost rollup compatible with weles' service_costs JSONB. */
  snapshot(): { cost_usd: number; service_costs: Record<string, number>; records: CostRecord[] } {
    const service_costs: Record<string, number> = {};
    for (const r of this.buffer) service_costs[r.service] = (service_costs[r.service] ?? 0) + r.cost_usd;
    return { cost_usd: this.total(), service_costs, records: this.buffer.slice() };
  }

  /** Publish every record present at this call, without resending accepted
   *  records. Concurrent callers share the current write before their tail. */
  async flush(): Promise<void> {
    const end = this.buffer.length;
    while (this.accepted < end) {
      if (this.flushing) {
        await this.flushing;
        continue;
      }
      const start = this.accepted;
      const stamped = new Array<CostRecord>(end - start);
      for (let index = start; index < end; index += 1) {
        stamped[index - start] = { ...this.buffer[index]!, agent_id: this.agent_id };
      }
      const writing = Promise.resolve()
        .then(() => this.sink.write(stamped))
        .then(() => { this.accepted = end; });
      this.flushing = writing;
      try {
        await writing;
      } finally {
        if (this.flushing === writing) this.flushing = undefined;
      }
    }
  }

  /** Underlying sink, for advanced usage (e.g. read-back during the same run). */
  getSink(): CostSink { return this.sink; }
}


function normalizeLlmService(model: string): string {
  const m = model.toLowerCase();
  if (m.includes('gemini')) return 'gemini';
  if (m.includes('claude')) return 'claude';
  if (m.includes('gpt')) return 'openai';
  return 'other';
}
