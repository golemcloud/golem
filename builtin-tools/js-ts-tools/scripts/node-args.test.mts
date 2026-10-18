import assert from 'node:assert/strict';

import {
  compileEvalSource,
  NodeOptionError,
  parseNodeArguments,
} from '../golem-temp/runtime-tests/javascript-tools/src/node-args.js';
import { npxTypeScriptShim } from '../golem-temp/runtime-tests/javascript-tools/src/npx-typescript-shim.js';

function rejects(args: string[], message: RegExp) {
  assert.throws(
    () => parseNodeArguments(args),
    (error: unknown) => error instanceof NodeOptionError && message.test(error.message),
  );
}

function parsesEntriesEvalAndVersion() {
  assert.deepEqual(parseNodeArguments(['main.ts', 'a', '-e']), {
    kind: 'entry',
    entry: 'main.ts',
    argv: ['node', 'main.ts', 'a', '-e'],
  });
  assert.deepEqual(parseNodeArguments(['-e', 'x', 'a']), {
    kind: 'eval',
    source: 'x',
    typescript: 'detect',
    argv: ['node', 'a'],
  });
  assert.deepEqual(parseNodeArguments(['--version']), { kind: 'version' });
  assert.deepEqual(parseNodeArguments(['--', '-script.js', 'a']), {
    kind: 'entry',
    entry: '-script.js',
    argv: ['node', '-script.js', 'a'],
  });
}

function acceptsTypeScriptAndNoOpOptions() {
  assert.deepEqual(
    parseNodeArguments(['--experimental-strip-types', '--no-warnings', 'main.ts', 'a']),
    { kind: 'entry', entry: 'main.ts', argv: ['node', 'main.ts', 'a'] },
  );
  for (const args of [
    ['--input-type=commonjs-typescript', '-e', 'x'],
    ['--input-type', 'commonjs-typescript', '--eval', 'x'],
  ]) {
    assert.deepEqual(parseNodeArguments(args), {
      kind: 'eval',
      source: 'x',
      typescript: true,
      argv: ['node'],
    });
  }
  assert.equal(
    (parseNodeArguments(['--input-type=commonjs', '-e', 'x']) as { typescript: unknown })
      .typescript,
    false,
  );
}

function rejectsUnsupportedOptions() {
  rejects(['--watch', 'main.js'], /^node: bad option: --watch$/);
  rejects(['--input-type=module', '-e', 'x'], /not supported.*\.mjs file/);
  rejects(['--input-type=module-typescript', '-e', 'x'], /not supported.*\.mts file/);
  rejects(['--input-type=json', '-e', 'x'], /--input-type must be/);
  rejects(['-e'], /-e requires an argument/);
  rejects([], /expected a script file/);
  rejects(['--no-warnings'], /expected a script file/);
}

async function evalDetectsTypeScriptOnlyOnParseErrors() {
  const strip = (source: string) => source.replace(': number', '');
  const calls: string[] = [];
  const tracked = (source: string) => {
    calls.push(source);
    return strip(source);
  };
  assert.equal(await compileEvalSource('return 1 + 1', 'detect', tracked)(), 2);
  assert.deepEqual(calls, []);
  assert.equal(await compileEvalSource('const x: number = 3; return x', 'detect', tracked)(), 3);
  assert.equal(calls.length, 1);
  await assert.rejects(compileEvalSource('throw new SyntaxError("runtime")', 'detect', tracked)(), {
    message: 'runtime',
  });
  assert.equal(calls.length, 1);
  assert.throws(() => compileEvalSource('const x: number = 3', false, tracked), SyntaxError);
  assert.throws(
    () =>
      compileEvalSource('const = ;', 'detect', () => {
        throw new Error('not TypeScript either');
      }),
    SyntaxError,
  );
  assert.equal(await compileEvalSource('const x: number = 4; return x', true, strip)(), 4);
}

function npxShimsOnlyMissingTypeScriptRunners() {
  const none = () => false;
  assert.deepEqual(npxTypeScriptShim(['tsx', 'main.ts', 'a'], '/w/p', none), {
    kind: 'node',
    args: ['--', 'main.ts', 'a'],
    notice: undefined,
  });
  assert.deepEqual(npxTypeScriptShim(['-y', '--', 'tsx', '-e', 'src', 'a'], '/w', none), {
    kind: 'node',
    args: ['--input-type=commonjs-typescript', '--eval', 'src', 'a'],
    notice: undefined,
  });
  const tsNode = npxTypeScriptShim(['--yes', 'ts-node', '-T', 'main.ts'], '/w', none);
  assert.equal(tsNode?.kind, 'node');
  assert.deepEqual((tsNode as { args: string[] }).args, ['--', 'main.ts']);
  assert.match((tsNode as { notice: string }).notice, /type checking is skipped/);

  for (const args of [['tsx', '--watch', 'main.ts'], ['tsx', 'watch', 'main.ts'], ['tsx']]) {
    const result = npxTypeScriptShim(args, '/w', none);
    assert.equal(result?.kind, 'error', args.join(' '));
    assert.match((result as { message: string }).message, /npm install -D tsx/);
  }

  assert.equal(npxTypeScriptShim(['cowsay', 'hi'], '/w', none), undefined);
  assert.equal(npxTypeScriptShim(['--package', 'tsx', 'tsx', 'main.ts'], '/w', none), undefined);
  assert.equal(npxTypeScriptShim(['-p', 'tsx', 'tsx', 'main.ts'], '/w', none), undefined);
  assert.equal(npxTypeScriptShim(['--call', 'tsx main.ts'], '/w', none), undefined);
  assert.equal(
    npxTypeScriptShim(['tsx', 'main.ts'], '/w/p', (path) => path === '/w/node_modules/.bin/tsx'),
    undefined,
  );
  assert.equal(
    npxTypeScriptShim(
      ['tsx', 'main.ts'],
      '/w/p',
      (path) => path === '/w/p/.golem-npm-prefix/bin/tsx',
    ),
    undefined,
  );
}

parsesEntriesEvalAndVersion();
acceptsTypeScriptAndNoOpOptions();
rejectsUnsupportedOptions();
await evalDetectsTypeScriptOnlyOnParseErrors();
npxShimsOnlyMissingTypeScriptRunners();
console.log('node argument checks passed');
