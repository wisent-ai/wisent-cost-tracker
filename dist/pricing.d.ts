/**
 * Pricing table loader. Reads `pricing/costs.json` from the package root
 * (next to this dir during build) so consumers always get the canonical
 * prices without re-declaring them in code.
 */
export interface PricingTable {
    version: number;
    updated_at: string;
    currency: string;
    captcha: Record<string, Record<string, number>>;
    sms: Record<string, Record<string, number>>;
    proxy_per_gb: Record<string, number>;
    llm: Record<string, {
        input_per_1k: number;
        output_per_1k: number;
    }>;
    compute_per_hour: Record<string, number>;
    email: Record<string, number>;
}
export declare function loadPricing(): PricingTable;
export declare const PRICES: PricingTable;
/** Look up captcha unit price. Falls back through service.default → 0.001. */
export declare function captchaPrice(service: string, taskType: string): number;
/** Look up SMS unit price. service is the SMS provider, platform is the
 *  target service ('reddit', 'twitter', etc). */
export declare function smsPrice(service: string, platform: string): number;
/** Compute per-GB proxy egress cost. provider matches table keys. */
export declare function proxyCostForBytes(provider: string, bytes: number, isMobile?: boolean): number;
/** Estimate LLM cost from token counts. Matches the model name as a substring
 *  so 'claude-3-5-sonnet@20240620' resolves to 'claude-3-5-sonnet'. */
export declare function llmCost(model: string, inputTokens: number, outputTokens: number): number;
/** Compute per-second compute cost. instanceType matches keys like
 *  'gcp_n2-standard-4' or 'runpod_a100_80gb'. */
export declare function computeCost(instanceType: string, seconds: number): number;
