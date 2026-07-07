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

// TypeScript smoke agent for issue #3393.
//
// This file is intentionally a TypeScript SDK smoke: it proves the read-only
// method variants compile, register, and are emitted into the component
// metadata, and that the resulting component builds via the QuickJS WASM
// injection / pre-initialization pipeline. Runtime cache semantics are covered
// by the Rust executor tests in golem-worker-executor/tests/readonly.rs and by
// the HTTP integration tests in integration-tests/tests/custom_api/readonly_http.rs.
//
// NOTE (fluent port): the decorator `@readonly()` supported four cache-policy
// variants (default `until-write`, principal-aware via a `Principal` parameter,
// `ttl`, and `no-cache`). The fluent `method({ readOnly: true })` surface only
// exposes a single boolean, which the runtime maps to `no-cache` /
// `usesPrincipal: false`. So the `until-write`, `ttl`, and principal-aware
// variants DEGRADE to `no-cache` here, and the `getCountFor` principal parameter
// is dropped (there is no `Principal` input schema). See the report.

import { z } from 'zod';
import { defineAgent, method, http } from '@golemcloud/golem-ts-sdk';

export const TsReadonlyAgent = defineAgent({
  name: 'TsReadonlyAgent',
  id: { agentName: z.string() },
  http: http.mount('/ts-readonly-agents/{agentName}'),
  methods: {
    // Non-read-only write, also exposed over HTTP so the TS agent could be
    // exercised end-to-end against the same fixtures as the Rust agent.
    increment: method({
      input: {},
      returns: z.number(),
      http: http.post('/increment'),
    }),

    // Decorator: default cache policy = 'until-write' (degrades to no-cache).
    getCount: method({
      input: {},
      returns: z.number(),
      readOnly: true,
      http: http.get('/count'),
    }),

    // Decorator: principal-aware (usesPrincipal auto-derived from a Principal
    // parameter). The fluent surface has no Principal input schema, so the
    // parameter is dropped and the method degrades to a plain no-cache read-only.
    getCountFor: method({
      input: {},
      returns: z.number(),
      readOnly: true,
      http: http.get('/count-for'),
    }),

    // Decorator: TTL cache policy (degrades to no-cache).
    readOnlyWithTtl: method({
      input: {},
      returns: z.number(),
      readOnly: true,
      http: http.get('/ttl-count'),
    }),

    // No-cache: pure compute, no host calls, runs every invocation.
    pureCompute: method({
      input: { x: z.number(), y: z.number() },
      returns: z.number(),
      readOnly: true,
    }),
  },
});

export const TsReadonlyAgentImpl = TsReadonlyAgent.implement({
  init: () => ({ count: 0 }),
  methods: {
    increment() {
      this.count += 1;
      return this.count;
    },
    getCount() {
      return this.count;
    },
    getCountFor() {
      return this.count;
    },
    readOnlyWithTtl() {
      return this.count;
    },
    pureCompute({ x, y }) {
      return Math.imul(x + y, 3);
    },
  },
});
