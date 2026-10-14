import { createHash } from 'node:crypto';

export const npmBuildPatches = new Map(
  [
    {
      id: 'npm-qrcode-legacy-octal-escape',
      package: 'npm',
      packageVersion: '10.9.9',
      relativePath: 'node_modules/qrcode-terminal/lib/main.js',
      sourceSha256: 'fa88a331a51dd411f8f0f068cfb8d88280cafa65554fb18bf8aee1f4325ef699',
      replacements: [['\\033', '\\x1b']],
    },
    {
      id: 'npm-config-reserved-protected-binding',
      package: 'npm',
      packageVersion: '10.9.9',
      relativePath: 'lib/commands/config.js',
      sourceSha256: '7b5f74387711742a179711f9717fc846974354e0c008adf2c88ad92549748637',
      replacements: [['protected', 'protectedKeys']],
    },
    {
      id: 'npm-cli-static-entry-for-rollup',
      package: 'npm',
      packageVersion: '10.9.9',
      relativePath: 'lib/cli.js',
      sourceSha256: '67666f06479f9b0bbc01412c198caadd4287d34d5ed74a02871ea78f186451e7',
      replacements: [
        [
          "const cliEntry = require('node:path').resolve(__dirname, 'cli/entry.js')",
          "const cliEntry = require('./cli/entry.js')",
        ],
        ['() => require(cliEntry)', '() => cliEntry'],
      ],
    },
  ].map((patch) => [patch.relativePath, patch]),
);

export function applyGuardedBuildPatch(patch, source) {
  const actual = createHash('sha256').update(source).digest('hex');
  if (actual !== patch.sourceSha256) {
    throw new Error(
      `${patch.id} source hash changed for ${patch.package}@${patch.packageVersion} ` +
        `(${patch.relativePath}): expected ${patch.sourceSha256}, received ${actual}`,
    );
  }
  let transformed = source;
  for (const [before, after] of patch.replacements) {
    if (!transformed.includes(before)) {
      throw new Error(`${patch.id} could not find its guarded source spelling: ${before}`);
    }
    transformed = transformed.replaceAll(before, after);
  }
  return transformed;
}
