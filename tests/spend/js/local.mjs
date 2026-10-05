import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { readFile, writeFile, mkdir, realpath } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { join, sep } from 'node:path';

const [directory, ...extra] = process.argv.slice(2);
assert.ok(directory && extra.length === 0, 'expected the owning run directory');
const run = JSON.parse(await readFile(join(directory, 'run.json'), 'utf8'));
const require = createRequire(join(directory, 'npm-site', 'package.json'));
const { CostTracker } = require('@wisent/cost-tracker');
const entry = await realpath(require.resolve('@wisent/cost-tracker'));
assert.ok(entry.startsWith(`${await realpath(join(directory, 'npm-site'))}${sep}`), 'SDK came from outside the installed package');
const cost = 1 / 65_536;
const agent = randomUUID();
const input = (marker, amount) => ({
  service: 'qualification_usage', usage_type: 'units', usage_amount: 1, cost_usd: amount,
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
assert.equal(memory.record(input('first', cost)).cost_usd, cost);
const first = memory.flush();
memory.record(input('second', 2 * cost));
const second = memory.flush();
await Promise.all([first, second]);
memory.record(input('explicit-free', 0));
await memory.flush();
const memoryRows = await memory.getSink().read();
assert.deepEqual(memoryRows.map(row => [row.reference_id, row.cost_usd, row.agent_id]), [
  ['first', cost, agent], ['second', 2 * cost, agent], ['explicit-free', 0, agent],
]);
assert.equal(memory.total(), 3 * cost);
assert.equal(memory.snapshot().service_costs.qualification_usage, 3 * cost);

const files = join(directory, 'files');
await mkdir(files, { recursive: true });
const path = join(files, 'node-records.json');
const tracker = new CostTracker({ agent_id: agent, sink: 'file', filePath: path, autoFlush: false });
tracker.record(input('file-first', cost));
const initialWrite = tracker.flush();
tracker.record(input('file-second', 2 * cost));
const laterWrite = tracker.flush();
await Promise.all([initialWrite, laterWrite]);
await tracker.flush();
const bytes = await readFile(path, 'utf8');
const fileRows = JSON.parse(bytes);
assert.deepEqual(fileRows.map(row => [row.reference_id, row.cost_usd, row.agent_id]), [
  ['file-first', cost, agent], ['file-second', 2 * cost, agent],
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

console.log(JSON.stringify({
  entry, node: process.version, executable: process.execPath, memoryRecords: memoryRows,
  file: path, fileRecords: fileRows, snapshot: tracker.snapshot(), corruptFilePreserved: corruptPath,
}));
