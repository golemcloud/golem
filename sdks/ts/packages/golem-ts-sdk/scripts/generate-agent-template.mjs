import { spawnSync } from 'node:child_process';
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';

// ---------------------------------------------------------------------------
// The TypeScript SDK is fully migrated to golem:core@2.0.0 and no longer ships
// golem:core@1.5.0 in its WIT (dropped by the `wit-sdks` task in Makefile.toml).
// Keeping both core versions would make wasm-rquickjs emit version-suffixed
// module paths (e.g. golem::core1_5_0) and break the generated wrapper crate,
// so we assert the legacy package is absent before generating the wrapper.
// ---------------------------------------------------------------------------

const sourceWit = resolve(process.cwd(), '../../wit');

function walk(dir) {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry);
    return statSync(path).isDirectory() ? walk(path) : [path];
  });
}

const legacyCoreRef = /golem:core(\/[a-z-]+)?@1\.5\.0/;
const offenders = walk(sourceWit)
  .filter((path) => path.endsWith('.wit'))
  .filter((path) => legacyCoreRef.test(readFileSync(path, 'utf8')));

if (offenders.length > 0) {
  throw new Error(
    `golem:core@1.5.0 must not be present in the TypeScript SDK WIT, but it is referenced by:\n` +
      offenders.join('\n') +
      `\nRe-run \`cargo make wit\` to re-sync the WIT dependencies.`,
  );
}

const result = spawnSync(
  'wasm-rquickjs',
  [
    'generate-wrapper-crate',
    '--wit',
    sourceWit,
    '--output',
    'agent-template',
    '--world',
    'agent-guest',
    '--js-modules',
    '@golemcloud/golem-ts-sdk=dist/index.mjs',
    '--js-modules',
    'user=@slot',
  ],
  {
    stdio: 'inherit',
  },
);

if (result.error) {
  throw result.error;
}

if (result.status !== 0) {
  process.exit(result.status ?? 1);
}
