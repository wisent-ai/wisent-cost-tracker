// A process that records one email through the built package with its exit
// hooks on, says `ready`, and waits on its standard input until a signal ends
// it. Its arguments: the package's entry, the agent, the mode, and the files
// its trackers write, one tracker per file.
//
// - `trackers`: no other listener, so the last tracker's flush raises the
//   signal again;
// - `application`: the application's listener flushes every tracker and exits.
// - `application-managed`: the listener stays alive until stdin closes.
// - `application-passive`: the application leaves persistence to the hooks.
// - `blocked-file`: the real file sink waits for a FIFO's writer.
// - `once-before` / `once-after`: an application once listener registered
//   before / after tracker construction keeps the process alive until EOF.
// - `copies`: the final argument is a second built package entry; its
//   tracker writes the second file.
// - `natural`: no stdin handle, so beforeExit performs the final flush.
// - `natural-recovery`: repair an owned failed sink and record new spend.
import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';

const [, , dist, agent, mode, ...files] = process.argv;
const { CostTracker } = createRequire(import.meta.url)(dist);
const handled = [];
const applicationOnce = () => {
  for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, () => {
    handled.push(signal);
    console.log('application handled signal');
  });
};
if (mode === 'once-before') applicationOnce();
const constructors = [CostTracker];
if (mode === 'copies') constructors.push(createRequire(import.meta.url)(files.pop()).CostTracker);
const trackers = files.map((file, index) => {
  const Tracker = constructors[index % constructors.length];
  return new Tracker({ agent_id: agent, sink: 'file', filePath: file });
});
for (const tracker of trackers) tracker.recordEmail('resend');
if (mode === 'once-after') applicationOnce();
if (mode === 'once-before' || mode === 'once-after') {
  process.stdin.on('end', () => console.log(`handled: ${JSON.stringify(handled)}`));
}
if (mode === 'application-passive' || mode === 'blocked-file') {
  for (const tracker of trackers) {
    const sink = tracker.getSink();
    const write = sink.write.bind(sink);
    sink.write = async (records) => {
      console.log('writing');
      await write(records);
      console.log(`persisted: ${JSON.stringify(records.map((record) => record.reference_id))}`);
    };
  }
}
if (mode === 'blocked-file') process.stdin.on('data', () => console.log('responsive'));
if (mode === 'application-passive') {
  for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => {
    handled.push(signal);
    console.log(`handled: ${JSON.stringify(handled)}`);
  });
  process.stdin.on('data', () => {
    for (const tracker of trackers) tracker.recordEmail('resend').reference_id = 'second';
    console.log('recorded second');
  });
}
if (mode === 'application-managed') {
  for (const signal of ['SIGINT', 'SIGTERM']) {
    process.on(signal, async () => {
      handled.push(signal);
      await Promise.all(trackers.map((tracker) => tracker.flush()));
      console.log('application handled signal');
    });
  }
  process.stdin.on('end', () => console.log(`handled: ${JSON.stringify(handled)}`));
}
if (mode === 'application') {
  for (const signal of ['SIGINT', 'SIGTERM']) {
    process.on(signal, async () => {
      await Promise.all(trackers.map((tracker) => tracker.flush()));
      console.log(`application listener ended the process on ${signal}`);
      process.exit();
    });
  }
}
if (mode === 'natural-recovery') {
  process.once('beforeExit', async () => {
    await assert.rejects(trackers[0].flush(), /FileSink cannot parse/);
    await writeFile(files[0], '[]');
    trackers[0].recordEmail('resend').reference_id = 'recovered';
  });
}
if (mode !== 'natural' && mode !== 'natural-recovery') process.stdin.resume();
console.log('ready');
