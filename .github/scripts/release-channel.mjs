import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

function parseVersion(version) {
  const match = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/.exec(version);
  if (!match) return null;
  const prerelease = match[4]?.split('.');
  if (prerelease?.some(id => !/^(0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)$/.test(id))) return null;
  return {
    core: match.slice(1, 4).map(BigInt),
    prerelease: prerelease?.map(id => /^\d+$/.test(id) ? BigInt(id) : id),
  };
}

function compareVersions(left, right) {
  for (let i = 0; i < 3; i++) {
    if (left.core[i] !== right.core[i]) return left.core[i] < right.core[i] ? -1 : 1;
  }
  if (!left.prerelease || !right.prerelease) {
    return Number(!left.prerelease) - Number(!right.prerelease);
  }
  for (let i = 0; i < Math.min(left.prerelease.length, right.prerelease.length); i++) {
    const a = left.prerelease[i];
    const b = right.prerelease[i];
    if (a === b) continue;
    if (typeof a !== typeof b) return typeof a === 'bigint' ? -1 : 1;
    return a < b ? -1 : 1;
  }
  return Math.sign(left.prerelease.length - right.prerelease.length);
}

// A maintenance publication must not replace a newer stable default.
export function releaseChannel(version, publishedVersions) {
  const current = parseVersion(version);
  if (!current) throw new Error(`Invalid release version: ${version}`);
  if (current.prerelease) return 'next';
  for (const published of publishedVersions) {
    const other = parseVersion(published);
    if (other && !other.prerelease && compareVersions(other, current) > 0) {
      return `release-${current.core[0]}.${current.core[1]}`;
    }
  }
  return 'latest';
}

export function npmReleaseChannel(version, publishedVersions) {
  const channel = releaseChannel(version, publishedVersions);
  const current = parseVersion(version);
  for (const published of publishedVersions) {
    const other = parseVersion(published);
    if (!other) throw new Error(`Invalid published npm version: ${published}`);
    const sameChannel = current.prerelease
      ? Boolean(other.prerelease)
      : !other.prerelease && other.core[0] === current.core[0] && other.core[1] === current.core[1];
    if (sameChannel && compareVersions(other, current) > 0) {
      throw new Error(`Refusing to move npm channel ${channel} backward from ${published} to ${version}`);
    }
  }
  return channel;
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [source, version, packageName] = process.argv.slice(2);
  let versions;
  if (source === 'golem') {
    versions = execFileSync('gh', [
      'api', '--paginate', 'repos/golemcloud/golem/releases', '--jq',
      '.[] | select(.draft == false and .prerelease == false) | .tag_name | ltrimstr("v")',
    ], { encoding: 'utf8' }).trim().split('\n');
  } else if (source === 'npm' && packageName) {
    versions = JSON.parse(execFileSync('npm', [
      'view', packageName, 'versions', '--json',
    ], { encoding: 'utf8' }));
    if (typeof versions === 'string') versions = [versions];
  } else {
    throw new Error('Usage: release-channel.mjs golem <version> | npm <version> <package>');
  }
  console.log(source === 'npm' ? npmReleaseChannel(version, versions) : releaseChannel(version, versions));
}
