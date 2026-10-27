import assert from 'node:assert/strict';
import { test } from 'node:test';
import { releaseChannel } from './release-channel.mjs';

test('the highest stable version owns latest, independently of branch and publication order', () => {
  const published = ['1.6.9', '2.0.0-rc1', '1.5.12', 'golem-rust-v9.0.0', '1.6.10'];
  assert.equal(releaseChannel('1.6.11', published), 'latest');
  assert.equal(releaseChannel('1.6.10', published), 'latest');
  assert.equal(releaseChannel('1.6.9', published), 'release-1.6');
  assert.equal(releaseChannel('1.5.13', published), 'release-1.5');
  assert.equal(releaseChannel('2.0.0-rc2', published), 'next');
  assert.equal(releaseChannel('2.0.0', published), 'latest');
  assert.equal(releaseChannel('1.6.11', [...published, '2.0.0']), 'release-1.6');
  assert.equal(releaseChannel('1.6.11', [...published, '1.7.0']), 'release-1.6');
  assert.equal(releaseChannel('1.0.0', []), 'latest');
  assert.throws(() => releaseChannel('main', published), /Invalid release version/);
});
