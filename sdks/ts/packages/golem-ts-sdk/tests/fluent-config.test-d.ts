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

// Type-only tests for the typed `this.config` / `InitContext.config` surface.
// Checked via `tsc --noEmit`; NOT executed by vitest (`.test-d.ts` suffix).

import { z } from 'zod';
import { defineAgent } from '../src/fluent/defineAgent';
import { method } from '../src/fluent/method';
import { s } from '../src/fluent/schema/markers';
import { Secret } from '../src/fluent/secret';

// ---------------------------------------------------------------------------
// Typed config: local field → decoded value; secret field → Secret<Inner>
// ---------------------------------------------------------------------------

const typedDef = defineAgent({
  name: 'CfgTyped',
  id: { name: z.string() },
  config: { greeting: z.string(), apiKey: s.secret(z.string()) },
  methods: { ping: method({ input: {}, returns: z.string() }) },
});

void typedDef.implement({
  init: (ctx) => {
    // InitContext.config is typed the same way.
    const g: string = ctx.config.greeting;
    const k: Secret<string> = ctx.config.apiKey;
    const kv: string = ctx.config.apiKey.get();
    void g;
    void k;
    void kv;
    return {};
  },
  methods: {
    ping() {
      // Local field is the decoded value.
      const g: string = this.config.greeting;
      // Secret field is a lazy Secret<string> handle; `.get()` yields the value.
      const k: Secret<string> = this.config.apiKey;
      const kv: string = this.config.apiKey.get();
      // @ts-expect-error 'typo' is not a declared config field
      void this.config.typo;
      void g;
      void k;
      void kv;
      return 'ok';
    },
  },
});

// A secret field is NOT the bare inner value (must call `.get()`).
void typedDef.implement({
  init: () => ({}),
  methods: {
    ping() {
      // @ts-expect-error apiKey is a Secret<string>, not a bare string
      const bad: string = this.config.apiKey;
      void bad;
      return 'ok';
    },
  },
});

// ---------------------------------------------------------------------------
// Nested config: secrets at any depth → Secret<Inner>; objects recursed;
// union/array read whole
// ---------------------------------------------------------------------------

const nestedDef = defineAgent({
  name: 'CfgNested',
  id: { name: z.string() },
  config: {
    top: z.string(),
    tags: z.array(z.string()),
    nested: z.object({
      a: z.string(),
      b: z.number(),
      c: s.secret(z.object({ d: z.string(), e: z.number() })),
    }),
  },
  methods: { ping: method({ input: {}, returns: z.string() }) },
});

void nestedDef.implement({
  init: () => ({}),
  methods: {
    ping() {
      // Nested local fields keep their decoded value types.
      const a: string = this.config.nested.a;
      const b: number = this.config.nested.b;
      // Nested secret → Secret<inner>; `.get()` yields the inner object.
      const c: Secret<{ d: string; e: number }> = this.config.nested.c;
      const cv: { d: string; e: number } = this.config.nested.c.get();
      // Top-level primitive + array read whole.
      const top: string = this.config.top;
      const tags: string[] = this.config.tags;
      // @ts-expect-error 'typo' is not a field of the nested object
      void this.config.nested.typo;
      void a;
      void b;
      void c;
      void cv;
      void top;
      void tags;
      return 'ok';
    },
  },
});

// A nested secret is NOT the bare inner value (must call `.get()`).
void nestedDef.implement({
  init: () => ({}),
  methods: {
    ping() {
      // @ts-expect-error nested.c is a Secret<{d;e}>, not a bare object
      const bad: { d: string; e: number } = this.config.nested.c;
      void bad;
      return 'ok';
    },
  },
});

// ---------------------------------------------------------------------------
// Config-less agent: `this.config` is `{}` — any field access is an error
// ---------------------------------------------------------------------------

const noCfgDef = defineAgent({
  name: 'CfgNone',
  id: { name: z.string() },
  methods: { ping: method({ input: {}, returns: z.string() }) },
});

void noCfgDef.implement({
  init: (ctx) => {
    // @ts-expect-error config-less agent's InitContext.config has no fields
    void ctx.config.anything;
    return {};
  },
  methods: {
    ping() {
      // @ts-expect-error config-less agent's this.config has no fields
      void this.config.anything;
      return 'ok';
    },
  },
});
