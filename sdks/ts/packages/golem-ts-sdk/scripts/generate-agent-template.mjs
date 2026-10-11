import { spawnSync } from 'node:child_process';
import { readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { join, relative, resolve, sep } from 'node:path';
import { templateMatrix } from './template-matrix.mjs';

// ---------------------------------------------------------------------------
// The generated Preview 3 wrapper must build its async WIT bindings with Golem's
// forked wit-bindgen, which adds an "outline-lift" optimization that shrinks the
// giant generated lift/lower wrappers. wasm-rquickjs hardcodes the upstream
// dependency in its skeleton Cargo.toml and exposes no flag to override it, so
// rewrite that dependency after generation.
// ---------------------------------------------------------------------------
const WIT_BINDGEN_GIT = 'https://github.com/golemcloud/wit-bindgen';
const WIT_BINDGEN_REVISION = '4407232ead86d9bcbd06cbebd790a52120a4087a';

function useForkedWitBindgen(cargoTomlPath) {
  const original = readFileSync(cargoTomlPath, 'utf8');

  const witBindgenLine =
    'wit-bindgen-p3 = { package = "wit-bindgen", version = "0.58.0", default-features = false, features = ["async", "async-spawn", "macros", "inter-task-wakeup"], optional = true }';
  const forkedLine = `wit-bindgen-p3 = { package = "wit-bindgen", git = "${WIT_BINDGEN_GIT}", rev = "${WIT_BINDGEN_REVISION}", version = "=0.59.0", default-features = false, features = ["async", "async-spawn", "macros", "inter-task-wakeup"], optional = true }`;

  const witBindgenCount = original.split(witBindgenLine).length - 1;
  if (witBindgenCount !== 1) {
    throw new Error(
      `Expected exactly one occurrence of the wit-bindgen dependency line in ${cargoTomlPath}, found ${witBindgenCount}. ` +
        `The wasm-rquickjs skeleton may have changed; update generate-agent-template.mjs.`,
    );
  }

  const updated = original.replace(witBindgenLine, forkedLine);

  if (!updated.includes(WIT_BINDGEN_REVISION) || updated.includes(witBindgenLine)) {
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
const sharedModules = [['@golemcloud/golem-ts-sdk/middleware', 'dist/middleware.mjs']];

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
    ],
    {
      stdio: 'inherit',
    },
  );

  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);

  useForkedWitBindgen(resolve(process.cwd(), template.wrapperDirectory, 'Cargo.toml'));

  // The pinned P3 Node HTTP reader must release its body even while a read is pending.
  // This owning-layer patch targets wasm-rquickjs v0.4.10 and fails on source drift.
  const root = spawnSync('git', ['rev-parse', '--show-toplevel'], { encoding: 'utf8' });
  if (root.error) throw root.error;
  if (root.status !== 0) throw new Error('Runtime patching requires the source Git checkout.');
  const gitRoot = root.stdout.trim();
  const wrapper = resolve(process.cwd(), template.wrapperDirectory);
  const patch = spawnSync(
    'git',
    [
      'apply',
      '--directory',
      relative(gitRoot, wrapper).split(sep).join('/'),
      resolve(process.cwd(), 'scripts/patches/wasm-rquickjs-0.4.10-http-cancellation.patch'),
    ],
    { cwd: gitRoot, stdio: 'inherit' },
  );
  if (patch.error) throw patch.error;
  if (patch.status !== 0) process.exit(patch.status ?? 1);
  if (
    !readFileSync(join(wrapper, 'src/builtin/node_http.js'), 'utf8').includes(
      'this._pumpPromise = this._pumpBody()',
    ) ||
    !readFileSync(join(wrapper, 'src/builtin/node_http_p3.rs'), 'utf8').includes(
      'struct IncomingBody',
    )
  ) {
    throw new Error('Node HTTP cancellation patch was not applied to the generated runtime.');
  }
}
