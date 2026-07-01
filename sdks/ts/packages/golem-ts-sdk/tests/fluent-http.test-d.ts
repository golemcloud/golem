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

// Type-only tests for the compile-time HTTP path/binding validators in
// `src/fluent/httpTypes.ts` (surfaced through `src/fluent/http.ts`,
// `defineAgent`, and `method`).
//
// Consumed via `tsc --noEmit` (this file is included by `tsconfig.json`'s
// `tests/**/*` glob) and NOT executed by vitest (whose default glob is
// `*.test.ts`, not `*.test-d.ts`).
//
// Negative cases use `// @ts-expect-error` so a regression — the validator
// silently accepting an invalid path / binding — fails `tsc` with
// `TS2578: Unused '@ts-expect-error' directive`.

import { z } from 'zod';
import { defineAgent } from '../src/fluent/defineAgent';
import { method } from '../src/fluent/method';
import * as http from '../src/fluent/http';
import { s } from '../src/fluent/schema/markers';
import type { BindableKeys } from '../src/fluent/httpTypes';

// ---------------------------------------------------------------------------
// Mount path shape: ValidMountPath
// ---------------------------------------------------------------------------

void http.mount('/');
void http.mount('/agents');
void http.mount('/agents/v1');
void http.mount('/counters/{name}');
void http.mount('/c/{agent-type}/{name}');

// @ts-expect-error missing leading '/'
void http.mount('agents');
// @ts-expect-error empty string is not a valid mount path
void http.mount('');
// @ts-expect-error trailing slash
void http.mount('/agents/');
// @ts-expect-error consecutive slashes
void http.mount('//agents');
// @ts-expect-error consecutive slashes
void http.mount('/a//b');
// @ts-expect-error mount may not include a query string
void http.mount('/agents?foo=1');
// @ts-expect-error empty '{}' variable name
void http.mount('/{}');
// @ts-expect-error literal text mixed with '{var}' in one segment
void http.mount('/foo{bar}');
// @ts-expect-error catch-all is not allowed in mount paths
void http.mount('/files/{*rest}');

// webhookSuffix is validated with the same rules
void http.mount('/agents/{name}', { webhookSuffix: '/inbox' });
void http.mount('/agents/{name}', { webhookSuffix: '/inbox/events' });
// @ts-expect-error webhookSuffix must start with '/'
void http.mount('/agents/{name}', { webhookSuffix: 'inbox' });
// @ts-expect-error webhookSuffix may not include a query string
void http.mount('/agents/{name}', { webhookSuffix: '/inbox?q={x}' });
// @ts-expect-error webhookSuffix may not contain '//'
void http.mount('/agents/{name}', { webhookSuffix: '//inbox' });
// @ts-expect-error catch-all is not allowed in the webhook suffix
void http.mount('/agents/{name}', { webhookSuffix: '/inbox/{*rest}' });

// ---------------------------------------------------------------------------
// Endpoint path shape: ValidEndpointPath
// ---------------------------------------------------------------------------

void http.get('/');
void http.get('/items');
void http.get('/items/{id}');
void http.get('/items?q={q}');
void http.get('/items?q={q}&p={p}');
void http.post('/items');
void http.put('/items/{id}');
void http.del('/items/{id}');
void http.patch('/items/{id}');
void http.head('/items/{id}');
void http.options('/items');
void http.trace('/items');
void http.connect('/items');
void http.custom('PURGE', '/items/{id}');
void http.get('/files/{*rest}');

// @ts-expect-error missing leading '/'
void http.get('items');
// @ts-expect-error trailing slash
void http.get('/items/');
// @ts-expect-error consecutive slashes
void http.get('/items//list');
// @ts-expect-error more than one '?'
void http.get('/items?a={a}?b={b}');
// @ts-expect-error empty query key after '?'
void http.get('/items?={a}');
// @ts-expect-error empty query key after '&'
void http.get('/items?a={a}&={b}');
// @ts-expect-error consecutive '&' producing empty pair
void http.get('/items?a={a}&&b={b}');
// @ts-expect-error trailing '&' producing empty pair
void http.get('/items?a={a}&');
// @ts-expect-error leading '&' producing empty pair
void http.get('/items?&a={a}');
// @ts-expect-error empty '{}' variable name
void http.get('/items/{}');
// @ts-expect-error literal text mixed with '{var}' in one segment
void http.get('/items/{id}suffix');
// @ts-expect-error catch-all is not the last segment
void http.get('/files/{*rest}/x');
// @ts-expect-error empty catch-all name
void http.get('/{*}');
// @ts-expect-error nested braces inside '{...}'
void http.get('/items/{a{b}}');
// @ts-expect-error post: missing leading '/'
void http.post('items');
// @ts-expect-error custom: missing leading '/'
void http.custom('PURGE', 'items');

// ---------------------------------------------------------------------------
// Duplicate query keys
// ---------------------------------------------------------------------------

void http.get('/x?a={a}&b={b}&c={c}');
void http.post('/items/{id}?q={q}&p={p}');
// @ts-expect-error duplicate query key 'a'
void http.get('/x?a={a}&a={b}');
// @ts-expect-error duplicate query key 'b' (middle vs last)
void http.get('/x?a={a}&b={b}&b={c}');
// @ts-expect-error duplicate query key 'q' on a custom verb
void http.custom('PURGE', '/items?q={a}&q={b}');

// ---------------------------------------------------------------------------
// Dynamic (non-literal) paths widen to `string` and skip compile-time checks
// ---------------------------------------------------------------------------

declare const dynMount: string;
declare const dynEndpoint: string;
void http.mount(dynMount);
void http.get(dynEndpoint);
void http.post(dynEndpoint, { headers: { 'X-Whatever': 'x' } });

// Segment-array escape hatch also widens (no compile-time checking).
void http.mount([http.literal('agents'), http.pathVar('name')]);
void http.get([http.literal('items'), http.pathVar('id')]);

// ---------------------------------------------------------------------------
// BindableKeys — plain schemas + scalar markers are bindable; multimodal /
// unstructured markers are excluded at the type level.
// ---------------------------------------------------------------------------

type _Bind1 = BindableKeys<{ id: z.ZodString; n: z.ZodNumber }>;
declare const _b1: _Bind1;
const _b1Ok: 'id' | 'n' = _b1;
void _b1Ok;

// Scalar markers stay bindable; multimodal / unstructured markers are filtered.
type _Bind2 = BindableKeys<{
  id: z.ZodString;
  amount: ReturnType<typeof s.u32>;
  text: ReturnType<typeof s.unstructuredText>;
  bin: ReturnType<typeof s.unstructuredBinary>;
  mm: ReturnType<typeof s.multimodal>;
}>;
declare const _b2: _Bind2;
const _b2Ok: 'id' | 'amount' = _b2; // no marker key leaks in
const _b2ScalarBindable: _Bind2 = 'amount'; // scalar marker stays bindable
void _b2Ok;
void _b2ScalarBindable;

// `BindableKeys<any>` collapses to `string`.
type _BindAny = BindableKeys<any>;
declare const _bAny: _BindAny;
const _bAnyOk: string = _bAny;
void _bAnyOk;

// ---------------------------------------------------------------------------
// A1 — binding a multimodal / unstructured param to a {var} is a compile error
// ---------------------------------------------------------------------------

// Positive — the unstructured param is simply left unbound (it travels in the
// JSON body of a bodyful verb); the bindable 'id' param is bound from the path.
void method({
  input: { id: z.string(), text: s.unstructuredText() },
  returns: z.string(),
  http: http.post('/items/{id}'),
});

// Negative — unstructured param bound from a path variable.
void method({
  input: { text: s.unstructuredText() },
  returns: z.string(),
  // @ts-expect-error unstructured param 'text' cannot be bound from a path variable
  http: http.post('/items/{text}'),
});

// Negative — unstructured param bound from a query variable.
void method({
  input: { text: s.unstructuredText() },
  returns: z.string(),
  // @ts-expect-error unstructured param 'text' cannot be bound from a query variable
  http: http.post('/items?t={text}'),
});

// Negative — multimodal param bound from a header.
void method({
  input: { payload: s.multimodal([{ name: 'chunk', schema: z.string() }]) },
  returns: z.string(),
  // @ts-expect-error multimodal param 'payload' cannot be bound from a header
  http: http.post('/items', { headers: { 'X-P': 'payload' } as const }),
});

// ---------------------------------------------------------------------------
// defineAgent: mount coverage of the id record + {var} ↔ id binding
// ---------------------------------------------------------------------------

// Positive — all id fields covered by the mount path.
void defineAgent({
  name: 'M_AllCovered',
  id: { name: z.string(), id: z.string() },
  http: http.mount('/agents/{name}/{id}'),
  methods: { op: method({ input: {}, returns: z.string() }) },
});

// Positive — agent without `http` is OK regardless of id fields.
void defineAgent({
  name: 'M_NoHttp',
  id: { name: z.string(), id: z.string() },
  methods: { op: method({ input: {}, returns: z.string() }) },
});

// Positive — empty id record with a literal mount path.
void defineAgent({
  name: 'M_NoId',
  id: {},
  http: http.mount('/agents'),
  methods: { op: method({ input: {}, returns: z.string() }) },
});

// Negative — mount path missing the `{id}` id field.
void defineAgent({
  name: 'M_MissingId',
  id: { name: z.string(), id: z.string() },
  // @ts-expect-error mount path is missing the `{id}` id field
  http: http.mount('/agents/{name}'),
  methods: { op: method({ input: {}, returns: z.string() }) },
});

// Negative — mount `{var}` typo does not match the id field `name`.
void defineAgent({
  name: 'M_VarTypo',
  id: { name: z.string() },
  // @ts-expect-error mount var '{naem}' does not cover id field 'name'
  http: http.mount('/greeter/{naem}'),
  methods: { op: method({ input: {}, returns: z.string() }) },
});

// ---------------------------------------------------------------------------
// webhook-suffix vars validated against id fields
// ---------------------------------------------------------------------------

// Positive — webhook var matches an id field.
void defineAgent({
  name: 'W_Pos',
  id: { tenant: z.string() },
  http: http.mount('/api/{tenant}', { webhookSuffix: '/inbox/{tenant}' }),
  methods: { op: method({ input: {}, returns: z.string() }) },
});

// Positive — webhook suffix uses only system variables.
void defineAgent({
  name: 'W_PosSystem',
  id: { tenant: z.string() },
  http: http.mount('/api/{tenant}', { webhookSuffix: '/inbox/{agent-type}' }),
  methods: { op: method({ input: {}, returns: z.string() }) },
});

// Negative — webhook var doesn't match any id field.
void defineAgent({
  name: 'W_NegUnknown',
  id: { tenant: z.string() },
  // @ts-expect-error webhook-suffix var '{nope}' is not an id field
  http: http.mount('/api/{tenant}', { webhookSuffix: '/inbox/{nope}' }),
  methods: { op: method({ input: {}, returns: z.string() }) },
});

// Negative — webhook var IS an id field but refers to an unstructured id field
// (non-bindable). The mount covers both id fields, so the rejection is
// unambiguously from WebhookVarsValid's multimodal/unstructured branch.
void defineAgent({
  name: 'W_NegUnstructured',
  id: { tenant: z.string(), payload: s.unstructuredText() },
  // @ts-expect-error webhook-suffix var '{payload}' refers to a non-bindable id field
  http: http.mount('/api/{tenant}/{payload}', { webhookSuffix: '/inbox/{payload}' }),
  methods: { op: method({ input: {}, returns: z.string() }) },
});

// ---------------------------------------------------------------------------
// method: {var} ↔ input binding
// ---------------------------------------------------------------------------

// Positive — path var bound to an input parameter.
void method({
  input: { id: z.string() },
  returns: z.string(),
  http: http.post('/items/{id}'),
});

// Positive — array of endpoints, each binding independently.
void method({
  input: { id: z.string() },
  returns: z.string(),
  http: [http.get('/items/{id}'), http.get('/items?id={id}')],
});

// Negative — path var '{ghost}' is not an input parameter.
void method({
  input: { id: z.string() },
  returns: z.string(),
  // @ts-expect-error endpoint binds '{ghost}', which is not a method input
  http: http.post('/items/{ghost}'),
});

// Negative — query var '{ghost}' is not an input parameter.
void method({
  input: { id: z.string() },
  returns: z.string(),
  // @ts-expect-error endpoint binds query '{ghost}', which is not a method input
  http: http.post('/items/{id}?g={ghost}'),
});

// ---------------------------------------------------------------------------
// method: a param can be bound at most once across path/query/header
// ---------------------------------------------------------------------------

// Positive — distinct names across path + query.
void method({
  input: { id: z.string(), q: z.string() },
  returns: z.string(),
  http: http.get('/items/{id}?q={q}'),
});

// Negative — 'id' bound from both path and query.
void method({
  input: { id: z.string() },
  returns: z.string(),
  // @ts-expect-error 'id' is bound from both path and query
  http: http.post('/items/{id}?id={id}'),
});

// Negative — 'x' bound from two distinct query keys.
void method({
  input: { x: z.string() },
  returns: z.string(),
  // @ts-expect-error 'x' is bound from two query keys
  http: http.get('/items?a={x}&b={x}'),
});

// ---------------------------------------------------------------------------
// A3 — explicit `query` map values are threaded into the phantoms
// ---------------------------------------------------------------------------

// Positive — explicit query map binds a real input parameter.
void method({
  input: { limit: z.string() },
  returns: z.string(),
  http: http.get('/items', { query: { limit: 'limit' } as const }),
});

// Negative — explicit query map binds 'nope', which is not an input parameter.
void method({
  input: { limit: z.string() },
  returns: z.string(),
  // @ts-expect-error explicit query binds 'nope', which is not a method input
  http: http.get('/items', { query: { q: 'nope' } as const }),
});

// Negative — 'x' bound from BOTH the inline query and the explicit query map.
void method({
  input: { x: z.string() },
  returns: z.string(),
  // @ts-expect-error 'x' is bound from both the inline query and the explicit query map
  http: http.get('/items?q={x}', { query: { r: 'x' } as const }),
});

// ---------------------------------------------------------------------------
// method: case-insensitive header-name uniqueness (record form)
// ---------------------------------------------------------------------------

// Positive — case-distinct header names.
void method({
  input: { a: z.string(), b: z.string() },
  returns: z.string(),
  http: http.post('/items', { headers: { 'X-A': 'a', 'X-B': 'b' } as const }),
});

// Negative — 'X-A' and 'x-a' collide case-insensitively.
void method({
  input: { a: z.string(), b: z.string() },
  returns: z.string(),
  // @ts-expect-error 'X-A' and 'x-a' are case-fold duplicate headers
  http: http.post('/items', { headers: { 'X-A': 'a', 'x-a': 'b' } as const }),
});

// Negative — case-fold duplicate via a custom verb.
void method({
  input: { a: z.string(), b: z.string() },
  returns: z.string(),
  // @ts-expect-error case-fold duplicate header via http.custom
  http: http.custom('PURGE', '/items', { headers: { 'X-A': 'a', 'x-a': 'b' } as const }),
});

// ---------------------------------------------------------------------------
// method: bodyless verbs (get/head) cannot have unbound input params
// ---------------------------------------------------------------------------

// Positive — get with no params.
void method({ input: {}, returns: z.number(), http: http.get('/value') });

// Positive — get with the param bound from a path variable.
void method({ input: { id: z.string() }, returns: z.string(), http: http.get('/items/{id}') });

// Positive — get with the param bound from an inline query variable
// (mirrors the demo `http.get('/hello?who={who}')`).
void method({ input: { who: z.string() }, returns: z.string(), http: http.get('/hello?who={who}') });

// Positive — get with the param bound from a header.
void method({
  input: { idem: z.string() },
  returns: z.string(),
  http: http.get('/items', { headers: { 'X-Idem': 'idem' } as const }),
});

// Positive — head with the param bound from a path variable.
void method({ input: { id: z.string() }, returns: z.string(), http: http.head('/items/{id}') });

// Negative — get with an unbound param.
void method({
  input: { payload: z.string() },
  returns: z.string(),
  // @ts-expect-error bodyless GET cannot have unbound 'payload'
  http: http.get('/op'),
});

// Negative — head with an unbound param.
void method({
  input: { payload: z.string() },
  returns: z.string(),
  // @ts-expect-error bodyless HEAD cannot have unbound 'payload'
  http: http.head('/op'),
});

// Negative — get binds SOME but not all params.
void method({
  input: { id: z.string(), name: z.string() },
  returns: z.string(),
  // @ts-expect-error bodyless GET binds 'id' but leaves 'name' unbound
  http: http.get('/items/{id}'),
});

// Negative — array with a bodyful (fine) + a bodyless-unbound (error) endpoint.
// The bodyless element makes the whole `method({...})` call fail to compile.
// @ts-expect-error bodyless GET in the array cannot have unbound 'payload'
void method({
  input: { payload: z.string() },
  returns: z.string(),
  http: [http.post('/op'), http.get('/op')],
});

// Positive — bodyful verbs are never subject to the bodyless check; the unbound
// 'payload' reaches the handler via the JSON request body.
void method({ input: { payload: z.string() }, returns: z.string(), http: http.post('/op') });
void method({ input: { payload: z.string() }, returns: z.string(), http: http.put('/op') });
void method({ input: { payload: z.string() }, returns: z.string(), http: http.del('/op') });
void method({ input: { payload: z.string() }, returns: z.string(), http: http.patch('/op') });
void method({ input: { payload: z.string() }, returns: z.string(), http: http.options('/op') });
void method({ input: { payload: z.string() }, returns: z.string(), http: http.trace('/op') });
void method({ input: { payload: z.string() }, returns: z.string(), http: http.connect('/op') });

// Positive — http.custom is always bodyful, so unbound params are allowed.
void method({ input: { payload: z.string() }, returns: z.string(), http: http.custom('PURGE', '/op') });
