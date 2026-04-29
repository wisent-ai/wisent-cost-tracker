import type { CostRecord, CostSink, UsageType } from './types.js';
import { MemorySink, FileSink, SupabaseSink, type SupabaseSinkOptions } from './sinks.js';
import { captchaPrice, smsPrice, proxyCostForBytes, llmCost, computeCost, PRICES } from './pricing.js';

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
  private flushed = false;

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

  private installExitHooks(): void {
    const flush = () => { void this.flush().catch(() => {}); };
    process.on('beforeExit', flush);
    process.on('SIGINT', () => { flush(); setTimeout(() => process.exit(130), 100); });
    process.on('SIGTERM', () => { flush(); setTimeout(() => process.exit(143), 100); });
  }

  /** Append a raw record. Computes cost from pricing if cost_usd absent. */
  record(rec: Omit<CostRecord, 'cost_usd'> & { cost_usd?: number }): CostRecord {
    const cost_usd = typeof rec.cost_usd === 'number' ? rec.cost_usd : 0;
    const full: CostRecord = {
      service: rec.service,
      resource: rec.resource,
      usage_type: rec.usage_type,
      usage_amount: rec.usage_amount,
      cost_usd: round4(cost_usd),
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
  total(): number { return round4(this.buffer.reduce((a, r) => a + r.cost_usd, 0)); }

  /** Per-service cost rollup compatible with weles' service_costs JSONB. */
  snapshot(): { cost_usd: number; service_costs: Record<string, number>; records: CostRecord[] } {
    const service_costs: Record<string, number> = {};
    for (const r of this.buffer) service_costs[r.service] = round4((service_costs[r.service] ?? 0) + r.cost_usd);
    return { cost_usd: this.total(), service_costs, records: this.buffer.slice() };
  }

  /** Flush buffered records to the sink. Idempotent — safe to call multiple
   *  times; second call is a no-op. */
  async flush(): Promise<void> {
    if (this.flushed) return;
    if (this.buffer.length === 0) { this.flushed = true; return; }
    const stamped = this.buffer.map(r => ({ ...r, agent_id: this.agent_id }));
    await this.sink.write(stamped as CostRecord[]);
    this.flushed = true;
  }

  /** Underlying sink, for advanced usage (e.g. read-back during the same run). */
  getSink(): CostSink { return this.sink; }
}

function round4(n: number): number { return Math.round(n * 10000) / 10000; }

function normalizeLlmService(model: string): string {
  const m = model.toLowerCase();
  if (m.includes('gemini')) return 'gemini';
  if (m.includes('claude')) return 'claude';
  if (m.includes('gpt')) return 'openai';
  return 'other';
}
