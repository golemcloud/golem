import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

// npm can intercept assertion failures and exit successfully before the checks finish.
// Keep the completion check outside the process that installs npm's handlers.
const result = spawnSync(
  process.execPath,
  [
    '--experimental-strip-types',
    fileURLToPath(new URL('./cli-completion.fixture.mts', import.meta.url)),
  ],
  {
    encoding: 'utf8',
    timeout: 30_000,
    killSignal: 'SIGKILL',
    maxBuffer: 1024 * 1024,
  },
);
const output = `stdout:\n${result.stdout ?? ''}\nstderr:\n${result.stderr ?? ''}`;
assert.ifError(result.error);
assert.equal(result.signal, null, output);
assert.equal(result.status, 0, output);
assert.ok(
  result.stdout.split(/\r?\n/).includes('Bundled CLI completion checks passed'),
  `Bundled CLI completion checks did not finish\n${output}`,
);
process.stdout.write(result.stdout);
process.stderr.write(result.stderr);
