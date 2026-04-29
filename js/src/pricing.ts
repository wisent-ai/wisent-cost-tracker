/**
 * Pricing table loader. Reads `pricing/costs.json` from the package root
 * (next to this dir during build) so consumers always get the canonical
 * prices without re-declaring them in code.
 */

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';

export interface PricingTable {
  version: number;
  updated_at: string;
  currency: string;
  captcha: Record<string, Record<string, number>>;
  sms: Record<string, Record<string, number>>;
  proxy_per_gb: Record<string, number>;
  llm: Record<string, { input_per_1k: number; output_per_1k: number }>;
  compute_per_hour: Record<string, number>;
  email: Record<string, number>;
}

let cached: PricingTable | null = null;

/** Resolve the canonical pricing JSON path. Build copies the file under
 *  `dist/pricing/costs.json`; in-source dev resolves up to `../../pricing/`. */
function resolvePricingPath(): string {
  // CJS build: __dirname is dist/. Source dev: __dirname is js/src/.
  const here = __dirname;
  const built = join(here, 'pricing', 'costs.json');
  try { readFileSync(built); return built; } catch { /* fall through */ }
  return join(here, '..', '..', 'pricing', 'costs.json');
}

export function loadPricing(): PricingTable {
  if (cached) return cached;
  const raw = readFileSync(resolvePricingPath(), 'utf8');
  cached = JSON.parse(raw) as PricingTable;
  return cached;
}

export const PRICES = loadPricing();

/** Look up captcha unit price. Falls back through service.default → 0.001. */
export function captchaPrice(service: string, taskType: string): number {
  const tbl = PRICES.captcha[service];
  return tbl?.[taskType] ?? tbl?.default ?? 0.001;
}

/** Look up SMS unit price. service is the SMS provider, platform is the
 *  target service ('reddit', 'twitter', etc). */
export function smsPrice(service: string, platform: string): number {
  const tbl = PRICES.sms[service];
  return tbl?.[platform.toLowerCase()] ?? tbl?.default ?? 0.30;
}

/** Compute per-GB proxy egress cost. provider matches table keys. */
export function proxyCostForBytes(provider: string, bytes: number, isMobile = false): number {
  const key = isMobile && provider === 'oxylabs' ? 'oxylabs_mobile' : provider;
  const perGb = PRICES.proxy_per_gb[key] ?? PRICES.proxy_per_gb.default;
  return (bytes / (1024 * 1024 * 1024)) * perGb;
}

/** Estimate LLM cost from token counts. Matches the model name as a substring
 *  so 'claude-3-5-sonnet@20240620' resolves to 'claude-3-5-sonnet'. */
export function llmCost(model: string, inputTokens: number, outputTokens: number): number {
  const ml = model.toLowerCase();
  let prices = PRICES.llm.default;
  for (const [k, v] of Object.entries(PRICES.llm)) {
    if (k !== 'default' && ml.includes(k.toLowerCase())) { prices = v; break; }
  }
  return (inputTokens / 1000) * prices.input_per_1k + (outputTokens / 1000) * prices.output_per_1k;
}

/** Compute per-second compute cost. instanceType matches keys like
 *  'gcp_n2-standard-4' or 'runpod_a100_80gb'. */
export function computeCost(instanceType: string, seconds: number): number {
  const perHour = PRICES.compute_per_hour[instanceType] ?? PRICES.compute_per_hour.default;
  return (seconds / 3600) * perHour;
}
