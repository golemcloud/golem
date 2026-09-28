import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const directory = mkdtempSync(join(tmpdir(), 'golem-ts-package-'));
const npm = (args, cwd = directory) =>
  execFileSync(process.platform === 'win32' ? 'npm.cmd' : 'npm', args, {
    cwd,
    encoding: 'utf8',
    shell: process.platform === 'win32',
    stdio: 'pipe',
  });

try {
  const packed = JSON.parse(npm(['pack', '--json', '--pack-destination', directory], root));
  writeFileSync(join(directory, 'package.json'), '{"private":true,"type":"module"}');
  npm(['install', '--ignore-scripts', join(directory, packed[0].filename)]);
  const installed = join(directory, 'node_modules/@golemcloud/golem-ts-sdk');
  const manifest = JSON.parse(readFileSync(join(installed, 'package.json'), 'utf8'));
  assert.equal(manifest.dependencies['@golemcloud/http-contract'], undefined);
  for (const name of readdirSync(join(installed, 'dist'))) {
    if (!/\.(?:mjs|d\.mts)$/.test(name)) continue;
    assert.ok(
      !readFileSync(join(installed, 'dist', name), 'utf8').includes('@golemcloud/http-contract'),
      name,
    );
  }
  assert.equal(
    readFileSync(join(installed, 'wasm/agent_guest.wasm')).subarray(0, 4).toString('hex'),
    '0061736d',
  );
  execFileSync(
    process.execPath,
    [
      '--input-type=module',
      '--eval',
      `
    import assert from 'node:assert/strict';
    import { compileFileMappings, serializeOpenApi, HttpRouterError } from '@golemcloud/golem-ts-sdk/http-router';
    assert.deepEqual(compileFileMappings([{ route: '/a%20b/*', path: '/content/$1' }]), [
      { tag: 'subtree', val: { publicPrefix: ['a b'], filesystemRoot: '/content' } }
    ]);
    const document = { openapi: '3.1.0', info: { title: 'Packaged', version: '1' }, paths: {} };
    assert.deepEqual(JSON.parse(serializeOpenApi(document)), document);
    assert.throws(() => compileFileMappings([{ route: '/a/*', path: '/content' }]), HttpRouterError);
  `,
    ],
    { cwd: directory, stdio: 'pipe' },
  );
  npm([
    'install',
    '--ignore-scripts',
    '--no-save',
    `typescript@${manifest.devDependencies.typescript}`,
    `@types/node@${manifest.devDependencies['@types/node']}`,
  ]);
  writeFileSync(
    join(directory, 'consumer.ts'),
    `
    import { compileFileMappings, type HttpRequest, type HttpResponse } from '@golemcloud/golem-ts-sdk/http-router';
    const response: HttpResponse<Uint8Array> = { status: 200, headers: [], body: new Uint8Array() };
    const request: HttpRequest<Uint8Array> = {
      method: 'GET', scheme: 'http', authority: 'example.test', path: '/', query: '', headers: [], body: response.body
    };
    compileFileMappings([{ route: request.path, path: '/index.html' }]);
  `,
  );
  execFileSync(
    process.execPath,
    [
      join(directory, 'node_modules/typescript/bin/tsc'),
      '--noEmit',
      '--strict',
      '--module',
      'NodeNext',
      '--moduleResolution',
      'NodeNext',
      '--target',
      'ES2022',
      'consumer.ts',
    ],
    { cwd: directory, stdio: 'inherit' },
  );
  writeFileSync(
    join(directory, 'root-consumer.ts'),
    `
    import type { HttpRequest } from '@golemcloud/golem-ts-sdk';
    import type { HttpRequest as PublicRequest } from '@golemcloud/golem-ts-sdk/http-router';
    const request: HttpRequest<Uint8Array> = {
      method: 'GET', scheme: 'http', authority: 'example.test', path: '/', query: '', headers: [], body: new Uint8Array()
    };
    const publicRequest: PublicRequest<Uint8Array> = request;
    // @ts-expect-error Canonical queries are strings, not numbers.
    const query: HttpRequest<Uint8Array>['query'] = 123;
  `,
  );
  // Match the SDK and generated applications for the root surface. The HTTP-only entry above
  // uses full declaration checking; root oplog declarations have unrelated type-only re-export errors.
  execFileSync(
    process.execPath,
    [
      join(directory, 'node_modules/typescript/bin/tsc'),
      '--noEmit',
      '--strict',
      '--skipLibCheck',
      '--module',
      'NodeNext',
      '--moduleResolution',
      'NodeNext',
      '--target',
      'ES2022',
      'root-consumer.ts',
    ],
    { cwd: directory, stdio: 'inherit' },
  );
  console.log(
    'Packed TypeScript SDK: host-free HTTP import, declarations, embedded WASM, and bundled private contract passed',
  );
} finally {
  rmSync(directory, { recursive: true, force: true });
}
