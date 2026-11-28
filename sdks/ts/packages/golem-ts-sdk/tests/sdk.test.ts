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
import {
  getAgentTypeByAgentId,
  Invocation as HostInvocation,
  WasmRpc,
} from 'golem:agent/host@2.0.0';
import type { Datetime } from 'golem:agent/host@2.0.0';
import { defineAgent } from '../src/defineAgent';
import { Invocation } from '../src/bridge/agent';
import type { AgentSpec } from '../src/defineAgent';
import { method } from '../src/method';
import {
  defineAgentClient,
  isRemoteCallError,
  RemoteCallError,
  RemoteOutputError,
} from '../src/client';
import { compileSchema } from '../src/schema/adapter';
import type { StandardSchemaV1 } from '../src/schema/standardSchema';
import { s } from '../src/schema/markers';
import { Uuid } from '../src/uuid';
import { AgentId } from '../src/agentId';
import { DynamicAgentClient } from '../src/dynamicClient';
import { AgentClassName } from '../src/agentClassName';
import { AgentTypeRegistry } from '../src/internal/registry/agentTypeRegistry';
import { AgentInitiatorRegistry } from '../src/internal/registry/agentInitiatorRegistry';
import {
  schemaValueFromWit,
  schemaValueToWit,
  typedSchemaValueToWit,
  v,
} from '../src/internal/schema-model';
import { PermissionCard, PolymorphicPermissionCard } from '../src/host/permissions';
import { PrincipalRef } from '../src/caller';
import {
  fakeRawCard,
  permissionCardFromRaw,
  polymorphicPermissionCardFromRaw,
  rawOfPermissionCard,
  rawOfScopeCard,
  scopeCardFromRaw,
} from './permissionCardTestSupport';
import {
  fakeHostInvocation,
  hostInvocationMetadata,
  hostInvocationResult,
  sdkInvocationMetadata,
  testInvocationUuid,
} from './invocationMetadataTestSupport';

async function remoteClientTypeChecks(): Promise<void> {
  const def = defineAgent({
    name: 'RemoteClientTypeChecks',
    id: { name: z.string() },
    methods: {
      ping: method({ input: {}, returns: z.string() }),
      add: method({ input: { by: z.number() }, returns: z.number() }),
    },
  });
  const factory = def.client;
  const client = factory.upsert({ name: 'counter' });
  const agentId: AgentId = await client.agentId();
  const controller = new AbortController();
  void client.ping({ signal: controller.signal });
  void client.add({ by: 1 }, { signal: controller.signal });
  // @ts-expect-error cancellation is an option on the normal call, not a separate operation
  void client.ping.abortable(controller.signal);
  const at: Datetime = { seconds: 1n, nanoseconds: 0 };
  const pingInvocation: Invocation<string> = await client.ping.schedule(at);
  const addInvocation: Invocation<number> = await client.add.trigger({ by: 1 });
  const added: number = await addInvocation.awaitResult({ signal: controller.signal });
  const cancelled: boolean = await pingInvocation.cancel();
  // @ts-expect-error schedule returns the handle; there is no cancelable variant
  void client.ping.scheduleCancelable(at);
  const created = await factory.create({ name: 'counter' });
  void created.ping();
  const keyed = await factory.create({ name: 'counter' }, { keyed: true, env: { A: 'b' } });
  void keyed.agentId();
  const byId = factory.getById(agentId);
  void byId.ping();
  // @ts-expect-error upsert takes no configuration overrides
  factory.upsert({ name: 'counter' }, { greeting: 'hello' });
  // @ts-expect-error getById takes a typed AgentId, not its text form
  void factory.getById('00000007-0000-8000-8000-000000000001');
  // @ts-expect-error the phantom factories are removed
  factory.newPhantom({ name: 'counter' });
  // @ts-expect-error the phantom factories are removed
  factory.getPhantom({ name: 'counter' }, agentId);
  // @ts-expect-error get is renamed to upsert
  factory.get({ name: 'counter' });
  // @ts-expect-error definitions derive no agent ID offline
  def.agentId({ name: 'counter' });

  const ephemeralDef = defineAgent({
    name: 'EphemeralRemoteClientTypeChecks',
    mode: 'ephemeral',
    id: { name: z.string() },
    methods: { ping: method({ input: {}, returns: z.string() }) },
  });
  const ephemeralFactory = ephemeralDef.client;
  const ephemeral = ephemeralFactory.upsert({ name: 'counter' });
  void ephemeral.ping().then(({ metadata, value }) => ({ metadata, value }));
  const ephemeralTriggered: Invocation<string> = await ephemeral.ping.trigger();
  const ephemeralScheduled: Invocation<string> = await ephemeral.ping.schedule(at);
  const ephemeralAgentId: AgentId = ephemeralTriggered.id.agentId;
  // @ts-expect-error invocation metadata is only part of awaited results
  void ephemeralScheduled.metadata;
  // @ts-expect-error ephemeral factories cannot create a durable agent
  void ephemeralFactory.create({ name: 'counter' });
  // @ts-expect-error ephemeral factories cannot address a stable agent
  void ephemeralFactory.getById(agentId);
  // @ts-expect-error ephemeral clients have no stable agent ID
  void ephemeral.agentId();
  void added;
  void cancelled;
  void ephemeralAgentId;

  const fullEphemeralDefinition = defineAgentClient({
    name: 'FullEphemeralClientTypeChecks',
    mode: 'ephemeral',
    id: { name: z.string() },
    methods: { ping: method({ input: {}, returns: z.string() }) },
  });
  const fullEphemeralClient = fullEphemeralDefinition.client.upsert({ name: 'counter' });
  void fullEphemeralClient.ping().then(({ metadata, value }) => ({
    agentId: metadata.agentId,
    invocationId: metadata.invocationId,
    value,
  }));

  const methodDefinition = defineAgentClient({
    methods: { ping: method({ input: {}, returns: z.string() }) },
  });
  void methodDefinition.getById(agentId).ping();
  void DynamicAgentClient.getById(agentId).method('ping');
  // @ts-expect-error method-only clients cannot declare a name without an ID shape
  defineAgentClient({ name: 'Named', methods: methodDefinition.methods });
  // @ts-expect-error lifecycle mode belongs to full constructor definitions, not ID bindings
  defineAgentClient({ mode: 'durable', methods: methodDefinition.methods });
  // @ts-expect-error method-only clients cannot declare a name or lifecycle mode
  defineAgentClient({ name: 'Named', mode: 'ephemeral', methods: methodDefinition.methods });
  // @ts-expect-error binding definitions expose no synthetic lifecycle mode
  void methodDefinition.mode;
  // @ts-expect-error method-only clients do not expose constructor-based factories
  methodDefinition.client.upsert({});

  const configuredDefinition = defineAgentClient({
    name: 'ConfiguredRemoteClient',
    id: {},
    config: { greeting: z.string(), apiKey: s.secret(z.string()) },
    methods: { ping: method({ input: {}, returns: z.string() }) },
  });
  void configuredDefinition.client.create({}, { config: { greeting: 'hello' } });
  void configuredDefinition.client.create({}, { keyed: true, config: { greeting: 'hello' } });
  // @ts-expect-error config overrides retain their declared value types
  void configuredDefinition.client.create({}, { config: { greeting: 42 } });
  // @ts-expect-error secret configuration cannot be overridden by an RPC caller
  void configuredDefinition.client.create({}, { config: { apiKey: 'secret' } });
  // @ts-expect-error env overrides are string values
  void configuredDefinition.client.create({}, { env: { A: 1 } });

  const ephemeralId = { name: z.string() };
  const ephemeralMethods = { ping: method({ input: {}, returns: z.string() }) };
  // @ts-expect-error an ephemeral AgentSpec must explicitly select ephemeral runtime behavior
  const omittedMode: AgentSpec<
    typeof ephemeralId,
    typeof ephemeralMethods,
    {},
    'name',
    never,
    StandardSchemaV1,
    'ephemeral'
  > = {
    name: 'OmittedEphemeralModeAgent',
    id: ephemeralId,
    methods: ephemeralMethods,
  };
  void omittedMode;
}
void remoteClientTypeChecks;

describe('Zod walker', () => {
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
    expect(compileSchema(z.bigint()).graph.root.body).toMatchObject({ tag: 's64' });
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

describe('schema markers', () => {
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

  it('maps and round-trips permissionCard values with the requested polymorphism', () => {
    const codec = compileSchema(s.permissionCard({ polymorphic: true }));
    expect(codec.graph.root.body).toEqual({
      tag: 'permission-card',
      spec: { polymorphic: true },
    });

    const raw = fakeRawCard({ id: 'opaque-permission-card' }) as never;
    const card = polymorphicPermissionCardFromRaw(raw);
    // A local decode of the card's own value would alias it into a second card.
    expect(() => codec.fromValue(codec.toValue(card))).toThrow(/already held by another card/);
    expect(card.isMoved()).toBe(false);
    const decoded = codec.fromValue(schemaValueFromWit(schemaValueToWit(codec.toValue(card))));
    expect(decoded).toBeInstanceOf(PolymorphicPermissionCard);
    expect(rawOfPermissionCard(decoded)).toBe(raw);
    expect(card.isMoved()).toBe(true);
    expect(() => codec.toValue({ id: 'raw-host-resource' })).toThrow();

    const concreteCodec = compileSchema(s.permissionCard({ polymorphic: false }));
    expect(concreteCodec.graph.root.body).toEqual({
      tag: 'permission-card',
      spec: { polymorphic: false },
    });
    const concreteRaw = fakeRawCard({ id: 'concrete-permission-card' }) as never;
    const concrete = concreteCodec.fromValue(
      schemaValueFromWit(
        schemaValueToWit(concreteCodec.toValue(permissionCardFromRaw(concreteRaw))),
      ),
    );
    expect(concrete).toBeInstanceOf(PermissionCard);
    expect(rawOfPermissionCard(concrete)).toBe(concreteRaw);
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

describe('defineAgent', () => {
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
    expect(agentType!.kind).toBe('regular');
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

  it('accepts an async init and exposes id helpers on `this` and the caller on `ctx`', () => {
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
        whoami(_input, ctx) {
          // Identity helpers are available on `this`; the caller is on `ctx`.
          const id = this.getId();
          void ctx.caller;
          return id.value;
        },
      },
    });

    expect(AgentInitiatorRegistry.exists('asyncCounter')).toBe(true);
  });
});

describe('numeric restrictions', () => {
  const f64 = (x: number) => {
    const view = new DataView(new ArrayBuffer(8));
    view.setFloat64(0, x);
    return { tag: 'float-bits', val: view.getBigUint64(0) };
  };

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

  it('z.number() exclusive gt/lt bounds are not published', () => {
    for (const schema of [z.number().gt(0), z.number().lt(10), z.number().positive()]) {
      const body = compileSchema(schema).graph.root.body as { restrictions?: unknown };
      expect(body.restrictions).toBeUndefined();
    }
  });

  it('z.number() publishes only the inclusive bounds next to exclusive ones', () => {
    const body = compileSchema(z.number().gte(1).gt(5).lt(8).lte(9)).graph.root.body as {
      restrictions?: { min?: unknown; max?: unknown };
    };
    expect(body.restrictions?.min).toEqual(f64(1));
    expect(body.restrictions?.max).toEqual(f64(9));
  });

  it('z.number() publishes the tightest inclusive bounds', () => {
    const body = compileSchema(z.number().min(5).min(2).max(3).max(9)).graph.root.body as {
      restrictions?: { min?: unknown; max?: unknown };
    };
    expect(body.restrictions?.min).toEqual(f64(5));
    expect(body.restrictions?.max).toEqual(f64(3));
  });

  it('plain z.number() has no restrictions', () => {
    const body = compileSchema(z.number()).graph.root.body as { restrictions?: unknown };
    expect(body.restrictions).toBeUndefined();
  });
});

describe('RPC client', () => {
  const clientDef = defineAgent({
    name: 'ClientTestAgent',
    id: { name: z.string() },
    config: { greeting: z.string() },
    methods: {
      ping: method({ input: {}, returns: z.string() }),
      add: method({ input: { by: z.number() }, returns: z.number() }),
    },
  });

  const latestRpc = () =>
    vi.mocked(WasmRpc.upsert).mock.results.at(-1)!.value as {
      invokeAndAwait: ReturnType<typeof vi.fn>;
      trigger: ReturnType<typeof vi.fn>;
      schedule: ReturnType<typeof vi.fn>;
    };

  const remoteAgentId = AgentId.parse('00000007-0000-8000-8000-000000000004');
  const createdRpc = () =>
    vi.mocked(WasmRpc.create).mock.results.at(-1)!.value as Promise<{
      client: ReturnType<typeof latestRpc> & { agentId: ReturnType<typeof vi.fn> };
    }>;

  it('exposes a cached client factory on authored agent definitions', () => {
    expect(clientDef.client).toBe(clientDef.client);
    const client = clientDef.client.upsert({ name: 'counter' });

    expect(client.ping).toBeTypeOf('function');
  });

  it('maps upsert to the host upsert with the encoded constructor parameters and no overrides', () => {
    clientDef.client.upsert({ name: 'counter' });
    expect(WasmRpc.upsert).toHaveBeenLastCalledWith(
      'ClientTestAgent',
      schemaValueToWit(v.record([v.string('counter')])),
    );
    expect(vi.mocked(WasmRpc.upsert).mock.calls.at(-1)).toHaveLength(2);
  });

  it('asks the host for the ID of an upsert client and returns a typed AgentId', async () => {
    const client = clientDef.client.upsert({ name: 'counter' });
    const rpc = vi.mocked(WasmRpc.upsert).mock.results.at(-1)!.value as {
      agentId: ReturnType<typeof vi.fn>;
    };
    const agentId = await client.agentId();
    expect(rpc.agentId).toHaveBeenCalledTimes(1);
    expect(agentId).toBe('00000007-0000-8000-8000-000000000004');

    rpc.agentId.mockRejectedValueOnce({ tag: 'denied', val: 'no view' });
    await expect(client.agentId()).rejects.toMatchObject({
      name: 'RemoteCallError',
      cause: { tag: 'denied', details: 'no view' },
    });
  });

  it('maps a create without keyed to the host create with keyed false and no overrides', async () => {
    const client = await clientDef.client.create({ name: 'counter' });
    expect(WasmRpc.create).toHaveBeenLastCalledWith(
      'ClientTestAgent',
      schemaValueToWit(v.record([v.string('counter')])),
      { keyed: false, agentConfig: [], env: [] },
    );
    expect(client.ping).toBeTypeOf('function');
    const rpc = (await createdRpc()).client;
    await expect(client.agentId()).resolves.toBe('00000007-0000-8000-8000-000000000004');
    expect(rpc.agentId).toHaveBeenCalledTimes(1);
  });

  it('passes keyed, the config overrides and the env overrides of create to the host', async () => {
    await clientDef.client.create(
      { name: 'counter' },
      { keyed: true, config: { greeting: 'hello' }, env: { REGION: 'eu', MODE: 'test' } },
    );
    const [typeName, tree, options] = vi.mocked(WasmRpc.create).mock.calls.at(-1)!;
    expect(typeName).toBe('ClientTestAgent');
    expect(tree).toEqual(schemaValueToWit(v.record([v.string('counter')])));
    expect(options.keyed).toBe(true);
    expect(options.env).toEqual([
      ['REGION', 'eu'],
      ['MODE', 'test'],
    ]);
    expect(options.agentConfig).toEqual([
      {
        path: ['greeting'],
        value: typedSchemaValueToWit({
          graph: compileSchema(z.string()).graph,
          value: v.string('hello'),
        }),
      },
    ]);
  });

  it('never copies the caller env into a create', async () => {
    await clientDef.client.create({ name: 'counter' }, { keyed: true });
    expect(vi.mocked(WasmRpc.create).mock.calls.at(-1)![2].env).toEqual([]);
  });

  it('rejects a keyed create of a current key with a typed conflict error', async () => {
    vi.mocked(WasmRpc.create).mockRejectedValueOnce({
      tag: 'conflict',
      val: 'ClientTestAgent("counter") already exists',
    });
    const created = clientDef.client.create({ name: 'counter' }, { keyed: true });
    await expect(created).rejects.toBeInstanceOf(RemoteCallError);
    await expect(created).rejects.toMatchObject({
      cause: { tag: 'conflict', details: 'ClientTestAgent("counter") already exists' },
    });
    expect(isRemoteCallError(await created.catch((error: unknown) => error))).toBe(true);
  });

  it('rejects config overrides of create before calling the host', async () => {
    const creates = vi.mocked(WasmRpc.create).mock.calls.length;
    await expect(
      clientDef.client.create({ name: 'counter' }, { config: { extra: 1 } as never }),
    ).rejects.toThrow("Unknown config path 'extra'");
    expect(WasmRpc.create).toHaveBeenCalledTimes(creates);
  });

  it('maps getById to the host get-by-id with the agent type and the typed ID', async () => {
    const client = clientDef.client.getById(remoteAgentId);
    expect(WasmRpc.getById).toHaveBeenLastCalledWith(
      'ClientTestAgent',
      AgentId.toWit(remoteAgentId),
    );
    expect(client.ping).toBeTypeOf('function');
  });

  it('throws on getById of an unknown or wrong-type ID with a typed not-found error', () => {
    vi.mocked(WasmRpc.getById).mockImplementationOnce(() => {
      throw { tag: 'not-found', val: 'no ClientTestAgent has this ID' };
    });
    const upserts = vi.mocked(WasmRpc.upsert).mock.calls.length;
    const creates = vi.mocked(WasmRpc.create).mock.calls.length;
    const error = thrownBy(() => clientDef.client.getById(remoteAgentId));
    expect(error).toBeInstanceOf(RemoteCallError);
    expect(error).toMatchObject({
      cause: { tag: 'not-found', details: 'no ClientTestAgent has this ID' },
    });
    expect(WasmRpc.upsert).toHaveBeenCalledTimes(upserts);
    expect(WasmRpc.create).toHaveBeenCalledTimes(creates);
  });

  it('throws a typed error when the host rejects an upsert', () => {
    vi.mocked(WasmRpc.upsert).mockImplementationOnce(() => {
      throw { tag: 'denied', val: 'no create permission' };
    });
    expect(() => clientDef.client.upsert({ name: 'counter' })).toThrow(RemoteCallError);
  });

  it('addresses an ephemeral type by upsert only', () => {
    const def = defineAgent({
      name: 'EphemeralFactoryAgent',
      mode: 'ephemeral',
      id: {},
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });
    const client = def.client.upsert({});
    expect(WasmRpc.upsert).toHaveBeenLastCalledWith(
      'EphemeralFactoryAgent',
      schemaValueToWit(v.record([])),
    );
    expect(Object.keys(def.client)).toEqual(['upsert']);
    expect('agentId' in client).toBe(false);
  });

  it('rejects a durable client whose agent type declares a method named agentId', () => {
    const def = defineAgentClient({
      name: 'ClientIdentityAgent',
      id: {},
      methods: { agentId: method({ input: {}, returns: z.string() }) },
    });
    expect(() => def.client.upsert({})).toThrow(/cannot have a method named 'agentId'/);
  });

  it('borrows the client default or per-call scope card for awaited calls only', async () => {
    const def = defineAgent({
      name: 'ScopeCardClientAgent',
      id: { name: z.string() },
      methods: {
        touch: method({ input: {}, returns: z.void() }),
        echo: method({ input: { text: z.string() }, returns: z.string() }),
      },
    });
    const defaultCard = scopeCardFromRaw({ id: 'default-scope' });
    const callCard = scopeCardFromRaw({ id: 'call-scope' });
    const client = def.client.upsert({ name: 'scoped' }, { scopeCard: defaultCard });
    const rpc = latestRpc();
    const scopeOfLastAwait = () => rpc.invokeAndAwait.mock.calls.at(-1)![2];
    rpc.trigger.mockResolvedValue(fakeHostInvocation());
    rpc.invokeAndAwait.mockResolvedValue(
      hostInvocationResult({ valueNodes: [{ tag: 'string-value', val: 'hi' }], root: 0 }),
    );

    await expect(client.echo({ text: 'a' })).resolves.toBe('hi');
    expect(scopeOfLastAwait()).toBe(rawOfScopeCard(defaultCard));
    await expect(client.echo({ text: 'b' }, { scopeCard: callCard })).resolves.toBe('hi');
    expect(scopeOfLastAwait()).toBe(rawOfScopeCard(callCard));
    rpc.invokeAndAwait.mockResolvedValue(hostInvocationResult());
    await client.touch({ scopeCard: null });
    expect(scopeOfLastAwait()).toBeUndefined();
    await client.touch.trigger();
    expect(rpc.trigger.mock.calls.at(-1)).toHaveLength(2);
    expect(defaultCard.isMoved()).toBe(false);
    expect(callCard.isMoved()).toBe(false);

    const awaited = rpc.invokeAndAwait.mock.calls.length;
    defaultCard.release();
    await expect(client.touch()).rejects.toThrow(/already been released/);
    expect(rpc.invokeAndAwait.mock.calls.length).toBe(awaited);
  });

  it('builds a client-only definition from Standard Schema libraries without registration', () => {
    const name = 'RuntimeBuiltClientOnlyAgent';
    const before = AgentTypeRegistry.getRegisteredAgents().length;
    const definition = defineAgentClient({
      name,
      id: { name: z.string() },
      methods: { ping: method({ input: { message: z.string() }, returns: z.string() }) },
    });
    const client = definition.client.upsert({ name: 'counter' });

    expect(client.ping).toBeTypeOf('function');
    expect(AgentTypeRegistry.getRegisteredAgents()).toHaveLength(before);
    expect(AgentTypeRegistry.exists(new AgentClassName(name))).toBe(false);
  });

  it('binds a method-only client by ID to the agent type that the ID tag names', async () => {
    const definition = defineAgentClient({
      methods: { ping: method({ input: { message: z.string() }, returns: z.string() }) },
    });
    vi.mocked(getAgentTypeByAgentId).mockReturnValueOnce({
      agentType: { typeName: 'FirstAgent', mode: 'durable' },
    } as never);
    const client = definition.getById(remoteAgentId);
    expect(getAgentTypeByAgentId).toHaveBeenLastCalledWith(AgentId.toWit(remoteAgentId));
    expect(WasmRpc.getById).toHaveBeenLastCalledWith('FirstAgent', AgentId.toWit(remoteAgentId));
    expect(client.ping).toBeTypeOf('function');
  });

  it('throws on a method-only getById whose ID tag names no deployed agent type', () => {
    const definition = defineAgentClient({
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });
    vi.mocked(getAgentTypeByAgentId).mockReturnValueOnce(undefined);
    const byIds = vi.mocked(WasmRpc.getById).mock.calls.length;
    expect(thrownBy(() => definition.getById(remoteAgentId))).toMatchObject({
      name: 'RemoteCallError',
      cause: { tag: 'not-found' },
    });
    expect(WasmRpc.getById).toHaveBeenCalledTimes(byIds);
  });

  it('throws on a method-only getById of an ephemeral agent type', () => {
    const definition = defineAgentClient({
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });
    vi.mocked(getAgentTypeByAgentId).mockReturnValueOnce({
      agentType: { typeName: 'EphemeralAgent', mode: 'ephemeral' },
    } as never);
    const byIds = vi.mocked(WasmRpc.getById).mock.calls.length;
    expect(thrownBy(() => definition.getById(remoteAgentId))).toMatchObject({
      name: 'RemoteCallError',
      cause: { tag: 'not-found', details: expect.stringMatching(/ephemeral agent type/) },
    });
    expect(WasmRpc.getById).toHaveBeenCalledTimes(byIds);
  });

  it('rejects lifecycle fields on a method-only JavaScript client', () => {
    expect(() =>
      defineAgentClient({
        mode: 'durable',
        methods: { ping: method({ input: {}, returns: z.string() }) },
      } as any),
    ).toThrow(
      'A method-only client may define only methods; name, id, config, and mode require a fully defined client',
    );
  });

  it('rejects a type name without an ID shape on a method-only client', () => {
    expect(() =>
      defineAgentClient({
        name: 'NamedBinding',
        methods: { ping: method({ input: {}, returns: z.string() }) },
      } as any),
    ).toThrow(
      'A method-only client may define only methods; name, id, config, and mode require a fully defined client',
    );
  });

  it('surfaces an incompatible target method as a structured remote error', async () => {
    const definition = defineAgentClient({
      methods: { analyze: method({ input: { topic: z.string() }, returns: z.string() }) },
    });
    vi.mocked(getAgentTypeByAgentId).mockReturnValueOnce({
      agentType: { typeName: 'ReportArchive', mode: 'durable' },
    } as never);
    const client = definition.getById(remoteAgentId);
    const rpc = vi.mocked(WasmRpc.getById).mock.results.at(-1)!.value;
    rpc.invokeAndAwait.mockRejectedValue({
      tag: 'remote-agent-error',
      val: { tag: 'invalid-method', val: 'analyze is not defined' },
    });

    await expect(client.analyze({ topic: 'demand' })).rejects.toMatchObject({
      cause: {
        tag: 'remote-agent-error',
        error: { tag: 'invalid-method', details: 'analyze is not defined' },
      },
    });
  });

  it('returns typed invocation handles from trigger and schedule', async () => {
    const client = clientDef.client.upsert({ name: 'counter' });
    const rpc = latestRpc();
    const triggered = fakeHostInvocation();
    triggered.status.mockResolvedValue('running');
    triggered.await_.mockResolvedValue({ valueNodes: [{ tag: 'f64-value', val: 3 }], root: 0 });
    triggered.cancel.mockRejectedValue({ tag: 'cannot-cancel-running', val: 'running' });
    const scheduled = fakeHostInvocation();
    scheduled.cancel.mockResolvedValue(true);
    rpc.trigger.mockResolvedValue(triggered);
    rpc.schedule.mockResolvedValue(scheduled);
    const at: Datetime = { seconds: 1n, nanoseconds: 0 };

    const added = await client.add.trigger({ by: 2 });
    expect(rpc.trigger).toHaveBeenCalledWith('add', expect.anything());
    expect(added).toBeInstanceOf(Invocation);
    expect(added.id).toEqual(sdkInvocationMetadata().invocationId);
    await expect(added.status()).resolves.toBe('running');
    await expect(added.awaitResult()).resolves.toBe(3);
    await expect(added.cancel()).rejects.toMatchObject({
      name: 'RemoteCallError',
      cause: { tag: 'cannot-cancel-running', details: 'running' },
    });

    const pinged = await client.ping.schedule(at);
    expect(rpc.schedule).toHaveBeenCalledWith(at, 'ping', expect.anything());
    await expect(pinged.cancel()).resolves.toBe(true);
    expect(rpc.invokeAndAwait).not.toHaveBeenCalled();
    expect('abortable' in client.ping).toBe(false);
    expect('scheduleCancelable' in client.ping).toBe(false);
  });

  it('rebuilds a handle from its ID with the output codec of the method spec', async () => {
    const raw = fakeHostInvocation();
    raw.await_.mockResolvedValue(schemaValueToWit(v.string('done')));
    vi.mocked(HostInvocation.get).mockReturnValue(raw as never);
    const id = sdkInvocationMetadata().invocationId;

    const invocation = Invocation.get(id, clientDef.methods.ping);
    expect(HostInvocation.get).toHaveBeenLastCalledWith({
      owner: { tag: 'agent', val: AgentId.toWit(id.agentId) },
      id: id.uuid,
    });
    expect(invocation.id).toBe(id);
    await expect(invocation.awaitResult()).resolves.toBe('done');

    raw.await_.mockResolvedValue(schemaValueToWit(v.u32(1)));
    await expect(Invocation.get(id, clientDef.methods.ping).awaitResult()).rejects.toBeInstanceOf(
      RemoteOutputError,
    );
    raw.await_.mockRejectedValue({ tag: 'cancelled', val: 'withdrawn before start' });
    await expect(Invocation.get(id, clientDef.methods.ping).awaitResult()).rejects.toMatchObject({
      name: 'RemoteCallError',
      cause: { tag: 'cancelled', details: 'withdrawn before start' },
    });
  });

  it('rolls back client input record capability adoption when a later field fails', async () => {
    const def = defineAgent({
      name: 'CapabilityInputAgent',
      id: {},
      methods: {
        send: method({
          input: { capability: s.secret(z.string()), later: z.string() },
          returns: z.void(),
        }),
      },
    });
    const raw = { id: 'client-capability' } as never;
    const capability = compileSchema(s.secret(z.string()));
    const client = def.client.upsert({});

    await expect(
      client.send.trigger({
        capability: raw,
        get later() {
          throw new Error('client record failed');
        },
      }),
    ).rejects.toThrow('client record failed');
    expect(capability.fromValue(capability.toValue(raw))).toBe(raw);
  });

  it('uses one logical client for ephemeral invocations and returns final identity metadata', async () => {
    const ephemeralDef = defineAgentClient({
      name: 'EphemeralClientTestAgent',
      mode: 'ephemeral',
      id: { name: z.string() },
      methods: { ping: method({ input: {}, returns: z.void() }) },
    });
    const client = ephemeralDef.client.upsert({ name: 'counter' });
    const rpc = vi.mocked(WasmRpc.upsert).mock.results.at(-1)!.value;
    const metadata = hostInvocationMetadata();
    let completeInvocation!: (value: ReturnType<typeof hostInvocationResult>) => void;
    rpc.invokeAndAwait.mockReturnValue(
      new Promise((resolve) => {
        completeInvocation = resolve;
      }),
    );

    const awaited = client.ping();
    await vi.waitFor(() => expect(rpc.invokeAndAwait).toHaveBeenCalled());
    completeInvocation({ metadata, result: undefined });
    await expect(awaited).resolves.toEqual({
      metadata: sdkInvocationMetadata(),
      value: undefined,
    });
    const finalAgentId = '00000007-0000-8000-8000-0000000000ff';
    rpc.trigger.mockResolvedValue(fakeHostInvocation(finalAgentId));
    const triggered = await client.ping.trigger();
    expect(triggered.id).toEqual(sdkInvocationMetadata(finalAgentId).invocationId);
    expect(triggered.id.toString()).toBe(`${finalAgentId}.${testInvocationUuid}`);
    expect('metadata' in triggered).toBe(false);
    rpc.schedule.mockResolvedValue(fakeHostInvocation(finalAgentId));
    const scheduled = await client.ping.schedule({ seconds: 1n, nanoseconds: 0 });
    expect(scheduled.id.agentId.toString()).toBe(finalAgentId);
  });

  it('rejects a missing wire value for a declared single output', async () => {
    const def = defineAgent({
      name: 'MissingSingleOutputAgent',
      id: { id: s.u64() },
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });
    const client = def.client.getById(remoteAgentId);
    const rpc = vi.mocked(WasmRpc.getById).mock.results.at(-1)!.value;
    rpc.invokeAndAwait.mockResolvedValue(hostInvocationResult());

    await expect(client.ping()).rejects.toThrow(
      `Remote agent MissingSingleOutputAgent[${remoteAgentId}].ping returned no value for a non-unit output`,
    );
  });

  it('rejects an unexpected wire value for a declared unit output', async () => {
    const def = defineAgentClient({
      name: 'UnitOutputAgent',
      id: {},
      methods: { ping: method({ input: {}, returns: z.void() }) },
    });
    const client = def.client.upsert({});
    const rpc = vi.mocked(WasmRpc.upsert).mock.results.at(-1)!.value;
    rpc.invokeAndAwait.mockResolvedValueOnce(
      hostInvocationResult(schemaValueToWit(v.string('unexpected'))),
    );
    await expect(client.ping()).rejects.toBeInstanceOf(RemoteOutputError);
  });

  it('rejects a wire value whose schema tag does not match the declared output', async () => {
    const def = defineAgent({
      name: 'MismatchedSingleOutputAgent',
      id: {},
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });
    const client = def.client.upsert({});
    const rpc = latestRpc();
    rpc.invokeAndAwait.mockResolvedValue(
      hostInvocationResult(schemaValueToWit(compileSchema(z.number()).toValue(42))),
    );

    await expect(client.ping()).rejects.toBeInstanceOf(RemoteOutputError);
  });

  it('preserves RemoteCallError when a remote custom error contains bigint values', async () => {
    const def = defineAgent({
      name: 'BigintRemoteErrorAgent',
      id: {},
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });
    const client = def.client.upsert({});
    const rpc = latestRpc();
    const errorCodec = compileSchema(s.u64());
    rpc.invokeAndAwait.mockRejectedValue({
      tag: 'remote-agent-error',
      val: {
        tag: 'custom-error',
        val: typedSchemaValueToWit({
          graph: errorCodec.graph,
          value: errorCodec.toValue(1n),
        }),
      },
    });

    try {
      await client.ping();
      throw new Error('expected the remote call to fail');
    } catch (error) {
      expect(isRemoteCallError(error)).toBe(true);
      if (!isRemoteCallError(error)) throw error;
      expect(error.cause).toMatchObject({
        tag: 'remote-agent-error',
        error: {
          tag: 'custom-error',
          value: { value: { tag: 'u64', value: 1n } },
        },
      });
    }
  });

  it('surfaces the error of a rejected call', async () => {
    const def = defineAgent({
      name: 'RejectedCallAgent',
      id: {},
      methods: { ping: method({ input: {}, returns: z.string() }) },
    });
    const client = def.client.upsert({});
    const rpc = latestRpc();
    rpc.invokeAndAwait.mockRejectedValue({ tag: 'denied', val: 'scope card rejected' });

    const error = await client.ping().then(
      () => undefined,
      (error: unknown) => error,
    );
    expect(isRemoteCallError(error)).toBe(true);
    expect(error).toMatchObject({ cause: { tag: 'denied', details: 'scope card rejected' } });
  });

  it('narrows RemoteCallError structurally across package entry identities', () => {
    expect(
      isRemoteCallError({
        _tag: 'RemoteCallError',
        message: 'Remote call failed',
        cause: { tag: 'denied', details: 'not allowed' },
      }),
    ).toBe(true);
    expect(isRemoteCallError(new Error('not remote'))).toBe(false);
  });

  it('sends every id field in RPC constructor input, a PrincipalRef included', () => {
    const def = defineAgent({
      name: 'PrincipalIdClientAgent',
      id: { tenant: z.string(), owner: s.principalRef() },
      methods: { ping: method({ input: {}, returns: z.void() }) },
    });
    const factory = def.client;
    const owner = new PrincipalRef('https://login.example', 'u-1');

    expect(() => factory.upsert({ tenant: 'acme', owner })).not.toThrow();
    const constructorTree = vi.mocked(WasmRpc.upsert).mock.calls.at(-1)![1];
    expect(schemaValueFromWit(constructorTree)).toEqual({
      tag: 'record',
      fields: [
        { tag: 'string', value: 'acme' },
        {
          tag: 'record',
          fields: [
            { tag: 'string', value: 'https://login.example' },
            { tag: 'string', value: 'u-1' },
          ],
        },
      ],
    });
  });

  it('treats an empty config object as containing no RPC overrides', async () => {
    const def = defineAgent({
      name: 'PrototypeNamedConfigClientAgent',
      id: {},
      config: { constructor: z.string(), toString: z.string() },
      methods: { ping: method({ input: {}, returns: z.void() }) },
    });

    await def.client.create({}, { config: {} });

    expect(vi.mocked(WasmRpc.create).mock.calls.at(-1)![2].agentConfig).toEqual([]);
  });

  it('rejects undeclared config override paths before creating the RPC', async () => {
    const def = defineAgentClient({
      name: 'StrictConfigClientAgent',
      id: {},
      config: {},
      methods: { ping: method({ input: {}, returns: z.void() }) },
    });
    const creates = vi.mocked(WasmRpc.create).mock.calls.length;
    await expect(def.client.create({}, { config: { extra: 1 } as never })).rejects.toThrow(
      "Unknown config path 'extra'",
    );
    expect(vi.mocked(WasmRpc.create).mock.calls).toHaveLength(creates);
  });

  it('rejects a non-object while traversing a nested RPC config override', async () => {
    const def = defineAgent({
      name: 'MalformedNestedConfigClientAgent',
      id: {},
      config: { nested: z.object({ retries: z.number() }) },
      methods: { ping: method({ input: {}, returns: z.void() }) },
    });

    await expect(def.client.create({}, { config: { nested: 'not-an-object' } })).rejects.toThrow();
  });

  it('accepts cancellation options on input and zero-input calls', async () => {
    const client = clientDef.client.upsert({ name: 'counter' });
    const rpc = latestRpc();
    const controller = new AbortController();
    controller.abort('cancelled');

    await expect(client.add({ by: 1 }, { signal: controller.signal })).rejects.toBe('cancelled');
    await expect(client.ping({ signal: controller.signal })).rejects.toBe('cancelled');
    expect(rpc.invokeAndAwait).not.toHaveBeenCalled();
  });
});

function thrownBy(operation: () => unknown): unknown {
  try {
    operation();
  } catch (error) {
    return error;
  }
  throw new Error('the operation did not throw');
}
