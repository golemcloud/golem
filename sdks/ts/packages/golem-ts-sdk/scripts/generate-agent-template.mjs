import { spawnSync } from 'node:child_process';
import { cpSync, existsSync, readdirSync, readFileSync, rmSync, statSync } from 'node:fs';
import { join, resolve } from 'node:path';

// ---------------------------------------------------------------------------
// Temporary Slice 4 migration workaround.
//
// The SDK still needs golem:core@1.5.0 for the deferred legacy value/type
// mapper (src/internal/mapping/**), but the `agent-guest` world only uses
// golem:core@2.0.0. Once golem:core@2.0.0 also declares parse-uuid /
// uuid-to-string (Slice 4), keeping BOTH core versions under the WIT path
// breaks wit_bindgen's Rust module naming: it version-suffixes the modules
// (golem::core2_0_0 / golem::core1_5_0), but wasm-rquickjs' generated
// `conversions.rs` assumes the unsuffixed `crate::bindings::golem::core::types`.
//
// Fix: generate the agent-template wrapper crate from a filtered copy of the
// SDK WIT that drops the unused golem:core@1.5.0 package, so only one core
// version is generated and the unsuffixed module path stays valid. The
// SDK-wide WIT directory is left untouched (generate-dts / rollup / the legacy
// mapper still need golem:core@1.5.0).
//
// Remove this filter in Slice 5, when the legacy mapper is deleted and
// golem:core@1.5.0 is dropped from sdks/ts/wit entirely.
// ---------------------------------------------------------------------------

const sourceWit = resolve(process.cwd(), '../../wit');
const filteredWit = resolve(process.cwd(), '.agent-template-wit');

rmSync(filteredWit, { recursive: true, force: true });
cpSync(sourceWit, filteredWit, { recursive: true });

const legacyCore = join(filteredWit, 'deps', 'golem-core');
const coreV2 = join(filteredWit, 'deps', 'golem-core-v2');

if (!existsSync(coreV2)) {
  throw new Error('Expected golem-core-v2 (golem:core@2.0.0) in the agent-template WIT');
}

// Guard: if the agent-guest world (or anything it pulls in) ever starts
// referencing golem:core@1.5.0, fail loudly instead of silently masking it.
function walk(dir) {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry);
    return statSync(path).isDirectory() ? walk(path) : [path];
  });
}

const legacyCoreRef = /golem:core\/[a-z-]+@1\.5\.0/;
const offenders = walk(filteredWit)
  .filter((path) => path.endsWith('.wit'))
  .filter((path) => !path.startsWith(legacyCore))
  .filter((path) => legacyCoreRef.test(readFileSync(path, 'utf8')));

if (offenders.length > 0) {
  rmSync(filteredWit, { recursive: true, force: true });
  throw new Error(
    `Cannot remove golem:core@1.5.0 from the agent-template WIT; it is referenced by:\n` +
      offenders.join('\n'),
  );
}

if (existsSync(legacyCore)) {
  rmSync(legacyCore, { recursive: true, force: true });
}

const result = spawnSync(
  'wasm-rquickjs',
  [
    'generate-wrapper-crate',
    '--wit',
    filteredWit,
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

rmSync(filteredWit, { recursive: true, force: true });

if (result.error) {
  throw result.error;
}

if (result.status !== 0) {
  process.exit(result.status ?? 1);
}
