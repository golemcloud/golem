// Runs the SDK micro-benchmarks under QuickJS (wasm-rquickjs), the same engine
// Golem TS components execute on in production. Node/V8 `vitest bench` numbers
// are not representative of the deployed runtime, so this harness bundles the
// `*.bench.ts` suites, wraps them into a wasm component with wasm-rquickjs, and
// runs them under wasmtime.
//
// Pipeline:
//   1. esbuild-bundle `tests/bench/quickjs/entry.ts` (+ suites) to a single ESM
//      module, rewriting the Vitest harness import to the QuickJS registry.
//   2. `wasm-rquickjs generate-wrapper-crate` to produce a wrapper crate for the
//      `golem:bench` world.
//   3. `cargo build --target wasm32-wasip2` (minimal feature set; `Blob`/fetch is
//      required by the builtin wiring, hence `-S http` at run time).
//   4. `wasmtime run --invoke 'run()'` and pretty-print the JSON results.
//
// Requires: wasm-rquickjs, cargo + wasm32-wasip2 target, wasmtime on PATH.

import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const packageDir = path.resolve(scriptDir, '..');
const quickjsDir = path.join(packageDir, 'tests', 'bench', 'quickjs');
const bundlePath = path.join(quickjsDir, 'dist', 'bench.mjs');
const registryPath = path.join(quickjsDir, 'registry.ts');
const witDir = path.join(quickjsDir, 'wit');
const crateDir = path.join(quickjsDir, 'crate');

const require = createRequire(import.meta.url);

// esbuild ships as a transitive dependency of Vitest. Resolve it from the
// normal module graph, falling back to the pnpm store so a hoist layout change
// does not break this dev tool.
function loadEsbuild() {
  try {
    return require('esbuild');
  } catch {
    const pnpmDir = path.resolve(packageDir, '..', '..', 'node_modules', '.pnpm');
    const match = fs.readdirSync(pnpmDir).find((d) => d.startsWith('esbuild@'));
    if (!match) {
      throw new Error('Could not locate esbuild (expected as a Vitest dependency).');
    }
    return require(path.join(pnpmDir, match, 'node_modules', 'esbuild'));
  }
}

// `normal` minus `logging`: `logging` imports `wasi:logging/logging`, which the
// wasmtime CLI does not provide; `fetch` is required because the builtin wiring
// unconditionally expects the `Blob` export from the http builtin.
const cargoFeatures = ['fetch', 'node-http', 'crypto', 'zlib', 'encoding'];

function run(cmd, args, opts = {}) {
  console.error(`\n> ${cmd} ${args.join(' ')}`);
  return execFileSync(cmd, args, { stdio: 'inherit', cwd: packageDir, ...opts });
}

// 1. Bundle the QuickJS entry + suites into a single ESM module. The Vitest
// harness import is rewritten to the QuickJS registry so importing a suite
// records its cases instead of scheduling them in Vitest. Host/runtime modules
// (`node:*`, `golem:*`, `wasi:*`, `agent-guest`) are kept external — they are
// provided by wasm-rquickjs or are erased type-only imports.
const esbuild = loadEsbuild();
console.error(`\n> esbuild bundle ${path.relative(packageDir, bundlePath)}`);
await esbuild.build({
  entryPoints: [path.join(quickjsDir, 'entry.ts')],
  outfile: bundlePath,
  bundle: true,
  format: 'esm',
  platform: 'neutral',
  target: 'es2022',
  tsconfig: path.join(packageDir, 'tsconfig.json'),
  resolveExtensions: ['.ts', '.mjs', '.js', '.json'],
  external: ['node:*', 'golem:*', 'wasi:*', 'agent-guest'],
  logLevel: 'info',
  plugins: [
    {
      name: 'bench-harness-alias',
      setup(build) {
        build.onResolve({ filter: /(^|\/)harness$/ }, () => ({ path: registryPath }));
      },
    },
  ],
});

// 2. Generate the wrapper crate.
fs.rmSync(crateDir, { recursive: true, force: true });
run('wasm-rquickjs', [
  'generate-wrapper-crate',
  '--js',
  path.relative(packageDir, bundlePath),
  '--wit',
  path.relative(packageDir, witDir),
  '--world',
  'bench',
  '--output',
  path.relative(packageDir, crateDir),
]);

// 3. Compile to a wasm component. The actual target directory is whatever cargo
// (or a cargo wrapper) decides, so the artifact path is read back from cargo's
// JSON output rather than assumed.
const cargoArgs = [
  'build',
  '--manifest-path',
  path.join(crateDir, 'Cargo.toml'),
  '--target',
  'wasm32-wasip2',
  '--release',
  '--no-default-features',
  '--features',
  cargoFeatures.join(','),
  '--message-format',
  'json-render-diagnostics',
];
console.error(`\n> cargo ${cargoArgs.join(' ')}`);
const cargoStdout = execFileSync('cargo', cargoArgs, {
  cwd: packageDir,
  encoding: 'utf8',
  maxBuffer: 256 * 1024 * 1024,
  stdio: ['inherit', 'pipe', 'inherit'],
});

let wasmPath;
for (const line of cargoStdout.split('\n')) {
  if (!line.trim()) continue;
  let msg;
  try {
    msg = JSON.parse(line);
  } catch {
    continue;
  }
  if (msg.reason === 'compiler-artifact' && Array.isArray(msg.filenames)) {
    const wasm = msg.filenames.find((f) => f.endsWith('.wasm'));
    if (wasm) wasmPath = wasm;
  }
}

if (!wasmPath || !fs.existsSync(wasmPath)) {
  console.error('Could not determine the compiled bench.wasm path from cargo output.');
  process.exit(1);
}

// 4. Run under wasmtime and capture the returned JSON string.
console.error(`\n> wasmtime run -S http=y --invoke 'run()' ${path.relative(packageDir, wasmPath)}`);
const stdout = execFileSync('wasmtime', ['run', '-S', 'http=y', '--invoke', 'run()', wasmPath], {
  cwd: packageDir,
  encoding: 'utf8',
  maxBuffer: 64 * 1024 * 1024,
});

// wasmtime prints the returned string as a JSON string literal; the payload is
// itself JSON (double-encoded). Find the line that decodes to our result object.
let parsed;
for (const line of stdout.split('\n').reverse()) {
  const trimmed = line.trim();
  if (!trimmed) continue;
  try {
    const inner = JSON.parse(trimmed);
    parsed = typeof inner === 'string' ? JSON.parse(inner) : inner;
    if (parsed && Array.isArray(parsed.results)) break;
  } catch {
    // not the result line
  }
}

if (!parsed || !Array.isArray(parsed.results)) {
  console.error('Could not parse benchmark results from wasmtime output:\n' + stdout);
  process.exit(1);
}

// Pretty-print grouped results.
const byGroup = new Map();
for (const r of parsed.results) {
  if (!byGroup.has(r.group)) byGroup.set(r.group, []);
  byGroup.get(r.group).push(r);
}

const fmtHz = (hz) => hz.toLocaleString('en-US', { maximumFractionDigits: 0 });
const fmtNs = (ns) => ns.toLocaleString('en-US', { maximumFractionDigits: 0 });

console.log(`\nQuickJS benchmark results (wasm-rquickjs):\n`);
for (const [group, rows] of byGroup) {
  console.log(`  ${group}`);
  const nameWidth = Math.max(...rows.map((r) => r.name.length));
  for (const r of rows) {
    console.log(
      `    ${r.name.padEnd(nameWidth)}  ${fmtHz(r.hz).padStart(12)} hz   ${fmtNs(
        r.nsPerOp,
      ).padStart(12)} ns/op`,
    );
  }
  console.log('');
}

// Also emit machine-readable JSON for diffing across runs.
const jsonOut = path.join(quickjsDir, 'dist', 'results.json');
fs.writeFileSync(jsonOut, JSON.stringify(parsed, null, 2));
console.error(`Wrote ${path.relative(packageDir, jsonOut)}`);
