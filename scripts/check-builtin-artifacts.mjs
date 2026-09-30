#!/usr/bin/env node

import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { spawnSync } from 'node:child_process';

const root = resolve(import.meta.dirname, '..');
const manifest = JSON.parse(readFileSync(resolve(root, 'builtin-artifacts.json'), 'utf8'));
const artifacts = manifest.artifacts;

if (!artifacts || typeof artifacts !== 'object' || Array.isArray(artifacts)) {
  throw new Error('builtin-artifacts.json must contain an artifacts object');
}

for (const [artifactId, source] of Object.entries(artifacts)) {
  if (!/^[a-z][a-z0-9_]*$/.test(artifactId)) {
    throw new Error(`invalid built-in artifact ID: ${artifactId}`);
  }
  const url = new URL(source.url);
  if (url.protocol !== 'https:' || url.hostname !== 'github.com') {
    throw new Error(`default built-in artifact '${artifactId}' must use a GitHub HTTPS URL`);
  }
  if (!/^\/golemcloud\/golem-builtins\/releases\/download\//.test(url.pathname)) {
    throw new Error(`default built-in artifact '${artifactId}' must use golemcloud/golem-builtins`);
  }
  if (!/^[0-9a-f]{64}$/.test(source.sha256 ?? '')) {
    throw new Error(`default built-in artifact '${artifactId}' must have a lowercase SHA-256`);
  }
}

const tracked = spawnSync(
  'git',
  ['ls-files', '--', 'builtin-tools/*.wasm', 'plugins/*.wasm'],
  { cwd: root, encoding: 'utf8' },
);
if (tracked.status !== 0) {
  process.stderr.write(tracked.stderr);
  process.exit(tracked.status ?? 1);
}
if (tracked.stdout.trim()) {
  throw new Error(`generated built-in WASMs must not be tracked:\n${tracked.stdout.trim()}`);
}

console.log(`validated ${Object.keys(artifacts).length} external built-in artifacts`);
