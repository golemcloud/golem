import assert from 'node:assert/strict';
import fs from 'node:fs';

import { installPrivateReadOnlyFiles } from '../golem-temp/runtime-tests/shared/private-vfs.js';

async function preservesNativeRealpathVariants() {
  const originalRealpathNative = fs.realpath.native;
  const originalRealpathSyncNative = fs.realpathSync.native;
  const restore = installPrivateReadOnlyFiles('/private-runtime', {
    'package/index.js': 'module.exports = 1;',
  });

  try {
    assert.equal(typeof fs.realpath.native, 'function');
    assert.equal(typeof fs.realpathSync.native, 'function');
    assert.equal(
      fs.realpathSync.native('/private-runtime/package/index.js'),
      '/private-runtime/package/index.js',
    );
    assert.equal(
      await new Promise<string>((resolve, reject) => {
        fs.realpath.native('/private-runtime/package/index.js', (error, value) => {
          if (error) reject(error);
          else resolve(value);
        });
      }),
      '/private-runtime/package/index.js',
    );
  } finally {
    restore();
  }

  assert.equal(fs.realpath.native, originalRealpathNative);
  assert.equal(fs.realpathSync.native, originalRealpathSyncNative);
}

await preservesNativeRealpathVariants();
console.log('private VFS checks passed');
