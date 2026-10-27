// What CostTracker attributes a record to, what it keeps of the caller's
// metadata, how its rollup sums every service, and how its exit hooks end a
// process, read from the package's own build (`npm run build` writes dist/).
// Files go under the checkout's ignored .build/ directory and are removed
// afterwards.
//
//   npm run build && node --test tests/spend/js/attribution.test.mjs
import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { cp, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { after, test } from 'node:test';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const dist = join(root, 'dist', 'index.js');
const { CostTracker } = createRequire(import.meta.url)(dist);
const scratch = join(root, '.build', 'tests', `attribution-${randomUUID()}`);
await mkdir(scratch, { recursive: true });
after(() => rm(scratch, { recursive: true, force: true }));

/** A record as a caller writes one, with nothing of an agent: the fields a
 *  helper records for one email at the pricing table's price, and `metadata`. */
function input(metadata) {
  const helper = new CostTracker({ agent_id: randomUUID(), sink: 'memory', autoFlush: false });
  const { agent_id: _agent, metadata: _metadata, created_at: _created, ...fields } = helper.recordEmail('resend');
  return { ...fields, metadata };
}

test('a record naming another agent is refused before it is buffered; the returned, snapshot and flushed records carry the tracker agent', async () => {
  const own = randomUUID();
  const other = randomUUID();
  const tracker = new CostTracker({ agent_id: own, sink: 'memory', autoFlush: false });
  assert.throws(() => tracker.record({ ...input({}), agent_id: other }), (error) =>
    error instanceof TypeError && error.message.includes(other) && error.message.includes(own));
  assert.deepEqual(tracker.snapshot().records, [], 'a refused record entered the buffer');
  const returned = [tracker.record({ ...input({}), agent_id: own }), tracker.record(input({}))];
  assert.deepEqual(returned.map((row) => row.agent_id), [own, own]);
  assert.deepEqual(tracker.snapshot().records.map((row) => row.agent_id), [own, own]);
  await tracker.flush();
  assert.deepEqual((await tracker.getSink().read()).map((row) => row.agent_id), [own, own]);
});

test('a returned record cannot move buffered spend to another agent or erase its attribution', async () => {
  const own = randomUUID();
  const file = join(scratch, `ownership-${randomUUID()}.json`);
  const tracker = new CostTracker({ agent_id: own, sink: 'file', filePath: file, autoFlush: false });
  const returned = tracker.record(input({}));
  assert.equal(Reflect.set(returned, 'agent_id', randomUUID()), false);
  assert.equal(Reflect.deleteProperty(returned, 'agent_id'), false);
  assert.throws(() => Object.defineProperty(returned, 'agent_id', { value: randomUUID() }), TypeError);
  assert.throws(() => { returned.agent_id = randomUUID(); }, TypeError);
  assert.equal(returned.agent_id, own);
  await tracker.flush();
  assert.deepEqual(JSON.parse(await readFile(file, 'utf8')).map((row) => row.agent_id), [own]);
});

test('a record keeps a shallow copy of the caller metadata, and an empty one without it', () => {
  const tracker = new CostTracker({ agent_id: randomUUID(), sink: 'memory', autoFlush: false });
  const given = { marker: randomUUID() };
  const kept = tracker.record(input(given));
  const marker = kept.metadata.marker;
  given.marker = randomUUID();
  assert.equal(kept.metadata.marker, marker, 'the record follows the caller object');
  assert.deepEqual(tracker.record(input(undefined)).metadata, {});
});

test('metadata rejects values that cannot preserve a plain record before buffering', () => {
  const tracker = new CostTracker({ agent_id: randomUUID(), sink: 'memory', autoFlush: false });
  for (const metadata of [null, [], new Date(), new Map(), 'metadata', true]) {
    assert.throws(() => tracker.record(input(metadata)), {
      name: 'TypeError',
      message: 'CostTracker.record: metadata must be a plain object when supplied; omit it when no metadata is available',
    });
    assert.deepEqual(tracker.snapshot().records, []);
  }
});

test('non-owner record fields remain mutable and the sink receives their current values', async () => {
  const tracker = new CostTracker({ agent_id: randomUUID(), sink: 'memory', autoFlush: false });
  const returned = tracker.record(input({}));
  returned.cost_usd = returned.cost_usd + returned.cost_usd;
  const expected = returned.cost_usd;
  await tracker.flush();
  assert.deepEqual((await tracker.getSink().read()).map((row) => row.cost_usd), [expected]);
});

test('the rollup sums each service independently, including __proto__', () => {
  const tracker = new CostTracker({ agent_id: randomUUID(), sink: 'memory', autoFlush: false });
  for (const service of ['__proto__', 'email_resend', '__proto__']) tracker.record({ ...input({}), service });
  const { service_costs } = tracker.snapshot();
  const cost = input({}).cost_usd;
  assert.deepEqual(service_costs, { ['__proto__']: cost + cost, email_resend: cost });
});

test('compute charges group qualified and bare instance types by provider', async () => {
  const file = join(scratch, `${randomUUID()}-compute.json`);
  const tracker = new CostTracker({ agent_id: randomUUID(), sink: 'file', filePath: file, autoFlush: false });
  const usage = input({});
  for (const instance of ['cloud_machine', 'local']) tracker.recordCompute(instance, usage.usage_amount, usage.cost_usd);
  await tracker.flush();
  assert.deepEqual(JSON.parse(await readFile(file, 'utf8')).map((row) => row.service), ['compute_cloud', 'compute_local']);
});

/** The child process for `mode` writing `files`, its readiness and its
 *  ending; a child that ends before it is ready rejects the readiness with
 *  how it ended. */
function child(t, mode, agent, files) {
  const script = fileURLToPath(new URL('./signal-child.mjs', import.meta.url));
  const running = spawn(process.execPath, [script, dist, agent, mode, ...files], { stdio: ['pipe', 'pipe', 'pipe'] });
  t.after(() => { if (running.exitCode === null && running.signalCode === null) running.kill('SIGKILL'); });
  let said = '';
  let stderr = '';
  running.stderr.on('data', (chunk) => { stderr += chunk; });
  const ended = new Promise((resolve, reject) => {
    running.once('error', reject);
    running.once('close', (code, signal) => resolve({ code, signal, said, stderr }));
  });
  function observed(line) {
    return new Promise((resolve, reject) => {
      const inspect = () => {
        if (said.includes(line)) {
          running.stdout.off('data', inspect);
          resolve();
        }
      };
      running.stdout.on('data', inspect);
      inspect();
      ended.then(
        (ending) => reject(new Error(`the child ended before ${JSON.stringify(line)}: ${JSON.stringify(ending)}`)),
        reject,
      ).finally(() => running.stdout.off('data', inspect));
    });
  }
  running.stdout.on('data', (chunk) => { said += chunk; });
  const ready = observed('ready\n');
  return { running, ready, ended, observed };
}

/** The agent of each record a file holds, and its service. */
async function written(file) {
  return JSON.parse(await readFile(file, 'utf8')).map((row) => [row.service, row.agent_id]);
}

test('natural exit persists its pending record without a signal', async (t) => {
  const agent = randomUUID();
  const file = join(scratch, `${agent}-natural.json`);
  const run = child(t, 'natural', agent, [file]);
  await run.ready;
  const ended = await run.ended;
  assert.equal(ended.signal, null);
  assert.equal(ended.code, 0);
  assert.equal(ended.stderr, '');
  assert.deepEqual(await written(file), [['email_resend', agent]]);
});

test('natural exit reports one failed file flush and still ends', async (t) => {
  const file = join(scratch, `${randomUUID()}-natural-invalid.json`);
  await writeFile(file, '{}');
  const run = child(t, 'natural', randomUUID(), [file]);
  await run.ready;
  const ended = await run.ended;
  assert.equal(ended.signal, null);
  assert.equal(ended.code, 0);
  assert.equal(ended.stderr, `CostTracker: flushing spend records failed: FileSink cannot parse ${file}: expected a JSON array\n`);
  assert.equal(await readFile(file, 'utf8'), '{}');
});

test('new spend re-arms natural exit after a failed flush', async (t) => {
  const file = join(scratch, `${randomUUID()}-recovered.json`);
  await writeFile(file, '{}');
  const run = child(t, 'natural-recovery', randomUUID(), [file]);
  await run.ready;
  const ended = await run.ended;
  assert.equal(ended.signal, null);
  assert.equal(ended.code, 0);
  assert.equal(ended.stderr, `CostTracker: flushing spend records failed: FileSink cannot parse ${file}: expected a JSON array\n`);
  assert.deepEqual(JSON.parse(await readFile(file, 'utf8')).map((row) => row.reference_id), [undefined, 'recovered']);
});

for (const signal of ['SIGINT', 'SIGTERM']) {
  test(`${signal} coordinates trackers loaded from separate package copies`, async (t) => {
    const agent = randomUUID();
    const second = join(scratch, `${agent}-package`);
    await cp(dirname(dist), second, { recursive: true });
    const files = [join(scratch, `${agent}-copy-a.json`), join(scratch, `${agent}-copy-b.json`)];
    const run = child(t, 'copies', agent, [...files, join(second, 'index.js')]);
    await run.ready;
    run.running.kill(signal);
    const ended = await run.ended;
    assert.equal(ended.signal, signal);
    for (const file of files) assert.deepEqual(await written(file), [['email_resend', agent]]);
  });

  for (const mode of ['once-before', 'once-after']) {
    test(`${signal} preserves an application once listener registered ${mode}`, async (t) => {
      const agent = randomUUID();
      const file = join(scratch, `${agent}-${mode}.json`);
      const run = child(t, mode, agent, [file]);
      await run.ready;
      run.running.kill(signal);
      await run.observed('application handled signal\n');
      run.running.stdin.end();
      const ended = await run.ended;
      assert.equal(ended.signal, null, JSON.stringify(ended));
      assert.match(ended.said, new RegExp(`handled: ${JSON.stringify([signal]).replaceAll('[', '\\[').replaceAll(']', '\\]')}`));
      assert.deepEqual(await written(file), [['email_resend', agent]]);
    });
  }

  test(`${signal} flushes both trackers of a process and ends it by ${signal}`, async (t) => {
    const agent = randomUUID();
    const files = [join(scratch, `${agent}-a.json`), join(scratch, `${agent}-b.json`)];
    const run = child(t, 'trackers', agent, files);
    await run.ready;
    run.running.kill(signal);
    const ended = await run.ended;
    assert.equal(ended.signal, signal, `the process ended with ${JSON.stringify(ended)}`);
    for (const file of files) assert.deepEqual(await written(file), [['email_resend', agent]]);
  });

  test(`an application ${signal} listener decides how the process ends`, async (t) => {
    const agent = randomUUID();
    const files = [join(scratch, `${agent}-application-a.json`), join(scratch, `${agent}-application-b.json`)];
    const run = child(t, 'application', agent, files);
    await run.ready;
    run.running.kill(signal);
    const ended = await run.ended;
    assert.equal(ended.signal, null, `the process ended with ${JSON.stringify(ended)}`);
    assert.match(ended.said, new RegExp(`application listener ended the process on ${signal}`));
    for (const file of files) assert.deepEqual(await written(file), [['email_resend', agent]]);
  });

  test(`a non-exiting application ${signal} listener runs once and retains its shutdown decision`, async (t) => {
    const agent = randomUUID();
    const file = join(scratch, `${agent}.json`);
    const run = child(t, 'application-managed', agent, [file]);
    await run.ready;
    const handled = run.observed('application handled signal\n');
    run.running.kill(signal);
    await handled;
    run.running.stdin.end();
    const ended = await run.ended;
    assert.equal(ended.signal, null, `the process ended with ${JSON.stringify(ended)}`);
    const signals = ended.said.split('\n').filter((line) => line.startsWith('handled: '));
    assert.deepEqual(signals, [`handled: ${JSON.stringify([signal])}`]);
    assert.deepEqual(await written(file), [['email_resend', agent]]);
  });

  test(`a surviving application receives ${signal} twice and records between signals are persisted`, async (t) => {
    const agent = randomUUID();
    const file = join(scratch, `${agent}-repeated.json`);
    const run = child(t, 'application-passive', agent, [file]);
    await run.ready;
    run.running.kill(signal);
    await run.observed('persisted: [null]\n');
    run.running.stdin.write('record\n');
    await run.observed('recorded second\n');
    run.running.kill(signal);
    await run.observed('persisted: ["second"]\n');
    assert.deepEqual(await written(file), [['email_resend', agent], ['email_resend', agent]]);
    run.running.stdin.end();
    const ended = await run.ended;
    assert.match(ended.said, new RegExp(`handled: ${JSON.stringify([signal, signal]).replaceAll('[', '\\[').replaceAll(']', '\\]')}`));
  });

  test(`${signal} reports the real file sink failure before ending without overwriting the file`, async (t) => {
    const agent = randomUUID();
    const file = join(scratch, `${agent}-invalid.json`);
    await writeFile(file, '{}');
    const run = child(t, 'trackers', agent, [file]);
    await run.ready;
    run.running.kill(signal);
    const ended = await run.ended;
    assert.equal(ended.signal, signal);
    assert.equal(ended.stderr, `CostTracker: flushing spend records failed: FileSink cannot parse ${file}: expected a JSON array\n`);
    assert.equal(await readFile(file, 'utf8'), '{}');
  });

  test(`repeated ${signal} waits for a blocked real file sink and persists after its writer answers`, async (t) => {
    const agent = randomUUID();
    const file = join(scratch, `${agent}.fifo`);
    execFileSync('mkfifo', [file]);
    const run = child(t, 'blocked-file', agent, [file]);
    await run.ready;
    run.running.kill(signal);
    await run.observed('writing\n');
    run.running.kill(signal);
    run.running.stdin.write('ping\n');
    await run.observed('responsive\n');
    await writeFile(file, '[]');
    const ended = await run.ended;
    assert.equal(ended.signal, signal);
    assert.equal(ended.stderr, '');
    assert.deepEqual(await written(file), [['email_resend', agent]]);
  });
}
