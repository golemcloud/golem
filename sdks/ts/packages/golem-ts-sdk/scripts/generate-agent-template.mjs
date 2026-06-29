import { spawnSync } from 'node:child_process';
import { readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';

// ---------------------------------------------------------------------------
// The generated wrapper crate must build its WIT bindings with Golem's forked
// wit-bindgen, which adds an "outline-lift" optimization that shrinks the giant
// generated lift/lower wrappers. wasm-rquickjs hardcodes the upstream wit-bindgen
// version in its skeleton Cargo.toml and exposes no flag to override it, so we
// rewrite the generated manifest after generation.
// ---------------------------------------------------------------------------
const WIT_BINDGEN_GIT = 'https://github.com/golemcloud/wit-bindgen';
const WIT_BINDGEN_BRANCH = 'golem-outline-lift-v0.58.0';

function useForkedWitBindgen(cargoTomlPath) {
  const original = readFileSync(cargoTomlPath, 'utf8');

  const witBindgenLine =
    'wit-bindgen = { version = "0.42.1", default-features = false, features = ["macros"] }';
  const witBindgenRtLine = 'wit-bindgen-rt = { version = "0.42.1", features = ["bitflags"] }';
  const forkedLine = `wit-bindgen = { git = "${WIT_BINDGEN_GIT}", branch = "${WIT_BINDGEN_BRANCH}", version = "=0.58.0", default-features = false, features = ["macros"] }`;

  const witBindgenCount = original.split(witBindgenLine).length - 1;
  if (witBindgenCount !== 1) {
    throw new Error(
      `Expected exactly one occurrence of the wit-bindgen dependency line in ${cargoTomlPath}, found ${witBindgenCount}. ` +
        `The wasm-rquickjs skeleton may have changed; update generate-agent-template.mjs.`,
    );
  }

  // The forked wit-bindgen embeds its runtime, so the separate wit-bindgen-rt
  // crate is dropped.
  const rtCount = original.split(witBindgenRtLine).length - 1;
  if (rtCount !== 1) {
    throw new Error(
      `Expected exactly one occurrence of the wit-bindgen-rt dependency line in ${cargoTomlPath}, found ${rtCount}. ` +
        `The wasm-rquickjs skeleton may have changed; update generate-agent-template.mjs.`,
    );
  }

  const updated = original.replace(`${witBindgenRtLine}\n`, '').replace(witBindgenLine, forkedLine);

  if (!updated.includes(WIT_BINDGEN_GIT) || updated.includes(witBindgenRtLine)) {
    throw new Error(`Failed to rewrite the wit-bindgen dependency in ${cargoTomlPath}.`);
  }

  writeFileSync(cargoTomlPath, updated);

  // wasm-rquickjs emits a Cargo.lock pinned to the upstream wit-bindgen deps it
  // hardcodes. After swapping in the fork, that lock conflicts (e.g. it pins
  // indexmap below what the fork's wit-parser requires), so drop it and let
  // cargo resolve a fresh lock during the build.
  rmSync(join(cargoTomlPath, '..', 'Cargo.lock'), { force: true });
}

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

useForkedWitBindgen(resolve(process.cwd(), 'agent-template', 'Cargo.toml'));
