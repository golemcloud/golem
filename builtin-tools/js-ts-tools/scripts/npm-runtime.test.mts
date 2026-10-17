import assert from 'node:assert/strict';
import {
  existsSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  realpathSync,
  rmSync,
  rmdirSync,
  symlinkSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import {
  installNpmRecursiveRmPatch,
  installNpmTransportWarningFilter,
  npmInvocation,
} from '../golem-temp/runtime-tests/javascript-tools/src/npm-runtime.js';

async function recursiveRemoveDoesNotFollowDirectorySymlinks() {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), 'golem-npm-rm-')));
  const target = join(directory, 'target');
  const directLink = join(directory, 'direct-link');
  const tree = join(directory, 'tree');
  const nestedLink = join(tree, 'nested-link');
  const marker = join(target, 'marker.txt');
  const promises: {
    rm: (path: string, options?: { force?: boolean; recursive?: boolean }) => Promise<void>;
  } = {
    async rm(_path, _options) {
      throw Object.assign(new Error('synthetic permission failure'), { code: 'EACCES' });
    },
  };
  const restore = installNpmRecursiveRmPatch(
    '10.9.9',
    '10.9.9',
    promises,
    { lstatSync, readdirSync, rmdirSync, unlinkSync },
    join,
  );

  try {
    mkdirSync(target);
    mkdirSync(tree);
    writeFileSync(marker, 'preserve me', { flag: 'wx' });
    symlinkSync(target, directLink, 'dir');
    await promises.rm(directLink, { recursive: true, force: true });
    assert.equal(existsSync(directLink), false);
    assert.equal(readFileSync(marker, 'utf8'), 'preserve me');

    symlinkSync(target, nestedLink, 'dir');
    await promises.rm(tree, { recursive: true, force: true });
    assert.equal(existsSync(tree), false);
    assert.equal(readFileSync(marker, 'utf8'), 'preserve me');
  } finally {
    restore();
    rmSync(directory, { recursive: true, force: true });
  }
}

function filtersOnlyTheAgentTransportWarning() {
  const emitted: unknown[][] = [];
  const target = {
    emitWarning(warning: string | Error, ...rest: unknown[]) {
      emitted.push([warning, ...rest]);
    },
  };
  const original = target.emitWarning;
  const restore = installNpmTransportWarningFilter(target);

  target.emitWarning('ignored hook', { code: 'WASM_RQUICKJS_HTTP_AGENT_TRANSPORT' });
  target.emitWarning('ignored hook', 'Warning', 'WASM_RQUICKJS_HTTP_AGENT_TRANSPORT');
  target.emitWarning(
    Object.assign(new Error('ignored hook'), { code: 'WASM_RQUICKJS_HTTP_AGENT_TRANSPORT' }),
  );
  target.emitWarning('kept', { code: 'OTHER' });
  target.emitWarning('kept without code');
  assert.deepEqual(emitted, [['kept', { code: 'OTHER' }], ['kept without code']]);

  restore();
  assert.equal(target.emitWarning, original);
}

function derivesPathsFromTheRootDirectory() {
  const { environment, directories } = npmInvocation('/', 'https://registry.npmjs.org/');
  assert.equal(environment.HOME, '/.golem-home');
  assert.equal(environment.NPM_CONFIG_CACHE, '/.golem-npm-cache');
  assert.ok(environment.PATH.startsWith('/node_modules/.bin:'), environment.PATH);
  assert.deepEqual(directories, ['/.golem-home', '/.golem-npm-cache', '/.golem-npm-prefix']);
}

await recursiveRemoveDoesNotFollowDirectorySymlinks();
derivesPathsFromTheRootDirectory();
filtersOnlyTheAgentTransportWarning();
console.log('npm runtime checks passed');
