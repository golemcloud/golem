import { spawnSync } from 'node:child_process';
import { readdirSync, readFileSync, rmSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { templateMatrix } from './template-matrix.mjs';
import {
  templateGeneratorArgs,
  toolProcessModules,
} from '../../../../javascript/tool-process-template.mjs';

// ---------------------------------------------------------------------------
// The TypeScript SDK is fully migrated to golem:core@2.0.0 and no longer ships
// golem:core@1.5.0 in its WIT (dropped by the `wit-sdks` task in Makefile.toml).
// Keeping both core versions would make wasm-rquickjs emit version-suffixed
// module paths (e.g. golem::core1_5_0) and break the generated wrapper crate,
// so we assert the legacy package is absent before generating the wrapper.
// ---------------------------------------------------------------------------

const sourceWit = resolve(process.cwd(), '../../wit');
const sharedModules = [
  ['@golemcloud/golem-ts-sdk/middleware', 'dist/middleware.mjs'],
  ...toolProcessModules,
];

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

for (const template of templateMatrix) {
  rmSync(template.wrapperDirectory, { recursive: true, force: true });

  const modules = [[template.sdkModuleName, template.sdkEntry], ...sharedModules];

  const result = spawnSync(
    'wasm-rquickjs',
    [
      'generate-wrapper-crate',
      '--wit',
      sourceWit,
      '--output',
      template.wrapperDirectory,
      '--world',
      template.world,
      '--target',
      'wasi-p3',
      ...modules.flatMap(([name, entry]) => ['--js-modules', `${name}=${entry}`]),
      '--js-modules',
      'user=@slot',
      ...templateGeneratorArgs,
    ],
    {
      stdio: 'inherit',
    },
  );

  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
