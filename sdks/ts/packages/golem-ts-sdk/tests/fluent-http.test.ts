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
import { defineAgent } from '../src/fluent/defineAgent';
import { method } from '../src/fluent/method';
import * as http from '../src/fluent/http';
import { AgentClassName } from '../src/agentClassName';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';

const get = (name: string) => AgentTypeRegistry.get(new AgentClassName(name));

describe('fluent agent HTTP routing (Phase 6)', () => {
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

  it('throws on a malformed mount route', () => {
    expect(() =>
      defineAgent({
        name: 'httpBadRoute',
        id: { name: z.string() },
        http: { path: 'counters/{name}' }, // missing leading slash
        methods: { ping: method({ input: {}, returns: z.string() }) },
      }),
    ).toThrow(/HTTP mount/);
  });

  it('throws when a mount path variable is not an id field', () => {
    expect(() =>
      defineAgent({
        name: 'httpBadMountVar',
        id: { name: z.string() },
        http: { path: '/c/{missing}' },
        methods: { ping: method({ input: {}, returns: z.string() }) },
      }),
    ).toThrow(/path variable "missing"/);
  });

  it('throws when an endpoint variable is not a method parameter', () => {
    expect(() =>
      defineAgent({
        name: 'httpBadEndpointVar',
        id: { name: z.string() },
        http: { path: '/c/{name}' },
        methods: {
          // `as string` widens the path away from a literal so the compile-time
          // binding gate short-circuits; this test targets the RUNTIME check.
          look: method({ input: {}, returns: z.string(), http: http.get('/look/{ghost}' as string) }),
        },
      }),
    ).toThrow(/path variable "ghost"/);
  });

  it('throws when a method declares endpoints but the agent has no mount', () => {
    expect(() =>
      defineAgent({
        name: 'httpNoMount',
        id: { name: z.string() },
        methods: {
          look: method({ input: {}, returns: z.string(), http: http.get('/look') }),
        },
      }),
    ).toThrow(/no HTTP mount/);
  });
});
