// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { z } from 'zod';
import { defineHttpRouter } from '../src/defineHttpRouter';
import { defineAgent, type MethodsRecord } from '../src/defineAgent';
import { AgentStream } from '../src/schema/agentStream';
import { s, WIT_MARKER } from '../src/schema/markers';
import { compileSchema } from '../src/schema/adapter';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';
import { AgentInitiatorRegistry } from '../src/internal/registry/agentInitiatorRegistry';
import { AgentClassName } from '../src/agentClassName';
import { compileFileMappings, type HttpRequest } from '../src/httpRouterContract';
import {
  httpRequestSchema,
  httpResponseSchema,
  httpStringSchema,
} from '../src/internal/http/routerSchema';
import { registerAgentType } from '../src/runtime';
import {
  t,
  v,
  schemaValueToWitAsync,
  schemaValueToWit,
  schemaValueFromWit,
} from '../src/internal/schema-model';
import * as agentHost from 'golem:agent/host@2.0.0';
import { getAllAgentTypes, getAgentType } from '../src/reflection';
import * as http from '../src/http';
import { webRequest, webResponse, withRawHeaders } from '../src/httpRouterWeb';

const corpus = JSON.parse(
  readFileSync(
    new URL(
      '../../../../../golem-service-base/tests/fixtures/http-handlers/corpus.json',
      import.meta.url,
    ),
    'utf8',
  ),
);
const get = (name: string) => AgentTypeRegistry.get(new AgentClassName(name))!;
const mappings = (values: string[][] = []) => values.map(([route, path]) => ({ route, path }));

describe('metadata corpus through real registration', () => {
  for (const test of corpus.cases.filter((c: { suite: string }) => c.suite === 'metadata')) {
    it(test.id, () => {
      const input = test.input;
      const schemaFor = (name: string) => {
        const actual = input.schema_aliases?.[name] ?? name;
        const schema =
          actual === 'HttpRequest'
            ? httpRequestSchema
            : actual === 'HttpResponse'
              ? httpResponseSchema
              : actual === 'stream<string>'
                ? s.stream(z.string())
                : httpStringSchema;
        if (input.schema_aliases?.[name]) {
          const codec = compileSchema(schema);
          return {
            ...schema,
            [WIT_MARKER]: () => ({
              ...codec,
              graph: {
                defs: new Map([[name, { name, body: codec.graph.root }]]),
                root: t.ref(name),
              },
            }),
          };
        }
        if (input.schema_overrides?.[name]) {
          const codec = compileSchema(schema);
          if (codec.graph.root.body.tag !== 'record') throw new Error('invalid test schema');
          const fields = codec.graph.root.body.fields.map((field) =>
            field.name === 'body' ? { ...field, body: t.list(t.u8()) } : field,
          );
          return {
            ...schema,
            [WIT_MARKER]: () => ({ ...codec, graph: { defs: new Map(), root: t.record(fields) } }),
          };
        }
        return schema;
      };
      const methods: MethodsRecord = {};
      for (const method of input.methods ?? []) {
        methods[method.name] = {
          input: Object.fromEntries(
            method.input.map(([name, type]: string[]) => [name, schemaFor(type)]),
          ),
          returns: schemaFor(method.output),
          http: method.endpoint_auth
            ? http.get('/', { auth: true })
            : input.kind === 'regular' && method.bindings?.length
              ? http.get(method.bindings[0][1])
              : undefined,
        };
      }
      const run = () =>
        registerAgentType(
          test.id,
          Object.fromEntries(input.constructor.map(([name]: string[]) => [name, z.string()])),
          methods,
          {
            mode: input.mode,
            snapshotting: input.snapshot ? 'default' : 'disabled',
            config: Object.fromEntries(
              (input.config ?? []).map((name: string) => [name, z.string()]),
            ),
            ...(input.kind === 'http-router'
              ? {
                  router: {
                    mount: input.mounts[0],
                    staticBindings: compileFileMappings(mappings(input.static_bindings)),
                    handlerMethod: input.methods.find((method: { bindings: string[][] }) =>
                      method.bindings?.some(([verb]) => verb === 'Any'),
                    )?.name,
                    openapiProviderMethod: input.provider,
                  },
                }
              : {
                  http: {
                    path: input.mounts[0],
                    phantomAgent: input.phantom,
                    exposeFiles: mappings(input.filesystem_bindings),
                  },
                }),
          },
        );
      if (test.expect.error) expect(run).toThrow();
      else {
        const type = run().agentType;
        expect(type.kind).toBe(input.kind);
        expect(type.mode).toBe(input.mode);
        if ('handler' in test.expect)
          expect(
            type.methods.find((m) => m.httpEndpoint.some((e) => e.httpMethod.tag === 'any'))
              ?.name ?? null,
          ).toBe(test.expect.handler);
        if ('provider' in test.expect)
          expect(type.httpMount?.openapiProviderMethod ?? null).toBe(test.expect.provider);
      }
    });
  }
});

it('registers static/provider/empty routers without invoking providers or inventing methods', () => {
  const provider = vi.fn(() => ({}));
  defineHttpRouter('AssetsOnly').mount('/assets').static('/*', '/assets/$1').implement();
  defineHttpRouter('ProviderOnly')
    .mount('/')
    .openApi(provider, { methodName: 'description' })
    .implement();
  defineHttpRouter('EmptyRouter').mount('/empty').implement();
  expect(get('AssetsOnly').methods).toEqual([]);
  expect(get('EmptyRouter').methods).toEqual([]);
  expect(get('ProviderOnly').methods.map((m) => m.name)).toEqual(['description']);
  expect(provider).not.toHaveBeenCalled();
  expect(AgentInitiatorRegistry.exists('AssetsOnly')).toBe(true);
  expect(get('AssetsOnly').snapshotting).toEqual({ tag: 'disabled' });
});

it('registers config and a named canonical handler, with no ordinary client surface', () => {
  const builder = defineHttpRouter('ConfiguredRouter', { config: { greeting: z.string() } });
  const impl = builder.mount('/').implement(() => new Response(), { methodName: 'serveWhatever' });
  expect(impl).toEqual({ name: 'ConfiguredRouter' });
  expect(get('ConfiguredRouter').config[0].path).toEqual(['greeting']);
  expect(get('ConfiguredRouter').methods[0].httpEndpoint[0].httpMethod).toEqual({ tag: 'any' });
  expect(builder).not.toHaveProperty('client');
  expect(() => builder.static('/', '/file')).toThrow('router-already-implemented');
});

it('rejects ephemeral, duplicate-capture, and principal-dependent file identities', () => {
  for (const [name, mode, id, path] of [
    ['FilesEphemeral', 'ephemeral', {}, '/files'],
    ['FilesDuplicate', 'durable', { id: z.string() }, '/{id}/{id}'],
    ['FilesPrincipal', 'durable', { who: s.principal() }, '/{who}'],
    ['FilesObject', 'durable', { id: z.object({ key: z.string() }) }, '/{id}'],
    ['FilesList', 'durable', { id: z.array(z.string()) }, '/{id}'],
    ['FilesOptional', 'durable', { id: z.string().optional() }, '/{id}'],
    ['FilesTraversal', 'durable', {}, '/..'],
  ] as const) {
    defineAgent({
      name,
      mode,
      id,
      methods: {},
      http: { path, exposeFiles: [{ route: '/*', path: '/files/$1' }] },
    } as never);
    expect(AgentTypeRegistry.getRegistrationError(name)).toBeDefined();
  }
});

const raw = (
  body: AgentStream<Uint8Array>,
  method = 'POST',
): HttpRequest<AgentStream<Uint8Array>> => ({
  method,
  scheme: 'https',
  authority: 'example.test:8443',
  path: '/api/%61',
  query: '',
  headers: [],
  body,
});

it('envelope-extension-and-bytes: Web view retains extension case, bytes, and raw canonical codec', async () => {
  const test = corpus.cases.find((c: { id: string }) => c.id === 'envelope-extension-and-bytes');
  const chunks = test.input.chunks_hex.map(
    (hex: string) => new Uint8Array(Buffer.from(hex, 'hex')),
  );
  const input = {
    ...raw(AgentStream.from(chunks), test.input.method),
    query: test.expect.query,
    headers: test.expect.headers.map((h: { name: string; value_hex: string }) => ({
      name: h.name,
      value: new Uint8Array(Buffer.from(h.value_hex, 'hex')),
    })),
  };
  const { request } = await webRequest(input);
  expect(request.method).toBe(test.expect.method);
  expect(request.url).toBe('https://example.test:8443/api/%61?q=+&q=%20');
  expect(Buffer.from(await request.arrayBuffer()).toString('hex')).toBe(test.expect.body_hex);
  const codec = compileSchema(httpRequestSchema);
  const encoded = codec.toValue({ ...input, body: AgentStream.from([]) });
  expect(encoded.tag).toBe('record');
  const decoded = codec.fromValue(encoded) as HttpRequest<AgentStream<Uint8Array>>;
  expect(decoded.headers).toEqual(input.headers);
  await decoded.body.return();
});

it('lifecycle-output-backpressure and early response: no eager input/output reads', async () => {
  const inputNext = vi.fn(async () => ({ done: false as const, value: new Uint8Array([7, 8]) }));
  const inputReturn = vi.fn(async () => ({ done: true as const, value: undefined }));
  const input = AgentStream.from({
    [Symbol.asyncIterator]: () => ({ next: inputNext, return: inputReturn }),
  });
  const { request, close } = await webRequest(raw(input));
  expect(inputNext).not.toHaveBeenCalled();
  const response = webResponse(new Response(request.body), close);
  expect(inputNext).not.toHaveBeenCalled();
  expect((await response.body.next()).value).toEqual(new Uint8Array([7, 8]));
  expect(inputNext).toHaveBeenCalledTimes(1);
  await response.body.return();
  expect(inputReturn).toHaveBeenCalledTimes(1);

  const earlyReturn = vi.fn(async () => ({ done: true as const, value: undefined }));
  const early = await webRequest(
    raw(
      AgentStream.from({
        [Symbol.asyncIterator]: () => ({ next: inputNext, return: earlyReturn }),
      }),
    ),
  );
  const accepted = webResponse(new Response('accepted', { status: 202 }), early.close);
  const data: number[] = [];
  for await (const chunk of accepted.body) data.push(...chunk);
  expect(new TextDecoder().decode(new Uint8Array(data))).toBe('accepted');
  expect(inputNext).toHaveBeenCalledTimes(1);
  expect(earlyReturn).toHaveBeenCalledTimes(1);
});

it('cancels a pending request read without waiting for another upload chunk', async () => {
  let release!: (value: IteratorResult<Uint8Array>) => void;
  const next = vi.fn(
    () =>
      new Promise<IteratorResult<Uint8Array>>((resolve) => {
        release = resolve;
      }),
  );
  const dispose = vi.fn(async () => {
    release({ done: true, value: undefined });
    return { done: true as const, value: undefined };
  });
  const converted = await webRequest(
    raw(AgentStream.from({ [Symbol.asyncIterator]: () => ({ next, return: dispose }) })),
  );
  const reader = converted.request.body!.getReader();
  const pending = reader.read();
  await vi.waitFor(() => expect(next).toHaveBeenCalledOnce());
  await reader.cancel();
  expect(await pending).toEqual({ done: true, value: undefined });
  expect(dispose).toHaveBeenCalledOnce();
  await converted.close();
  expect(dispose).toHaveBeenCalledOnce();
});

it('envelope-head-unpolled: disposing a producer does not poll it and releases both sides', async () => {
  const pull = vi.fn();
  const cancel = vi.fn();
  const closeInput = vi.fn(async () => {});
  const response = webResponse(
    new Response(new ReadableStream({ pull, cancel }, { highWaterMark: 0 })),
    closeInput,
  );
  await response.body.return();
  expect(pull).not.toHaveBeenCalled();
  expect(cancel).toHaveBeenCalledOnce();
  expect(closeInput).toHaveBeenCalledOnce();
});

it('envelope-cookie-order: preserves raw cookies and byte values independently of Headers', async () => {
  const headers = [
    { name: 'set-cookie', value: new Uint8Array([97, 61, 128]) },
    { name: 'set-cookie', value: new Uint8Array([98, 61, 255]) },
  ];
  const response = webResponse(withRawHeaders(new Response(null), headers), async () => {});
  expect(response.headers).toEqual(headers);
  await response.body.return();
  const web = new Response(null, {
    headers: [
      ['set-cookie', 'a=first'],
      ['set-cookie', 'a=second'],
    ],
  });
  const converted = webResponse(web, async () => {});
  expect(converted.headers.map((h) => new TextDecoder().decode(h.value))).toEqual([
    'a=first',
    'a=second',
  ]);
  await converted.body.return();
});

it('propagates stream failure rather than successful EOF and releases input', async () => {
  const failure = new Error('producer failed');
  const close = vi.fn(async () => {});
  const response = webResponse(
    new Response(
      new ReadableStream(
        {
          pull() {
            throw failure;
          },
        },
        { highWaterMark: 0 },
      ),
    ),
    close,
  );
  await expect(response.body.next()).rejects.toBe(failure);
  expect(close).toHaveBeenCalledOnce();
});

it('preserves absent versus empty query through canonical encoding', () => {
  const codec = compileSchema(httpRequestSchema);
  for (const query of [undefined, '']) {
    const value = codec.toValue({ ...raw(AgentStream.from([])), query });
    expect(value.tag === 'record' && value.fields[4]).toEqual(
      v.option(query === undefined ? undefined : v.string('')),
    );
  }
});

it.each([
  ['HEAD', 200],
  ['GET', 204],
  ['GET', 205],
  ['GET', 304],
] as const)(
  'disposes %s/%s before the runtime encodes and wraps the output producer',
  async (method, status) => {
    const name = `Unpolled${method}${status}`;
    const pull = vi.fn(async () => ({ done: false as const, value: new Uint8Array([99]) }));
    const close = vi.fn(async () => ({ done: true as const, value: undefined }));
    defineHttpRouter(name)
      .mount('/')
      .implementRaw(() => ({
        status,
        headers: [],
        body: AgentStream.from({ [Symbol.asyncIterator]: () => ({ next: pull, return: close }) }),
      }));
    (globalThis as any).currentAgentId =
      `${name}(${JSON.stringify(schemaValueToWit(v.record([])))})`;
    const initiated = await AgentInitiatorRegistry.lookup(name)!.initiate(v.record([]) as never, {
      tag: 'anonymous',
    });
    if (initiated.tag !== 'ok') throw new Error('test initialization failed');
    const input = await schemaValueToWitAsync(
      v.record([compileSchema(httpRequestSchema).toValue(raw(AgentStream.from([]), method))]),
    );
    const result = await initiated.val.invoke('handle', input, { tag: 'anonymous' });
    expect(result.tag).toBe('ok');
    expect(pull).not.toHaveBeenCalled();
    expect(close).toHaveBeenCalledOnce();
    if (result.tag === 'ok') {
      const response = compileSchema(httpResponseSchema).fromValue(
        schemaValueFromWit(result.val!),
      ) as { body: AgentStream<Uint8Array> };
      expect(await response.body.next()).toEqual({ done: true, value: undefined });
    }
  },
);

it('tooling-router-catalog and tooling-name-not-kind: callable reflection filters by kind only', () => {
  defineHttpRouter('OrdinaryLookingName').mount('/').implement();
  defineAgent({
    name: 'HttpRouterLookingName',
    id: {},
    methods: {},
    http: http.mount('/', { exposeFiles: [{ route: '/', path: '/file' }] }),
  });
  const registered = ['OrdinaryLookingName', 'HttpRouterLookingName'].map((name) => ({
    agentType: get(name),
    implementedBy: { uuid: { highBits: 0n, lowBits: 1n } },
  }));
  const all = vi.spyOn(agentHost, 'getAllAgentTypes').mockReturnValue(registered);
  const one = vi
    .spyOn(agentHost, 'getAgentType')
    .mockImplementation((name) => registered.find((r) => r.agentType.typeName === name));
  try {
    expect(getAllAgentTypes().map((type) => type.name)).toEqual(['HttpRouterLookingName']);
    expect(getAgentType('OrdinaryLookingName')).toBeUndefined();
    expect(getAgentType('HttpRouterLookingName')!.client).toBeDefined();
    expect(get('OrdinaryLookingName').kind).toBe('http-router');
  } finally {
    all.mockRestore();
    one.mockRestore();
  }
});

it('request EOF reports cleanup failure and concurrent close callers join it', async () => {
  const failure = new Error('cleanup failed');
  let reject!: (error: Error) => void;
  const pending = new Promise<void>((_, fail) => {
    reject = fail;
  });
  const release = vi.fn(() => pending.then(() => ({ done: true as const, value: undefined })));
  const view = await webRequest(
    raw(
      AgentStream.from<Uint8Array>({
        [Symbol.asyncIterator]: () => ({
          next: async () => ({ done: true, value: undefined }),
          return: release,
        }),
      }),
    ),
  );
  const consumed = expect(view.request.arrayBuffer()).rejects.toBe(failure);
  await vi.waitFor(() => expect(release).toHaveBeenCalledOnce());
  const first = expect(view.close()).rejects.toBe(failure);
  const second = expect(view.close()).rejects.toBe(failure);
  reject(failure);
  await Promise.all([consumed, first, second]);
  expect(release).toHaveBeenCalledOnce();
});

it.each(['header', 'status'] as const)(
  'invalid response %s releases the unpolled producer',
  async (invalid) => {
    const name = `InvalidHead${invalid}`;
    const pull = vi.fn(async () => ({ done: false as const, value: new Uint8Array([17]) }));
    const release = vi.fn(async () => {
      throw new Error('secondary cleanup error');
    });
    defineHttpRouter(name)
      .mount('/')
      .implementRaw(() => ({
        status: invalid === 'status' ? 200.5 : 200,
        headers: invalid === 'header' ? [{ name: 'x-test', value: new Uint8Array([13]) }] : [],
        body: AgentStream.from({ [Symbol.asyncIterator]: () => ({ next: pull, return: release }) }),
      }));
    (globalThis as any).currentAgentId =
      `${name}(${JSON.stringify(schemaValueToWit(v.record([])))})`;
    const initiated = await AgentInitiatorRegistry.lookup(name)!.initiate(v.record([]) as never, {
      tag: 'anonymous',
    });
    if (initiated.tag !== 'ok') throw new Error('test initialization failed');
    const input = await schemaValueToWitAsync(
      v.record([compileSchema(httpRequestSchema).toValue(raw(AgentStream.from([])))]),
    );
    const result = await initiated.val.invoke('handle', input, { tag: 'anonymous' });
    expect(result.tag).toBe('err');
    expect(JSON.stringify(result)).toContain(`invalid-${invalid}`);
    expect(JSON.stringify(result)).not.toContain('secondary cleanup error');
    expect(pull).not.toHaveBeenCalled();
    expect(release).toHaveBeenCalledOnce();
  },
);

it('invalid Web headers dispose the unpolled output and input, preserving the head error', async () => {
  const pull = vi.fn();
  const cancel = vi.fn(() => {
    throw new Error('secondary cancellation');
  });
  const inputRelease = vi.fn(async () => ({ done: true as const, value: undefined }));
  const name = 'InvalidWebHead';
  defineHttpRouter(name)
    .mount('/')
    .implement(
      () =>
        new Response(new ReadableStream({ pull, cancel }, { highWaterMark: 0 }), {
          headers: { 'x-test': '\u0001' },
        }),
    );
  (globalThis as any).currentAgentId = `${name}(${JSON.stringify(schemaValueToWit(v.record([])))})`;
  const initiated = await AgentInitiatorRegistry.lookup(name)!.initiate(v.record([]) as never, {
    tag: 'anonymous',
  });
  if (initiated.tag !== 'ok') throw new Error('test initialization failed');
  const inputBody = AgentStream.from<Uint8Array>({
    [Symbol.asyncIterator]: () => ({
      next: async () => ({ done: true, value: undefined }),
      return: inputRelease,
    }),
  });
  const input = await schemaValueToWitAsync(
    v.record([compileSchema(httpRequestSchema).toValue(raw(inputBody))]),
  );
  const result = await initiated.val.invoke('handle', input, { tag: 'anonymous' });
  expect(result.tag).toBe('err');
  expect(JSON.stringify(result)).toContain('invalid-header');
  expect(JSON.stringify(result)).not.toContain('secondary cancellation');
  expect(pull).not.toHaveBeenCalled();
  expect(cancel).toHaveBeenCalledOnce();
  expect(inputRelease).toHaveBeenCalledOnce();
});

it('invalid raw header override disposes the Web response body', async () => {
  const pull = vi.fn();
  const cancel = vi.fn();
  const inputRelease = vi.fn(async () => ({ done: true as const, value: undefined }));
  const name = 'InvalidRawHeaderOverride';
  defineHttpRouter(name)
    .mount('/')
    .implement(() =>
      withRawHeaders(new Response(new ReadableStream({ pull, cancel }, { highWaterMark: 0 })), [
        { name: 'x-test', value: new Uint8Array([13]) },
      ]),
    );
  (globalThis as any).currentAgentId = `${name}(${JSON.stringify(schemaValueToWit(v.record([])))})`;
  const initiated = await AgentInitiatorRegistry.lookup(name)!.initiate(v.record([]) as never, {
    tag: 'anonymous',
  });
  if (initiated.tag !== 'ok') throw new Error('test initialization failed');
  const inputBody = AgentStream.from<Uint8Array>({
    [Symbol.asyncIterator]: () => ({
      next: async () => ({ done: true, value: undefined }),
      return: inputRelease,
    }),
  });
  const input = await schemaValueToWitAsync(
    v.record([compileSchema(httpRequestSchema).toValue(raw(inputBody))]),
  );
  const result = await initiated.val.invoke('handle', input, { tag: 'anonymous' });
  expect(result.tag).toBe('err');
  expect(JSON.stringify(result)).toContain('invalid-header');
  expect(pull).not.toHaveBeenCalled();
  expect(cancel).toHaveBeenCalledOnce();
  expect(inputRelease).toHaveBeenCalledOnce();
});
