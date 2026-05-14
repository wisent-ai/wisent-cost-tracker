"use strict";
/**
 * Pricing table loader. Reads `pricing/costs.json` from the package root
 * (next to this dir during build) so consumers always get the canonical
 * prices without re-declaring them in code.
 */
Object.defineProperty(exports, "__esModule", { value: true });
exports.PRICES = void 0;
exports.loadPricing = loadPricing;
exports.captchaPrice = captchaPrice;
exports.smsPrice = smsPrice;
exports.proxyCostForBytes = proxyCostForBytes;
exports.llmCost = llmCost;
exports.computeCost = computeCost;
const node_fs_1 = require("node:fs");
const node_path_1 = require("node:path");
let cached = null;
/** Resolve the canonical pricing JSON path. Build copies the file under
 *  `dist/pricing/costs.json`; in-source dev resolves up to `../../pricing/`. */
function resolvePricingPath() {
    // CJS build: __dirname is dist/. Source dev: __dirname is js/src/.
    const here = __dirname;
    const built = (0, node_path_1.join)(here, 'pricing', 'costs.json');
    try {
        (0, node_fs_1.readFileSync)(built);
        return built;
    }
    catch { /* fall through */ }
    return (0, node_path_1.join)(here, '..', '..', 'pricing', 'costs.json');
}
function loadPricing() {
    if (cached)
        return cached;
    const raw = (0, node_fs_1.readFileSync)(resolvePricingPath(), 'utf8');
    cached = JSON.parse(raw);
    return cached;
}
exports.PRICES = loadPricing();
/** Look up captcha unit price. Falls back through service.default → 0.001. */
function captchaPrice(service, taskType) {
    const tbl = exports.PRICES.captcha[service];
    return tbl?.[taskType] ?? tbl?.default ?? 0.001;
}
/** Look up SMS unit price. service is the SMS provider, platform is the
 *  target service ('reddit', 'twitter', etc). */
function smsPrice(service, platform) {
    const tbl = exports.PRICES.sms[service];
    return tbl?.[platform.toLowerCase()] ?? tbl?.default ?? 0.30;
}
/** Compute per-GB proxy egress cost. provider matches table keys. */
function proxyCostForBytes(provider, bytes, isMobile = false) {
    const key = isMobile && provider === 'oxylabs' ? 'oxylabs_mobile' : provider;
    const perGb = exports.PRICES.proxy_per_gb[key] ?? exports.PRICES.proxy_per_gb.default;
    return (bytes / (1024 * 1024 * 1024)) * perGb;
}
/** Estimate LLM cost from token counts. Matches the model name as a substring
 *  so 'claude-3-5-sonnet@20240620' resolves to 'claude-3-5-sonnet'. */
function llmCost(model, inputTokens, outputTokens) {
    const ml = model.toLowerCase();
    let prices = exports.PRICES.llm.default;
    for (const [k, v] of Object.entries(exports.PRICES.llm)) {
        if (k !== 'default' && ml.includes(k.toLowerCase())) {
            prices = v;
            break;
        }
    }
    return (inputTokens / 1000) * prices.input_per_1k + (outputTokens / 1000) * prices.output_per_1k;
}
/** Compute per-second compute cost. instanceType matches keys like
 *  'gcp_n2-standard-4' or 'runpod_a100_80gb'. */
function computeCost(instanceType, seconds) {
    const perHour = exports.PRICES.compute_per_hour[instanceType] ?? exports.PRICES.compute_per_hour.default;
    return (seconds / 3600) * perHour;
}
//# sourceMappingURL=pricing.js.map