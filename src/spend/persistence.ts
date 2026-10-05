import { randomUUID } from 'node:crypto';
import { mkdir, open, rename, rm } from 'node:fs/promises';
import { basename, dirname, join } from 'node:path';

/** Publish a complete replacement, never an interrupted JSON document. */
export async function replaceFile(path: string, contents: string): Promise<void> {
  const directory = dirname(path);
  await mkdir(directory, { recursive: true });
  const temporary = join(directory, `.${basename(path)}.${randomUUID()}.tmp`);
  const file = await open(temporary, 'wx', 0o600);
  let errors: unknown[] | undefined;
  try {
    await file.writeFile(contents, 'utf8');
    await file.sync();
  } catch (cause) {
    errors = [cause];
  }
  try {
    await file.close();
  } catch (cause) {
    if (errors === undefined) errors = [cause];
    else errors.push(cause);
  }
  if (!errors) {
    try {
      await rename(temporary, path);
      return;
    } catch (cause) {
      errors = [cause];
    }
  }
  try {
    await rm(temporary, { force: true });
  } catch (cause) {
    errors.push(cause);
  }
  const detail = errors.map(error => error instanceof Error ? error.message : String(error)).join('\n');
  throw new AggregateError(errors, `FileSink replacement of ${path} failed:\n${detail}`);
}
