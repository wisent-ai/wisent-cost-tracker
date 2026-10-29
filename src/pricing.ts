/**
 * Pricing table loader. Reads `pricing/costs.json` from the package root
 * (next to this dir during build) so consumers always get the canonical
 * prices without re-declaring them in code.
 */

import { existsSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

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

/** Bytes in a gibibyte, the unit proxy prices are declared per
 *  (IEC 80000-13 binary prefixes, https://www.iso.org/standard/87648.html). */
const BYTES_PER_GIB = 1024 * 1024 * 1024;
/** Tokens in the block an LLM price is declared per: the catalogue's
 *  `input_per_1k` and `output_per_1k` keys
 *  (https://github.com/wisent-ai/wisent-cost-tracker/blob/main/pricing/costs.json). */
const TOKENS_PER_PRICED_BLOCK = 1000;
/** Seconds in an hour, the unit compute prices are declared per
 *  (SI Brochure, https://www.bipm.org/en/publications/si-brochure). */
const SECONDS_PER_HOUR = 3600;

let cached: PricingTable | null = null;

/** Resolve the canonical pricing JSON path. Build copies the file under
 *  `dist/pricing/costs.json`; in-source dev resolves up to `../../pricing/`.
 *  Neither present is refused naming both places looked in. */
function resolvePricingPath(): string {
  // CJS build: __dirname is dist/. Source dev: __dirname is js/src/.
  const built = join(__dirname, 'pricing', 'costs.json');
  if (existsSync(built)) return built;
  const source = join(__dirname, '..', '..', 'pricing', 'costs.json');
  if (existsSync(source)) return source;
  throw new Error(`the pricing catalogue is in neither the build (${built}) nor the source tree (${source})`);
}

export function loadPricing(): PricingTable {
  if (cached) return cached;
  const raw = readFileSync(resolvePricingPath(), 'utf8');
  cached = JSON.parse(raw) as PricingTable;
  return cached;
}

export const PRICES = loadPricing();

/** Look up a declared captcha unit price, then the provider's declared default. */
export function captchaPrice(service: string, taskType: string): number {
  const tbl = PRICES.captcha[service];
  const price = tbl?.[taskType] ?? tbl?.default;
  if (price === undefined) {
    throw new RangeError(`No declared captcha price for service=${JSON.stringify(service)}, resource=${JSON.stringify(taskType)}; supply an explicit cost override`);
  }
  return price;
}

/** Look up SMS unit price. service is the SMS provider, platform is the
 *  target service ('reddit', 'twitter', etc). */
export function smsPrice(service: string, platform: string): number {
  const tbl = PRICES.sms[service];
  const price = tbl?.[platform.toLowerCase()] ?? tbl?.default;
  if (price === undefined) {
    throw new RangeError(`No declared sms price for service=${JSON.stringify(service)}, resource=${JSON.stringify(platform.toLowerCase())}; supply an explicit cost override`);
  }
  return price;
}

/** Compute per-GB proxy egress cost. provider matches table keys. */
export function proxyCostForBytes(provider: string, bytes: number, isMobile = false): number {
  const key = isMobile && provider === 'oxylabs' ? 'oxylabs_mobile' : provider;
  const perGb = PRICES.proxy_per_gb[key] ?? PRICES.proxy_per_gb.default;
  return (bytes / BYTES_PER_GIB) * perGb;
}

/** The catalogue key pricing `model`: the key that is the model name (case
 *  aside), else `default`: a model the catalogue does not list is priced at
 *  the default, never at a listed model whose key its name happens to
 *  contain. Records name the key so a default price reads as one. */
export function llmPriceKey(model: string): string {
  const name = model.toLowerCase();
  const declared = Object.keys(PRICES.llm).find((key) => key !== 'default' && key.toLowerCase() === name);
  return declared === undefined ? 'default' : declared;
}

export function llmCost(model: string, inputTokens: number, outputTokens: number): number {
  const prices = PRICES.llm[llmPriceKey(model)];
  return (inputTokens / TOKENS_PER_PRICED_BLOCK) * prices.input_per_1k + (outputTokens / TOKENS_PER_PRICED_BLOCK) * prices.output_per_1k;
}

/** Compute per-second compute cost. instanceType matches keys like
 *  'gcp_n2-standard-4' or 'runpod_a100_80gb'. */
export function computeCost(instanceType: string, seconds: number): number {
  const perHour = PRICES.compute_per_hour[instanceType] ?? PRICES.compute_per_hour.default;
  return (seconds / SECONDS_PER_HOUR) * perHour;
}
