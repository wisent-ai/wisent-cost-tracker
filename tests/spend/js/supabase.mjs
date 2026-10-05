import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { readFile, realpath } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join, sep } from 'node:path';

const [directory, ...extra] = process.argv.slice(2);
assert.ok(directory && extra.length === 0, 'expected the owning run directory');
const run = JSON.parse(await readFile(join(directory, 'run.json'), 'utf8'));
const owned = JSON.parse(await readFile(join(directory, 'owned/node.json'), 'utf8'));
assert.equal(owned.run_id, run.id);
const require = createRequire(join(directory, 'npm-site/package.json'));
const { CostTracker, SupabaseSink, BudgetManager, SupabaseRequestError } = require('@wisent/cost-tracker');
const entry = await realpath(require.resolve('@wisent/cost-tracker'));
assert.ok(entry.startsWith(`${await realpath(join(directory, 'npm-site'))}${sep}`));
const fixture = run.fixture.supabase;
const key = process.env[fixture.key_env];
assert.ok(key, `missing real provider credential ${fixture.key_env}`);
const cost = 1 / 65_536;
const tracker = new CostTracker({ agent_id: owned.agent, sink: 'supabase',
  supabase: { url: fixture.url, key }, autoFlush: false });
const sink = tracker.getSink();
const manager = new BudgetManager({ agent_id: owned.agent, sink });
const starts = new Date(owned.starts_at);
const input = (index, amount) => ({ service: 'qualification_usage', usage_type: 'units',
  usage_amount: 1, cost_usd: amount, reference_id: `record-${index}`,
  metadata: { qualification_run: run.id, marker: `record-${index}` } });
await manager.setBudget('all', 2 * cost, 'weekly', starts);
tracker.record(input(0, cost));
await tracker.flush();
assert.equal(await manager.isOverBudget('all'), false);
assert.equal(await manager.remaining('all'), cost);
const allowed = await manager.getStatus('all');
const second = tracker.record(input(1, 2 * cost));
await tracker.flush();
await tracker.flush();
assert.equal(await manager.isOverBudget('all'), true);
assert.equal(await manager.remaining('all'), -cost);
const denied = await manager.getStatus('all');
for (let index = 2; index < fixture.pagination_records; index++) tracker.record(input(index, cost));
await tracker.flush();
await tracker.flush();
const records = await sink.read(owned.agent, new Date(0));
assert.equal(records.length, fixture.pagination_records);
const markers = new Set();
for (const row of records) {
  const index = Number(row.reference_id.replace(/^record-/, ''));
  assert.ok(Number.isInteger(index) && index >= 0 && index < fixture.pagination_records);
  assert.equal(markers.has(index), false);
  markers.add(index);
  assert.equal(Number(row.cost_usd), index === 1 ? 2 * cost : cost);
  assert.equal(row.metadata.qualification_run, run.id);
}
const budgets = await sink.readBudgets(owned.agent);
const expected = new Set(['all', ...Array.from({ length: fixture.pagination_budgets - 1 },
  (_, index) => `qualification-${run.id}-budget-${index + 1}`)]);
assert.equal(budgets.length, expected.size);
assert.deepEqual(new Set(budgets.map(row => row.category)), expected);
await manager.setBudget('all', (fixture.pagination_records + 2) * cost, 'weekly', starts);
assert.equal(await manager.isOverBudget('all'), false);
assert.equal(await manager.remaining('all'), cost);
const restored = await manager.getStatus('all');
let authentication;
await assert.rejects(new SupabaseSink({ url: fixture.url, key: randomUUID() }).read(owned.agent, new Date(0)), error => {
  assert.ok(error instanceof SupabaseRequestError);
  assert.ok(error.statusCode === 401 || error.statusCode === 403);
  authentication = { status: error.statusCode, body: error.responseBody };
  return true;
});
const table = `qualification_missing_${randomUUID().replaceAll('-', '')}_${'absent_'.repeat(40)}`;
let failure;
await assert.rejects(new SupabaseSink({ url: fixture.url, key, table }).write([second]), error => {
  assert.ok(error instanceof SupabaseRequestError);
  assert.equal(error.method, 'POST');
  assert.ok(error.url.endsWith(table));
  assert.ok([...error.responseBody].length > 200, 'real provider reply must cross the old clipping boundary');
  assert.ok(error.message.includes(error.responseBody));
  failure = { status: error.statusCode, body: JSON.parse(error.responseBody), message: error.message };
  return true;
});
console.log(JSON.stringify({ entry, allowed, denied, restored, records, budgets, authentication, failure }));
