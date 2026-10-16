import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(import.meta.dirname, '..');
const outputRoot = path.join(root, 'licenses');
const npmRoot = path.join(root, 'node_modules/npm');
const typescriptRoot = path.join(root, 'node_modules/typescript');
const check = process.argv.includes('--check');

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}

function normalizedNotice(file) {
  const lines = fs.readFileSync(file, 'utf8').replaceAll('\r\n', '\n').split('\n');
  return `${lines
    .map((line) => line.trimEnd())
    .join('\n')
    .trimEnd()}\n`;
}

function collectPackages(packageRoot) {
  const packages = new Map();
  const visit = (directory) => {
    const manifestPath = path.join(directory, 'package.json');
    if (fs.existsSync(manifestPath)) {
      const manifest = readJson(manifestPath);
      const declaredLicense =
        (typeof manifest.license === 'string' ? manifest.license : manifest.license?.type) ??
        manifest.licenses?.[0]?.type;
      if (!manifest.name || !manifest.version || !declaredLicense) {
        throw new Error(`missing name, version, or license in ${manifestPath}`);
      }
      const license = declaredLicense === 'Apache 2.0' ? 'Apache-2.0' : declaredLicense;
      const key = `${manifest.name}@${manifest.version}`;
      const current = packages.get(key);
      if (current && current.license !== license) {
        throw new Error(`conflicting licenses for ${key}`);
      }
      packages.set(key, {
        name: manifest.name,
        version: manifest.version,
        license,
        source: manifest.repository?.url ?? '',
      });
    }
    const modules = path.join(directory, 'node_modules');
    if (!fs.existsSync(modules)) return;
    for (const entry of fs.readdirSync(modules, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      if (entry.name.startsWith('@')) {
        for (const scoped of fs.readdirSync(path.join(modules, entry.name), {
          withFileTypes: true,
        })) {
          if (scoped.isDirectory()) visit(path.join(modules, entry.name, scoped.name));
        }
      } else {
        visit(path.join(modules, entry.name));
      }
    }
  };
  visit(packageRoot);
  return [...packages.values()].sort((left, right) =>
    `${left.name}@${left.version}`.localeCompare(`${right.name}@${right.version}`),
  );
}

function spdx(name, namespace, packages) {
  return `${JSON.stringify(
    {
      spdxVersion: 'SPDX-2.3',
      dataLicense: 'CC0-1.0',
      SPDXID: 'SPDXRef-DOCUMENT',
      name,
      documentNamespace: namespace,
      creationInfo: {
        created: '2026-09-30T00:00:00Z',
        creators: ['Tool: golem-builtin-js-ts-tools-compliance'],
      },
      packages: packages.map((item, index) => ({
        name: item.name,
        SPDXID: `SPDXRef-Package-${index + 1}`,
        versionInfo: item.version,
        downloadLocation: item.source || 'NOASSERTION',
        filesAnalyzed: false,
        licenseConcluded: 'NOASSERTION',
        licenseDeclared: item.license,
        copyrightText: 'NOASSERTION',
      })),
    },
    null,
    2,
  )}\n`;
}

const provenance = readJson(path.join(root, 'provenance.json'));
const npmPackages = collectPackages(npmRoot);
const typescriptManifest = readJson(path.join(typescriptRoot, 'package.json'));
const generated = new Map([
  ['npm/LICENSE', normalizedNotice(path.join(npmRoot, 'LICENSE'))],
  [
    'npm/DEPENDENCY-NOTICES.md',
    `# npm ${provenance.npm.version} dependency notices\n\nGenerated from the pinned registry package tree.\n\n| Package | Version | License | Source |\n| --- | --- | --- | --- |\n${npmPackages
      .map(
        (item) =>
          `| ${item.name.replaceAll('|', '\\|')} | ${item.version} | ${item.license} | ${item.source || 'not declared'} |`,
      )
      .join('\n')}\n`,
  ],
  [
    'npm/sbom.spdx.json',
    spdx(
      'Golem bundled npm tool source',
      `https://golem.cloud/spdx/toolchain/npm/${provenance.npm.artifactSha256}`,
      npmPackages,
    ),
  ],
  ['typescript/LICENSE.txt', normalizedNotice(path.join(typescriptRoot, 'LICENSE.txt'))],
  [
    'typescript/ThirdPartyNoticeText.txt',
    normalizedNotice(path.join(typescriptRoot, 'ThirdPartyNoticeText.txt')),
  ],
  [
    'typescript/sbom.spdx.json',
    spdx(
      'Golem bundled TypeScript tool source',
      `https://golem.cloud/spdx/toolchain/typescript/${provenance.typescript.artifactSha256}`,
      [
        {
          name: typescriptManifest.name,
          version: typescriptManifest.version,
          license: typescriptManifest.license,
          source: typescriptManifest.repository?.url ?? '',
        },
      ],
    ),
  ],
  [
    'SOURCE.md',
    `# JavaScript toolchain source and notices\n\nThe embedded command implementations come from unmodified upstream package trees. Golem-owned wrappers apply version-guarded runtime compatibility patches and source-hash-guarded bundle transforms without modifying those trees.\n\n- npm ${provenance.npm.version}: ${provenance.npm.sourceTag}\n  - Registry archive: ${provenance.npm.registryTarball}\n  - Integrity: \`${provenance.npm.registryIntegrity}\`\n  - Repacked artifact SHA-256: \`${provenance.npm.artifactSha256}\`\n- TypeScript ${provenance.typescript.version}: ${provenance.typescript.sourceTag}\n  - Registry archive: ${provenance.typescript.registryTarball}\n  - Integrity: \`${provenance.typescript.registryIntegrity}\`\n  - Repacked artifact SHA-256: \`${provenance.typescript.artifactSha256}\`\n\nThe npm and TypeScript names are used descriptively. No upstream logos are used, no endorsement is implied, and the respective trademarks belong to their owners. Production publication requires legal and release-owner approval.\n`,
  ],
]);

let stale = false;
for (const [relative, content] of generated) {
  const target = path.join(outputRoot, relative);
  if (check) {
    if (!fs.existsSync(target) || fs.readFileSync(target, 'utf8') !== content) {
      console.error(`stale compliance artifact: ${path.relative(root, target)}`);
      stale = true;
    }
  } else {
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, content);
  }
}

if (check && stale) process.exitCode = 1;
else {
  const digest = crypto
    .createHash('sha256')
    .update([...generated.values()].join('\0'))
    .digest('hex');
  console.log(`${check ? 'verified' : 'generated'} compliance bundle ${digest}`);
}
