import type { CostRecord, CostSink, RecordInput, UsageType } from '../types.js';
import { MemorySink, FileSink, SupabaseSink, type SupabaseSinkOptions } from './sinks.js';
import { captchaPrice, smsPrice, proxyCostForBytes, llmCost, computeCost, PRICES } from '../pricing.js';

// Symbol.for identifies listeners across independently loaded package copies.
const TRACKER_SIGNAL = Symbol.for('@wisent/cost-tracker:signal-listener');

// Per-call quantity declared in docs/record-helpers.md:
// https://github.com/wisent-ai/wisent-cost-tracker/blob/main/docs/record-helpers.md
const SINGLE_EVENT_USAGE = 1;

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
  private accepted = this.buffer.length;
  private flushing: Promise<void> | undefined;
  private failedExitHook?: () => void;

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

  // The last tracker restores the signal's default action only when no
  // application listener remains. Applications retain control of later signals.
  private installExitHooks(): void {
    const reportFailure = (error: unknown) => {
      console.error(`CostTracker: flushing spend records failed: ${error instanceof Error ? error.message : String(error)}`);
    };
    const flush = () => this.flush().catch(reportFailure);
    const beforeExit = () => {
      const pending = this.buffer.length;
      void this.flush().catch((error) => {
        if (this.buffer.length === pending) {
          process.off('beforeExit', beforeExit);
          this.failedExitHook = beforeExit;
        }
        reportFailure(error);
      });
    };
    process.on('beforeExit', beforeExit);
    const onSignal = (signal: NodeJS.Signals) => {
      const hasApplicationListener = () => process.listeners(signal).some((listener) =>
        Reflect.get(listener, TRACKER_SIGNAL) !== true);
      // once listeners disappear before their callback runs; remember ownership
      // before awaiting the sink, not only after the flush has completed.
      const applicationOwnsSignal = hasApplicationListener();
      void flush().finally(() => {
        if (applicationOwnsSignal || hasApplicationListener()) return;
        if (!process.listeners(signal).includes(onSignal)) return;
        process.off(signal, onSignal);
        if (!process.listeners(signal).length) process.kill(process.pid, signal);
      });
    };
    Object.defineProperty(onSignal, TRACKER_SIGNAL, { value: true });
    // Run before application listeners already installed with once().
    process.prependListener('SIGINT', onSignal);
    process.prependListener('SIGTERM', onSignal);
  }

  /** Append a raw record with an explicitly supplied, finite amount, as this
   *  tracker's agent's: the record returned and flushed carries the
   *  tracker's agent_id, and one that names another agent is refused before
   *  anything is buffered. The record keeps a shallow copy of the caller's
   *  metadata object, an empty one when none is given. */
  record(rec: RecordInput): CostRecord {
    const claimed = (rec as Partial<CostRecord>).agent_id;
    if (claimed !== undefined && claimed !== this.agent_id) {
      throw new TypeError(`CostTracker.record: agent_id ${JSON.stringify(claimed)} is not this tracker's agent ${JSON.stringify(this.agent_id)}; record another agent's spend with a CostTracker for that agent`);
    }
    const cost_usd = rec.cost_usd;
    if (typeof cost_usd !== 'number' || !Number.isFinite(cost_usd)) {
      throw new TypeError('CostTracker.record: cost_usd must be an explicit finite number; use a pricing helper when the amount is unknown');
    }
    if (rec.metadata !== undefined && (rec.metadata === null ||
        typeof rec.metadata !== 'object' ||
        ![Object.prototype, null].includes(Object.getPrototypeOf(rec.metadata)))) {
      throw new TypeError('CostTracker.record: metadata must be a plain object when supplied; omit it when no metadata is available');
    }
    const full: CostRecord = {
      service: rec.service,
      resource: rec.resource,
      usage_type: rec.usage_type,
      usage_amount: rec.usage_amount,
      cost_usd,
      reference_id: rec.reference_id ?? this.reference_id,
      metadata: { ...rec.metadata },
      created_at: rec.created_at ?? new Date().toISOString(),
      agent_id: this.agent_id,
    };
    // Returned and snapshot records share the buffer's entry; its owner cannot change.
    Object.defineProperty(full, 'agent_id', { writable: false, configurable: false });
    this.buffer.push(full);
    if (this.failedExitHook) {
      process.on('beforeExit', this.failedExitHook);
      this.failedExitHook = undefined;
    }
    return full;
  }

  recordCaptcha(service: string, taskType: string, override?: number): CostRecord {
    return this.record({
      service: `captcha_${service}`,
      resource: taskType,
      usage_type: 'solves',
      usage_amount: SINGLE_EVENT_USAGE,
      cost_usd: override ?? captchaPrice(service, taskType),
    });
  }

  recordSms(provider: string, platform: string, override?: number): CostRecord {
    return this.record({
      service: `sms_${provider}`,
      resource: platform,
      usage_type: 'units',
      usage_amount: SINGLE_EVENT_USAGE,
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

  /** Filed under `llm` with the model as the resource: the provider is not
   *  declared anywhere the tracker can read, so it is not guessed from words
   *  in the model name. */
  recordLlm(model: string, inputTokens: number, outputTokens: number, override?: number): CostRecord {
    return this.record({
      service: 'llm',
      resource: model,
      usage_type: 'tokens',
      usage_amount: inputTokens + outputTokens,
      cost_usd: override === undefined ? llmCost(model, inputTokens, outputTokens) : override,
      metadata: { input_tokens: inputTokens, output_tokens: outputTokens },
    });
  }

  /** Filed under `compute` with the instance type as the resource: an
   *  undeclared instance type names no provider, so none is cut out of its
   *  spelling. */
  recordCompute(instanceType: string, seconds: number, override?: number): CostRecord {
    return this.record({
      service: 'compute',
      resource: instanceType,
      usage_type: 'seconds',
      usage_amount: seconds,
      cost_usd: override ?? computeCost(instanceType, seconds),
    });
  }

  recordEmail(provider: string, count = SINGLE_EVENT_USAGE, override?: number): CostRecord {
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
  total(): number {
    // The empty sum is the additive identity: https://en.wikipedia.org/wiki/Empty_sum.
    return this.buffer.reduce((sum, record) => sum + record.cost_usd, 0);
  }

  /** Per-service cost rollup compatible with weles' service_costs JSONB.
   *  Every name, including __proto__, is its own key. Grouping additions
   *  can round differently from total(), which adds in record order. */
  snapshot(): { cost_usd: number; service_costs: Record<string, number>; records: CostRecord[] } {
    const sums = new Map<string, number>();
    for (const r of this.buffer) {
      const sum = sums.get(r.service);
      sums.set(r.service, sum === undefined ? r.cost_usd : sum + r.cost_usd);
    }
    return { cost_usd: this.total(), service_costs: Object.fromEntries(sums), records: this.buffer.slice() };
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
      const pending = this.buffer.slice(this.accepted, end);
      const writing = Promise.resolve()
        .then(() => this.sink.write(pending))
        .then(() => { this.accepted += pending.length; });
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
