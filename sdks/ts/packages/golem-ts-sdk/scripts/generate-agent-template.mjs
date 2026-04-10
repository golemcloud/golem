import { spawnSync } from 'node:child_process';

const result = spawnSync(
  'wasm-rquickjs',
  [
    'generate-wrapper-crate',
    '--wit',
    '../../wit',
    '--output',
    'agent-template',
    '--world',
    'agent-guest',
    '--js-modules',
    '@golemcloud/golem-ts-sdk=dist/index.mjs',
    '--js-modules',
    'user=@slot',
  ],
  {
    stdio: 'inherit',
  },
);

if (result.error) {
  throw result.error;
}

if (result.status !== 0) {
  process.exit(result.status ?? 1);
}
