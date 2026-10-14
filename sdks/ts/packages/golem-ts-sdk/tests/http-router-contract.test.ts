// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
  compileFileMappings,
  compileRouterMount,
  copyHttpHeaders,
  serializeOpenApi,
} from '../src/httpRouterContract';

const corpus = JSON.parse(
  readFileSync(
    new URL(
      '../../../../../golem-service-base/tests/fixtures/http-handlers/corpus.json',
      import.meta.url,
    ),
    'utf8',
  ),
);

describe('shared mapping corpus', () => {
  for (const test of corpus.cases.filter((c: { suite: string }) => c.suite === 'mapping')) {
    it(test.id, () => {
      const run = () =>
        compileFileMappings(
          test.input.mappings.map(([route, path]: string[]) => ({ route, path })),
        );
      if (test.expect.error) expect(run).toThrow(test.expect.error);
      else
        expect(
          run().map((m) =>
            m.tag === 'exact'
              ? { Exact: { public_path: m.val.publicPath, file_path: m.val.filePath } }
              : {
                  Subtree: {
                    public_prefix: m.val.publicPrefix,
                    filesystem_root: m.val.filesystemRoot,
                  },
                },
          ),
        ).toEqual(test.expect.compiled);
    });
  }
});

it('compiles optional roles without a fake handler or policy override', () => {
  const result = compileRouterMount({
    mount: '/my',
    auth: true,
    cors: ['https://example.com'],
    openapiProviderMethod: 'documentation',
  });
  expect(result.handlerBinding).toBeUndefined();
  expect(result.mount.openapiProviderMethod).toBe('documentation');
  expect(result.mount.authDetails).toEqual({ required: true });
  expect(compileRouterMount({ mount: '/', handlerMethod: 'anything' }).handlerBinding).toEqual({
    httpMethod: { tag: 'any' },
    pathSuffix: [],
    headerVars: [],
    queryVars: [],
    authDetails: undefined,
    corsOptions: { allowedPatterns: [] },
  });
  expect(() => compileRouterMount({ mount: '/{id}' })).toThrow('source-path');
  expect(() =>
    compileRouterMount({ mount: '/', handlerMethod: 'same', openapiProviderMethod: 'same' }),
  ).toThrow('router-methods');
});

it('preserves opaque bytes and duplicate header order', () => {
  const headers = [
    { name: 'set-cookie', value: new Uint8Array([128, 255]) },
    { name: 'set-cookie', value: new Uint8Array([97]) },
  ];
  expect(copyHttpHeaders(headers)).toEqual(headers);
  expect(() => copyHttpHeaders([{ name: 'x-a', value: new Uint8Array([13]) }])).toThrow(
    'invalid-header',
  );
  expect(() => copyHttpHeaders([{ name: 'X-A', value: new Uint8Array() }])).toThrow(
    'invalid-header',
  );
});

const doc = (extra = {}) => ({
  openapi: '3.1.0',
  info: { title: 'Example', version: '1' },
  paths: {},
  ...extra,
});

it('serializes a JSON document deterministically without resolving references or changing examples', () => {
  const document = doc({
    'x-array': [2, 1],
    components: {
      schemas: { Node: { $ref: '#/components/schemas/Node', example: { $ref: 'data' } } },
    },
  });
  const result = serializeOpenApi(document);
  expect(JSON.parse(result)).toEqual(document);
  expect(result.indexOf('"components"')).toBeLessThan(result.indexOf('"info"'));
  expect(serializeOpenApi(doc({ 'x-keys': { '\u{10000}': 1, '\ue000': 2 } }))).toContain(
    '"x-keys":{"":2,"𐀀":1}',
  );
});

it('rejects lossy values, unsupported root sections, cycles, invalid Unicode, and excessive depth', () => {
  for (const value of [undefined, NaN, Infinity, 1n, () => 0, new Date(), '\ud800']) {
    expect(() => serializeOpenApi(doc({ 'x-value': value }))).toThrow();
  }
  expect(() => serializeOpenApi(doc({ webhooks: {} }))).toThrow('openapi-section');
  const cyclic: Record<string, unknown> = {};
  cyclic.self = cyclic;
  expect(() => serializeOpenApi(doc({ 'x-cycle': cyclic }))).toThrow('openapi-cycle');
  let nested: unknown = {};
  for (let i = 0; i < 64; i++) nested = { child: nested };
  expect(() => serializeOpenApi(doc({ 'x-depth': nested }))).toThrow('openapi-depth');
});

it('enforces the inclusive UTF-8 size limit', () => {
  const overhead = new TextEncoder().encode(serializeOpenApi(doc({ 'x-padding': '' }))).length;
  expect(
    new TextEncoder().encode(serializeOpenApi(doc({ 'x-padding': 'a'.repeat(1048576 - overhead) })))
      .length,
  ).toBe(1048576);
  expect(() => serializeOpenApi(doc({ 'x-padding': 'a'.repeat(1048577 - overhead) }))).toThrow(
    'openapi-size',
  );
  expect(() => serializeOpenApi(doc({ 'x-padding': 'é'.repeat(524288) }))).toThrow('openapi-size');
});
