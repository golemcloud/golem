// Snapshot ergonomics: typed `state` schema (scoped + validated),
// config exclusion, and custom `save`/`restore` factories.

import { afterEach, describe, expect, it, vi } from 'vitest';
import { z } from 'zod';
import { defineAgent } from '../src/defineAgent';
import { method } from '../src/method';
import { AgentInitiatorRegistry } from '../src/internal/registry/agentInitiatorRegistry';
import { schemaValueToWit, v } from '../src/internal/schema-model';
import type { Principal } from '../src/principal';
import type { SavedAgentSnapshot } from '../src/internal/resolvedAgent';
import type { SnapshotDatabases } from '../src/internal/databaseSnapshot';

interface Resolved {
  saveSnapshot(): Promise<SavedAgentSnapshot>;
}

function typed(saved: SavedAgentSnapshot): Extract<SavedAgentSnapshot, { kind: 'typed' }> {
  if (saved.kind !== 'typed') {
    throw new Error(`expected a typed snapshot, got a ${saved.kind} one`);
  }
  return saved;
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
    { inMemory: [], fileDatabases: {} },
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

  defineAgent({
    name: 'SnapshotBarePolicyRequiresSave',
    id: {},
    snapshotting: 'default',
    methods: {},
  }).implement({
    init: () => ({}),
    methods: {},
    // @ts-expect-error snapshotting without a state schema requires custom save and load
    snapshot: { load: () => ({}) },
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

defineAgent({
  name: 'SnapTypedSaveCustomLoad',
  id: { name: z.string() },
  snapshotting: { state: z.object({ count: z.number() }) },
  methods: {},
}).implement({
  init: () => ({ count: 4 }),
  methods: {},
  snapshot: {
    load(bytes) {
      return {
        count: JSON.parse(new TextDecoder().decode(bytes)).count,
      };
    },
  },
});

describe('snapshot — typed state', () => {
  it('serializes the declared state fields without config or helpers', async () => {
    const agent = await initiate('SnapTypedCounter');
    const snap = typed(await agent.saveSnapshot());
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
  it('uses typed saving with a custom load-only restoration factory', async () => {
    const initial = await initiate('SnapTypedSaveCustomLoad');
    const snapshot = typed(await initial.saveSnapshot());
    expect(snapshot.mimeType).toBe('application/json');
    expect(jsonOf(snapshot.data)).toEqual({ count: 4 });

    const restored = await restore('SnapTypedSaveCustomLoad', snapshot.data);
    expect(jsonOf((await restored.saveSnapshot()).data)).toEqual({ count: 4 });
  });

  it('returns the bytes of a custom save as a custom result and restores from them', async () => {
    const agent = await initiate('SnapCustom');
    const snap = await agent.saveSnapshot();
    expect(snap.kind).toBe('custom');
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

type Constructed = { path: string; options: unknown };

/**
 * Loads a fresh copy of the SDK with fakes of the `node:sqlite` builtin and of `existsSync`,
 * which record the databases that the SDK opens, warms, serializes and restores.
 */
async function isolate() {
  vi.resetModules();
  const constructed: Constructed[] = [];
  const warmed: string[] = [];
  class FakeDatabaseSync {
    bytes = new Uint8Array();
    inTransaction = false;
    isOpen = true;
    pragmas: Record<string, number> = { page_count: 1, page_size: 4096, cache_size: -2000 };
    returnArrays = false;
    constructor(
      readonly path: string,
      options?: unknown,
    ) {
      constructed.push({ path, options });
    }
    location() {
      return this.path === ':memory:' || this.path === '' ? null : this.path;
    }
    prepare(sql: string) {
      const pragma = sql.match(/^PRAGMA (\w+)$/)?.[1];
      return {
        get: () => {
          if (pragma !== undefined) {
            return this.returnArrays ? [this.pragmas[pragma]] : { [pragma]: this.pragmas[pragma] };
          }
          warmed.push(`${this.path}: ${sql}`);
          return undefined;
        },
      };
    }
  }
  class FakeStatementSync {}
  class FakeSession {}
  class FakeSqlTagStore {}
  const serializeDatabaseSync = vi.fn((db: FakeDatabaseSync) => db.bytes.slice());
  const restoreDatabaseSync = vi.fn((db: FakeDatabaseSync, bytes: Uint8Array) => {
    db.bytes = bytes.slice();
  });
  vi.doMock('../src/internal/sqlite', () => ({
    DatabaseSync: FakeDatabaseSync,
    StatementSync: FakeStatementSync,
    Session: FakeSession,
    SQLTagStore: FakeSqlTagStore,
    serializeDatabaseSync,
    restoreDatabaseSync,
    isAutocommitDatabaseSync: (db: FakeDatabaseSync) => {
      if (!db.isOpen) throw new Error('database is not open');
      return !db.inTransaction;
    },
  }));
  const existingFiles = new Set(['/data/app.db', '/data/other.db']);
  const existsSync = vi.fn((path: string) => existingFiles.has(path));
  vi.doMock('node:fs', async (importOriginal) => ({
    ...(await importOriginal<typeof import('node:fs')>()),
    existsSync,
  }));
  const [
    { defineAgent: isolatedDefineAgent },
    { method: isolatedMethod },
    isolatedGuest,
    multipart,
    { AgentInitiatorRegistry: isolatedInitiators },
    { REOPENED_DATABASE_OPTIONS },
  ] = await Promise.all([
    import('../src/defineAgent'),
    import('../src/method'),
    import('../src'),
    import('../src/internal/multipart'),
    import('../src/internal/registry/agentInitiatorRegistry'),
    import('../src/internal/databaseSnapshot'),
  ]);
  const idValue = v.record([v.string('database')]);
  const select = (name: string) => {
    (globalThis as { currentAgentId?: string }).currentAgentId =
      `${name}(${JSON.stringify(schemaValueToWit(idValue))})`;
  };
  const initiateIsolated = async (name: string) => {
    select(name);
    const result = await isolatedInitiators
      .lookup(name)!
      .initiate(idValue as never, { tag: 'anonymous' });
    if (result.tag === 'err') throw result.val;
    return result.val as unknown as {
      saveSnapshot(): Promise<SavedAgentSnapshot>;
    };
  };
  const boundaryOf = (mimeType: string) => mimeType.match(/boundary=([^\s;]+)/)![1];
  return {
    FakeDatabaseSync,
    FakeStatementSync,
    existingFiles,
    existsSync,
    constructed,
    warmed,
    serializeDatabaseSync,
    restoreDatabaseSync,
    isolatedDefineAgent,
    isolatedMethod,
    isolatedGuest,
    multipart,
    REOPENED_DATABASE_OPTIONS,
    select,
    initiateIsolated,
    boundaryOf,
  };
}

function unmockSqlite() {
  vi.doUnmock('../src/internal/sqlite');
  vi.doUnmock('node:fs');
  vi.resetModules();
}

describe('snapshot — multipart databases', () => {
  afterEach(unmockSqlite);

  it('attaches restored databases to fresh schema-backed state before installation', async () => {
    const env = await isolate();
    let initializeCalls = 0;
    env
      .isolatedDefineAgent({
        name: 'NestedDatabaseState',
        id: { name: z.string() },
        snapshotting: { state: z.object({ nested: z.any() }) },
        methods: {},
      })
      .implement({
        init: () => ({ nested: { database: new env.FakeDatabaseSync(':memory:') } }),
        methods: {},
      });
    env
      .isolatedDefineAgent({
        name: 'OpenDatabaseState',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: {},
      })
      .implement({
        init: () => {
          const database = new env.FakeDatabaseSync(':memory:');
          database.inTransaction = true;
          return { count: 1, database };
        },
        methods: {},
      });
    env
      .isolatedDefineAgent({
        name: 'MultipartRestore',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: { get: env.isolatedMethod({ input: {}, returns: z.number() }) },
      })
      .implement({
        init: () => {
          initializeCalls += 1;
          return { count: 0 };
        },
        methods: {
          get() {
            return this.count;
          },
        },
        snapshot: {
          load(bytes) {
            const { count } = JSON.parse(new TextDecoder().decode(bytes));
            return { count, database: new env.FakeDatabaseSync(':memory:') };
          },
        },
      });

    const nestedDatabase = await env.initiateIsolated('NestedDatabaseState');
    await expect(nestedDatabase.saveSnapshot()).rejects.toContain(
      'Cannot automatically snapshot nested resource field "nested.database"',
    );
    const openDatabase = await env.initiateIsolated('OpenDatabaseState');
    await expect(openDatabase.saveSnapshot()).rejects.toContain('an open transaction exists');

    env.select('MultipartRestore');
    const databaseBytes = new Uint8Array([4, 5, 6]);
    const encoded = env.multipart.encodeMultipart([
      {
        name: 'state',
        contentType: 'application/json',
        body: new TextEncoder().encode(
          JSON.stringify({
            version: 1,
            principal: { tag: 'anonymous' },
            state: { count: 17 },
            fileDatabases: {},
          }),
        ),
      },
      {
        name: 'db:database',
        contentType: 'application/x-sqlite3',
        body: databaseBytes,
      },
    ]);

    await env.isolatedGuest.loadSnapshot.load({
      payload: encoded.data,
      mimeType: `multipart/mixed; boundary=${encoded.boundary}`,
    });
    expect(initializeCalls).toBe(0);
    expect(env.restoreDatabaseSync).toHaveBeenCalledTimes(1);
    expect(env.restoreDatabaseSync.mock.calls[0][1]).toEqual(databaseBytes);
    expect(env.warmed).toEqual([':memory:: SELECT count(*) FROM sqlite_master']);

    const saved = await env.isolatedGuest.saveSnapshot.save();
    const parts = env.multipart.decodeMultipart(saved.payload, env.boundaryOf(saved.mimeType));
    const statePart = parts.find((part) => part.name === 'state');
    const databasePart = parts.find((part) => part.name === 'db:database');
    expect(JSON.parse(new TextDecoder().decode(statePart!.body)).state).toEqual({ count: 17 });
    expect(databasePart!.body).toEqual(databaseBytes);
  });
});

describe('snapshot — database plan', () => {
  it('records a field named __proto__ as an own field', async () => {
    const { planDatabases, takeDatabases } = await import('../src/internal/databaseSnapshot');
    const plan = planDatabases([{ name: '__proto__', autocommit: true, location: '/data/app.db' }]);
    if (plan.tag !== 'ok') throw new Error(plan.val);
    expect(Object.hasOwn(plan.val.fileDatabases, '__proto__')).toBe(true);
    expect(JSON.parse(JSON.stringify(plan.val.fileDatabases))).toEqual(
      JSON.parse('{"__proto__":"/data/app.db"}'),
    );

    const { ordinary } = takeDatabases([['__proto__', 5]]);
    expect(Object.hasOwn(ordinary, '__proto__')).toBe(true);
    expect(JSON.stringify(ordinary)).toBe('{"__proto__":5}');
  });

  it('serializes databases without a location and records the location of the others', async () => {
    const { planDatabases } = await import('../src/internal/databaseSnapshot');
    expect(
      planDatabases([
        { name: 'memory', autocommit: true, location: null },
        { name: 'file', autocommit: true, location: '/data/app.db' },
      ]),
    ).toEqual({
      tag: 'ok',
      val: { inMemory: ['memory'], fileDatabases: { file: '/data/app.db' } },
    });
    expect(planDatabases([])).toEqual({ tag: 'ok', val: { inMemory: [], fileDatabases: {} } });
  });

  it('fails on an in-memory field name that a multipart part name cannot hold', async () => {
    const { planDatabases } = await import('../src/internal/databaseSnapshot');
    for (const name of ['a"b', 'a\rb', 'a\nb', 'db\uD800x', 'db\uDC00x', 'db\uD800']) {
      const plan = planDatabases([{ name, autocommit: true, location: null }]);
      expect(plan).toEqual({
        tag: 'err',
        val: expect.stringContaining(
          `Cannot snapshot in-memory database ${JSON.stringify(name)}: its field name contains`,
        ),
      });
    }
  });

  it('rejects a snapshot that names a field both in memory and in fileDatabases', async () => {
    const { decodeSnapshotDatabases } = await import('../src/internal/databaseSnapshot');
    const parts = [
      { name: 'state', contentType: 'application/json', body: new Uint8Array() },
      { name: 'db:db', contentType: 'application/x-sqlite3', body: new Uint8Array([1]) },
    ];
    expect(() =>
      decodeSnapshotDatabases(
        parts,
        { fileDatabases: { db: '/data/app.db' } },
        'multipart state part',
      ),
    ).toThrow(
      `multipart state part names database field "db" both in memory and in 'fileDatabases'`,
    );
    expect(
      decodeSnapshotDatabases(parts, { fileDatabases: { other: '/data/app.db' } }, 'x'),
    ).toEqual({
      inMemory: [{ name: 'db', bytes: new Uint8Array([1]) }],
      fileDatabases: { other: '/data/app.db' },
    });
  });

  it('keeps an in-memory field name with a surrogate pair', async () => {
    const { planDatabases } = await import('../src/internal/databaseSnapshot');
    expect(planDatabases([{ name: 'db\uD83D\uDE00', autocommit: true, location: null }])).toEqual({
      tag: 'ok',
      val: { inMemory: ['db\uD83D\uDE00'], fileDatabases: {} },
    });
  });

  it('keeps a file-backed field name that a multipart part name cannot hold, as a JSON key', async () => {
    const { planDatabases, decodeSnapshotDatabases } =
      await import('../src/internal/databaseSnapshot');
    const name = 'a"b\r\nc';
    const plan = planDatabases([{ name, autocommit: true, location: '/data/app.db' }]);
    if (plan.tag !== 'ok') throw new Error(plan.val);
    const envelope = JSON.parse(JSON.stringify({ fileDatabases: plan.val.fileDatabases }));
    expect(decodeSnapshotDatabases([], envelope, 'JSON snapshot').fileDatabases).toEqual({
      [name]: '/data/app.db',
    });
  });

  it('fails on an open transaction of any database and names the field', async () => {
    const { planDatabases } = await import('../src/internal/databaseSnapshot');
    for (const location of [null, '/data/app.db']) {
      const plan = planDatabases([
        { name: 'first', autocommit: true, location: null },
        { name: 'busy', autocommit: false, location },
      ]);
      expect(plan.tag).toBe('err');
      expect(plan.val).toContain('Cannot snapshot database "busy": an open transaction exists');
    }
  });
});

describe('snapshot — restore plan', () => {
  it('reads every page only of a database that fits in its page cache', async () => {
    const { fitsInPageCache } = await import('../src/internal/databaseSnapshot');
    // The default cache_size of -2000 is 2000 KiB. With 88 bytes of extra space per page, that
    // is a limit of floor(2048000 / 4184) = 489 pages of 4096 bytes, and SQLite keeps at most 488.
    expect(fitsInPageCache({ pageCount: 488, pageSize: 4096, cacheSize: -2000 })).toBe(true);
    expect(fitsInPageCache({ pageCount: 489, pageSize: 4096, cacheSize: -2000 })).toBe(false);
    expect(fitsInPageCache({ pageCount: 500, pageSize: 4096, cacheSize: -2000 })).toBe(false);
    // 1024-byte pages: floor(2048000 / 1112) = 1841 pages, so at most 1840.
    expect(fitsInPageCache({ pageCount: 1840, pageSize: 1024, cacheSize: -2000 })).toBe(true);
    expect(fitsInPageCache({ pageCount: 1841, pageSize: 1024, cacheSize: -2000 })).toBe(false);
    // A positive cache_size is a limit in pages, so at most that many pages minus one stay.
    expect(fitsInPageCache({ pageCount: 99, pageSize: 4096, cacheSize: 100 })).toBe(true);
    expect(fitsInPageCache({ pageCount: 100, pageSize: 4096, cacheSize: 100 })).toBe(false);
    expect(fitsInPageCache({ pageCount: 1, pageSize: 4096, cacheSize: 0 })).toBe(false);
    expect(fitsInPageCache({ pageCount: NaN, pageSize: 4096, cacheSize: -2000 })).toBe(false);
  });

  const bytes = new Uint8Array([1, 2, 3]);

  it('opens a new database for each empty field and warms every new database once', async () => {
    const { planRestore } = await import('../src/internal/databaseSnapshot');
    expect(
      planRestore([['count', { kind: 'other' }]], {
        inMemory: [{ name: 'memDb', bytes }],
        fileDatabases: { fileDb: '/data/app.db' },
      }),
    ).toEqual({
      tag: 'ok',
      val: {
        open: [
          { name: 'memDb', location: null },
          { name: 'fileDb', location: '/data/app.db' },
        ],
        warm: [
          { name: 'memDb', allPages: false },
          { name: 'fileDb', allPages: true },
        ],
      },
    });
  });

  it('keeps the databases that the state holds and warms each open one once', async () => {
    const { planRestore } = await import('../src/internal/databaseSnapshot');
    expect(
      planRestore(
        [
          ['memDb', { kind: 'database', instance: 0, open: true, location: null }],
          ['fileDb', { kind: 'database', instance: 1, open: true, location: '/data/app.db' }],
          ['alias', { kind: 'database', instance: 1, open: true, location: '/data/app.db' }],
          ['closedDb', { kind: 'database', instance: 2, open: false, location: null }],
          ['otherDb', { kind: 'database', instance: 3, open: true, location: '/data/other.db' }],
        ],
        { inMemory: [{ name: 'memDb', bytes }], fileDatabases: { fileDb: '/data/app.db' } },
      ),
    ).toEqual({
      tag: 'ok',
      val: {
        open: [],
        warm: [
          { name: 'memDb', allPages: false },
          { name: 'fileDb', allPages: true },
          { name: 'otherDb', allPages: true },
        ],
      },
    });
  });

  it('fails when a file that the load must open does not exist', async () => {
    const { missingDatabaseFile } = await import('../src/internal/databaseSnapshot');
    const plan = {
      open: [
        { name: 'memDb', location: null },
        { name: 'fileDb', location: '/data/app.db' },
        { name: 'otherDb', location: '/data/other.db' },
      ],
      warm: [],
    };
    expect(missingDatabaseFile(plan, new Set(['/data/app.db', '/data/other.db']))).toBeNull();
    expect(missingDatabaseFile(plan, new Set(['/data/app.db']))).toBe(
      'snapshot database field "otherDb": no database file at /data/other.db',
    );
    expect(missingDatabaseFile(plan, new Set())).toBe(
      'snapshot database field "fileDb": no database file at /data/app.db',
    );
  });

  it('fails when a recorded field holds a value that is not a database', async () => {
    const { planRestore } = await import('../src/internal/databaseSnapshot');
    const cases: SnapshotDatabases[] = [
      { inMemory: [{ name: 'db', bytes }], fileDatabases: {} },
      { inMemory: [], fileDatabases: { db: '/data/app.db' } },
    ];
    for (const databases of cases) {
      expect(planRestore([['db', { kind: 'other' }]], databases)).toEqual({
        tag: 'err',
        val: 'snapshot database field "db" is not a DatabaseSync',
      });
    }
  });
});

describe('snapshot — in-memory and file-backed databases', () => {
  afterEach(unmockSqlite);

  it('serializes an in-memory database as a db part and records a file-backed database by its location', async () => {
    const env = await isolate();
    env
      .isolatedDefineAgent({
        name: 'MixedDatabases',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: {},
      })
      .implement({
        init: () => {
          const memDb = new env.FakeDatabaseSync(':memory:');
          memDb.bytes = new Uint8Array([1, 2, 3]);
          return { count: 1, memDb, fileDb: new env.FakeDatabaseSync('/data/app.db') };
        },
        methods: {},
      });

    const saved = typed(await (await env.initiateIsolated('MixedDatabases')).saveSnapshot());

    expect(saved.mimeType).toMatch(/^multipart\/mixed; boundary=/);
    const parts = env.multipart.decodeMultipart(saved.data, env.boundaryOf(saved.mimeType));
    expect(parts.map((part) => part.name)).toEqual(['state', 'db:memDb']);
    expect(parts[1].body).toEqual(new Uint8Array([1, 2, 3]));
    expect(saved.fileDatabases).toEqual({ fileDb: '/data/app.db' });
    expect(env.serializeDatabaseSync).toHaveBeenCalledTimes(1);
  });

  it('saves application/json when only file-backed databases are present', async () => {
    const env = await isolate();
    env
      .isolatedDefineAgent({
        name: 'FileDatabaseOnly',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: {},
      })
      .implement({
        init: () => ({ count: 2, fileDb: new env.FakeDatabaseSync('/data/app.db') }),
        methods: {},
      });

    const saved = typed(await (await env.initiateIsolated('FileDatabaseOnly')).saveSnapshot());

    expect(saved.mimeType).toBe('application/json');
    expect(jsonOf(saved.data)).toEqual({ count: 2 });
    expect(saved.fileDatabases).toEqual({ fileDb: '/data/app.db' });
    expect(env.serializeDatabaseSync).not.toHaveBeenCalled();
  });

  it('serializes a temporary database whose location is null', async () => {
    const env = await isolate();
    env
      .isolatedDefineAgent({
        name: 'TemporaryDatabase',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: {},
      })
      .implement({
        init: () => ({ count: 3, tempDb: new env.FakeDatabaseSync('') }),
        methods: {},
      });

    const saved = typed(await (await env.initiateIsolated('TemporaryDatabase')).saveSnapshot());

    const parts = env.multipart.decodeMultipart(saved.data, env.boundaryOf(saved.mimeType));
    expect(parts.map((part) => part.name)).toEqual(['state', 'db:tempDb']);
    expect(saved.fileDatabases).toEqual({});
  });

  it.each([
    ['file-backed', '/data/app.db'],
    ['in-memory', ':memory:'],
  ])('round-trips a database in a field named __proto__ (%s)', async (_description, location) => {
    const env = await isolate();
    env
      .isolatedDefineAgent({
        name: 'ProtoNamedDatabase',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: {},
      })
      .implement({
        init: () => {
          const db = new env.FakeDatabaseSync(location);
          db.bytes = new Uint8Array([4, 2]);
          return { count: 1, ['__proto__']: db };
        },
        methods: {},
      });

    const first = await env.initiateIsolated('ProtoNamedDatabase');
    const saved = typed(await first.saveSnapshot());
    const fileDatabases = location === ':memory:' ? {} : JSON.parse(`{"__proto__":"${location}"}`);
    expect(JSON.stringify(saved.fileDatabases)).toBe(JSON.stringify(fileDatabases));
    const parts =
      saved.mimeType === 'application/json'
        ? []
        : env.multipart.decodeMultipart(saved.data, env.boundaryOf(saved.mimeType));
    expect(parts.map((part) => part.name)).toEqual(
      location === ':memory:' ? ['state', 'db:__proto__'] : [],
    );

    const envelope = JSON.stringify({
      version: 1,
      principal: { tag: 'anonymous' },
      state: { count: 1 },
      fileDatabases: saved.fileDatabases,
    });
    env.select('ProtoNamedDatabase');
    await env.isolatedGuest.loadSnapshot.load(
      location === ':memory:'
        ? (() => {
            const encoded = env.multipart.encodeMultipart([
              {
                name: 'state',
                contentType: 'application/json',
                body: new TextEncoder().encode(envelope),
              },
              parts[1],
            ]);
            return {
              payload: encoded.data,
              mimeType: `multipart/mixed; boundary=${encoded.boundary}`,
            };
          })()
        : { payload: new TextEncoder().encode(envelope), mimeType: 'application/json' },
    );
    expect(env.constructed.at(-1)?.path).toBe(location);

    const resaved = await env.isolatedGuest.saveSnapshot.save();
    const resavedEnvelope =
      location === ':memory:'
        ? new TextDecoder().decode(
            env.multipart.decodeMultipart(resaved.payload, env.boundaryOf(resaved.mimeType))[0]
              .body,
          )
        : new TextDecoder().decode(resaved.payload);
    expect(resavedEnvelope).toBe(envelope);
  });

  it('fails the save of an in-memory database whose field name a part name cannot hold', async () => {
    const env = await isolate();
    env
      .isolatedDefineAgent({
        name: 'QuotedDatabaseName',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: {},
      })
      .implement({
        init: () => ({ count: 1, ['a"b']: new env.FakeDatabaseSync(':memory:') }),
        methods: {},
      });

    await expect(
      (await env.initiateIsolated('QuotedDatabaseName')).saveSnapshot(),
    ).rejects.toContain('Cannot snapshot in-memory database "a\\"b": its field name contains');
  });

  it('fails the save of a closed database and names its field', async () => {
    const env = await isolate();
    env
      .isolatedDefineAgent({
        name: 'ClosedDatabaseSave',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: {},
      })
      .implement({
        init: () => {
          const fileDb = new env.FakeDatabaseSync('/data/app.db');
          fileDb.isOpen = false;
          return { count: 1, fileDb };
        },
        methods: {},
      });

    await expect((await env.initiateIsolated('ClosedDatabaseSave')).saveSnapshot()).rejects.toBe(
      'Cannot snapshot database "fileDb": database is not open',
    );
  });

  it('fails the save when a file-backed database has an open transaction', async () => {
    const env = await isolate();
    env
      .isolatedDefineAgent({
        name: 'OpenFileDatabase',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: {},
      })
      .implement({
        init: () => {
          const fileDb = new env.FakeDatabaseSync('/data/app.db');
          fileDb.inTransaction = true;
          return { count: 4, fileDb };
        },
        methods: {},
      });

    await expect((await env.initiateIsolated('OpenFileDatabase')).saveSnapshot()).rejects.toContain(
      'Cannot snapshot database "fileDb": an open transaction exists',
    );
  });

  it('fails the save when two fields hold the same database', async () => {
    const env = await isolate();
    env
      .isolatedDefineAgent({
        name: 'SharedDatabase',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: {},
      })
      .implement({
        init: () => {
          const db = new env.FakeDatabaseSync('/data/app.db');
          return { count: 0, first: db, second: db };
        },
        methods: {},
      });

    await expect((await env.initiateIsolated('SharedDatabase')).saveSnapshot()).rejects.toContain(
      'Multiple agent fields reference the same DatabaseSync instance (field "second")',
    );
  });

  it('fails the save when a field holds a prepared statement', async () => {
    const env = await isolate();
    env
      .isolatedDefineAgent({
        name: 'StatementField',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: {},
      })
      .implement({
        init: () => ({ count: 0, statement: new env.FakeStatementSync() }),
        methods: {},
      });

    await expect((await env.initiateIsolated('StatementField')).saveSnapshot()).rejects.toContain(
      'Cannot automatically snapshot resource field "statement"',
    );
  });

  it('reopens a file-backed database at its recorded location on a typed load and warms it', async () => {
    const env = await isolate();
    env
      .isolatedDefineAgent({
        name: 'ReopenFileDatabase',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: {},
      })
      .implement({
        init: () => {
          throw new Error('init must not run on load');
        },
        methods: {},
      });
    env.select('ReopenFileDatabase');

    const envelope = {
      version: 1,
      principal: { tag: 'anonymous' },
      state: { count: 5 },
      fileDatabases: { fileDb: '/data/app.db' },
    };
    await env.isolatedGuest.loadSnapshot.load({
      payload: new TextEncoder().encode(JSON.stringify(envelope)),
      mimeType: 'application/json',
    });

    expect(env.existsSync).toHaveBeenCalledWith('/data/app.db');
    expect(env.constructed).toEqual([
      { path: '/data/app.db', options: env.REOPENED_DATABASE_OPTIONS },
    ]);
    expect(env.warmed).toEqual(['/data/app.db: SELECT count(*) FROM sqlite_master']);
    expect(env.serializeDatabaseSync.mock.calls.map(([db]) => db.path)).toEqual(['/data/app.db']);
    expect(env.restoreDatabaseSync).not.toHaveBeenCalled();

    const saved = await env.isolatedGuest.saveSnapshot.save();
    expect(saved.mimeType).toBe('application/json');
    expect(jsonOf(saved.payload)).toEqual(envelope);
  });

  it('fails the load when no file exists at a recorded location', async () => {
    const env = await isolate();
    env
      .isolatedDefineAgent({
        name: 'MissingFileDatabase',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: {},
      })
      .implement({
        init: () => {
          throw new Error('init must not run on load');
        },
        methods: {},
      });
    env.select('MissingFileDatabase');
    env.existingFiles.delete('/data/app.db');

    await expect(
      env.isolatedGuest.loadSnapshot.load({
        payload: new TextEncoder().encode(
          JSON.stringify({
            version: 1,
            principal: { tag: 'anonymous' },
            state: { count: 5 },
            fileDatabases: { fileDb: '/data/app.db' },
          }),
        ),
        mimeType: 'application/json',
      }),
    ).rejects.toContain('snapshot database field \\"fileDb\\": no database file at /data/app.db');
    expect(env.existsSync).toHaveBeenCalledWith('/data/app.db');
    expect(env.constructed).toEqual([]);
  });

  it('round-trips in-memory and file-backed databases through a multipart snapshot', async () => {
    const env = await isolate();
    env
      .isolatedDefineAgent({
        name: 'MultipartDatabases',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: {},
      })
      .implement({
        init: () => {
          throw new Error('init must not run on load');
        },
        methods: {},
      });
    env.select('MultipartDatabases');

    const envelope = {
      version: 1,
      principal: { tag: 'anonymous' },
      state: { count: 6 },
      fileDatabases: { fileDb: '/data/app.db' },
    };
    const memoryBytes = new Uint8Array([7, 8, 9]);
    const encoded = env.multipart.encodeMultipart([
      {
        name: 'state',
        contentType: 'application/json',
        body: new TextEncoder().encode(JSON.stringify(envelope)),
      },
      { name: 'db:memDb', contentType: 'application/x-sqlite3', body: memoryBytes },
    ]);
    await env.isolatedGuest.loadSnapshot.load({
      payload: encoded.data,
      mimeType: `multipart/mixed; boundary=${encoded.boundary}`,
    });

    expect(env.constructed).toEqual([
      { path: ':memory:', options: env.REOPENED_DATABASE_OPTIONS },
      { path: '/data/app.db', options: env.REOPENED_DATABASE_OPTIONS },
    ]);
    expect(env.restoreDatabaseSync).toHaveBeenCalledTimes(1);
    expect(env.warmed).toEqual([
      ':memory:: SELECT count(*) FROM sqlite_master',
      '/data/app.db: SELECT count(*) FROM sqlite_master',
    ]);
    expect(env.serializeDatabaseSync.mock.calls.map(([db]) => db.path)).toEqual(['/data/app.db']);

    const saved = await env.isolatedGuest.saveSnapshot.save();
    const parts = env.multipart.decodeMultipart(saved.payload, env.boundaryOf(saved.mimeType));
    expect(parts.map((part) => part.name)).toEqual(['state', 'db:memDb']);
    expect(JSON.parse(new TextDecoder().decode(parts[0].body))).toEqual(envelope);
    expect(parts[1].body).toEqual(memoryBytes);
  });

  it('warms a database that a custom load opens and keeps it in place of a reopen', async () => {
    const env = await isolate();
    env
      .isolatedDefineAgent({
        name: 'CustomLoadFileDatabase',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: {},
      })
      .implement({
        init: () => {
          throw new Error('init must not run on load');
        },
        methods: {},
        snapshot: {
          load(bytes) {
            const { count } = JSON.parse(new TextDecoder().decode(bytes));
            return {
              count,
              fileDb: new env.FakeDatabaseSync('/data/app.db'),
              otherDb: new env.FakeDatabaseSync('/data/other.db'),
            };
          },
        },
      });
    env.select('CustomLoadFileDatabase');
    env.existingFiles.clear();

    await env.isolatedGuest.loadSnapshot.load({
      payload: new TextEncoder().encode(
        JSON.stringify({
          version: 1,
          principal: { tag: 'anonymous' },
          state: { count: 7 },
          fileDatabases: { fileDb: '/data/app.db' },
        }),
      ),
      mimeType: 'application/json',
    });

    expect(env.constructed.map((database) => database.path)).toEqual([
      '/data/app.db',
      '/data/other.db',
    ]);
    expect(env.warmed).toEqual([
      '/data/app.db: SELECT count(*) FROM sqlite_master',
      '/data/other.db: SELECT count(*) FROM sqlite_master',
    ]);
    expect(env.serializeDatabaseSync.mock.calls.map(([db]) => db.path)).toEqual([
      '/data/app.db',
      '/data/other.db',
    ]);
    expect(env.existsSync).not.toHaveBeenCalled();
  });

  it.each([
    ['null', null],
    ['a value that is not a database', { path: '/data/app.db' }],
  ])(
    'fails the load when a custom load returns %s for a recorded database field',
    async (_description, value) => {
      const env = await isolate();
      env
        .isolatedDefineAgent({
          name: 'WrongDatabaseField',
          id: { name: z.string() },
          snapshotting: { state: z.object({ count: z.number() }) },
          methods: {},
        })
        .implement({
          init: () => ({ count: 0 }),
          methods: {},
          snapshot: {
            load() {
              return { count: 9, fileDb: value };
            },
          },
        });
      env.select('WrongDatabaseField');

      await expect(
        env.isolatedGuest.loadSnapshot.load({
          payload: new TextEncoder().encode(
            JSON.stringify({
              version: 1,
              principal: { tag: 'anonymous' },
              state: { count: 9 },
              fileDatabases: { fileDb: '/data/app.db' },
            }),
          ),
          mimeType: 'application/json',
        }),
      ).rejects.toContain('snapshot database field \\"fileDb\\" is not a DatabaseSync');
      expect(env.constructed).toEqual([]);
    },
  );

  it('reads only the schema of a database that does not fit in its page cache', async () => {
    const env = await isolate();
    env
      .isolatedDefineAgent({
        name: 'LargeFileDatabase',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: {},
      })
      .implement({
        init: () => ({ count: 0 }),
        methods: {},
        snapshot: {
          load() {
            const fileDb = new env.FakeDatabaseSync('/data/app.db');
            fileDb.pragmas = { page_count: 489, page_size: 4096, cache_size: -2000 };
            const smallDb = new env.FakeDatabaseSync('/data/other.db');
            return { count: 8, fileDb, smallDb };
          },
        },
      });
    env.select('LargeFileDatabase');

    await env.isolatedGuest.loadSnapshot.load({
      payload: new TextEncoder().encode(
        JSON.stringify({
          version: 1,
          principal: { tag: 'anonymous' },
          state: { count: 8 },
          fileDatabases: {},
        }),
      ),
      mimeType: 'application/json',
    });

    expect(env.warmed).toEqual([
      '/data/app.db: SELECT count(*) FROM sqlite_master',
      '/data/other.db: SELECT count(*) FROM sqlite_master',
    ]);
    expect(env.serializeDatabaseSync.mock.calls.map(([db]) => db.path)).toEqual(['/data/other.db']);
  });

  it('reads the page cache of a connection that returns rows as arrays', async () => {
    const env = await isolate();
    env
      .isolatedDefineAgent({
        name: 'ArrayRowsDatabase',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: {},
      })
      .implement({
        init: () => ({ count: 0 }),
        methods: {},
        snapshot: {
          load() {
            const fileDb = new env.FakeDatabaseSync('/data/app.db');
            fileDb.returnArrays = true;
            const largeDb = new env.FakeDatabaseSync('/data/other.db');
            largeDb.returnArrays = true;
            largeDb.pragmas = { page_count: 489, page_size: 4096, cache_size: -2000 };
            return { count: 8, fileDb, largeDb };
          },
        },
      });
    env.select('ArrayRowsDatabase');

    await env.isolatedGuest.loadSnapshot.load({
      payload: new TextEncoder().encode(
        JSON.stringify({
          version: 1,
          principal: { tag: 'anonymous' },
          state: { count: 8 },
          fileDatabases: {},
        }),
      ),
      mimeType: 'application/json',
    });

    expect(env.serializeDatabaseSync.mock.calls.map(([db]) => db.path)).toEqual(['/data/app.db']);
  });

  it('warms a database once when a custom load puts it in two fields', async () => {
    const env = await isolate();
    env
      .isolatedDefineAgent({
        name: 'AliasedDatabase',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: {},
      })
      .implement({
        init: () => ({ count: 0 }),
        methods: {},
        snapshot: {
          load() {
            const fileDb = new env.FakeDatabaseSync('/data/app.db');
            return { count: 8, fileDb, alias: fileDb };
          },
        },
      });
    env.select('AliasedDatabase');

    await env.isolatedGuest.loadSnapshot.load({
      payload: new TextEncoder().encode(
        JSON.stringify({
          version: 1,
          principal: { tag: 'anonymous' },
          state: { count: 8 },
          fileDatabases: {},
        }),
      ),
      mimeType: 'application/json',
    });

    expect(env.warmed).toEqual(['/data/app.db: SELECT count(*) FROM sqlite_master']);
    expect(env.serializeDatabaseSync).toHaveBeenCalledTimes(1);
  });

  it('does not warm a database that a custom load leaves closed', async () => {
    const env = await isolate();
    env
      .isolatedDefineAgent({
        name: 'ClosedDatabase',
        id: { name: z.string() },
        snapshotting: { state: z.object({ count: z.number() }) },
        methods: {},
      })
      .implement({
        init: () => ({ count: 0 }),
        methods: {},
        snapshot: {
          load() {
            const closedDb = new env.FakeDatabaseSync('/data/closed.db');
            closedDb.isOpen = false;
            return { count: 8, closedDb };
          },
        },
      });
    env.select('ClosedDatabase');

    await env.isolatedGuest.loadSnapshot.load({
      payload: new TextEncoder().encode(
        JSON.stringify({
          version: 1,
          principal: { tag: 'anonymous' },
          state: { count: 8 },
          fileDatabases: {},
        }),
      ),
      mimeType: 'application/json',
    });

    expect(env.warmed).toEqual([]);
    expect(env.serializeDatabaseSync).not.toHaveBeenCalled();
  });
});
