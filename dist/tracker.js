"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.CostTracker = void 0;
const sinks_js_1 = require("./sinks.js");
const pricing_js_1 = require("./pricing.js");
class CostTracker {
    sink;
    buffer = [];
    agent_id;
    reference_id;
    flushed = false;
    constructor(opts) {
        this.agent_id = opts.agent_id;
        this.reference_id = opts.reference_id;
        if (opts.sink === 'supabase') {
            if (!opts.supabase)
                throw new Error('CostTracker: sink=supabase requires opts.supabase');
            this.sink = new sinks_js_1.SupabaseSink(opts.supabase);
        }
        else if (opts.sink === 'file') {
            if (!opts.filePath)
                throw new Error('CostTracker: sink=file requires opts.filePath');
            this.sink = new sinks_js_1.FileSink(opts.filePath);
        }
        else {
            this.sink = new sinks_js_1.MemorySink();
        }
        if (opts.autoFlush !== false)
            this.installExitHooks();
    }
    installExitHooks() {
        const flush = () => { void this.flush().catch(() => { }); };
        process.on('beforeExit', flush);
        process.on('SIGINT', () => { flush(); setTimeout(() => process.exit(130), 100); });
        process.on('SIGTERM', () => { flush(); setTimeout(() => process.exit(143), 100); });
    }
    /** Append a raw record. Computes cost from pricing if cost_usd absent. */
    record(rec) {
        const cost_usd = typeof rec.cost_usd === 'number' ? rec.cost_usd : 0;
        const full = {
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
    recordCaptcha(service, taskType, override) {
        return this.record({
            service: `captcha_${service}`,
            resource: taskType,
            usage_type: 'solves',
            usage_amount: 1,
            cost_usd: override ?? (0, pricing_js_1.captchaPrice)(service, taskType),
        });
    }
    recordSms(provider, platform, override) {
        return this.record({
            service: `sms_${provider}`,
            resource: platform,
            usage_type: 'units',
            usage_amount: 1,
            cost_usd: override ?? (0, pricing_js_1.smsPrice)(provider, platform),
        });
    }
    recordProxyBytes(provider, bytes, isMobile = false) {
        const key = isMobile && provider === 'oxylabs' ? 'oxylabs_mobile' : provider;
        return this.record({
            service: `proxy_${key}`,
            resource: isMobile ? 'mobile' : 'residential',
            usage_type: 'bytes',
            usage_amount: bytes,
            cost_usd: (0, pricing_js_1.proxyCostForBytes)(provider, bytes, isMobile),
        });
    }
    recordLlm(model, inputTokens, outputTokens, override) {
        return this.record({
            service: `llm_${normalizeLlmService(model)}`,
            resource: model,
            usage_type: 'tokens',
            usage_amount: inputTokens + outputTokens,
            cost_usd: override ?? (0, pricing_js_1.llmCost)(model, inputTokens, outputTokens),
            metadata: { input_tokens: inputTokens, output_tokens: outputTokens },
        });
    }
    recordCompute(instanceType, seconds, override) {
        return this.record({
            service: `compute_${instanceType.split('_')[0] ?? 'other'}`,
            resource: instanceType,
            usage_type: 'seconds',
            usage_amount: seconds,
            cost_usd: override ?? (0, pricing_js_1.computeCost)(instanceType, seconds),
        });
    }
    recordEmail(provider, count = 1, override) {
        const unit = pricing_js_1.PRICES.email[provider] ?? pricing_js_1.PRICES.email.default;
        return this.record({
            service: `email_${provider}`,
            resource: provider,
            usage_type: 'emails',
            usage_amount: count,
            cost_usd: override ?? unit * count,
        });
    }
    /** Total $ across all buffered records. */
    total() { return round4(this.buffer.reduce((a, r) => a + r.cost_usd, 0)); }
    /** Per-service cost rollup compatible with weles' service_costs JSONB. */
    snapshot() {
        const service_costs = {};
        for (const r of this.buffer)
            service_costs[r.service] = round4((service_costs[r.service] ?? 0) + r.cost_usd);
        return { cost_usd: this.total(), service_costs, records: this.buffer.slice() };
    }
    /** Flush buffered records to the sink. Idempotent — safe to call multiple
     *  times; second call is a no-op. */
    async flush() {
        if (this.flushed)
            return;
        if (this.buffer.length === 0) {
            this.flushed = true;
            return;
        }
        const stamped = this.buffer.map(r => ({ ...r, agent_id: this.agent_id }));
        await this.sink.write(stamped);
        this.flushed = true;
    }
    /** Underlying sink, for advanced usage (e.g. read-back during the same run). */
    getSink() { return this.sink; }
}
exports.CostTracker = CostTracker;
function round4(n) { return Math.round(n * 10000) / 10000; }
function normalizeLlmService(model) {
    const m = model.toLowerCase();
    if (m.includes('gemini'))
        return 'gemini';
    if (m.includes('claude'))
        return 'claude';
    if (m.includes('gpt'))
        return 'openai';
    return 'other';
}
//# sourceMappingURL=tracker.js.map