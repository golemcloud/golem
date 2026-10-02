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

import { installNpmRecursiveRmPatch } from '../golem-temp/runtime-tests/javascript-tools/src/npm-runtime.js';

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

await recursiveRemoveDoesNotFollowDirectorySymlinks();
console.log('npm runtime checks passed');
