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
import { defineAgent } from '../src/defineAgent';
import { method } from '../src/method';
import { s } from '../src/schema/markers';
import { Secret } from '../src/secret';
import type { ConfigOverrides } from '../src/client';

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
    rows: z.array(z.object({ label: z.string() })),
    optionalRows: z.array(z.object({ label: z.string() })).optional(),
    readonlyRows: z.array(z.object({ label: z.string() })).readonly(),
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
      const row: string = this.config.rows[0].label;
      const optionalRow: string | undefined = this.config.optionalRows?.[0].label;
      const readonlyRow: string = this.config.readonlyRows[0].label;
      // @ts-expect-error an array does not expose its element's fields directly
      void this.config.rows.label;
      // @ts-expect-error optional arrays retain their array shape
      void this.config.optionalRows?.label;
      // @ts-expect-error readonly arrays retain their array shape
      void this.config.readonlyRows.label;
      // @ts-expect-error 'typo' is not a field of the nested object
      void this.config.nested.typo;
      void a;
      void b;
      void c;
      void cv;
      void top;
      void tags;
      void row;
      void optionalRow;
      void readonlyRow;
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

const staticDef = defineAgent({
  name: 'TypedStaticConfig',
  id: {},
  config: {
    settings: s.default(z.object({ label: z.string(), count: s.u32() }), {
      label: 'default',
      count: 31,
    }),
    slots: s.default(s.tuple([s.option(s.s32()), s.option(s.s32()), z.string()]), [
      null,
      7,
      'retained',
    ]),
    choice: s.union([
      {
        tag: 'mail',
        schema: z.object({ kind: z.literal('mail'), address: z.string() }),
        discriminator: { tag: 'field-equals', val: { fieldName: 'kind', literal: 'mail' } },
      },
      {
        tag: 'queue',
        schema: z.object({ kind: z.literal('queue'), queue: z.string() }),
        discriminator: { tag: 'field-equals', val: { fieldName: 'kind', literal: 'queue' } },
      },
    ]),
  },
  methods: { read: method({ input: {}, returns: z.string() }) },
});
void staticDef.implement({
  init: () => ({}),
  methods: {
    read() {
      const count: number = this.config.settings.count;
      const retained: string = this.config.slots[2];
      const optional: number | undefined = this.config.slots[0];
      void retained;
      void optional;
      // @ts-expect-error defaults do not erase the declared output type
      const invalid: boolean = this.config.settings.label;
      void invalid;
      return this.config.choice.kind === 'mail'
        ? this.config.choice.address
        : this.config.choice.queue + count;
    },
  },
});

const overrideConfig = {
  settings: z.object({ label: z.string(), count: s.u32() }),
  service: z.object({ timeout: z.number(), apiKey: s.secret(z.string()) }),
  optionalService: z.object({ timeout: z.number(), apiKey: s.secret(z.string()) }).optional(),
};
const wholeRecord: ConfigOverrides<typeof overrideConfig> = {
  settings: { label: 'whole', count: 7 },
  service: { timeout: 13 },
  optionalService: { timeout: 19 },
};
void wholeRecord;
void defineAgent({
  name: 'OptionalManagedConfig',
  id: {},
  config: overrideConfig,
  methods: {},
}).implement({
  init: ({ config }) => {
    const timeout: number | undefined = config.optionalService?.timeout;
    const key: string | undefined = config.optionalService?.apiKey.get();
    // @ts-expect-error an optional managed group may be absent
    const required: number = config.optionalService.timeout;
    void timeout;
    void key;
    void required;
    return {};
  },
  methods: {},
});
const partialRecord: ConfigOverrides<typeof overrideConfig> = {
  // @ts-expect-error a static record is one declaration, not a partial namespace
  settings: { label: 'incomplete' },
};
void partialRecord;
const secretOverride: ConfigOverrides<typeof overrideConfig> = {
  // @ts-expect-error a namespace cannot supply a managed Secret
  service: { apiKey: 'secret' },
};
void secretOverride;
