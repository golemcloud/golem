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

import { describe, expect, it, vi, beforeAll } from 'vitest';

// ─── Mock types and state ────────────────────────────────────────────────────

interface MockDbState {
  data: Uint8Array;
  autocommit: boolean;
  readOnly: boolean;
  databases: Array<{ name: string }>;
}

// vi.hoisted runs before vi.mock hoisting, so the shared mock exports are
// available when the mock factories execute.  This ensures `instanceof`
// checks in baseAgent.ts see the same constructor identity.
const { dbStateMap, sharedMockExports } = vi.hoisted(() => {
  const dbStateMap = new WeakMap<object, MockDbState>();

  class MockDatabaseSync {
    constructor() {
      dbStateMap.set(this, {
        data: new Uint8Array([0xdb, 0x01]),
        autocommit: true,
        readOnly: false,
        databases: [{ name: 'main' }, { name: 'temp' }],
      });
    }

    prepare(_sql: string): MockStatementSync {
      const state = dbStateMap.get(this);
      return new MockStatementSync(state?.databases ?? []);
    }
  }

  class MockStatementSync {
    private readonly rows: Array<{ name: string }>;

    constructor(rows: Array<{ name: string }>) {
      this.rows = rows;
    }

    all(): Array<{ name: string }> {
      return this.rows;
    }
  }

  class MockSession {}
  class MockSQLTagStore {}

  function serializeDatabaseSync(db: MockDatabaseSync): Uint8Array {
    const state = dbStateMap.get(db);
    if (!state) throw new Error('Unknown MockDatabaseSync');
    return state.data;
  }

  function restoreDatabaseSync(db: MockDatabaseSync, bytes: Uint8Array): void {
    const state = dbStateMap.get(db);
    if (!state) throw new Error('Unknown MockDatabaseSync');
    state.data = bytes;
  }

  function isAutocommitDatabaseSync(db: MockDatabaseSync): boolean {
    const state = dbStateMap.get(db);
    if (!state) throw new Error('Unknown MockDatabaseSync');
    return state.autocommit;
  }

  const sharedMockExports = {
    DatabaseSync: MockDatabaseSync,
    StatementSync: MockStatementSync,
    Session: MockSession,
    SQLTagStore: MockSQLTagStore,
    serializeDatabaseSync,
    restoreDatabaseSync,
    isAutocommitDatabaseSync,
    constants: {},
    backup: () => {},
  };

  return { dbStateMap, sharedMockExports };
});

// Mock both node:sqlite and the internal adapter so that regardless of
// which path resolves first, the same mock classes are used.
vi.mock('node:sqlite', () => sharedMockExports);
vi.mock('../src/internal/sqlite', () => sharedMockExports);

// ─── Dynamically loaded modules ─────────────────────────────────────────────
// Because testSetup.ts loads baseAgent.ts before our mocks take effect,
// we must use vi.resetModules() + dynamic import to get a fresh BaseAgent
// that picks up the mocked sqlite classes.

type BaseAgentClass = typeof import('../src/baseAgent').BaseAgent;
type DatabaseSyncClass = (typeof sharedMockExports)['DatabaseSync'];
type StatementSyncClass = (typeof sharedMockExports)['StatementSync'];
type SessionClass = (typeof sharedMockExports)['Session'];
type SQLTagStoreClass = (typeof sharedMockExports)['SQLTagStore'];

let BaseAgent: BaseAgentClass;
let DatabaseSync: DatabaseSyncClass;
let StatementSync: StatementSyncClass;
let Session: SessionClass;
let SQLTagStore: SQLTagStoreClass;
let decodeMultipart: typeof import('../src/internal/multipart').decodeMultipart;
let encodeMultipart: typeof import('../src/internal/multipart').encodeMultipart;

beforeAll(async () => {
  vi.resetModules();
  const ba = await import('../src/baseAgent');
  BaseAgent = ba.BaseAgent;
  const sqlite = await import('../src/internal/sqlite');
  DatabaseSync = sqlite.DatabaseSync as unknown as DatabaseSyncClass;
  StatementSync = sqlite.StatementSync as unknown as StatementSyncClass;
  Session = sqlite.Session as unknown as SessionClass;
  SQLTagStore = sqlite.SQLTagStore as unknown as SQLTagStoreClass;
  const mp = await import('../src/internal/multipart');
  decodeMultipart = mp.decodeMultipart;
  encodeMultipart = mp.encodeMultipart;
});

// ─── Helpers ─────────────────────────────────────────────────────────────────

function createMockDb(overrides?: Partial<MockDbState>): InstanceType<DatabaseSyncClass> {
  const db = new sharedMockExports.DatabaseSync();
  const state = dbStateMap.get(db)!;
  if (overrides) Object.assign(state, overrides);
  return db;
}

function parseMultipartResult(result: { data: Uint8Array; mimeType: string }) {
  const boundaryMatch = result.mimeType.match(/boundary=([^\s;]+)/);
  expect(boundaryMatch).not.toBeNull();
  return decodeMultipart(result.data, boundaryMatch![1]);
}

// ─── Tests ───────────────────────────────────────────────────────────────────

describe('BaseAgent snapshotting', () => {
  describe('No DBs — backward-compatible plain JSON snapshot', () => {
    it('saveSnapshot returns application/json', async () => {
      class PlainAgent extends BaseAgent {
        counter = 7;
        label = 'world';
      }
      const agent = new PlainAgent();

      const result = (await agent.saveSnapshot()) as { data: Uint8Array; mimeType: string };
      expect(result.mimeType).toBe('application/json');

      const state = JSON.parse(new TextDecoder().decode(result.data));
      expect(state.counter).toBe(7);
      expect(state.label).toBe('world');
    });

    it('loadSnapshot restores properties from JSON', async () => {
      class PlainAgent extends BaseAgent {
        counter = 0;
        label = '';
      }
      const agent = new PlainAgent();
      const snapshot = new TextEncoder().encode(JSON.stringify({ counter: 99, label: 'restored' }));

      await agent.loadSnapshot(snapshot);
      expect(agent.counter).toBe(99);
      expect(agent.label).toBe('restored');
    });
  });

  describe('Single :memory: DB roundtrip', () => {
    it('saveSnapshot returns multipart/mixed with state and db parts', async () => {
      class SingleDbAgent extends BaseAgent {
        counter = 42;
        myDb = createMockDb({ data: new Uint8Array([0xca, 0xfe]) });
      }
      const agent = new SingleDbAgent();

      const result = (await agent.saveSnapshot()) as { data: Uint8Array; mimeType: string };
      expect(result.mimeType).toMatch(/^multipart\/mixed/);

      const parts = parseMultipartResult(result);
      const statePart = parts.find((p) => p.name === 'state');
      expect(statePart).toBeDefined();
      expect(statePart!.contentType).toBe('application/json');

      const state = JSON.parse(new TextDecoder().decode(statePart!.body));
      expect(state.counter).toBe(42);
      expect(state).not.toHaveProperty('myDb');

      const dbPart = parts.find((p) => p.name === 'db:myDb');
      expect(dbPart).toBeDefined();
      expect(dbPart!.contentType).toBe('application/x-sqlite3');
      expect(Array.from(dbPart!.body)).toEqual([0xca, 0xfe]);
    });

    it('loadSnapshot restores both properties and database', async () => {
      class SingleDbAgent extends BaseAgent {
        counter = 42;
        myDb = createMockDb({ data: new Uint8Array([0xca, 0xfe]) });
      }
      const agent = new SingleDbAgent();
      const result = (await agent.saveSnapshot()) as { data: Uint8Array; mimeType: string };

      const agent2 = new SingleDbAgent();
      await agent2.loadSnapshot(result.data, result.mimeType);

      expect(agent2.counter).toBe(42);
      const restoredState = dbStateMap.get(agent2.myDb)!;
      expect(Array.from(restoredState.data)).toEqual([0xca, 0xfe]);
    });
  });

  describe('Multiple databases on same agent', () => {
    it('both appear as separate db: parts', async () => {
      class MultiDbAgent extends BaseAgent {
        notes = 'multi';
        dbA = createMockDb({ data: new Uint8Array([0xaa]) });
        dbB = createMockDb({ data: new Uint8Array([0xbb]) });
      }
      const agent = new MultiDbAgent();

      const result = (await agent.saveSnapshot()) as { data: Uint8Array; mimeType: string };
      const parts = parseMultipartResult(result);

      const dbAPart = parts.find((p) => p.name === 'db:dbA');
      const dbBPart = parts.find((p) => p.name === 'db:dbB');

      expect(dbAPart).toBeDefined();
      expect(dbBPart).toBeDefined();
      expect(Array.from(dbAPart!.body)).toEqual([0xaa]);
      expect(Array.from(dbBPart!.body)).toEqual([0xbb]);
    });

    it('each restored independently', async () => {
      class MultiDbAgent extends BaseAgent {
        notes = 'multi';
        dbA = createMockDb({ data: new Uint8Array([0xaa]) });
        dbB = createMockDb({ data: new Uint8Array([0xbb]) });
      }
      const agent = new MultiDbAgent();
      const result = (await agent.saveSnapshot()) as { data: Uint8Array; mimeType: string };

      const agent2 = new MultiDbAgent();
      await agent2.loadSnapshot(result.data, result.mimeType);

      expect(Array.from(dbStateMap.get(agent2.dbA)!.data)).toEqual([0xaa]);
      expect(Array.from(dbStateMap.get(agent2.dbB)!.data)).toEqual([0xbb]);
    });
  });

  describe('StatementSync, Session, SQLTagStore excluded from JSON', () => {
    it('these fields are not present in state JSON or as db parts', async () => {
      class MixedFieldsAgent extends BaseAgent {
        counter = 1;
        myDb = createMockDb();
        stmt = new StatementSync([]);
        session = new Session();
        tags = new SQLTagStore();
      }
      const agent = new MixedFieldsAgent();

      const result = (await agent.saveSnapshot()) as { data: Uint8Array; mimeType: string };
      const parts = parseMultipartResult(result);

      const statePart = parts.find((p) => p.name === 'state')!;
      const state = JSON.parse(new TextDecoder().decode(statePart.body));

      expect(state).toHaveProperty('counter');
      expect(state).not.toHaveProperty('stmt');
      expect(state).not.toHaveProperty('session');
      expect(state).not.toHaveProperty('tags');
      expect(state).not.toHaveProperty('myDb');

      expect(parts.find((p) => p.name === 'db:stmt')).toBeUndefined();
      expect(parts.find((p) => p.name === 'db:session')).toBeUndefined();
      expect(parts.find((p) => p.name === 'db:tags')).toBeUndefined();
    });
  });

  describe('Open transaction fails', () => {
    it('saveSnapshot throws when autocommit is false', async () => {
      class TxnAgent extends BaseAgent {
        db = createMockDb({ autocommit: false });
      }

      const agent = new TxnAgent();
      await expect(agent.saveSnapshot()).rejects.toThrow(
        /Cannot snapshot database "db".*open transaction/,
      );
    });
  });

  describe("ATTACH'd database fails", () => {
    it('saveSnapshot throws when extra schemas exist', async () => {
      class AttachAgent extends BaseAgent {
        db = createMockDb({
          databases: [{ name: 'main' }, { name: 'temp' }, { name: 'extra_schema' }],
        });
      }

      const agent = new AttachAgent();
      await expect(agent.saveSnapshot()).rejects.toThrow(
        /Cannot snapshot database "db".*ATTACH.*extra_schema/,
      );
    });
  });

  describe('Duplicate DB reference fails', () => {
    it('saveSnapshot throws when two fields point to the same instance', async () => {
      const sharedDb = createMockDb();

      class DuplicateAgent extends BaseAgent {
        dbX: InstanceType<DatabaseSyncClass>;
        dbY: InstanceType<DatabaseSyncClass>;
        constructor() {
          super();
          this.dbX = sharedDb;
          this.dbY = sharedDb;
        }
      }

      const agent = new DuplicateAgent();
      await expect(agent.saveSnapshot()).rejects.toThrow(
        /Multiple agent fields reference the same DatabaseSync/,
      );
    });
  });

  describe('Property renamed between versions — graceful skip', () => {
    it('loadSnapshot warns and skips when db part has no matching property', async () => {
      class SourceAgent extends BaseAgent {
        counter = 42;
        myDb = createMockDb({ data: new Uint8Array([0xca, 0xfe]) });
      }
      const agent = new SourceAgent();
      const result = (await agent.saveSnapshot()) as { data: Uint8Array; mimeType: string };

      // Agent that has a differently-named DB property
      class RenamedAgent extends BaseAgent {
        counter = 0;
        renamedDb = createMockDb();
      }

      const agent2 = new RenamedAgent();

      const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
      await agent2.loadSnapshot(result.data, result.mimeType);

      expect(warnSpy).toHaveBeenCalledWith(expect.stringContaining('myDb'));
      warnSpy.mockRestore();
    });
  });

  describe('DB part maps to non-DatabaseSync property fails', () => {
    it('loadSnapshot throws when db:foo maps to a string property', async () => {
      class SourceAgent extends BaseAgent {
        counter = 42;
        myDb = createMockDb({ data: new Uint8Array([0xca, 0xfe]) });
      }
      const agent = new SourceAgent();
      const result = (await agent.saveSnapshot()) as { data: Uint8Array; mimeType: string };

      // Agent where myDb is a string instead of a DatabaseSync
      class WrongTypeAgent extends BaseAgent {
        counter = 0;
        myDb = 'not-a-database';
      }

      const agent2 = new WrongTypeAgent();
      await expect(agent2.loadSnapshot(result.data, result.mimeType)).rejects.toThrow(
        /non-DatabaseSync/,
      );
    });
  });

  describe('Constructor side effects overwritten', () => {
    it('restoreDatabaseSync is called after constructor', async () => {
      class ConstructorAgent extends BaseAgent {
        counter = 100;
        myDb = createMockDb({ data: new Uint8Array([0x00, 0x00]) });
      }

      const source = new ConstructorAgent();
      dbStateMap.get(source.myDb)!.data = new Uint8Array([0xff, 0xee]);
      const result = (await source.saveSnapshot()) as { data: Uint8Array; mimeType: string };

      // New agent starts with constructor defaults
      const target = new ConstructorAgent();
      expect(Array.from(dbStateMap.get(target.myDb)!.data)).toEqual([0x00, 0x00]);

      await target.loadSnapshot(result.data, result.mimeType);

      // After restore, the data should match the source, not the constructor default
      expect(Array.from(dbStateMap.get(target.myDb)!.data)).toEqual([0xff, 0xee]);
    });
  });

  describe('serializeTrackedDatabases / restoreTrackedDatabases helpers', () => {
    it('serializeTrackedDatabases returns all database fields', () => {
      class MultiDbAgent extends BaseAgent {
        notes = 'multi';
        dbA = createMockDb({ data: new Uint8Array([0xaa]) });
        dbB = createMockDb({ data: new Uint8Array([0xbb]) });
      }
      const agent = new MultiDbAgent();
      const databases = (agent as any).serializeTrackedDatabases() as Array<{
        name: string;
        bytes: Uint8Array;
      }>;

      expect(databases).toHaveLength(2);
      const names = databases.map((d) => d.name).sort();
      expect(names).toEqual(['dbA', 'dbB']);

      const dbAEntry = databases.find((d) => d.name === 'dbA')!;
      expect(Array.from(dbAEntry.bytes)).toEqual([0xaa]);
    });

    it('restoreTrackedDatabases restores database fields', () => {
      class MultiDbAgent extends BaseAgent {
        notes = 'multi';
        dbA = createMockDb({ data: new Uint8Array([0xaa]) });
        dbB = createMockDb({ data: new Uint8Array([0xbb]) });
      }
      const agent = new MultiDbAgent();
      const databases = [
        { name: 'dbA', bytes: new Uint8Array([0xaa]) },
        { name: 'dbB', bytes: new Uint8Array([0xbb]) },
      ];

      (agent as any).restoreTrackedDatabases(databases);

      expect(Array.from(dbStateMap.get(agent.dbA)!.data)).toEqual([0xaa]);
      expect(Array.from(dbStateMap.get(agent.dbB)!.data)).toEqual([0xbb]);
    });

    it('restoreTrackedDatabases warns and skips missing properties', () => {
      class PlainAgent extends BaseAgent {
        counter = 0;
      }
      const agent = new PlainAgent();
      const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

      (agent as any).restoreTrackedDatabases([
        { name: 'nonExistent', bytes: new Uint8Array([0x01]) },
      ]);

      expect(warnSpy).toHaveBeenCalledWith(expect.stringContaining('nonExistent'));
      warnSpy.mockRestore();
    });

    it('restoreTrackedDatabases throws for non-DatabaseSync property', () => {
      class BadAgent extends BaseAgent {
        foo = 'a string';
      }

      const agent = new BadAgent();
      expect(() =>
        (agent as any).restoreTrackedDatabases([{ name: 'foo', bytes: new Uint8Array([0x01]) }]),
      ).toThrow(/not a DatabaseSync/);
    });

    it('serializeTrackedDatabases throws on duplicate reference', () => {
      const sharedDb = createMockDb();

      class DupHelperAgent extends BaseAgent {
        a: InstanceType<DatabaseSyncClass>;
        b: InstanceType<DatabaseSyncClass>;
        constructor() {
          super();
          this.a = sharedDb;
          this.b = sharedDb;
        }
      }

      const agent = new DupHelperAgent();
      expect(() => (agent as any).serializeTrackedDatabases()).toThrow(
        /Multiple agent fields reference the same DatabaseSync/,
      );
    });

    it('serializeTrackedDatabases throws on open transaction', () => {
      class TxnHelperAgent extends BaseAgent {
        db = createMockDb({ autocommit: false });
      }

      const agent = new TxnHelperAgent();
      expect(() => (agent as any).serializeTrackedDatabases()).toThrow(/open transaction/);
    });
  });
});

describe('index.ts save/load envelope format', () => {
  describe('save() wraps snapshot with version/principal in state part', () => {
    it('JSON snapshot gets envelope with version and principal', async () => {
      class PlainAgent extends BaseAgent {
        counter = 5;
      }
      const agent = new PlainAgent();

      const agentResult = (await agent.saveSnapshot()) as { data: Uint8Array; mimeType: string };
      expect(agentResult.mimeType).toBe('application/json');

      // Simulate what save() does for JSON snapshots
      const state = JSON.parse(new TextDecoder().decode(agentResult.data));
      const principal = { tag: 'anonymous' as const };
      const envelope = { version: 1, principal, state };
      const wrapped = new TextEncoder().encode(JSON.stringify(envelope));

      const parsed = JSON.parse(new TextDecoder().decode(wrapped));
      expect(parsed.version).toBe(1);
      expect(parsed.principal).toEqual({ tag: 'anonymous' });
      expect(parsed.state.counter).toBe(5);
    });

    it('multipart snapshot injects version/principal into state part', async () => {
      class SingleDbAgent extends BaseAgent {
        counter = 42;
        myDb = createMockDb({ data: new Uint8Array([0xca, 0xfe]) });
      }
      const agent = new SingleDbAgent();

      const agentResult = (await agent.saveSnapshot()) as { data: Uint8Array; mimeType: string };
      expect(agentResult.mimeType).toMatch(/^multipart\/mixed/);

      // Simulate what save() does for multipart snapshots
      const boundaryMatch = agentResult.mimeType.match(/boundary=([^\s;]+)/)!;
      const parts = decodeMultipart(agentResult.data, boundaryMatch[1]);
      const stateIdx = parts.findIndex((p) => p.name === 'state');
      const stateJson = JSON.parse(new TextDecoder().decode(parts[stateIdx].body));
      const principal = { tag: 'system' as const };
      const envelope = { version: 1, principal, state: stateJson };
      parts[stateIdx] = {
        ...parts[stateIdx],
        body: new TextEncoder().encode(JSON.stringify(envelope)),
      };

      // Verify the envelope structure in the modified state part
      const modifiedState = JSON.parse(new TextDecoder().decode(parts[stateIdx].body));
      expect(modifiedState.version).toBe(1);
      expect(modifiedState.principal).toEqual({ tag: 'system' });
      expect(modifiedState.state.counter).toBe(42);
    });
  });

  describe('load() unwraps envelope and passes to loadSnapshot', () => {
    it('JSON envelope unwraps state and extracts principal', async () => {
      const principal = { tag: 'system' as const };
      const state = { counter: 77, label: 'from-load' };
      const envelope = { version: 1, principal, state };
      const bytes = new TextEncoder().encode(JSON.stringify(envelope));

      // Simulate what load() does for JSON snapshots
      const parsed = JSON.parse(new TextDecoder().decode(bytes));
      expect(parsed.principal).toEqual({ tag: 'system' });

      const agentSnapshot = new TextEncoder().encode(JSON.stringify(parsed.state));

      class PlainAgent extends BaseAgent {
        counter = 0;
        label = '';
      }
      const agent = new PlainAgent();
      await agent.loadSnapshot(agentSnapshot, 'application/json');
      expect(agent.counter).toBe(77);
      expect(agent.label).toBe('from-load');
    });

    it('multipart envelope unwraps state from envelope in state part', async () => {
      class SingleDbAgent extends BaseAgent {
        counter = 42;
        myDb = createMockDb({ data: new Uint8Array([0xca, 0xfe]) });
      }

      // Build a multipart snapshot
      const agent = new SingleDbAgent();
      const agentResult = (await agent.saveSnapshot()) as { data: Uint8Array; mimeType: string };
      const boundaryMatch = agentResult.mimeType.match(/boundary=([^\s;]+)/)!;
      const parts = decodeMultipart(agentResult.data, boundaryMatch[1]);

      // Wrap the state part in an envelope (as save() would)
      const stateIdx = parts.findIndex((p) => p.name === 'state');
      const stateJson = JSON.parse(new TextDecoder().decode(parts[stateIdx].body));
      const principal = { tag: 'anonymous' as const };
      const envelope = { version: 1, principal, state: stateJson };
      parts[stateIdx] = {
        ...parts[stateIdx],
        body: new TextEncoder().encode(JSON.stringify(envelope)),
      };

      // Simulate what load() does: unwrap envelope, re-encode parts
      const loadedEnvelope = JSON.parse(new TextDecoder().decode(parts[stateIdx].body));
      expect(loadedEnvelope.state).toBeDefined();

      // Replace state part body with just the agent properties
      parts[stateIdx] = {
        ...parts[stateIdx],
        body: new TextEncoder().encode(JSON.stringify(loadedEnvelope.state)),
      };

      // Re-encode and load
      const { data: reencoded, boundary: newBoundary } = encodeMultipart(parts);
      const newMimeType = `multipart/mixed; boundary=${newBoundary}`;

      const agent2 = new SingleDbAgent();
      await agent2.loadSnapshot(reencoded, newMimeType);

      expect(agent2.counter).toBe(42);
      expect(Array.from(dbStateMap.get(agent2.myDb)!.data)).toEqual([0xca, 0xfe]);
    });
  });
});
