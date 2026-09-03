// Snapshot ergonomics: typed `state` schema (scoped + validated),
// config exclusion, and custom `save`/`restore` factories.

import { describe, expect, it, vi } from 'vitest';
import { z } from 'zod';
import { defineAgent } from '../src/defineAgent';
import { method } from '../src/method';
import { AgentInitiatorRegistry } from '../src/internal/registry/agentInitiatorRegistry';
import { schemaValueToWit, v } from '../src/internal/schema-model';
import type { Principal } from '../src/principal';

interface Resolved {
  saveSnapshot(): Promise<{ data: Uint8Array; mimeType: string }>;
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

async function restore(name: string, data: Uint8Array): Promise<Resolved> {
  const idValue = v.record([v.string('c1')]);
  (globalThis as any).currentAgentId = `${name}(${JSON.stringify(schemaValueToWit(idValue))})`;
  const initiator = AgentInitiatorRegistry.lookup(name);
  if (!initiator) throw new Error(`${name} not registered`);
  const res = await initiator.loadSnapshot(
    idValue as never,
    { tag: 'anonymous' },
    data,
    'application/json',
    [],
  );
  if (res.tag !== 'ok') throw new Error(`restore failed: ${JSON.stringify(res.val)}`);
  return res.val;
}

const jsonOf = (data: Uint8Array) => JSON.parse(new TextDecoder().decode(data));

let separationInitCount = 0;
let separationRestoreCount = 0;
let restoredPrincipal: Principal | undefined;
let restoredId: unknown;
let restoredAgentId: unknown;
let restoredConfig: unknown;

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
  init: () => ({ count: 7 }),
  methods: {
    inc() {
      this.count += 1;
      return this.count;
    },
  },
});

defineAgent({
  name: 'SnapUndeclaredState',
  id: { name: z.string() },
  snapshotting: { state: z.object({ count: z.number() }) },
  methods: {},
}).implement({
  init: () => ({ count: 7, scratch: 'undeclared' }),
  methods: {},
});

defineAgent({
  name: 'SnapFunctionState',
  id: { name: z.string() },
  snapshotting: { state: z.object({ callback: z.any() }) },
  methods: {},
}).implement({
  init: () => ({ callback: () => 'not restorable' }),
  methods: {},
});

defineAgent({
  name: 'SnapSeparatedFactories',
  id: { name: z.string() },
  snapshotting: 'default',
  config: { greeting: z.string() },
  methods: { get: method({ input: {}, returns: z.number() }) },
}).implement({
  init: () => {
    separationInitCount += 1;
    return { count: 1 };
  },
  methods: {
    get() {
      return this.count;
    },
  },
  snapshot: {
    save() {
      return new TextEncoder().encode(
        JSON.stringify({
          count: this.count,
          principal: this.getPrincipal().tag,
          agentId: this.getId().value,
        }),
      );
    },
    load(bytes, ctx) {
      separationRestoreCount += 1;
      restoredPrincipal = ctx.principal;
      restoredId = ctx.id;
      restoredAgentId = ctx.agentId;
      restoredConfig = ctx.config;
      return JSON.parse(new TextDecoder().decode(bytes));
    },
  },
});

// ── Schema-backed state with config: config must NOT be snapshotted ────────────
defineAgent({
  name: 'SnapReflConfig',
  id: { name: z.string() },
  snapshotting: { state: z.object({ count: z.number() }) },
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
    load(bytes, _ctx) {
      return { count: Number(new TextDecoder().decode(bytes).split('=')[1]) };
    },
  },
});

defineAgent({
  name: 'SnapCustomWithStateSchema',
  id: { name: z.string() },
  snapshotting: { state: z.object({ count: z.number() }) },
  methods: {},
}).implement({
  init: () => ({ count: 0, resource: { marker: 'fresh' } }),
  methods: {},
  snapshot: {
    save() {
      return new TextEncoder().encode(JSON.stringify(this));
    },
    load() {
      return { count: 11, resource: { marker: 'restored' } };
    },
  },
});

describe('snapshot — typed state', () => {
  it('serializes the declared state fields without config or helpers', async () => {
    const agent = await initiate('SnapTypedCounter');
    const snap = await agent.saveSnapshot();
    expect(snap.mimeType).toBe('application/json');
    expect(jsonOf(snap.data)).toEqual({ count: 7 });
  });

  it('rejects ordinary state fields missing from the declared snapshot schema', async () => {
    const agent = await initiate('SnapUndeclaredState');
    await expect(agent.saveSnapshot()).rejects.toContain('undeclared fields: scratch');
  });

  it('rejects user function state instead of silently omitting it', async () => {
    const agent = await initiate('SnapFunctionState');
    await expect(agent.saveSnapshot()).rejects.toContain(
      'Cannot automatically snapshot function field "callback"',
    );
  });

  it('round-trips through the schema on load', async () => {
    const agent = await restore(
      'SnapTypedCounter',
      new TextEncoder().encode(JSON.stringify({ count: 42 })),
    );
    expect(jsonOf((await agent.saveSnapshot()).data)).toEqual({ count: 42 });
  });

  it('rejects a snapshot that violates the declared schema', async () => {
    await expect(
      restore('SnapTypedCounter', new TextEncoder().encode(JSON.stringify({ count: 'nope' }))),
    ).rejects.toBeTruthy();
  });
});

describe('snapshot — schema-backed config', () => {
  it('does NOT serialize the live config accessor', async () => {
    const agent = await initiate('SnapReflConfig');
    const state = jsonOf((await agent.saveSnapshot()).data);
    expect(state).toEqual({ count: 3 });
    expect('config' in state).toBe(false);
  });
});

describe('snapshot — custom save/load', () => {
  it('uses the user bytes verbatim (octet-stream) and restores from them', async () => {
    const agent = await initiate('SnapCustom');
    const snap = await agent.saveSnapshot();
    expect(snap.mimeType).toBe('application/octet-stream');
    expect(new TextDecoder().decode(snap.data)).toBe('count=5');

    const restored = await restore('SnapCustom', new TextEncoder().encode('count=99'));
    expect(new TextDecoder().decode((await restored.saveSnapshot()).data)).toBe('count=99');
  });

  it('keeps the complete custom-restored state when a state schema is also declared', async () => {
    const restored = await restore('SnapCustomWithStateSchema', new Uint8Array());
    expect(jsonOf((await restored.saveSnapshot()).data)).toMatchObject({
      count: 11,
      resource: { marker: 'restored' },
    });
  });

  it('uses restoration as an alternative factory and supplies identity and principal', async () => {
    separationInitCount = 0;
    separationRestoreCount = 0;
    const restored = await restore(
      'SnapSeparatedFactories',
      new TextEncoder().encode('{"count":12}'),
    );

    expect(separationInitCount).toBe(0);
    expect(separationRestoreCount).toBe(1);
    expect(restoredPrincipal).toEqual({ tag: 'anonymous' });
    expect(restoredId).toEqual({ name: 'c1' });
    expect((restoredAgentId as { value: string }).value).toContain('SnapSeparatedFactories(');
    expect(Object.getOwnPropertyDescriptor(restoredConfig, 'greeting')?.get).toBeTypeOf('function');
    const saved = jsonOf((await restored.saveSnapshot()).data);
    expect(saved).toMatchObject({ count: 12, principal: 'anonymous' });
    expect(saved.agentId).toContain('SnapSeparatedFactories(');

    await initiate('SnapSeparatedFactories');
    expect(separationInitCount).toBe(1);
    expect(separationRestoreCount).toBe(1);
  });
});

describe('snapshot — multipart databases', () => {
  it('attaches restored databases to fresh schema-backed state before installation', async () => {
    vi.resetModules();

    const warmDatabase = vi.fn();
    const prepare = vi.fn(() => ({ get: warmDatabase }));
    class FakeDatabaseSync {
      restored = new Uint8Array();
      inTransaction = false;
      prepare = prepare;
      constructor(_path: string) {}
    }
    class FakeStatementSync {}
    class FakeSession {}
    class FakeSqlTagStore {}
    const restoreDatabaseSync = vi.fn((db: FakeDatabaseSync, bytes: Uint8Array) => {
      db.restored = bytes.slice();
    });
    vi.doMock('../src/internal/sqlite', () => ({
      DatabaseSync: FakeDatabaseSync,
      StatementSync: FakeStatementSync,
      Session: FakeSession,
      SQLTagStore: FakeSqlTagStore,
      serializeDatabaseSync: (db: FakeDatabaseSync) => db.restored.slice(),
      restoreDatabaseSync,
      isAutocommitDatabaseSync: (db: FakeDatabaseSync) => !db.inTransaction,
    }));

    try {
      const [
        { defineAgent: isolatedDefineAgent },
        { method: isolatedMethod },
        isolatedGuest,
        { encodeMultipart, decodeMultipart },
        { AgentInitiatorRegistry: isolatedInitiators },
      ] = await Promise.all([
        import('../src/defineAgent'),
        import('../src/method'),
        import('../src'),
        import('../src/internal/multipart'),
        import('../src/internal/registry/agentInitiatorRegistry'),
      ]);

      let initializeCalls = 0;
      isolatedDefineAgent({
        name: 'NestedDatabaseState',
        id: { name: z.string() },
        snapshotting: { state: z.object({ nested: z.any() }) },
        methods: {},
      }).implement({
        init: () => ({ nested: { database: new FakeDatabaseSync(':memory:') } }),
        methods: {},
      });
      isolatedDefineAgent({
        name: 'OpenDatabaseState',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: {},
      }).implement({
        init: () => {
          const database = new FakeDatabaseSync(':memory:');
          database.inTransaction = true;
          return { count: 1, database };
        },
        methods: {},
      });
      isolatedDefineAgent({
        name: 'MultipartRestore',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: { get: isolatedMethod({ input: {}, returns: z.number() }) },
      }).implement({
        init: () => {
          initializeCalls += 1;
          return { count: 0 };
        },
        methods: {
          get() {
            return this.count;
          },
        },
      });

      const idValue = v.record([v.string('database')]);
      const initiateIsolated = async (name: string) => {
        (globalThis as { currentAgentId?: string }).currentAgentId =
          `${name}(${JSON.stringify(schemaValueToWit(idValue))})`;
        const result = await isolatedInitiators
          .lookup(name)!
          .initiate(idValue as never, { tag: 'anonymous' });
        if (result.tag === 'err') throw result.val;
        return result.val;
      };

      const nestedDatabase = await initiateIsolated('NestedDatabaseState');
      await expect(nestedDatabase.saveSnapshot()).rejects.toContain(
        'Cannot automatically snapshot nested resource field "nested.database"',
      );
      const openDatabase = await initiateIsolated('OpenDatabaseState');
      await expect(openDatabase.saveSnapshot()).rejects.toContain('an open transaction exists');

      (globalThis as { currentAgentId?: string }).currentAgentId =
        `MultipartRestore(${JSON.stringify(schemaValueToWit(idValue))})`;
      const databaseBytes = new Uint8Array([4, 5, 6]);
      const encoded = encodeMultipart([
        {
          name: 'state',
          contentType: 'application/json',
          body: new TextEncoder().encode(
            JSON.stringify({
              version: 1,
              principal: { tag: 'anonymous' },
              state: { count: 17 },
            }),
          ),
        },
        {
          name: 'db:database',
          contentType: 'application/x-sqlite3',
          body: databaseBytes,
        },
      ]);

      await isolatedGuest.loadSnapshot.load({
        payload: encoded.data,
        mimeType: `multipart/mixed; boundary=${encoded.boundary}`,
      });
      expect(initializeCalls).toBe(0);
      expect(restoreDatabaseSync).toHaveBeenCalledTimes(1);
      expect(restoreDatabaseSync.mock.calls[0][1]).toEqual(databaseBytes);
      expect(prepare).toHaveBeenCalledWith('SELECT count(*) FROM sqlite_master');
      expect(warmDatabase).toHaveBeenCalledTimes(1);

      const saved = await isolatedGuest.saveSnapshot.save();
      const boundary = saved.mimeType.match(/boundary=([^\s;]+)/)?.[1];
      expect(boundary).toBeDefined();
      const parts = decodeMultipart(saved.payload, boundary!);
      const statePart = parts.find((part) => part.name === 'state');
      const databasePart = parts.find((part) => part.name === 'db:database');
      expect(JSON.parse(new TextDecoder().decode(statePart!.body)).state).toEqual({ count: 17 });
      expect(databasePart!.body).toEqual(databaseBytes);
    } finally {
      vi.doUnmock('../src/internal/sqlite');
      vi.resetModules();
    }
  });
});
