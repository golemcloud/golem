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
    {
      id: 'npm-init-default-input-runtime-path',
      package: 'npm',
      packageVersion: '10.9.9',
      relativePath: 'node_modules/init-package-json/lib/init-package-json.js',
      sourceSha256: '9b45c6a7e9c0975ff64e0d65e42435a6705fc77c4f9790b2045c892b81365097',
      replacements: [
        [
          "const def = require.resolve('./default-input.js')",
          "const def = path.join(__dirname, 'default-input.js')",
        ],
      ],
    },
    {
      id: 'npm-init-default-input-bundled-dependencies',
      package: 'npm',
      packageVersion: '10.9.9',
      relativePath: 'node_modules/promzard/lib/index.js',
      sourceSha256: '4d15120547acffc5d1d453e1630167677b9c2d732de4bacc33d902b5242710a0',
      replacements: [
        [
          'const files = {}',
          [
            'const files = {}',
            "const defaultInput = require('path').join(__dirname, '../../init-package-json/lib/default-input.js')",
            'const defaultInputDependencies = {',
            "  'npm-package-arg': () => require('npm-package-arg'),",
            "  semver: () => require('semver'),",
            "  'validate-npm-package-license': () => require('validate-npm-package-license'),",
            "  'validate-npm-package-name': () => require('validate-npm-package-name'),",
            '}',
          ].join('\n'),
        ],
        [
          'this.#ctx.require = (p) => mod.require(p)',
          'this.#ctx.require = (p) => this.#file === defaultInput && Object.hasOwn(defaultInputDependencies, p) ? defaultInputDependencies[p]() : mod.require(p)',
        ],
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
