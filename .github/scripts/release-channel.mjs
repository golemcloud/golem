import { execFileSync } from 'node:child_process';
import { pathToFileURL } from 'node:url';

// A maintenance publication must not replace a newer stable default.
export function releaseChannel(version, publishedVersions) {
  const match = /^(\d+)\.(\d+)\.(\d+)(-[0-9A-Za-z.-]+)?$/.exec(version);
  if (!match) throw new Error(`Invalid release version: ${version}`);
  if (match[4]) return 'next';
  const current = match.slice(1, 4).map(Number);
  for (const published of publishedVersions) {
    if (!/^\d+\.\d+\.\d+$/.test(published)) continue;
    const other = published.split('.').map(Number);
    for (let i = 0; i < 3; i++) {
      if (other[i] > current[i]) return `release-${current[0]}.${current[1]}`;
      if (other[i] < current[i]) break;
    }
  }
  return 'latest';
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
  console.log(releaseChannel(version, versions));
}
