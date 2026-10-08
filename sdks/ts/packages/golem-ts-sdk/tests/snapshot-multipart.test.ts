import { beforeEach, describe, expect, it, vi } from 'vitest';
import { z } from 'zod';
import { Snapshot, SnapshotError } from '../src/snapshot';
import { encodeMultipart, decodeMultipart, extractBoundary } from '../src/internal/multipart';
import { schemaValueToWit, v } from '../src/internal/schema-model';
import type { StandardSchemaV1 } from '../src/schema/standardSchema';

const input = schemaValueToWit(v.record([]));
const bytes: Uint8Array = new Uint8Array([0, 255, 13, 10]);
const principal = {
  tag: 'oidc',
  val: { sub: 'Árvíz 🦀', issuer: 'issuer', claims: '{}' },
} as const;

const wireSnapshot = (
  state: unknown,
  extras: Array<{ name: string; contentType: string; body: Uint8Array }> = [],
  metadata: Record<string, unknown> = {},
) => {
  const { data, boundary } = encodeMultipart([
    {
      name: 'state',
      contentType: 'application/json',
      body: new TextEncoder().encode(JSON.stringify({ version: 1, principal, state, ...metadata })),
    },
    ...extras,
  ]);
  return { payload: data, mimeType: `multipart/mixed; boundary=${boundary}` };
};

describe('explicit multipart snapshots', () => {
  beforeEach(() => vi.resetModules());

  it('persists schema input, restores transformed output into independent live state, and preserves principal/bytes', async () => {
    const sdk = await import('../src');
    let initCalls = 0;
    let loads = 0;
    let loadedBytes: Uint8Array | undefined;
    sdk
      .defineAgent({
        name: 'MultipartIndex',
        id: {},
        methods: {},
        snapshotting: {
          multipart: { state: z.object({ revision: z.string().transform(Number) }) },
        },
      })
      .implement({
        init: () => {
          initCalls++;
          return { counter: 0, index: bytes };
        },
        methods: {},
        snapshot: {
          async save() {
            return {
              state: { revision: String(this.counter) },
              parts: new Map([
                ['index', { bytes: this.index, contentType: 'Application/Octet-Stream' }],
                ['__proto__', { bytes: new Uint8Array(), contentType: 'text/plain' }],
                ['state', { bytes: new Uint8Array([1]), contentType: 'application/json' }],
              ]),
            };
          },
          async load(saved, ctx) {
            loads++;
            expect(saved.state.revision).toBe(17);
            expect(ctx.principal).toMatchObject({ sub: principal.val.sub });
            loadedBytes = sdk.Snapshot.requirePart(
              saved.parts,
              'index',
              'APPLICATION/OCTET-STREAM',
            );
            expect([...saved.parts.keys()]).toEqual(['index', '__proto__', 'state']);
            return { counter: saved.state.revision, index: loadedBytes };
          },
        },
      });
    (globalThis as { currentAgentId?: string }).currentAgentId =
      `MultipartIndex(${JSON.stringify(input)})`;
    await sdk.loadSnapshot.load(
      wireSnapshot({ revision: '17' }, [
        { name: 'part:index', contentType: 'Application/Octet-Stream', body: bytes },
        { name: 'part:__proto__', contentType: 'text/plain', body: new Uint8Array() },
        { name: 'part:state', contentType: 'application/json', body: new Uint8Array([1]) },
      ]),
    );
    expect(initCalls).toBe(0);
    expect(loads).toBe(1);
    expect(loadedBytes).toEqual(bytes);
    const saved = await sdk.saveSnapshot.save();
    const parts = decodeMultipart(saved.payload, extractBoundary(saved.mimeType)!);
    expect(JSON.parse(new TextDecoder().decode(parts[0].body))).toMatchObject({
      version: 1,
      principal,
      state: { revision: '17' },
    });
    expect(JSON.parse(new TextDecoder().decode(parts[0].body))).not.toHaveProperty('fileDatabases');
    expect(parts[1]).toEqual({
      name: 'part:index',
      contentType: 'application/octet-stream',
      body: bytes,
    });
  });

  it('always emits multipart with null state and no parts', async () => {
    const sdk = await import('../src');
    sdk
      .defineAgent({
        name: 'NullParts',
        id: {},
        methods: {},
        snapshotting: { multipart: { state: z.null() } },
      })
      .implement({
        init: () => ({}),
        methods: {},
        snapshot: { save: async () => ({ state: null, parts: new Map() }), load: async () => ({}) },
      });
    (globalThis as { currentAgentId?: string }).currentAgentId =
      `NullParts(${JSON.stringify(input)})`;
    await sdk.loadSnapshot.load(wireSnapshot(null));
    const saved = await sdk.saveSnapshot.save();
    expect(saved.mimeType).toMatch(/^multipart\/mixed;/);
    const parts = decodeMultipart(saved.payload, extractBoundary(saved.mimeType)!);
    expect(JSON.parse(new TextDecoder().decode(parts[0].body))).not.toHaveProperty('fileDatabases');
  });

  it('validates mode, DB inventory, schema and namespaces before load, and publishes only after success', async () => {
    const sdk = await import('../src');
    let loads = 0;
    let fail = true;
    sdk
      .defineAgent({
        name: 'FailureParts',
        id: {},
        methods: {},
        snapshotting: { multipart: { state: z.null() } },
      })
      .implement({
        init: () => {
          throw new Error('init must not run');
        },
        methods: {},
        snapshot: {
          save: () => ({ state: null, parts: new Map() }),
          load: async () => {
            loads++;
            if (fail) throw new Error('load failed');
            return {};
          },
        },
      });
    (globalThis as { currentAgentId?: string }).currentAgentId =
      `FailureParts(${JSON.stringify(input)})`;
    for (const snapshot of [
      {
        payload: new TextEncoder().encode(JSON.stringify({ version: 1, principal, state: null })),
        mimeType: 'application/json',
      },
      wireSnapshot(null, [{ name: 'db:main', contentType: 'application/x-sqlite3', body: bytes }]),
      wireSnapshot(null, [], { fileDatabases: {} }),
      wireSnapshot(null, [], { fileDatabases: { main: '/data/app.db' } }),
      wireSnapshot(null, [{ name: 'db:main', contentType: 'application/x-sqlite3', body: bytes }], {
        fileDatabases: {},
      }),
      wireSnapshot(7),
      wireSnapshot(null, [{ name: 'unknown:x', contentType: 'text/plain', body: bytes }]),
      wireSnapshot(null, [{ name: 'part:a/b', contentType: 'text/plain', body: bytes }]),
      wireSnapshot(null, [
        { name: 'part:a', contentType: 'text/plain; charset=utf-8', body: bytes },
      ]),
    ])
      await expect(sdk.loadSnapshot.load(snapshot)).rejects.toBeDefined();
    expect(loads).toBe(0);
    await expect(sdk.loadSnapshot.load(wireSnapshot(null))).rejects.toContain('load failed');
    await expect(sdk.saveSnapshot.save()).rejects.toThrow(/not initialized/);
    fail = false;
    await sdk.loadSnapshot.load(wireSnapshot(null));
    expect(loads).toBe(2);
  });

  it('rejects duplicate JSON keys, lexical versions and invalid UTF-8', async () => {
    const sdk = await import('../src');
    const load = vi.fn(() => ({}));
    sdk
      .defineAgent({
        name: 'MalformedParts',
        id: {},
        methods: {},
        snapshotting: { multipart: { state: z.null() } },
      })
      .implement({
        init: () => ({}),
        methods: {},
        snapshot: {
          save: () => ({ state: null, parts: new Map() }),
          load,
        },
      });
    (globalThis as { currentAgentId?: string }).currentAgentId =
      `MalformedParts(${JSON.stringify(input)})`;
    for (const body of [
      new TextEncoder().encode(
        '{"version":1,"version":1,"principal":{"tag":"anonymous"},"state":null}',
      ),
      new TextEncoder().encode('{"version":1.0,"principal":{"tag":"anonymous"},"state":null}'),
      new TextEncoder().encode('{"version":1e0,"principal":{"tag":"anonymous"},"state":null}'),
      new Uint8Array([255]),
    ]) {
      const { data, boundary } = encodeMultipart([
        { name: 'state', contentType: 'application/json', body },
      ]);
      await expect(
        sdk.loadSnapshot.load({ payload: data, mimeType: `multipart/mixed; boundary=${boundary}` }),
      ).rejects.toBeDefined();
    }
    expect(load).not.toHaveBeenCalled();
    await expect(sdk.saveSnapshot.save()).rejects.toThrow(/not initialized/);
  });

  it('simple modes reject user parts instead of silently losing them', async () => {
    const sdk = await import('../src');
    sdk
      .defineAgent({
        name: 'SimpleParts',
        id: {},
        methods: {},
        snapshotting: { state: z.object({ count: z.number() }) },
      })
      .implement({ init: () => ({ count: 0 }), methods: {} });
    (globalThis as { currentAgentId?: string }).currentAgentId =
      `SimpleParts(${JSON.stringify(input)})`;
    await expect(
      sdk.loadSnapshot.load(
        wireSnapshot({ count: 3 }, [
          { name: 'part:index', contentType: 'application/octet-stream', body: bytes },
        ]),
      ),
    ).rejects.toContain('cannot restore user parts');
  });

  it('requires database metadata in typed multipart, including state-only snapshots', async () => {
    const sdk = await import('../src');
    sdk
      .defineAgent({
        name: 'TypedInventoryParts',
        id: {},
        methods: {},
        snapshotting: { state: z.object({ count: z.number() }) },
      })
      .implement({ init: () => ({ count: 0 }), methods: {} });
    (globalThis as { currentAgentId?: string }).currentAgentId =
      `TypedInventoryParts(${JSON.stringify(input)})`;
    await expect(sdk.loadSnapshot.load(wireSnapshot({ count: 3 }))).rejects.toContain(
      "missing 'fileDatabases'",
    );
    await expect(
      sdk.loadSnapshot.load(
        wireSnapshot({ count: 3 }, [
          { name: 'db:main', contentType: 'application/x-sqlite3', body: bytes },
        ]),
      ),
    ).rejects.toBeDefined();
    await sdk.loadSnapshot.load(wireSnapshot({ count: 3 }, [], { fileDatabases: {} }));
    const saved = await sdk.saveSnapshot.save();
    expect(JSON.parse(new TextDecoder().decode(saved.payload)).state).toEqual({ count: 3 });
  });

  it('validates JSON projection and user part metadata on save', async () => {
    const sdk = await import('../src');
    let state: unknown = null;
    let name = 'index';
    let contentType = 'application/octet-stream';
    sdk
      .defineAgent({
        name: 'SaveValidationParts',
        id: {},
        methods: {},
        snapshotting: { multipart: { state: z.unknown() } },
      })
      .implement({
        init: () => ({}),
        methods: {},
        snapshot: {
          save: () => ({ state, parts: new Map([[name, { bytes, contentType }]]) }),
          load: () => ({}),
        },
      });
    (globalThis as { currentAgentId?: string }).currentAgentId =
      `SaveValidationParts(${JSON.stringify(input)})`;
    await sdk.loadSnapshot.load(wireSnapshot(null));
    for (const invalid of [undefined, NaN, Infinity, 1n, new Uint8Array(), { x: undefined }]) {
      state = invalid;
      await expect(sdk.saveSnapshot.save()).rejects.toBeDefined();
    }
    state = null;
    for (const invalid of ['', 'a/b', 'db:main', '-leading', 'db😀', 'cache db', 'db\\cache']) {
      name = invalid;
      await expect(sdk.saveSnapshot.save()).rejects.toBeDefined();
    }
    name = 'index';
    for (const invalid of ['text/plain; charset=utf-8', 'text/plain\r\nx: y', 'téxt/plain']) {
      contentType = invalid;
      await expect(sdk.saveSnapshot.save()).rejects.toBeDefined();
    }
    contentType = 'Application/Octet-Stream';
    expect((await sdk.saveSnapshot.save()).mimeType).toMatch(/^multipart\/mixed;/);
  });

  it('rejects incomplete hooks and conflicting configuration at runtime', async () => {
    const sdk = await import('../src');
    const missing = sdk.defineAgent({
      name: 'IncompleteParts',
      id: {},
      methods: {},
      snapshotting: { multipart: { state: z.null() } },
    });
    missing.implement({
      init: () => ({}),
      methods: {},
      snapshot: { save: () => ({ state: null, parts: new Map() }) },
    } as never);
    (globalThis as { currentAgentId?: string }).currentAgentId =
      `IncompleteParts(${JSON.stringify(input)})`;
    await expect(sdk.loadSnapshot.load(wireSnapshot(null))).rejects.toContain(
      'snapshot.save and snapshot.load',
    );
    sdk.defineAgent({
      name: 'ConflictingParts',
      id: {},
      methods: {},
      snapshotting: { state: z.null(), multipart: { state: z.null() } },
    } as never);
    (globalThis as { currentAgentId?: string }).currentAgentId =
      `ConflictingParts(${JSON.stringify(input)})`;
    await expect(sdk.loadSnapshot.load(wireSnapshot(null))).rejects.toContain('mutually exclusive');
    for (const [i, multipart] of [
      undefined,
      {},
      { state: null },
      { state: {} },
      { state: { '~standard': { version: 1, vendor: 'test' } } },
    ].entries()) {
      const name = `IncompleteConfig${i}`;
      sdk
        .defineAgent({ name, id: {}, methods: {}, snapshotting: { multipart } } as never)
        .implement({
          init: () => ({}),
          methods: {},
          snapshot: { save: () => bytes, load: () => ({}) },
        } as never);
      (globalThis as { currentAgentId?: string }).currentAgentId =
        `${name}(${JSON.stringify(input)})`;
      await expect(sdk.loadSnapshot.load(wireSnapshot(null))).rejects.toContain(
        'Standard Schema state validator',
      );
    }
  });

  it('captures schema input before a mutating validator runs', async () => {
    const sdk = await import('../src');
    const schema: StandardSchemaV1<{ revision: string }, { revision: number }> = {
      '~standard': {
        version: 1,
        vendor: 'mutating',
        validate(value) {
          const state = value as { revision: string | number };
          if (typeof state.revision !== 'string')
            return { issues: [{ message: 'revision must be input string' }] };
          state.revision = Number(state.revision);
          return { value: state as { revision: number } };
        },
      },
    };
    const projection = { revision: '17' };
    sdk
      .defineAgent({
        name: 'MutatingParts',
        id: {},
        methods: {},
        snapshotting: { multipart: { state: schema } },
      })
      .implement({
        init: () => ({ counter: 0 }),
        methods: {},
        snapshot: {
          save: () => ({ state: projection, parts: new Map() }),
          load: (saved) => ({ counter: saved.state.revision }),
        },
      });
    (globalThis as { currentAgentId?: string }).currentAgentId =
      `MutatingParts(${JSON.stringify(input)})`;
    await sdk.loadSnapshot.load(wireSnapshot({ revision: '17' }));
    for (let i = 0; i < 2; i++) {
      const saved = await sdk.saveSnapshot.save();
      const parts = decodeMultipart(saved.payload, extractBoundary(saved.mimeType)!);
      expect(JSON.parse(new TextDecoder().decode(parts[0].body)).state).toEqual({ revision: '17' });
      expect(projection).toEqual({ revision: '17' });
    }
  });

  it('rejects malformed multipart principals before loading or installing state', async () => {
    const sdk = await import('../src');
    const load = vi.fn(() => ({}));
    sdk
      .defineAgent({
        name: 'PrincipalParts',
        id: {},
        methods: {},
        snapshotting: { multipart: { state: z.null() } },
      })
      .implement({
        init: () => ({}),
        methods: {},
        snapshot: { save: () => ({ state: null, parts: new Map() }), load },
      });
    (globalThis as { currentAgentId?: string }).currentAgentId =
      `PrincipalParts(${JSON.stringify(input)})`;
    const invalid: unknown[] = [
      null,
      [],
      { tag: 'oidc', val: null },
      { tag: 'agent', val: { componentId: '00000000-0000-0000-0000-000000000001', agentId: 17 } },
      { tag: 'golem-user', val: { accountId: 17 } },
    ];
    for (const field of [
      'sub',
      'issuer',
      'claims',
      'email',
      'name',
      'givenName',
      'familyName',
      'picture',
      'preferredUsername',
      'emailVerified',
    ]) {
      invalid.push({
        tag: 'oidc',
        val: { ...principal.val, [field]: field === 'emailVerified' ? 'false' : 17 },
      });
    }
    for (const principal of invalid) {
      const { data, boundary } = encodeMultipart([
        {
          name: 'state',
          contentType: 'application/json',
          body: new TextEncoder().encode(JSON.stringify({ version: 1, principal, state: null })),
        },
      ]);
      await expect(
        sdk.loadSnapshot.load({ payload: data, mimeType: `multipart/mixed; boundary=${boundary}` }),
      ).rejects.toBeDefined();
      expect(load).not.toHaveBeenCalled();
      await expect(sdk.saveSnapshot.save()).rejects.toThrow(/not initialized/);
    }
  });

  it.each(['$db', 'db😀', 'cache db', 'db\\cache'])(
    'preserves automatic SQLite field name %s outside the user-part grammar',
    async (fieldName) => {
      class DatabaseSync {
        isOpen = true;
        location() {
          return null;
        }
        prepare() {
          return { get() {} };
        }
      }
      const restore = vi.fn();
      vi.doMock('../src/internal/sqlite', () => ({
        DatabaseSync,
        StatementSync: class {},
        Session: class {},
        SQLTagStore: class {},
        isAutocommitDatabaseSync: () => true,
        serializeDatabaseSync: () => bytes,
        restoreDatabaseSync: restore,
      }));
      try {
        const register = async () => {
          const sdk = await import('../src');
          sdk
            .defineAgent({
              name: 'DollarDbParts',
              id: {},
              methods: {},
              snapshotting: { state: z.object({ count: z.number() }) },
            })
            .implement({
              init: () => ({ count: 17, [fieldName]: new DatabaseSync() }),
              methods: {},
            });
          return sdk;
        };
        (globalThis as { currentAgentId?: string }).currentAgentId =
          `DollarDbParts(${JSON.stringify(input)})`;
        const initial = await register();
        await initial.golemAgent200Guest.initialize('DollarDbParts', input, { tag: 'anonymous' });
        const snapshot = await initial.saveSnapshot.save();
        const parts = decodeMultipart(snapshot.payload, extractBoundary(snapshot.mimeType)!);
        expect(parts[1].name).toBe(`db:${fieldName}`);
        vi.resetModules();
        const restored = await register();
        await restored.loadSnapshot.load(snapshot);
        expect(restore).toHaveBeenCalledTimes(1);
        expect(restore.mock.calls[0][1]).toEqual(bytes);
        const resaved = await restored.saveSnapshot.save();
        const restoredParts = decodeMultipart(resaved.payload, extractBoundary(resaved.mimeType)!);
        expect(restoredParts[1].name).toBe(`db:${fieldName}`);
        expect(restoredParts[1].body).toEqual(bytes);
      } finally {
        vi.doUnmock('../src/internal/sqlite');
      }
    },
  );

  it('requires parts with normalized bare MIME and throws SnapshotError', () => {
    const parts = new Map([['index', { bytes, contentType: 'Application/Octet-Stream' }]]);
    expect(Snapshot.requirePart(parts, 'index', 'APPLICATION/OCTET-STREAM')).toBe(bytes);
    for (const [name, mime] of [
      ['missing', 'text/plain'],
      ['index', 'text/plain'],
      ['index', 'application/octet-stream; charset=utf-8'],
    ]) {
      expect(() => Snapshot.requirePart(parts, name, mime)).toThrow(SnapshotError);
    }
  });
});
