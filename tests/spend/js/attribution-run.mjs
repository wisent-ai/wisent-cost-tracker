// Build and exercise attribution plus the installed SDK's local spend journey,
// retaining source identity, hashes, commands, exit statuses and process output.
// Usage: node tests/spend/js/attribution-run.mjs [Node test-runner options]
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { closeSync, mkdirSync, openSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../../', import.meta.url));
const [, , ...options] = process.argv;
const id = randomUUID();
const directory = join(root, '.build', 'attribution', id);
mkdirSync(directory, { recursive: true });
const report = { directory, startedAt: new Date().toISOString(), source: {}, commands: [] };
const reportFile = join(directory, 'report.json');
report.runtime = { node: process.version, executable: process.execPath, platform: process.platform, arch: process.arch };
function save() {
  writeFileSync(reportFile, JSON.stringify(report, null, '\t'));
}
function command(program, args, name) {
  const stdout = join(directory, `${name}.stdout`);
  const stderr = join(directory, `${name}.stderr`);
  const out = openSync(stdout, 'w');
  let result;
  try {
    const err = openSync(stderr, 'w');
    try {
      result = spawnSync(program, args, { cwd: root, stdio: ['ignore', out, err] });
    } finally {
      closeSync(err);
    }
  } finally {
    closeSync(out);
  }
  const output = readFileSync(stdout, 'utf8');
  const errors = readFileSync(stderr, 'utf8');
  const run = {
    program, args, cwd: root, exitStatus: result.status, signal: result.signal,
    error: result.error?.message, stdout, stderr,
  };
  report.commands.push(run);
  save();
  process.stdout.write(output);
  process.stderr.write(errors);
  if (result.error || result.signal || result.status === null || result.status) {
    throw new Error(`${program} ${args.join(' ')} failed: ${JSON.stringify(run)}`);
  }
  return output.trim();
}
function hashes(paths) {
  const result = {};
  for (const path of paths) {
    result[path] = createHash('sha256').update(readFileSync(join(root, path))).digest('hex');
  }
  return result;
}
function files(directory) {
  return readdirSync(join(root, directory), { withFileTypes: true }).flatMap((entry) => {
    const path = join(directory, entry.name);
    return entry.isDirectory() ? files(path) : [path];
  });
}
function sourcePaths() {
  return [
    ...files('src'), ...files('tests/spend/js'),
    'README.md', 'docs/record-helpers.md', 'pricing/costs.json',
    'package.json', 'package-lock.json', 'tsconfig.json',
  ].sort();
}
try {
  report.source.revision = command('git', ['rev-parse', 'HEAD'], 'revision');
  report.source.status = command('git', ['status', '--porcelain'], 'status');
  const paths = sourcePaths();
  command('git', ['diff', '--cached', '--binary', '--', ...paths], 'source-diff');
  command('git', ['ls-files', '--error-unmatch', '--', ...paths], 'tracked-inputs');
  command('git', ['diff', '--exit-code', '--', ...paths], 'index-agreement');
  report.source.stagedTree = command('git', ['write-tree'], 'staged-tree');
  report.source.hashes = hashes(paths);
  save();
  command('npm', ['run', 'build'], 'build');
  report.artifacts = hashes(files('dist').sort());
  save();
  command(process.execPath, ['--test', ...options, 'tests/spend/js/attribution.test.mjs'], 'journey');
  const packed = JSON.parse(command('npm', ['pack', '--ignore-scripts', '--json', '--pack-destination', directory], 'pack'));
  const archive = packed.find((entry) => entry.name === '@wisent/cost-tracker');
  if (!archive?.filename) throw new Error('npm pack did not return the @wisent/cost-tracker archive');
  command('npm', ['install', '--prefix', join(directory, 'npm-site'), '--ignore-scripts', '--no-audit', '--no-fund', join(directory, archive.filename)], 'install');
  writeFileSync(join(directory, 'run.json'), JSON.stringify({ id }));
  command(process.execPath, ['tests/spend/js/local.mjs', directory], 'local-spend');
  if (JSON.stringify(sourcePaths()) !== JSON.stringify(paths) ||
      JSON.stringify(hashes(paths)) !== JSON.stringify(report.source.hashes)) {
    throw new Error('Source changed during the run; the result does not qualify this source');
  }
  report.passed = true;
} catch (error) {
  report.passed = false;
  report.error = error instanceof Error ? error.message : String(error);
  throw error;
} finally {
  report.finishedAt = new Date().toISOString();
  save();
  console.log(`Attribution report: ${relative(root, reportFile)}`);
}
