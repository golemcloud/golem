// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

import { describe, it, expect } from 'vitest';
import { z } from 'zod';
import { defineAgent } from '../src/defineAgent';
import { method } from '../src/method';
import * as http from '../src/http';
import { AgentClassName } from '../src/agentClassName';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';

const get = (name: string) => AgentTypeRegistry.get(new AgentClassName(name));

describe('agent HTTP routing (Phase 6)', () => {
  it('publishes required and optional path/query selectors without constructor bindings', () => {
    for (const source of ['path', 'query'] as const) {
      for (const optional of [false, true]) {
        const name = `selected_${source}_${optional}`;
        defineAgent({
          name,
          id: { customer: z.string() },
          http: http.mount(source === 'path' ? '/c/{customer}/{instance}' : '/c/{customer}', {
            phantomId: http.phantomId[source]('instance', { optional }),
            phantomAgent: true,
          }),
          methods: { read: method({ input: {}, returns: z.string(), http: http.get('/read') }) },
        });
        expect(get(name)?.httpMount?.phantomIdBinding).toEqual({
          tag: source,
          val: { name: 'instance', optional },
        });
      }
    }
  });

  it('rejects selector ownership, endpoint collisions and restricted mounts', () => {
    let registration = 0;
    const register = (
      mount: http.HttpMountSpec,
      ephemeral = false,
      endpoint: http.HttpEndpointSpec = http.post('/read'),
    ) => {
      const name = `invalidSelector${registration++}`;
      defineAgent({
        name,
        id: {},
        mode: ephemeral ? 'ephemeral' : 'durable',
        http: mount,
        methods: {
          read: method({ input: { value: z.string() }, returns: z.string(), http: endpoint }),
        },
      });
      expect(get(name)).toBeUndefined();
      return AgentTypeRegistry.getRegistrationError(name)?.join('\n');
    };
    for (const source of ['path', 'query'] as const) {
      for (const optional of ['"false"', 'null', '0']) {
        expect(
          register(
            http.mount(source === 'path' ? '/c/{instance}' : '/c', {
              phantomId: http.phantomId[source]('instance', JSON.parse(`{"optional":${optional}}`)),
            }),
          ),
        ).toMatch(/optionality must be a boolean/);
      }
    }
    expect(register(http.mount('/c', { phantomId: http.phantomId.path('instance') }))).toMatch(
      /exactly one/,
    );
    expect(
      register(http.mount('/c/{instance}/{other}', { phantomId: http.phantomId.path('instance') })),
    ).toMatch(/other/);
    expect(
      register(http.mount('/c', { phantomId: http.phantomId.query('instance') }), true),
    ).toMatch(/durable/);
    expect(
      register(
        http.mount('/c', {
          phantomId: http.phantomId.query('instance'),
          exposeFiles: [{ route: '/a', path: '/a' }],
        }),
      ),
    ).toMatch(/file exposure/);
    expect(
      register(
        http.mount('/c', { phantomId: http.phantomId.query('instance') }),
        false,
        http.post('/read?instance={value}'),
      ),
    ).toMatch(/conflicts/);
    expect(
      register(
        http.mount('/c/{instance}', { phantomId: http.phantomId.path('instance') }),
        false,
        http.post('/{instance}'),
      ),
    ).toMatch(/conflicts/);
  });

  it('compiles full durable stream route options without losing explicit false values', () => {
    const durableStreams = {
      slots: [
        { source: 'input' as const, slot: 'input', name: 'messages' },
        {
          source: 'output' as const,
          slot: '$result',
          name: 'results',
          contentType: 'application/vnd.golem.events',
        },
      ],
      allowExternalWrites: true,
      allowStreamDelete: false,
      allowInvocationDelete: false,
      load: {
        maxConcurrentReadersPerStream: 8,
        maxAppendRequestsPerSecondPerStream: 25,
      },
    };

    const expected = {
      slots: [
        { source: { tag: 'input', val: 'input' }, name: 'messages', contentType: undefined },
        {
          source: { tag: 'output', val: '$result' },
          name: 'results',
          contentType: 'application/vnd.golem.events',
        },
      ],
      allowExternalWrites: true,
      allowStreamDelete: false,
      allowInvocationDelete: false,
      load: {
        maxConcurrentReadersPerStream: 8,
        maxAppendRequestsPerSecondPerStream: 25,
      },
    };

    expect(http.compileEndpoint(http.post('/events', { durableStreams })).durableStreams).toEqual(
      expected,
    );

    defineAgent({
      name: 'durableStreamRoute',
      id: {},
      http: http.mount('/durable-stream-route'),
      methods: {
        events: method({
          input: { input: z.string() },
          returns: z.string(),
          http: http.post('/events', { durableStreams }),
        }),
      },
    });
    expect(get('durableStreamRoute')!.methods[0].httpEndpoint[0].durableStreams).toEqual(expected);
  });

  it('omits durable stream metadata and preserves omitted nested options', () => {
    expect(http.compileEndpoint(http.post('/plain')).durableStreams).toBeUndefined();
    expect(
      http.compileEndpoint(
        http.post('/stream', {
          durableStreams: { slots: [{ source: 'input', slot: 'input' }] },
        }),
      ).durableStreams,
    ).toEqual({
      slots: [{ source: { tag: 'input', val: 'input' }, name: undefined, contentType: undefined }],
      allowExternalWrites: undefined,
      allowStreamDelete: undefined,
      allowInvocationDelete: undefined,
      load: undefined,
    });
  });

  it('rejects malformed durable stream option shapes at runtime', () => {
    expect(() =>
      http.compileEndpoint(http.post('/bad', { durableStreams: { slots: 'input' } as never })),
    ).toThrow('durableStreams.slots must be an array');
    expect(() =>
      http.compileEndpoint(
        http.post('/bad', {
          durableStreams: { slots: [{ source: 'external', slot: 'input' }] } as never,
        }),
      ),
    ).toThrow('source must be "input" or "output"');
  });

  it('emits an http mount + per-method endpoint into the AgentType', () => {
    defineAgent({
      name: 'httpCounter',
      id: { name: z.string() },
      http: { path: '/counters/{name}', cors: ['*'] },
      methods: {
        value: method({ input: {}, returns: z.number(), http: http.get('/value') }),
        add: method({
          input: { by: z.number() },
          returns: z.number(),
          http: http.post('/add'),
        }),
      },
    });

    const at = get('httpCounter')!;
    expect(at).toBeDefined();

    // Mount: /counters/{name} → [literal "counters", path-variable "name"]
    expect(at.httpMount).toBeDefined();
    expect(at.httpMount!.pathPrefix).toEqual([
      { tag: 'literal', val: 'counters' },
      { tag: 'path-variable', val: { variableName: 'name' } },
    ]);
    expect(at.httpMount!.corsOptions).toEqual({ allowedPatterns: ['*'] });
    expect(at.httpMount!.authDetails).toEqual({ required: false });
    expect(at.httpMount!.phantomAgent).toBe(false);
    expect(at.httpMount!.webhookSuffix).toEqual([]);
    expect(at.httpMount!.staticBindings).toEqual([]);
    expect(at.httpMount!.filesystemBindings).toEqual([]);
    expect(at.httpMount!.openapiProviderMethod).toBeUndefined();

    const methods = Object.fromEntries(at.methods.map((m) => [m.name, m]));

    // GET /value
    expect(methods['value'].httpEndpoint).toHaveLength(1);
    expect(methods['value'].httpEndpoint[0].httpMethod).toEqual({ tag: 'get' });
    expect(methods['value'].httpEndpoint[0].pathSuffix).toEqual([{ tag: 'literal', val: 'value' }]);

    // POST /add
    expect(methods['add'].httpEndpoint[0].httpMethod).toEqual({ tag: 'post' });
    expect(methods['add'].httpEndpoint[0].pathSuffix).toEqual([{ tag: 'literal', val: 'add' }]);
  });

  it('binds path, query, and header variables to method parameters', () => {
    defineAgent({
      name: 'httpBindings',
      id: { name: z.string() },
      http: http.mount('/rooms/{name}'),
      methods: {
        // path var {messageId}, query ?limit={limit}, header X-Trace → trace
        getMessage: method({
          input: { messageId: z.string(), limit: z.number(), trace: z.string() },
          returns: z.string(),
          http: http.get('/messages/{messageId}?limit={limit}', {
            headers: { 'X-Trace': 'trace' } as const,
          }),
        }),
      },
    });

    const ep = get('httpBindings')!.methods.find((m) => m.name === 'getMessage')!.httpEndpoint[0];
    expect(ep.pathSuffix).toEqual([
      { tag: 'literal', val: 'messages' },
      { tag: 'path-variable', val: { variableName: 'messageId' } },
    ]);
    expect(ep.queryVars).toEqual([{ queryParamName: 'limit', variableName: 'limit' }]);
    expect(ep.headerVars).toEqual([{ headerName: 'X-Trace', variableName: 'trace' }]);
  });

  it('supports multiple endpoints on one method', () => {
    defineAgent({
      name: 'httpMulti',
      id: { name: z.string() },
      http: { path: '/m/{name}' },
      methods: {
        add: method({
          input: { by: z.number() },
          returns: z.number(),
          http: [http.post('/add'), http.get('/add?by={by}')],
        }),
      },
    });

    const eps = get('httpMulti')!.methods.find((m) => m.name === 'add')!.httpEndpoint;
    expect(eps).toHaveLength(2);
    expect(eps[0].httpMethod).toEqual({ tag: 'post' });
    expect(eps[1].httpMethod).toEqual({ tag: 'get' });
    expect(eps[1].queryVars).toEqual([{ queryParamName: 'by', variableName: 'by' }]);
  });

  it('leaves httpMount undefined and endpoints empty when no http is declared', () => {
    defineAgent({
      name: 'httpNone',
      id: { name: z.string() },
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });

    const at = get('httpNone')!;
    expect(at.httpMount).toBeUndefined();
    expect(at.methods[0].httpEndpoint).toEqual([]);
  });

  it('defers an error for a malformed mount route', () => {
    expect(() =>
      defineAgent({
        name: 'httpBadRoute',
        id: { name: z.string() },
        http: { path: 'counters/{name}' }, // missing leading slash
        methods: { ping: method({ input: {}, returns: z.string() }) },
      }),
    ).not.toThrow();
    expect(get('httpBadRoute')).toBeUndefined();
  });

  it('defers an error when a mount path variable is not an id field', () => {
    expect(() =>
      defineAgent({
        name: 'httpBadMountVar',
        id: { name: z.string() },
        http: { path: '/c/{missing}' },
        methods: { ping: method({ input: {}, returns: z.string() }) },
      }),
    ).not.toThrow();
    expect(get('httpBadMountVar')).toBeUndefined();
  });

  it('defers an error when an endpoint variable is not a method parameter', () => {
    expect(() =>
      defineAgent({
        name: 'httpBadEndpointVar',
        id: { name: z.string() },
        http: { path: '/c/{name}' },
        methods: {
          // `as string` widens the path away from a literal so the compile-time
          // binding gate short-circuits; this test targets the RUNTIME check.
          look: method({
            input: {},
            returns: z.string(),
            http: http.get('/look/{ghost}' as string),
          }),
        },
      }),
    ).not.toThrow();
    expect(get('httpBadEndpointVar')).toBeUndefined();
  });

  it('defers an error when a method declares endpoints but the agent has no mount', () => {
    expect(() =>
      // @ts-expect-error intentionally bypass the compile-time gate to test runtime validation
      defineAgent({
        name: 'httpNoMount',
        id: { name: z.string() },
        methods: {
          look: method({ input: {}, returns: z.string(), http: http.get('/look') }),
        },
      }),
    ).not.toThrow();
    expect(get('httpNoMount')).toBeUndefined();
  });
});
