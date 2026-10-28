import assert from 'node:assert/strict';
import { test } from 'node:test';
import { releaseChannel, npmReleaseChannel } from './release-channel.mjs';

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

test('npm rejects an older patch within its stable release line', () => {
  const published = ['1.6.9', '1.6.10', '2.0.0'];
  assert.throws(() => npmReleaseChannel('1.6.9', published), /Refusing to move npm channel release-1.6 backward/);
  assert.equal(npmReleaseChannel('1.6.10', published), 'release-1.6');
  assert.equal(npmReleaseChannel('1.6.11', published), 'release-1.6');
  assert.equal(npmReleaseChannel('1.5.12', published), 'release-1.5');
  assert.equal(npmReleaseChannel('2.0.1', published), 'latest');
});

test('npm next follows semantic prerelease ordering, not string or publication ordering', () => {
  const ordered = [
    '1.9.9-rc.99',
    '2.0.0-alpha',
    '2.0.0-alpha.1',
    '2.0.0-alpha.beta',
    '2.0.0-beta',
    '2.0.0-beta.2',
    '2.0.0-beta.11',
    '2.0.0-rc.1',
    '2.0.0-rc.2',
    '2.0.0-rc.10',
    '2.0.0-rc1',
    '2.0.0-rc2',
  ];
  for (let i = 1; i < ordered.length; i++) {
    const older = ordered[i - 1];
    const newer = ordered[i];
    assert.throws(() => npmReleaseChannel(older, [newer]), /Refusing to move npm channel next backward/);
    assert.equal(npmReleaseChannel(newer, [older]), 'next');
  }
  assert.equal(npmReleaseChannel('2.0.0-rc.2+build.1', ['2.0.0-rc.2+build.2']), 'next');
  assert.equal(npmReleaseChannel('2.0.0-rc.2', ['2.0.0-rc.2']), 'next');
  assert.throws(() => npmReleaseChannel('2.0.0-rc.02', []), /Invalid release version/);
});
