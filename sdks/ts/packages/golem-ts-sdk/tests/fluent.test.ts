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

import { describe, it, expect, vi } from 'vitest';
import { z } from 'zod';
import { WasmRpc } from 'golem:agent/host@2.0.0';
import type { CancellationToken, Datetime } from 'golem:agent/host@2.0.0';
import { defineAgent } from '../src/fluent/defineAgent';
import { method } from '../src/fluent/method';
import { clientFor } from '../src/fluent/client';
import { compileSchema } from '../src/fluent/schema/adapter';
import { s } from '../src/fluent/schema/markers';
import { Uuid } from '../src/uuid';
import { AgentClassName } from '../src/agentClassName';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';
import { AgentInitiatorRegistry } from '../src/internal/registry/agentInitiatorRegistry';

function remoteClientTypeChecks(): void {
  const def = defineAgent({
    name: 'RemoteClientTypeChecks',
    id: { name: z.string() },
    methods: {
      ping: method({ input: {}, returns: z.string() }),
      add: method({ input: { by: z.number() }, returns: z.number() }),
    },
  });
  const factory = clientFor(def);
  const client = factory({ name: 'counter' });
  const controller = new AbortController();
  void client.ping({ signal: controller.signal });
  void client.add({ by: 1 }, { signal: controller.signal });
  // @ts-expect-error cancellation is an option on the normal call, not a separate operation
  void client.ping.abortable(controller.signal);
  const at: Datetime = { seconds: 1n, nanoseconds: 0 };
  const pingToken: CancellationToken = client.ping.schedule(at);
  const addToken: CancellationToken = client.add.schedule(at, { by: 1 });
  // @ts-expect-error schedule now always returns a token; there is no separate variant
  void client.ping.scheduleCancelable(at);
  const phantom = factory.newPhantom({ name: 'counter' });
  const phantomId: Uuid = phantom.phantomId;
  void phantom.client.ping();
  void pingToken;
  void addToken;
  void phantomId;
}
void remoteClientTypeChecks;

describe('fluent Zod walker', () => {
  it('maps primitive schemas to schema types and round-trips values', () => {
    const s = compileSchema(z.string());
    expect(s.graph.root.body).toMatchObject({ tag: 'string' });
    expect(s.toValue('hi')).toEqual({ tag: 'string', value: 'hi' });
    expect(s.fromValue({ tag: 'string', value: 'hi' })).toBe('hi');

    const n = compileSchema(z.number());
    expect(n.graph.root.body).toMatchObject({ tag: 'f64' });
    expect(n.toValue(5)).toEqual({ tag: 'f64', value: 5 });
    expect(n.fromValue({ tag: 'f64', value: 8 })).toBe(8);

    expect(compileSchema(z.boolean()).graph.root.body).toMatchObject({ tag: 'bool' });
    expect(compileSchema(z.bigint()).graph.root.body).toMatchObject({ tag: 'u64' });
  });

  it('maps optional/nullable to option and unwraps default', () => {
    const opt = compileSchema(z.number().optional());
    expect(opt.graph.root.body).toMatchObject({ tag: 'option' });
    expect(opt.toValue(undefined)).toEqual({ tag: 'option', value: undefined });
    expect(opt.toValue(3)).toEqual({ tag: 'option', value: { tag: 'f64', value: 3 } });
    expect(opt.fromValue({ tag: 'option', value: undefined })).toBeUndefined();

    const nullable = compileSchema(z.number().nullable());
    expect(nullable.fromValue(nullable.toValue(null))).toBeNull();

    const nested = compileSchema(z.number().nullable().optional());
    expect(nested.fromValue(nested.toValue(undefined))).toBeUndefined();
    expect(nested.fromValue(nested.toValue(null))).toBeNull();
    // `.default()` is transparent at the wire level.
    expect(compileSchema(z.number().int().default(1)).graph.root.body).toMatchObject({
      tag: 'f64',
    });
  });

  it('maps arrays element-wise', () => {
    const arr = compileSchema(z.array(z.string()));
    expect(arr.graph.root.body).toMatchObject({
      tag: 'list',
      element: { body: { tag: 'string' } },
    });
    expect(arr.fromValue(arr.toValue(['a', 'b']))).toEqual(['a', 'b']);
  });

  it('maps objects to records preserving field order', () => {
    const obj = compileSchema(z.object({ x: z.string(), y: z.number() }));
    expect(obj.graph.root.body).toMatchObject({ tag: 'record' });
    const fields = (obj.graph.root.body as { tag: 'record'; fields: { name: string }[] }).fields;
    expect(fields.map((f) => f.name)).toEqual(['x', 'y']);
    const value = { x: 'hi', y: 2 };
    expect(obj.fromValue(obj.toValue(value))).toEqual(value);
  });

  it('maps tuples element-wise', () => {
    const tup = compileSchema(z.tuple([z.string(), z.number(), z.boolean()]));
    expect(tup.graph.root.body).toMatchObject({ tag: 'tuple' });
    const value = ['a', 1, true];
    expect(tup.fromValue(tup.toValue(value))).toEqual(value);
  });

  it('maps string enums to enum nodes by case index', () => {
    const en = compileSchema(z.enum(['red', 'green', 'blue']));
    expect(en.graph.root.body).toMatchObject({ tag: 'enum', cases: ['red', 'green', 'blue'] });
    expect(en.toValue('green')).toEqual({ tag: 'enum', caseIndex: 1 });
    expect(en.fromValue({ tag: 'enum', caseIndex: 2 })).toBe('blue');
  });

  it('maps a literal to its base primitive', () => {
    const lit = compileSchema(z.literal('ok'));
    expect(lit.graph.root.body).toMatchObject({ tag: 'string' });
    expect(lit.fromValue(lit.toValue('ok'))).toBe('ok');
  });

  it('maps z.record to a map node', () => {
    const rec = compileSchema(z.record(z.string(), z.number()));
    expect(rec.graph.root.body).toMatchObject({ tag: 'map' });
    const value = { a: 1, b: 2 };
    expect(rec.fromValue(rec.toValue(value))).toEqual(value);
  });

  it('maps z.map to a map node (arbitrary keys)', () => {
    const m = compileSchema(z.map(z.string(), z.number()));
    expect(m.graph.root.body).toMatchObject({ tag: 'map' });
    const value = new Map([
      ['a', 1],
      ['b', 2],
    ]);
    expect(m.fromValue(m.toValue(value))).toEqual(value);
  });

  it('maps a discriminated union to a variant', () => {
    const du = compileSchema(
      z.discriminatedUnion('kind', [
        z.object({ kind: z.literal('a'), x: z.string() }),
        z.object({ kind: z.literal('b'), y: z.number() }),
      ]),
    );
    expect(du.graph.root.body).toMatchObject({ tag: 'variant' });
    const a = { kind: 'a' as const, x: 'hi' };
    const b = { kind: 'b' as const, y: 7 };
    expect(du.toValue(a)).toMatchObject({ tag: 'variant', caseIndex: 0 });
    expect(du.fromValue(du.toValue(a))).toEqual(a);
    expect(du.toValue(b)).toMatchObject({ tag: 'variant', caseIndex: 1 });
    expect(du.fromValue(du.toValue(b))).toEqual(b);
  });

  it('maps a plain (non-discriminated) union to a variant and round-trips by case', () => {
    const u = compileSchema(z.union([z.string(), z.number(), z.boolean()]));
    expect(u.graph.root.body).toMatchObject({ tag: 'variant' });
    // Auto-named cases, structurally compatible with a discriminated union.
    expect(
      (u.graph.root.body as { tag: 'variant'; cases: { name: string }[] }).cases.map((c) => c.name),
    ).toEqual(['case0', 'case1', 'case2']);
    // Encode by structural disambiguation; decode by caseIndex.
    for (const [val, idx] of [['hi', 0] as const, [5, 1] as const, [true, 2] as const]) {
      expect(u.toValue(val)).toMatchObject({ tag: 'variant', caseIndex: idx });
      expect(u.fromValue(u.toValue(val))).toEqual(val);
    }
  });

  it('picks the right union-of-objects case by structure', () => {
    const u = compileSchema(z.union([z.object({ a: z.string() }), z.object({ b: z.number() })]));
    expect(u.graph.root.body).toMatchObject({ tag: 'variant' });
    const va = { a: 'x' };
    const vb = { b: 7 };
    expect(u.toValue(va)).toMatchObject({ tag: 'variant', caseIndex: 0 });
    expect(u.fromValue(u.toValue(va))).toEqual(va);
    expect(u.toValue(vb)).toMatchObject({ tag: 'variant', caseIndex: 1 });
    expect(u.fromValue(u.toValue(vb))).toEqual(vb);
  });

  it('throws a clear error when no union member accepts the value', () => {
    const u = compileSchema(z.union([z.string(), z.number()]));
    expect(() => u.toValue(true)).toThrow(/no ?.*member accepts|none matched/i);
  });

  it('rejects non-Standard-Schema values', () => {
    expect(() => compileSchema({} as never)).toThrow(/Standard Schema/);
  });
});

describe('fluent schema markers', () => {
  it('pins integer numerics to their WIT width and round-trips', () => {
    const small: {
      name: 'u8' | 'u16' | 'u32' | 's8' | 's16' | 's32';
      make: () => unknown;
      sample: number;
    }[] = [
      { name: 'u8', make: () => s.u8(), sample: 200 },
      { name: 'u16', make: () => s.u16(), sample: 40000 },
      { name: 'u32', make: () => s.u32(), sample: 4000000000 },
      { name: 's8', make: () => s.s8(), sample: -100 },
      { name: 's16', make: () => s.s16(), sample: -30000 },
      { name: 's32', make: () => s.s32(), sample: -2000000000 },
    ];
    for (const { name, make, sample } of small) {
      const codec = compileSchema(make());
      expect(codec.graph.root.body.tag).toBe(name);
      expect(codec.fromValue(codec.toValue(sample))).toBe(sample);
    }
  });

  it('pins 64-bit numerics (bigint) and round-trips', () => {
    const big: { name: 'u64' | 's64'; make: () => unknown; sample: bigint }[] = [
      { name: 'u64', make: () => s.u64(), sample: 12345678901234567890n },
      { name: 's64', make: () => s.s64(), sample: -1234567890123456789n },
    ];
    for (const { name, make, sample } of big) {
      const codec = compileSchema(make());
      expect(codec.graph.root.body.tag).toBe(name);
      expect(codec.fromValue(codec.toValue(sample))).toBe(sample);
    }
  });

  it('pins f32 and round-trips', () => {
    const codec = compileSchema(s.f32());
    expect(codec.graph.root.body.tag).toBe('f32');
    expect(codec.fromValue(codec.toValue(1.5))).toBe(1.5);
  });

  it('maps char to a char node and round-trips', () => {
    const codec = compileSchema(s.char());
    expect(codec.graph.root.body.tag).toBe('char');
    expect(codec.fromValue(codec.toValue('x'))).toBe('x');
  });

  it('maps datetime to a datetime node and round-trips', () => {
    const codec = compileSchema(s.datetime());
    expect(codec.graph.root.body.tag).toBe('datetime');
    const dt = { seconds: 1700000000n, nanoseconds: 500 };
    expect(codec.fromValue(codec.toValue(dt))).toEqual(dt);
  });

  it('maps duration to a duration node and round-trips', () => {
    const codec = compileSchema(s.duration());
    expect(codec.graph.root.body.tag).toBe('duration');
    expect(codec.fromValue(codec.toValue(42n))).toBe(42n);
  });

  it('maps url to a url node and round-trips', () => {
    const codec = compileSchema(s.url());
    expect(codec.graph.root.body.tag).toBe('url');
    expect(codec.fromValue(codec.toValue('https://golem.cloud'))).toBe('https://golem.cloud');
  });

  it('maps bytes to a list<u8> node and round-trips', () => {
    const codec = compileSchema(s.bytes());
    expect(codec.graph.root.body).toMatchObject({ tag: 'list', element: { body: { tag: 'u8' } } });
    const bytes = new Uint8Array([1, 2, 3, 255]);
    expect(codec.fromValue(codec.toValue(bytes))).toEqual(bytes);
  });

  it('wraps an inner schema in a secret capability node', () => {
    const codec = compileSchema(s.secret(z.string()));
    expect(codec.graph.root.body.tag).toBe('secret');
    expect(
      (codec.graph.root.body as { tag: 'secret'; inner: { body: { tag: string } } }).inner.body.tag,
    ).toBe('string');
  });

  it('maps quotaToken to a quota-token capability node', () => {
    const codec = compileSchema(s.quotaToken());
    expect(codec.graph.root.body.tag).toBe('quota-token');
  });

  it('maps unstructuredText to a role-tagged variant and round-trips', () => {
    const codec = compileSchema(s.unstructuredText());
    expect(codec.graph.root.body.tag).toBe('variant');
    expect(codec.graph.root.metadata.role).toMatchObject({ tag: 'unstructured-text' });
    const url = { tag: 'url' as const, val: 'https://x' };
    const inline = { tag: 'inline' as const, val: 'hello', languageCode: 'en' };
    expect(codec.fromValue(codec.toValue(url))).toEqual(url);
    expect(codec.fromValue(codec.toValue(inline))).toEqual(inline);
  });

  it('maps unstructuredBinary to a role-tagged variant and round-trips', () => {
    const codec = compileSchema(s.unstructuredBinary());
    expect(codec.graph.root.body.tag).toBe('variant');
    expect(codec.graph.root.metadata.role).toMatchObject({ tag: 'unstructured-binary' });
    const inline = {
      tag: 'inline' as const,
      val: new Uint8Array([1, 2, 3]),
      mimeType: 'image/png',
    };
    expect(codec.fromValue(codec.toValue(inline))).toEqual(inline);
  });

  it('maps multimodal to a role-tagged list<variant> and round-trips', () => {
    const codec = compileSchema(
      s.multimodal([
        { name: 'text', schema: s.unstructuredText() },
        { name: 'image', schema: s.unstructuredBinary() },
      ]),
    );
    expect(codec.graph.root.body.tag).toBe('list');
    expect(codec.graph.root.metadata.role).toMatchObject({ tag: 'multimodal' });
    const value = [
      { tag: 'text', value: { tag: 'inline', val: 'hi' } },
      { tag: 'image', value: { tag: 'url', val: 'https://img' } },
    ];
    expect(codec.fromValue(codec.toValue(value))).toEqual(value);
  });
});

describe('fluent defineAgent', () => {
  it('registers a well-formed AgentType for the counter', () => {
    const counterDef = defineAgent({
      name: 'counter',
      id: { name: z.string() },
      methods: {
        increment: method({ input: { by: z.number() }, returns: z.number() }),
        current: method({ input: {}, returns: z.number() }),
      },
    });

    counterDef.implement({
      init: () => ({ count: 0 }),
      methods: {
        increment({ by }) {
          this.count += by;
          return this.count;
        },
        current() {
          return this.count;
        },
      },
    });

    const agentType = AgentTypeRegistry.get(new AgentClassName('counter'));
    expect(agentType).toBeDefined();
    expect(agentType!.typeName).toBe('counter');
    expect(agentType!.sourceLanguage).toBe('typescript');

    // Constructor: single identity parameter `name`.
    const ctorInput = agentType!.constructor.inputSchema;
    expect(ctorInput.tag).toBe('parameters');
    expect(
      (ctorInput as { tag: 'parameters'; val: { name: string }[] }).val.map((f) => f.name),
    ).toEqual(['name']);

    // Methods: `increment(by)` returns a value; `current()` returns a value.
    const methods = Object.fromEntries(agentType!.methods.map((m) => [m.name, m]));
    expect(Object.keys(methods).sort()).toEqual(['current', 'increment']);

    const increment = methods['increment'];
    expect(increment.inputSchema.tag).toBe('parameters');
    expect(
      (increment.inputSchema as { tag: 'parameters'; val: { name: string }[] }).val.map(
        (f) => f.name,
      ),
    ).toEqual(['by']);
    expect(increment.outputSchema.tag).toBe('single');

    const current = methods['current'];
    expect((current.inputSchema as { tag: 'parameters'; val: unknown[] }).val).toEqual([]);
    expect(current.outputSchema.tag).toBe('single');

    // The initiator was registered so the guest can instantiate the agent.
    expect(AgentInitiatorRegistry.exists('counter')).toBe(true);
  });

  it('accepts an async init and exposes id/principal helpers on `this`', () => {
    const asyncCounter = defineAgent({
      name: 'asyncCounter',
      id: { name: z.string() },
      methods: {
        whoami: method({ input: {}, returns: z.string() }),
      },
    });

    asyncCounter.implement({
      // async `init` is now supported (may return State | Promise<State>).
      init: async () => ({ count: 0 }),
      methods: {
        whoami() {
          // Identity + principal helpers are available on `this`.
          const id = this.getId();
          void this.getPhantomId();
          void this.getPrincipal();
          return id.value;
        },
      },
    });

    expect(AgentInitiatorRegistry.exists('asyncCounter')).toBe(true);
  });
});

describe('fluent numeric restrictions', () => {
  it('s.u8({ min, max }) carries unsigned restrictions and round-trips', () => {
    const c = compileSchema(s.u8({ min: 1, max: 200 }));
    const body = c.graph.root.body as {
      tag: 'u8';
      restrictions?: { min?: { tag: string; val: bigint }; max?: { tag: string; val: bigint } };
    };
    expect(body.tag).toBe('u8');
    expect(body.restrictions?.min).toEqual({ tag: 'unsigned', val: 1n });
    expect(body.restrictions?.max).toEqual({ tag: 'unsigned', val: 200n });
    expect(c.fromValue(c.toValue(50))).toBe(50);
  });

  it('s.s16({ min, max }) carries signed restrictions', () => {
    const body = compileSchema(s.s16({ min: -10, max: 10 })).graph.root.body as {
      restrictions?: { min?: { tag: string }; max?: { tag: string } };
    };
    expect(body.restrictions?.min).toEqual({ tag: 'signed', val: -10n });
    expect(body.restrictions?.max).toEqual({ tag: 'signed', val: 10n });
  });

  it('s.u8() with no opts has no restrictions', () => {
    const body = compileSchema(s.u8()).graph.root.body as { tag: 'u8'; restrictions?: unknown };
    expect(body.tag).toBe('u8');
    expect(body.restrictions).toBeUndefined();
  });

  it('z.number().min().max() maps to f64 float-bits restrictions', () => {
    const c = compileSchema(z.number().min(2).max(9));
    const body = c.graph.root.body as {
      tag: 'f64';
      restrictions?: { min?: { tag: string }; max?: { tag: string } };
    };
    expect(body.tag).toBe('f64');
    expect(body.restrictions?.min?.tag).toBe('float-bits');
    expect(body.restrictions?.max?.tag).toBe('float-bits');
    expect(c.fromValue(c.toValue(5))).toBe(5);
  });

  it('plain z.number() has no restrictions', () => {
    const body = compileSchema(z.number()).graph.root.body as { restrictions?: unknown };
    expect(body.restrictions).toBeUndefined();
  });
});

describe('fluent RPC client', () => {
  const clientDef = defineAgent({
    name: 'FluentClientTestAgent',
    id: { name: z.string() },
    config: { greeting: z.string() },
    methods: {
      ping: method({ input: {}, returns: z.string() }),
      add: method({ input: { by: z.number() }, returns: z.number() }),
    },
  });

  const latestRpc = () =>
    vi.mocked(WasmRpc).mock.results.at(-1)!.value as {
      asyncInvokeAndAwait: ReturnType<typeof vi.fn>;
      scheduleInvocation: ReturnType<typeof vi.fn>;
      scheduleCancelableInvocation: ReturnType<typeof vi.fn>;
    };

  it('creates a fresh phantom client and exposes the generated id', () => {
    const phantomId = new Uuid(1n, 2n);
    const generate = vi.spyOn(Uuid, 'generate').mockReturnValue(phantomId);
    const phantom = clientFor(clientDef).newPhantom({ name: 'counter' }, { greeting: 'hello' });

    expect(phantom.phantomId).toBe(phantomId);
    expect(phantom.client.ping).toBeTypeOf('function');
    const constructorArgs = vi.mocked(WasmRpc).mock.calls.at(-1)!;
    expect(constructorArgs[2]).toBe(phantomId);
    expect(constructorArgs[3]).toHaveLength(1);
    generate.mockRestore();
  });

  it('preserves a declared remote method named phantomId on phantom clients', () => {
    const def = defineAgent({
      name: 'PhantomIdMethodAgent',
      id: {},
      methods: {
        phantomId: method({ input: {}, returns: z.string() }),
      },
    });
    const phantom = clientFor(def).newPhantom({});

    expect(typeof phantom.client.phantomId).toBe('function');
    expect(phantom.phantomId).toBeInstanceOf(Uuid);
  });

  it('returns cancellation tokens from schedule and removes the old operations', () => {
    const client = clientFor(clientDef)({ name: 'counter' });
    const rpc = latestRpc();
    const token = { cancel: vi.fn() };
    rpc.scheduleCancelableInvocation.mockReturnValue(token);
    const at: Datetime = { seconds: 1n, nanoseconds: 0 };

    expect(client.ping.schedule(at)).toBe(token);
    expect(client.add.schedule(at, { by: 2 })).toBe(token);
    expect(rpc.scheduleCancelableInvocation).toHaveBeenCalledTimes(2);
    expect(rpc.scheduleInvocation).not.toHaveBeenCalled();
    expect('abortable' in client.ping).toBe(false);
    expect('scheduleCancelable' in client.ping).toBe(false);
  });

  it('accepts cancellation options on input and zero-input calls', async () => {
    const client = clientFor(clientDef)({ name: 'counter' });
    const rpc = latestRpc();
    const controller = new AbortController();
    controller.abort('cancelled');

    await expect(client.add({ by: 1 }, { signal: controller.signal })).rejects.toBe('cancelled');
    await expect(client.ping({ signal: controller.signal })).rejects.toBe('cancelled');
    expect(rpc.asyncInvokeAndAwait).not.toHaveBeenCalled();
  });
});
