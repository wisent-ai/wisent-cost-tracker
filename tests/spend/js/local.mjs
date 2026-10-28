import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { readFile, writeFile, mkdir, realpath } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join, sep } from 'node:path';

// Node's argv is [executable, script, ...arguments]:
// https://nodejs.org/api/process.html#processargv
const [, , directory, ...extra] = process.argv;
assert.ok(directory, 'expected the owning run directory');
assert.deepEqual(extra, [], 'expected only the owning run directory');
const run = JSON.parse(await readFile(join(directory, 'run.json'), 'utf8'));
const require = createRequire(join(directory, 'npm-site', 'package.json'));
const { CostTracker } = require('@wisent/cost-tracker');
const entry = await realpath(require.resolve('@wisent/cost-tracker'));
assert.ok(entry.startsWith(`${await realpath(join(directory, 'npm-site'))}${sep}`), 'SDK came from outside the installed package');
// The gap between one and the next double, a power of two, so a sum of a few
// such charges is exact: https://tc39.es/ecma262/#sec-number.epsilon
const cost = Number.EPSILON;
const agent = randomUUID();
const input = (marker, amount) => ({
  service: 'qualification_usage', usage_type: 'units', usage_amount: marker.length, cost_usd: amount,
  reference_id: marker, metadata: { qualification_run: run.id, marker },
});

const memory = new CostTracker({ agent_id: agent, sink: 'memory', autoFlush: false });
const missingAmount = input('missing-amount', undefined);
delete missingAmount.cost_usd;
for (const record of [
  missingAmount,
  input('not-a-number', NaN),
  input('infinite-amount', Infinity),
  input('string-amount', '0'),
  input('null-amount', null),
]) {
  assert.throws(() => memory.record(record), error =>
    error instanceof TypeError && error.message.includes('cost_usd'));
}
assert.deepEqual(memory.snapshot().records, [], 'a rejected record must not enter the buffer');
await memory.flush();
assert.deepEqual(await memory.getSink().read(), [], 'a refused amount must not become a free charge');
// A known free charge, measured as what a tracker holding nothing totals.
const free = memory.total();
assert.equal(memory.record(input('first', cost)).cost_usd, cost);
const first = memory.flush();
memory.record(input('second', cost + cost));
const second = memory.flush();
await Promise.all([first, second]);
memory.record(input('explicit-free', free));
await memory.flush();
const memoryRows = await memory.getSink().read();
assert.deepEqual(memoryRows.map(row => [row.reference_id, row.cost_usd, row.agent_id]), [
  ['first', cost, agent], ['second', cost + cost, agent], ['explicit-free', free, agent],
]);
assert.equal(memory.total(), cost + cost + cost);
assert.equal(memory.snapshot().service_costs.qualification_usage, cost + cost + cost);

const files = join(directory, 'files');
await mkdir(files, { recursive: true });
const path = join(files, 'node-records.json');
const tracker = new CostTracker({ agent_id: agent, sink: 'file', filePath: path, autoFlush: false });
tracker.record(input('file-first', cost));
const initialWrite = tracker.flush();
tracker.record(input('file-second', cost + cost));
const laterWrite = tracker.flush();
await Promise.all([initialWrite, laterWrite]);
await tracker.flush();
const bytes = await readFile(path, 'utf8');
const fileRows = JSON.parse(bytes);
assert.deepEqual(fileRows.map(row => [row.reference_id, row.cost_usd, row.agent_id]), [
  ['file-first', cost, agent], ['file-second', cost + cost, agent],
]);
assert.deepEqual(await tracker.getSink().read(), fileRows);
assert.equal(await readFile(path, 'utf8'), bytes);

const corruptPath = join(files, 'node-corrupt.json');
const corrupt = '{unfinished-records\n';
await writeFile(corruptPath, corrupt, { flag: 'wx' });
const refused = new CostTracker({ agent_id: agent, sink: 'file', filePath: corruptPath, autoFlush: false });
refused.record(input('must-remain-pending', cost));
await assert.rejects(refused.flush(), Error);
assert.equal(await readFile(corruptPath, 'utf8'), corrupt);
assert.equal(refused.total(), cost);
assert.throws(() => new CostTracker({ agent_id: agent, sink: 'supabase', autoFlush: false }), Error);
assert.deepEqual(await tracker.getSink().read(), fileRows);

const prices = JSON.parse(await readFile(new URL('../../../pricing/costs.json', import.meta.url), 'utf8'));
const pricingPath = join(files, 'node-pricing.json');
const priced = new CostTracker({ agent_id: agent, sink: 'file', filePath: pricingPath, autoFlush: false });
// One block of input and one of output tokens: the catalogue declares LLM
// prices per thousand tokens, its `input_per_1k` and `output_per_1k` keys
// (https://github.com/wisent-ai/wisent-cost-tracker/blob/main/pricing/costs.json).
const block = 1000;
const blockCost = ({ input_per_1k, output_per_1k }) => input_per_1k + output_per_1k;
// A name that contains a listed key without being one is priced at the default.
const containing = 'qualification-gpt-4o-mini-variant';
priced.recordLlm('GPT-4O-MINI', block, block);
priced.recordLlm(containing, block, block);
priced.recordLlm('qualification-unpriced-model', block, block);
priced.recordCaptcha('capsolver', 'qualification-unlisted-task');
const expectedPrices = [
  ['llm', 'GPT-4O-MINI', blockCost(prices.llm['gpt-4o-mini'])],
  ['llm', containing, blockCost(prices.llm.default)],
  ['llm', 'qualification-unpriced-model', blockCost(prices.llm.default)],
  ['captcha_capsolver', 'qualification-unlisted-task', prices.captcha.capsolver.default],
];
const unpriced = `qualification_${randomUUID()}`;
const pricingRefusals = [];
for (const [method, service] of [['recordCaptcha', 'captcha'], ['recordSms', 'sms']]) {
  const before = structuredClone(priced.snapshot());
  assert.throws(() => priced[method](unpriced, 'qualification-resource'), error => {
    assert.ok(error instanceof RangeError);
    assert.ok(error.message.includes(unpriced) && error.message.includes('qualification-resource'));
    pricingRefusals.push({ operation: method, error: error.message });
    return true;
  });
  assert.deepEqual(priced.snapshot(), before, 'unpriced usage must not enter the buffer');
  priced[method](unpriced, 'qualification-resource', free);
  expectedPrices.push([`${service}_${unpriced}`, 'qualification-resource', free]);
}
await priced.flush();
const pricedRows = JSON.parse(await readFile(pricingPath, 'utf8'));
assert.deepEqual(pricedRows.map(row => [row.service, row.resource, row.cost_usd]), expectedPrices);
assert.deepEqual(await priced.getSink().read(), pricedRows);

console.log(JSON.stringify({
  entry, node: process.version, executable: process.execPath, memoryRecords: memoryRows,
  file: path, fileRecords: fileRows, snapshot: tracker.snapshot(), corruptFilePreserved: corruptPath,
  pricingFile: pricingPath, pricedRecords: pricedRows, pricingRefusals,
}));
