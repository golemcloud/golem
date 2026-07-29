// Item 8 — fluent snapshot ergonomics: typed `state` schema (scoped + validated),
// the config-leak fix in the reflective default, and custom `save`/`load`.

import { describe, expect, it } from 'vitest';
import { z } from 'zod';
import { defineAgent } from '../src/fluent/defineAgent';
import { method } from '../src/fluent/method';
import { AgentInitiatorRegistry } from '../src/internal/registry/agentInitiatorRegistry';
import { schemaValueToWit, v } from '../src/internal/schema-model';

interface Resolved {
  saveSnapshot(): Promise<{ data: Uint8Array; mimeType: string }>;
  loadSnapshot(bytes: Uint8Array, mimeType?: string): Promise<void>;
}

async function initiate(name: string): Promise<Resolved> {
  // id is `{ name: z.string() }` → a one-field record.
  const idValue = v.record([v.string('c1')]);
  // The self-agent-id embeds a JSON WIT value tree (mirrors the mocked makeAgentId).
  (globalThis as any).currentAgentId = `${name}(${JSON.stringify(schemaValueToWit(idValue))})`;
  const initiator = AgentInitiatorRegistry.lookup(name);
  if (!initiator) throw new Error(`${name} not registered`);
  const res = await initiator.initiate(idValue as never, { tag: 'anonymous' });
  if (res.tag !== 'ok') throw new Error(`initiate failed: ${JSON.stringify(res.val)}`);
  return res.val as unknown as Resolved;
}

const jsonOf = (data: Uint8Array) => JSON.parse(new TextDecoder().decode(data));

function snapshotStateTypeChecks(): void {
  defineAgent({
    name: 'SnapshotTypeCompatible',
    id: {},
    snapshotting: { state: z.object({ count: z.number() }) },
    methods: {},
  }).implement({
    init: () => ({ count: 0, transient: true }),
    methods: {},
  });

  defineAgent({
    name: 'SnapshotTypeMissingField',
    id: {},
    snapshotting: { state: z.object({ count: z.number() }) },
    methods: {},
  }).implement({
    // @ts-expect-error snapshot state schema requires a numeric `count` field
    init: () => ({}),
    methods: {},
  });

  defineAgent({
    name: 'SnapshotTypeWrongField',
    id: {},
    snapshotting: { state: z.object({ count: z.number() }) },
    methods: {},
  }).implement({
    // @ts-expect-error snapshot state schema requires `count` to be a number
    init: () => ({ count: 'zero' }),
    methods: {},
  });

  defineAgent({
    name: 'SnapshotBarePolicyUnconstrained',
    id: {},
    snapshotting: 'default',
    methods: {},
  }).implement({
    init: () => ({ arbitrary: true }),
    methods: {},
  });
}
void snapshotStateTypeChecks;

// ── Typed state: only the schema fields are persisted ──────────────────────────
defineAgent({
  name: 'SnapTypedCounter',
  id: { name: z.string() },
  snapshotting: { state: z.object({ count: z.number() }), policy: { everyNInvocations: 5 } },
  methods: { inc: method({ input: {}, returns: z.number() }) },
}).implement({
  init: () => ({ count: 7, scratch: 'not-persisted' }),
  methods: {
    inc() {
      this.count += 1;
      return this.count;
    },
  },
});

// ── Reflective default with config: config must NOT be snapshotted ─────────────
defineAgent({
  name: 'SnapReflConfig',
  id: { name: z.string() },
  snapshotting: 'default',
  config: { greeting: z.string() },
  methods: { get: method({ input: {}, returns: z.number() }) },
}).implement({
  init: () => ({ count: 3 }),
  methods: {
    get() {
      return this.count;
    },
  },
});

// ── Custom save/load: user owns the bytes ──────────────────────────────────────
defineAgent({
  name: 'SnapCustom',
  id: { name: z.string() },
  snapshotting: 'default',
  methods: { get: method({ input: {}, returns: z.number() }) },
}).implement({
  init: () => ({ count: 5 }),
  methods: {
    get() {
      return this.count;
    },
  },
  snapshot: {
    save() {
      return new TextEncoder().encode(`count=${this.count}`);
    },
    load(bytes) {
      this.count = Number(new TextDecoder().decode(bytes).split('=')[1]);
    },
  },
});

describe('fluent snapshot — typed state', () => {
  it('serializes ONLY the declared state fields (drops scratch, config, helpers)', async () => {
    const agent = await initiate('SnapTypedCounter');
    const snap = await agent.saveSnapshot();
    expect(snap.mimeType).toBe('application/json');
    expect(jsonOf(snap.data)).toEqual({ count: 7 });
  });

  it('round-trips through the schema on load', async () => {
    const agent = await initiate('SnapTypedCounter');
    await agent.loadSnapshot(
      new TextEncoder().encode(JSON.stringify({ count: 42 })),
      'application/json',
    );
    expect(jsonOf((await agent.saveSnapshot()).data)).toEqual({ count: 42 });
  });

  it('rejects a snapshot that violates the declared schema', async () => {
    const agent = await initiate('SnapTypedCounter');
    await expect(
      agent.loadSnapshot(
        new TextEncoder().encode(JSON.stringify({ count: 'nope' })),
        'application/json',
      ),
    ).rejects.toBeTruthy();
  });
});

describe('fluent snapshot — reflective default', () => {
  it('does NOT serialize the live config accessor', async () => {
    const agent = await initiate('SnapReflConfig');
    const state = jsonOf((await agent.saveSnapshot()).data);
    expect(state).toEqual({ count: 3 });
    expect('config' in state).toBe(false);
  });
});

describe('fluent snapshot — custom save/load', () => {
  it('uses the user bytes verbatim (octet-stream) and restores from them', async () => {
    const agent = await initiate('SnapCustom');
    const snap = await agent.saveSnapshot();
    expect(snap.mimeType).toBe('application/octet-stream');
    expect(new TextDecoder().decode(snap.data)).toBe('count=5');

    await agent.loadSnapshot(new TextEncoder().encode('count=99'));
    expect(new TextDecoder().decode((await agent.saveSnapshot()).data)).toBe('count=99');
  });
});
