// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import { readFileSync } from 'node:fs';
import * as nativeHttp from 'node:http';
import { once } from 'node:events';
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
import { createServer, Server, ServerResponse, nodeHttpHandler } from '../src/nodeHttp';

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

it('emits ordered file response headers for live and static mounts', () => {
  const liveHeaders = {
    'content-security-policy': "default-src 'none'",
    'referrer-policy': 'no-referrer',
  };
  defineAgent({
    name: 'HeaderFiles',
    id: { id: z.string() },
    methods: {},
    http: http.mount('/headers/{id}', {
      exposeFiles: [{ route: '/*', path: '/data/$1' }],
      fileResponseHeaders: liveHeaders,
    }),
  });
  const staticHeaders = {
    'content-security-policy': "default-src 'self'",
    'referrer-policy': 'same-origin',
  };
  defineHttpRouter('HeaderAssets')
    .mount('/assets', { fileResponseHeaders: staticHeaders })
    .static('/*', '/assets/$1')
    .implement();
  liveHeaders['content-security-policy'] = 'mutated';
  staticHeaders['content-security-policy'] = 'mutated';

  expect(get('HeaderFiles').httpMount!.fileResponseHeaders).toEqual([
    { name: 'content-security-policy', value: "default-src 'none'" },
    { name: 'referrer-policy', value: 'no-referrer' },
  ]);
  expect(get('HeaderAssets').httpMount!.fileResponseHeaders).toEqual([
    { name: 'content-security-policy', value: "default-src 'self'" },
    { name: 'referrer-policy', value: 'same-origin' },
  ]);
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

describe('socket-free Node server', () => {
  it('constructs locally, accepts later listeners, and retains public IncomingMessage identity', async () => {
    const server = createServer();
    expect(server).toBeInstanceOf(Server);
    expect(server).not.toBeInstanceOf(nativeHttp.Server);
    let request: nativeHttp.IncomingMessage | undefined;
    server.on('request', (req, res) => {
      request = req;
      res.end('later listener');
      return 'not a response';
    });
    const response = await nodeHttpHandler(server)(raw(AgentStream.from([])), { config: {} });
    expect(request).toBeInstanceOf(nativeHttp.IncomingMessage);
    const chunks = [];
    for await (const chunk of response.body) chunks.push(Buffer.from(chunk));
    expect(Buffer.concat(chunks).toString()).toBe('later listener');
    request!.destroy();
  });

  it('rejects server options and listen instead of opening a socket', () => {
    expect(() => createServer({} as never)).toThrow(/options/);
    expect(() => new Server({} as never)).toThrow(/options/);
    expect(() => (createServer as Function)(undefined, () => {})).toThrow(/options/);
    expect(() => createServer().listen()).toThrow(/numeric/);
    expect(() => createServer().listen(3000)).toThrow(/implementRaw/);
    expect(() => nodeHttpHandler(nativeHttp.createServer())).toThrow(/component build/);
  });
});

describe('Node router requests', () => {
  it.each([undefined, '', 'x=%ff'])(
    'preserves the mounted URL and canonical byte headers (%s)',
    async (query) => {
      let request!: nativeHttp.IncomingMessage;
      const response = await nodeHttpHandler(
        createServer((req, res) => {
          request = req;
          res.end();
        }),
      )(
        {
          ...raw(AgentStream.from([]), 'CUSTOM'),
          query,
          headers: [
            { name: 'X-Byte', value: new Uint8Array([255]) },
            { name: 'x-byte', value: Buffer.from('right') },
            { name: 'cookie', value: Buffer.from('a=1') },
            { name: 'cookie', value: Buffer.from('b=2') },
            { name: 'content-type', value: Buffer.from('first') },
            { name: 'content-type', value: Buffer.from('second') },
            { name: 'set-cookie', value: Buffer.from('left') },
            { name: 'set-cookie', value: Buffer.from('right') },
          ],
        },
        { config: {} },
      );
      expect(request.method).toBe('CUSTOM');
      expect(request.url).toBe('/api/%61' + (query === undefined ? '' : `?${query}`));
      expect(request.headers).toMatchObject({
        host: 'example.test:8443',
        'x-byte': 'ÿ, right',
        cookie: 'a=1; b=2',
        'content-type': 'first',
        'set-cookie': ['left', 'right'],
      });
      expect(request.headersDistinct['content-type']).toEqual(['first', 'second']);
      expect(request.rawHeaders).toEqual([
        'X-Byte',
        'ÿ',
        'x-byte',
        'right',
        'cookie',
        'a=1',
        'cookie',
        'b=2',
        'content-type',
        'first',
        'content-type',
        'second',
        'set-cookie',
        'left',
        'set-cookie',
        'right',
        'host',
        'example.test:8443',
      ]);
      expect(request.socket).toBeNull();
      expect(request.httpVersion).toBeUndefined();
      await response.body.return();
      request.destroy();
    },
  );

  it('does not read eagerly, streams Buffer chunks, and completes on EOF', async () => {
    let opened = false;
    const source = AgentStream.from<Uint8Array>({
      async *[Symbol.asyncIterator]() {
        opened = true;
        yield new Uint8Array([0, 255, 2]);
        yield new Uint8Array([9, 4]);
      },
    });
    let request!: nativeHttp.IncomingMessage;
    const response = await nodeHttpHandler(
      createServer((req, res) => {
        request = req;
        res.flushHeaders();
      }),
    )(raw(source), { config: {} });
    expect(opened).toBe(false);
    const chunks: Buffer[] = [];
    for await (const chunk of request) {
      expect(Buffer.isBuffer(chunk)).toBe(true);
      chunks.push(chunk);
    }
    expect(Buffer.concat(chunks)).toEqual(Buffer.from([0, 255, 2, 9, 4]));
    expect(request.complete).toBe(true);
    expect(request.aborted).toBe(false);
    await response.body.return();
  });

  it('closes during a pending read independently of the late read result', async () => {
    let settleRead!: (item: IteratorResult<Uint8Array>) => void;
    let settleReturn!: () => void;
    let pulls = 0;
    let disposals = 0;
    const source = AgentStream.from<Uint8Array>({
      [Symbol.asyncIterator]: () => ({
        next: () => {
          pulls++;
          return new Promise((resolve) => {
            settleRead = resolve;
          });
        },
        return: async () => {
          disposals++;
          await new Promise<void>((resolve) => {
            settleReturn = resolve;
          });
          return { done: true as const, value: undefined };
        },
      }),
    });
    let request!: nativeHttp.IncomingMessage;
    const response = await nodeHttpHandler(
      createServer((req, res) => {
        request = req;
        res.flushHeaders();
      }),
    )(raw(source), { config: {} });
    let aborted = 0;
    let data = 0;
    request.on('aborted', () => {
      aborted++;
    });
    request.on('data', () => {
      data++;
    });
    request.read(0);
    expect(pulls).toBe(1);
    const closed = once(request, 'close');
    request.destroy();
    request.destroy();
    expect(disposals).toBe(1);
    settleReturn();
    await closed;
    expect(aborted).toBe(1);
    expect(request.complete).toBe(false);
    settleRead({ done: false, value: new Uint8Array([3]) });
    await Promise.resolve();
    await Promise.resolve();
    expect(data).toBe(0);
    expect(pulls).toBe(1);
    expect(request.complete).toBe(false);
    await expect(response.body.return()).rejects.toThrow('Request destroyed before completion');
  });

  it('bounds read-ahead, permits empty chunks, and disposes once at EOF', async () => {
    let pulls = 0;
    let active = 0;
    let maximum = 0;
    let disposals = 0;
    let request!: nativeHttp.IncomingMessage;
    const source = AgentStream.from<Uint8Array>({
      [Symbol.asyncIterator]: () => ({
        async next() {
          pulls++;
          active++;
          maximum = Math.max(maximum, active);
          await Promise.resolve();
          active--;
          if (pulls === 1)
            return { done: false, value: Buffer.alloc(request.readableHighWaterMark, 7) };
          if (pulls === 2) return { done: false, value: new Uint8Array() };
          if (pulls === 3) return { done: false, value: Buffer.from([9, 2]) };
          return { done: true, value: undefined };
        },
        async return() {
          disposals++;
          return { done: true, value: undefined };
        },
      }),
    });
    const response = await nodeHttpHandler(
      createServer((req, res) => {
        request = req;
        res.flushHeaders();
      }),
    )(raw(source), { config: {} });
    const readable = once(request, 'readable');
    request.read(0);
    await readable;
    expect(pulls).toBe(1);
    expect(request.read()).toEqual(Buffer.alloc(request.readableHighWaterMark, 7));
    const closed = once(request, 'close');
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    await closed;
    expect(Buffer.concat(chunks)).toEqual(Buffer.from([9, 2]));
    expect(maximum).toBe(1);
    expect(disposals).toBe(1);
    expect(request.complete).toBe(true);
    expect(request.aborted).toBe(false);
    await response.body.return();
  });

  it.each([undefined, new Error('source failure')])(
    'surfaces cleanup failure without replacing the source error (%s)',
    async (primary) => {
      let request!: nativeHttp.IncomingMessage;
      const cleanup = new Error('cleanup failure');
      const source = AgentStream.from<Uint8Array>({
        [Symbol.asyncIterator]: () => ({
          async next() {
            if (primary) throw primary;
            return { done: true, value: undefined };
          },
          async return() {
            throw cleanup;
          },
        }),
      });
      const response = await nodeHttpHandler(
        createServer((req, res) => {
          request = req;
          res.flushHeaders();
        }),
      )(raw(source), { config: {} });
      const error = once(request, 'error');
      const closed = new Promise<void>((resolve) => request.once('close', resolve));
      request.resume();
      expect((await error)[0]).toBe(primary ?? cleanup);
      await closed;
      await expect(response.body.return()).rejects.toBe(primary ?? cleanup);
    },
  );

  it('propagates a source error before headers and disposes the source', async () => {
    let closed = false;
    const source = AgentStream.from<Uint8Array>({
      [Symbol.asyncIterator]: () => ({
        next: async () => {
          throw new Error('upload failed');
        },
        return: async () => {
          closed = true;
          return { done: true as const, value: undefined };
        },
      }),
    });
    const handler = nodeHttpHandler(createServer((req) => req.resume()));
    await expect(handler(raw(source), { config: {} })).rejects.toThrow('upload failed');
    expect(closed).toBe(true);
  });
});

describe('Node router responses', () => {
  it('commits a stable head before end and retains repeated byte-string headers', async () => {
    let response!: ServerResponse;
    const output = await nodeHttpHandler(
      createServer((_req, res) => {
        response = res;
        res.setHeader('X-Remove', 'remove');
        expect(res.hasHeader('x-remove')).toBe(true);
        res.removeHeader('x-remove');
        res.setHeader('Set-Cookie', ['left=1', 'right=2']);
        res.setHeader('x-byte', 'ÿ');
        res.writeHead(201);
      }),
    )(raw(AgentStream.from([])), { config: {} });
    expect(response.headersSent).toBe(true);
    expect(response.writableEnded).toBe(false);
    response.statusCode = 500;
    (response.getHeader('set-cookie') as string[]).push('too late');
    expect(() => response.setHeader('x-late', 'late')).toThrow();
    expect(() => response.removeHeader('set-cookie')).toThrow();
    expect(() => response.writeHead(202)).toThrow();
    response.flushHeaders();
    expect(output.status).toBe(201);
    expect(
      output.headers.map(({ name, value }) => [name, Buffer.from(value).toString('latin1')]),
    ).toEqual([
      ['set-cookie', 'left=1'],
      ['set-cookie', 'right=2'],
      ['x-byte', 'ÿ'],
    ]);
    response.write('early');
    expect(Buffer.from((await output.body.next()).value!).toString()).toBe('early');
    expect(response.writableEnded).toBe(false);
    const finished = once(response, 'finish');
    response.end('late');
    expect(Buffer.from((await output.body.next()).value!).toString()).toBe('late');
    expect((await output.body.next()).done).toBe(true);
    await finished;
    expect(response.writableFinished).toBe(true);
  });

  it('serializes queued writes with callbacks and cooperative backpressure', async () => {
    let response!: ServerResponse;
    const output = await nodeHttpHandler(
      createServer((_req, res) => {
        response = res;
        res.flushHeaders();
      }),
    )(raw(AgentStream.from([])), { config: {} });
    const first = Buffer.alloc(response.writableHighWaterMark, 5);
    const completed: string[] = [];
    expect(response.write(first, () => completed.push('first'))).toBe(false);
    response.write(Buffer.from([1, 9]), () => completed.push('second'));
    expect(completed).toEqual([]);
    expect(Buffer.from((await output.body.next()).value!)).toEqual(first);
    const drained = once(response, 'drain');
    expect(Buffer.from((await output.body.next()).value!)).toEqual(Buffer.from([1, 9]));
    await drained;
    expect(completed).toEqual(['first', 'second']);
    const finished = once(response, 'finish');
    response.end();
    await finished;
    expect((await output.body.next()).done).toBe(true);
  });

  it('preserves repeated raw writeHead headers', async () => {
    const output = await nodeHttpHandler(
      createServer((_req, res) => {
        res.setHeader('set-cookie', 'replaced');
        res.writeHead(200, ['Set-Cookie', 'a=1', 'set-cookie', 'b=2']);
        res.end();
      }),
    )(raw(AgentStream.from([])), { config: {} });
    expect(output.headers.map(({ name, value }) => [name, Buffer.from(value).toString()])).toEqual([
      ['set-cookie', 'a=1'],
      ['set-cookie', 'b=2'],
    ]);
    await output.body.return();
  });

  it('settles writes and pending body reads when destroyed after commitment', async () => {
    let response!: ServerResponse;
    const output = await nodeHttpHandler(
      createServer((_req, res) => {
        response = res;
        res.flushHeaders();
      }),
    )(raw(AgentStream.from([])), { config: {} });
    const waiting = output.body.next();
    const failure = new Error('response failure');
    response.destroy(failure);
    await expect(waiting).rejects.toBe(failure);
    const second = await nodeHttpHandler(
      createServer((_req, res) => {
        response = res;
        res.write('pending');
      }),
    )(raw(AgentStream.from([])), { config: {} });
    const callback = new Promise<Error | null | undefined>((resolve) =>
      response.write('queued', resolve),
    );
    response.destroy(failure);
    expect(await callback).toBe(failure);
    await expect(second.body.next()).rejects.toBe(failure);
  });

  it.each([
    (res: ServerResponse) => res.writeHead(101),
    (res: ServerResponse) => res.writeHead(200, 'Custom'),
    (res: ServerResponse) => res.addTrailers(),
    (res: ServerResponse) => res.writeContinue(),
    (res: ServerResponse) => res.writeProcessing(),
    (res: ServerResponse) => res.writeEarlyHints(),
    (res: ServerResponse) => {
      res.statusMessage = 'Custom';
    },
  ])('rejects unsupported response operations', async (operation) => {
    await expect(
      nodeHttpHandler(createServer((_req, res) => operation(res)))(raw(AgentStream.from([])), {
        config: {},
      }),
    ).rejects.toBeInstanceOf(Error);
  });
});

describe('Node router exchange cleanup', () => {
  it('completes a zero-length body when the native producer pulls before disposal', async () => {
    let disposed = false;
    const input = AgentStream.from<Uint8Array>({
      [Symbol.asyncIterator]: () => ({
        next: async () => ({ done: true, value: undefined }),
        return: async () => {
          disposed = true;
          return { done: true, value: undefined };
        },
      }),
    });
    const output = await nodeHttpHandler(
      createServer((_req, res) => {
        res.writeHead(200, { 'content-length': '000' });
      }),
    )(raw(input), { config: {} });
    expect(await output.body.next()).toEqual({ done: true, value: undefined });
    expect(disposed).toBe(true);
  });

  it.each([
    ['HEAD', 200, undefined],
    ['GET', 204, undefined],
    ['GET', 205, undefined],
    ['GET', 304, undefined],
    ['GET', 200, '0'],
    ['GET', 200, ' \t000\t '],
  ] as const)(
    'disposes %s/%s/%s before a reader or end without a write/dispose cycle',
    async (method, status, length) => {
      let response!: ServerResponse;
      let released!: () => void;
      const written = new Promise<void>((resolve) => {
        released = resolve;
      });
      let pulls = 0;
      let disposals = 0;
      const source = AgentStream.from<Uint8Array>({
        [Symbol.asyncIterator]: () => ({
          async next() {
            pulls++;
            return { done: true, value: undefined };
          },
          async return() {
            disposals++;
            await written;
            return { done: true, value: undefined };
          },
        }),
      });
      const errors: Error[] = [];
      const output = await nodeHttpHandler(
        createServer((_req, res) => {
          response = res;
          res.on('error', (error) => errors.push(error));
          res.writeHead(status, length === undefined ? {} : { 'content-length': length });
          res.write('not sent', (error) => {
            if (error) errors.push(error);
            released();
          });
        }),
      )(raw(source, method), { config: {} });
      expect(response.writableEnded).toBe(false);
      await output.body.return();
      expect(disposals).toBe(1);
      expect(pulls).toBe(0);
      expect(response.writableEnded).toBe(false);
      expect(errors).toEqual([]);
      const finished = once(response, 'finish');
      response.end('also discarded');
      await finished;
      expect(errors).toEqual([]);
    },
  );

  it('local disposal interrupts output writes and releases an unread input', async () => {
    let response!: ServerResponse;
    let released = false;
    const source = AgentStream.from<Uint8Array>({
      [Symbol.asyncIterator]: () => ({
        async next() {
          return { done: false, value: new Uint8Array([1]) };
        },
        async return() {
          released = true;
          return { done: true, value: undefined };
        },
      }),
    });
    const results: Array<Error | null | undefined> = [];
    const output = await nodeHttpHandler(
      createServer((_req, res) => {
        response = res;
        res.write('active', (error) => results.push(error));
        res.write('queued', (error) => results.push(error));
      }),
    )(raw(source), { config: {} });
    await output.body.return();
    expect(results).toHaveLength(2);
    expect(results.every((error) => error instanceof Error)).toBe(true);
    expect(released).toBe(true);
    expect(response.destroyed).toBe(true);
  });

  it('keeps overlapping exchanges independent when one output is disposed', async () => {
    const responses = new Map<string, ServerResponse>();
    const server = createServer((req, res) => {
      responses.set(req.url!, res);
      res.setHeader('x-id', req.url!);
      res.flushHeaders();
    });
    const handler = nodeHttpHandler(server);
    const first = await handler(
      { ...raw(AgentStream.from([])), path: '/first', query: undefined },
      { config: {} },
    );
    const second = await handler(
      { ...raw(AgentStream.from([])), path: '/second', query: undefined },
      { config: {} },
    );
    await first.body.return();
    expect(responses.get('/first')!.destroyed).toBe(true);
    expect(responses.get('/second')!.destroyed).toBe(false);
    responses.get('/second')!.end('second only');
    const chunks = [];
    for await (const chunk of second.body) chunks.push(Buffer.from(chunk));
    expect(Buffer.concat(chunks).toString()).toBe('second only');
    expect(second.headers.map(({ name, value }) => [name, Buffer.from(value).toString()])).toEqual([
      ['x-id', '/second'],
    ]);
  });

  it('settles headers and input cleanup on a listener failure or early request destruction', async () => {
    for (const listener of [
      () => {
        throw new Error('listener failure');
      },
      (req: nativeHttp.IncomingMessage) => req.destroy(),
    ]) {
      let released = false;
      const source = AgentStream.from<Uint8Array>({
        [Symbol.asyncIterator]: () => ({
          async next() {
            return { done: true, value: undefined };
          },
          async return() {
            released = true;
            return { done: true, value: undefined };
          },
        }),
      });
      await expect(
        nodeHttpHandler(createServer(listener))(raw(source), { config: {} }),
      ).rejects.toBeInstanceOf(Error);
      expect(released).toBe(true);
    }
    await expect(
      nodeHttpHandler(createServer())(raw(AgentStream.from([])), { config: {} }),
    ).rejects.toThrow(/listener/);
  });

  it('does not hide failures after commitment in suppressed responses', async () => {
    const failure = new Error('after commitment');
    await expect(
      nodeHttpHandler(
        createServer((_req, res) => {
          res.writeHead(204);
          throw failure;
        }),
      )(raw(AgentStream.from([])), { config: {} }),
    ).rejects.toBe(failure);
    let response!: ServerResponse;
    const output = await nodeHttpHandler(
      createServer((_req, res) => {
        response = res;
        res.writeHead(204);
      }),
    )(raw(AgentStream.from([])), { config: {} });
    response.destroy(failure);
    await expect(output.body.return()).rejects.toBe(failure);
  });

  it('does not hide a response failure arriving during suppression cleanup', async () => {
    let started!: () => void;
    let release!: () => void;
    const disposing = new Promise<void>((resolve) => {
      started = resolve;
    });
    const disposed = new Promise<void>((resolve) => {
      release = resolve;
    });
    let disposals = 0;
    const source = AgentStream.from<Uint8Array>({
      [Symbol.asyncIterator]: () => ({
        async next() {
          return { done: true, value: undefined };
        },
        async return() {
          disposals++;
          started();
          await disposed;
          return { done: true, value: undefined };
        },
      }),
    });
    let response!: ServerResponse;
    const output = await nodeHttpHandler(
      createServer((_req, res) => {
        response = res;
        res.writeHead(204);
      }),
    )(raw(source), { config: {} });
    const pending = output.body.return();
    await disposing;
    const failure = new Error('failure during cleanup');
    const observed = once(response, 'error');
    response.destroy(failure);
    await observed;
    release();
    await expect(pending).rejects.toBe(failure);
    expect(disposals).toBe(1);
  });

  it.each(['write', 'end'] as const)(
    'settles an implicit %s header failure without finish',
    async (operation) => {
      let response!: ServerResponse;
      let finished = false;
      let callbackError: Error | null | undefined;
      await expect(
        nodeHttpHandler(
          createServer((_req, res) => {
            response = res;
            res.statusCode = 199;
            res.on('finish', () => {
              finished = true;
            });
            if (operation === 'write')
              res.write('invalid', (error) => {
                callbackError = error;
              });
            else
              res.end((error?: Error) => {
                callbackError = error;
              });
          }),
        )(raw(AgentStream.from([])), { config: {} }),
      ).rejects.toBeInstanceOf(RangeError);
      expect(response.headersSent).toBe(false);
      expect(finished).toBe(false);
      expect(callbackError).toBeInstanceOf(RangeError);
    },
  );
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
