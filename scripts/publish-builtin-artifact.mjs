#!/usr/bin/env node

import { createHash } from 'node:crypto';
import { cpSync, existsSync, mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';

const repository = 'golemcloud/golem-builtins';
const root = resolve(import.meta.dirname, '..');
const [component, version] = process.argv.slice(2);

if (process.env.CI) {
  console.error('built-in artifacts must be published manually from a local checkout, never from CI');
  process.exit(1);
}

const components = {
  'otlp-exporter': {
    buildTask: 'build-otlp-exporter',
    wasm: 'plugins/otlp-exporter.wasm',
    exports: { plugin: 'golem-otlp-exporter', version: '1.5.3' },
  },
  'filesystem-tools': {
    buildTask: 'build-filesystem-tools',
    wasm: 'builtin-tools/filesystem-tools.wasm',
    exports: { tools: ['read-file@0.3.0', 'write-file@0.3.0', 'edit-file@0.3.0'] },
  },
  'javascript-tools': {
    buildTask: 'build-javascript-tools',
    wasm: 'builtin-tools/javascript-tools.wasm',
    exports: { tools: ['node@0.1.0', 'npm@10.9.9', 'npx@10.9.9'] },
    licenses: 'builtin-tools/js-ts-tools/licenses/npm',
    sourceNotice: 'builtin-tools/js-ts-tools/licenses/SOURCE.md',
    sbom: 'builtin-tools/js-ts-tools/licenses/npm/sbom.spdx.json',
  },
  'typescript-tools': {
    buildTask: 'build-typescript-tools',
    wasm: 'builtin-tools/typescript-tools.wasm',
    exports: { tools: ['tsc@5.9.2'] },
    licenses: 'builtin-tools/js-ts-tools/licenses/typescript',
    sourceNotice: 'builtin-tools/js-ts-tools/licenses/SOURCE.md',
    sbom: 'builtin-tools/js-ts-tools/licenses/typescript/sbom.spdx.json',
  },
};

function fail(message) {
  console.error(message);
  process.exit(1);
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    cwd: root,
    encoding: 'utf8',
    stdio: options.capture ? 'pipe' : 'inherit',
    env: process.env,
  });
  if (result.status !== 0 && !options.allowFailure) {
    if (options.capture) {
      process.stderr.write(result.stdout ?? '');
      process.stderr.write(result.stderr ?? '');
    }
    fail(`${command} ${args.join(' ')} failed with exit code ${result.status}`);
  }
  return result;
}

if (!component || !components[component]) {
  fail(`BUILTIN_COMPONENT must be one of: ${Object.keys(components).join(', ')}`);
}
if (!version || !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) {
  fail('BUILTIN_VERSION must be a semantic version without a leading v');
}

const definition = components[component];
if (process.env.BUILTIN_SKIP_BUILD !== '1') {
  run('cargo', ['make', definition.buildTask]);
}

const wasmPath = join(root, definition.wasm);
if (!existsSync(wasmPath)) {
  fail(`built artifact does not exist: ${definition.wasm}`);
}
run('wasm-tools', ['validate', '--features', 'all', wasmPath]);

const bytes = readFileSync(wasmPath);
const sha256 = createHash('sha256').update(bytes).digest('hex');
const tag = `${component}-v${version}`;
const assetName = `${component}.wasm`;
const releaseUrl = `https://github.com/${repository}/releases/download/${tag}/${assetName}`;
const staging = mkdtempSync(join(tmpdir(), `golem-${component}-`));
const stagedWasm = join(staging, assetName);
cpSync(wasmPath, stagedWasm);
const checksumPath = `${stagedWasm}.sha256`;
writeFileSync(checksumPath, `${sha256}  ${assetName}\n`);

const sourceCommit = run('git', ['rev-parse', 'HEAD'], { capture: true }).stdout.trim();
const sourceRef = run('git', ['rev-parse', '--abbrev-ref', 'HEAD'], { capture: true }).stdout.trim();
const sourceStatus = run('git', ['status', '--porcelain'], { capture: true }).stdout.trim();
const provenance = {
  schemaVersion: 1,
  artifact: {
    component,
    version,
    asset: assetName,
    sha256,
    size: bytes.length,
    exports: definition.exports,
  },
  source: {
    repository: 'https://github.com/golemcloud/golem',
    commit: sourceCommit,
    ref: sourceRef,
  },
  build: {
    task: `cargo make ${definition.buildTask}`,
  },
};
const provenancePath = join(staging, 'provenance.json');
writeFileSync(provenancePath, `${JSON.stringify(provenance, null, 2)}\n`);

const licenseRoot = join(staging, 'licenses');
if (definition.licenses) {
  cpSync(join(root, definition.licenses), licenseRoot, { recursive: true });
  cpSync(join(root, definition.sourceNotice), join(licenseRoot, 'SOURCE.md'));
} else {
  cpSync(join(root, 'LICENSE'), join(staging, 'LICENSE'));
}
const licenseArchive = join(staging, `${component}-licenses.tar.gz`);
run('tar', [
  '-czf',
  licenseArchive,
  '-C',
  definition.licenses ? licenseRoot : staging,
  definition.licenses ? '.' : 'LICENSE',
]);

const assets = [stagedWasm, checksumPath, provenancePath, licenseArchive];
if (definition.sbom) {
  const sbomPath = join(staging, `${component}.spdx.json`);
  cpSync(join(root, definition.sbom), sbomPath);
  assets.push(sbomPath);
}

const summary = {
  repository,
  tag,
  component,
  version,
  asset: assetName,
  sha256,
  size: bytes.length,
  sourceCommit,
  manifestEntry: { url: releaseUrl, sha256 },
};

if (process.env.BUILTIN_DRY_RUN === '1') {
  console.log(JSON.stringify(summary, null, 2));
  process.exit(0);
}

if (sourceStatus) {
  fail('refusing to publish from a dirty source tree; commit the exact source first');
}

const existing = run('gh', ['release', 'view', tag, '--repo', repository], {
  capture: true,
  allowFailure: true,
});
if (existing.status === 0) {
  fail(`immutable built-in release already exists: ${repository}@${tag}`);
}

const notesPath = join(staging, 'release-notes.md');
writeFileSync(
  notesPath,
  [
    `Built from [golem@${sourceCommit}](https://github.com/golemcloud/golem/commit/${sourceCommit}).`,
    '',
    `SHA-256: \`${sha256}\``,
    '',
    'The source and build tooling for this artifact live in the golem repository.',
    '',
  ].join('\n'),
);
run('gh', [
  'release',
  'create',
  tag,
  '--repo',
  repository,
  '--title',
  tag,
  '--notes-file',
  notesPath,
  ...assets,
]);
console.log(JSON.stringify(summary, null, 2));
