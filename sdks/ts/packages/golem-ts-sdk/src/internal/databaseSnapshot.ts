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

// The SQLite databases of a typed snapshot: the bytes of each in-memory database, and the
// location of each file-backed database, whose file the executor's filesystem snapshot restores.

import {
  DatabaseSync,
  isAutocommitDatabaseSync,
  restoreDatabaseSync,
  serializeDatabaseSync,
  Session,
  SQLTagStore,
  StatementSync,
} from './sqlite';
import { existsSync } from 'node:fs';
import type { MultipartPart } from './multipart';

/** The SQLite databases of a typed snapshot, keyed by the agent field that holds each one. */
export type SnapshotDatabases = {
  /** The bytes of each in-memory or temporary database. */
  inMemory: Array<{ name: string; bytes: Uint8Array }>;
  /** The location of each file-backed database, as `DatabaseSync.location()` reports it. */
  fileDatabases: Record<string, string>;
};

/** What a save reads from one `DatabaseSync` field. */
type DatabaseField = {
  name: string;
  /** False when the connection has an open transaction. */
  autocommit: boolean;
  /** `DatabaseSync.location()`: null for an in-memory or temporary database. */
  location: string | null;
};

/** Which databases go into the snapshot as bytes, and which by their location. */
type DatabasePlan = {
  inMemory: string[];
  fileDatabases: Record<string, string>;
};

/**
 * The open options of a database that a load creates: every option of the `DatabaseSync`
 * constructor of the wasm-rquickjs `node:sqlite` builtin, each set to the value that the builtin
 * uses when the option is not given.
 */
export const REOPENED_DATABASE_OPTIONS = {
  open: true,
  readOnly: false,
  enableForeignKeyConstraints: true,
  enableDoubleQuotedStringLiterals: false,
  allowExtension: false,
  timeout: 0,
  defensive: true,
  readBigInts: false,
  returnArrays: false,
  allowBareNamedParameters: true,
  allowUnknownNamedParameters: false,
} as const;

const IN_MEMORY_LOCATION = ':memory:';
const DATABASE_PART_PREFIX = 'db:';
const DATABASE_PART_CONTENT_TYPE = 'application/x-sqlite3';
/** The characters that end the quoted name or the line of a multipart part header. */
const PART_NAME_BREAKING_CHARACTERS = /["\r\n]/;

function isDatabaseSync(val: unknown): val is DatabaseSync {
  return val instanceof DatabaseSync;
}

/** Whether `val` is a SQLite object that a snapshot cannot hold as plain state. */
export function isSqliteResource(val: unknown): boolean {
  return (
    isDatabaseSync(val) ||
    isInstance(val, StatementSync) ||
    isInstance(val, Session) ||
    isInstance(val, SQLTagStore)
  );
}

/** `val instanceof Ctor`, including builtins whose constructors are not public. */
function isInstance(val: unknown, Ctor: Function): boolean {
  return typeof Ctor === 'function' && val instanceof Ctor;
}

/**
 * Decides how each database goes into the snapshot. A database with an open transaction fails
 * the save, also when it is file-backed. A database without a location goes in with its bytes,
 * as a multipart part named after its field, so a field name that a part name cannot hold fails
 * the save. A database with a location goes in with its location only, as a JSON key.
 */
export function planDatabases(
  fields: readonly DatabaseField[],
): { tag: 'ok'; val: DatabasePlan } | { tag: 'err'; val: string } {
  const inMemory: string[] = [];
  const fileDatabases: Array<[string, string]> = [];
  for (const field of fields) {
    if (!field.autocommit) {
      return {
        tag: 'err',
        val: `Cannot snapshot database "${field.name}": an open transaction exists. Commit or rollback before saving.`,
      };
    }
    if (field.location === null) {
      if (PART_NAME_BREAKING_CHARACTERS.test(field.name)) {
        return {
          tag: 'err',
          val: `Cannot snapshot in-memory database ${JSON.stringify(field.name)}: its field name contains a double quote, a carriage return or a line feed, which a multipart part name cannot hold. Rename the field.`,
        };
      }
      inMemory.push(field.name);
    } else {
      fileDatabases.push([field.name, field.location]);
    }
  }
  return { tag: 'ok', val: { inMemory, fileDatabases: Object.fromEntries(fileDatabases) } };
}

/**
 * Takes the SQLite fields out of the state fields of a save. Gives the other fields, a
 * `db:<field>` multipart part for each in-memory database, and the location of each file-backed
 * database. Throws a string when a field holds a `StatementSync`, `Session` or `SQLTagStore`,
 * when two fields hold the same database, when a database is closed, or as `planDatabases`
 * decides. Of a file-backed database it reads only the transaction state and the location.
 */
export function takeDatabases(fields: ReadonlyArray<readonly [string, unknown]>): {
  ordinary: Record<string, unknown>;
  databaseParts: MultipartPart[];
  fileDatabases: Record<string, string>;
} {
  const ordinary: Array<[string, unknown]> = [];
  const databases: Array<[string, DatabaseSync]> = [];
  const seen = new Set<DatabaseSync>();
  for (const [name, val] of fields) {
    if (isDatabaseSync(val)) {
      if (seen.has(val)) {
        throw `Multiple agent fields reference the same DatabaseSync instance (field "${name}").`;
      }
      seen.add(val);
      databases.push([name, val]);
    } else if (isSqliteResource(val)) {
      throw `Cannot automatically snapshot resource field "${name}"; use custom save/load functions.`;
    } else {
      ordinary.push([name, val]);
    }
  }
  const plan = planDatabases(databases.map(([name, db]) => readDatabaseField(name, db)));
  if (plan.tag === 'err') {
    throw plan.val;
  }
  const byName = new Map(databases);
  return {
    ordinary: Object.fromEntries(ordinary),
    databaseParts: plan.val.inMemory.map((name) => ({
      name: `${DATABASE_PART_PREFIX}${name}`,
      contentType: DATABASE_PART_CONTENT_TYPE,
      body: serializeDatabaseSync(byName.get(name)!),
    })),
    fileDatabases: plan.val.fileDatabases,
  };
}

/** Reads what a save needs of the database in field `name`; throws a string that names the field. */
function readDatabaseField(name: string, db: DatabaseSync): DatabaseField {
  try {
    return { name, autocommit: isAutocommitDatabaseSync(db), location: db.location() };
  } catch (error) {
    throw `Cannot snapshot database "${name}": ${error instanceof Error ? error.message : String(error)}`;
  }
}

/**
 * Reads the databases of a loaded snapshot from its multipart parts and the `fileDatabases`
 * field of its JSON envelope. Throws a string, which starts with `description`, when the
 * envelope has no `fileDatabases` field, when the field does not map field names to locations,
 * or when a field is both an in-memory part and a `fileDatabases` entry.
 */
export function decodeSnapshotDatabases(
  parts: readonly MultipartPart[],
  envelope: object,
  description: string,
): SnapshotDatabases {
  if (!Object.hasOwn(envelope, 'fileDatabases')) {
    throw `${description} missing 'fileDatabases' field`;
  }
  const fileDatabases: unknown = (envelope as { fileDatabases: unknown }).fileDatabases;
  if (
    fileDatabases === null ||
    typeof fileDatabases !== 'object' ||
    Array.isArray(fileDatabases) ||
    !Object.values(fileDatabases).every((location) => typeof location === 'string')
  ) {
    throw `${description} 'fileDatabases' must map field names to locations`;
  }
  const inMemory = parts
    .filter((part) => part.name.startsWith(DATABASE_PART_PREFIX))
    .map((part) => ({ name: part.name.slice(DATABASE_PART_PREFIX.length), bytes: part.body }));
  const both = inMemory.find(({ name }) => Object.hasOwn(fileDatabases, name));
  if (both) {
    throw `${description} names database field ${JSON.stringify(both.name)} both in memory and in 'fileDatabases'`;
  }
  return { inMemory, fileDatabases: fileDatabases as Record<string, string> };
}

/** What a load finds in one field of the restored state. */
type LoadedField =
  | { kind: 'empty' }
  | { kind: 'database'; instance: number; open: boolean; location: string | null }
  | { kind: 'other' };

/**
 * The steps of a load, by field name: the databases to open (in memory when `location` is null,
 * else at `location`, whose file must exist), and the databases to warm.
 */
type RestorePlan = {
  open: Array<{ name: string; location: string | null }>;
  warm: Array<{ name: string; allPages: boolean }>;
};

/**
 * Decides how a load puts the snapshot databases into the restored state. A field that holds a
 * database keeps it. An empty field gets a new database: in memory for an in-memory entry, at
 * its location for a file entry, whose file must exist. A field that holds any other value fails
 * the load. Each open database is then warmed once: it reads its schema, and a file-backed one
 * (`allPages`) also reads every page when all its pages stay in its page cache.
 */
export function planRestore(
  fields: ReadonlyArray<readonly [string, LoadedField]>,
  databases: SnapshotDatabases,
): { tag: 'ok'; val: RestorePlan } | { tag: 'err'; val: string } {
  const byName = new Map<string, LoadedField>(fields);
  const names = fields.map(([name]) => name);
  const plan: RestorePlan = { open: [], warm: [] };
  const entries: Array<[string, string | null]> = [
    ...databases.inMemory.map(({ name }): [string, null] => [name, null]),
    ...Object.entries(databases.fileDatabases),
  ];
  for (const [name, location] of entries) {
    const field = byName.get(name) ?? { kind: 'empty' };
    if (field.kind === 'other') {
      return { tag: 'err', val: `snapshot database field "${name}" is not a DatabaseSync` };
    }
    if (field.kind === 'empty') {
      plan.open.push({ name, location });
      byName.set(name, { kind: 'database', instance: -plan.open.length, open: true, location });
      names.push(name);
    }
  }
  const warmed = new Set<number>();
  for (const name of names) {
    const field = byName.get(name);
    if (field?.kind === 'database' && field.open && !warmed.has(field.instance)) {
      warmed.add(field.instance);
      plan.warm.push({ name, allPages: field.location !== null });
    }
  }
  return { tag: 'ok', val: plan };
}

/**
 * The error of a load that must open a file-backed database at a location where no file exists,
 * given the locations of `plan.open` that exist; null when every such file exists.
 */
export function missingDatabaseFile(
  plan: RestorePlan,
  existing: ReadonlySet<string>,
): string | null {
  const missing = plan.open.find(({ location }) => location !== null && !existing.has(location));
  return missing
    ? `snapshot database field "${missing.name}": no database file at ${missing.location}`
    : null;
}

/**
 * Puts the snapshot databases into `state` as `planRestore` decides, and fails the load as
 * `missingDatabaseFile` decides. Restores the bytes of each in-memory entry into its database,
 * then warms the databases of the plan. A database to read in full reads every page only when
 * `fitsInPageCache` says that all its pages stay in the page cache of its connection; a larger one
 * reads its schema only, because the cache cannot hold its pages. The warm-up makes the first
 * recorded statements after the load match a live connection that holds its pages in cache;
 * otherwise snapshot recovery falls back to an older snapshot or a full replay.
 */
export function restoreDatabases(state: Record<string, unknown>, databases: SnapshotDatabases) {
  const instances = new Map<DatabaseSync, number>();
  const plan = planRestore(
    Object.entries(state).map(([name, value]) => [name, loadedField(value, instances)]),
    databases,
  );
  if (plan.tag === 'err') {
    throw new Error(plan.val);
  }
  const missing = missingDatabaseFile(
    plan.val,
    new Set(
      plan.val.open.flatMap(({ location }) =>
        location !== null && existsSync(location) ? [location] : [],
      ),
    ),
  );
  if (missing !== null) {
    throw new Error(missing);
  }
  for (const { name, location } of plan.val.open) {
    setField(
      state,
      name,
      new DatabaseSync(location ?? IN_MEMORY_LOCATION, REOPENED_DATABASE_OPTIONS),
    );
  }
  for (const { name, bytes } of databases.inMemory) {
    restoreDatabaseSync(state[name] as DatabaseSync, bytes);
  }
  for (const { name, allPages } of plan.val.warm) {
    const database = state[name] as DatabaseSync;
    database.prepare('SELECT count(*) FROM sqlite_master').get();
    if (allPages && fitsInPageCache(pageCacheOf(database))) {
      serializeDatabaseSync(database);
    }
  }
}

/** The page cache of a connection, as its `PRAGMA`s report it. */
type PageCache = {
  /** `PRAGMA page_count`: the number of pages of the database. */
  pageCount: number;
  /** `PRAGMA page_size`: the size of a page in bytes. */
  pageSize: number;
  /** `PRAGMA cache_size`: a limit in KiB when negative, a number of pages when positive. */
  cacheSize: number;
};

/**
 * The bytes that SQLite's page cache keeps with each page besides the page itself, in the
 * wasm32 build of the `node:sqlite` builtin: `ROUND8(sizeof(MemPage))` from `btree.c`. The
 * `MemPage` struct has 36 bytes of integer fields and 12 pointers, 84 bytes on a target with
 * 4-byte pointers, rounded up to 88. A 64-bit build keeps 136 bytes.
 */
const SQLITE_PAGE_CACHE_EXTRA_BYTES = 88;

/**
 * Whether every page of a database stays in the page cache of its connection after one read of
 * all pages. SQLite turns a negative `cache_size` of N KiB into a limit of
 * `floor(N * 1024 / (page_size + extra))` pages and a positive one into that many pages, and it
 * recycles a page before the cache reaches the limit, so it holds at most the limit minus one.
 */
export function fitsInPageCache({ pageCount, pageSize, cacheSize }: PageCache): boolean {
  const limit =
    cacheSize < 0
      ? Math.floor((-cacheSize * 1024) / (pageSize + SQLITE_PAGE_CACHE_EXTRA_BYTES))
      : cacheSize;
  return pageCount <= limit - 1;
}

function pageCacheOf(database: DatabaseSync): PageCache {
  return {
    pageCount: pragmaNumber(database, 'page_count'),
    pageSize: pragmaNumber(database, 'page_size'),
    cacheSize: pragmaNumber(database, 'cache_size'),
  };
}

function pragmaNumber(database: DatabaseSync, name: string): number {
  const row = database.prepare(`PRAGMA ${name}`).get() as Record<string, unknown> | undefined;
  return Number(row?.[name]);
}

/**
 * Sets the own field `name` of `state`, also for a name such as `__proto__`, which an assignment
 * would take as the prototype.
 */
function setField(state: Record<string, unknown>, name: string, value: unknown) {
  Object.defineProperty(state, name, {
    value,
    writable: true,
    enumerable: true,
    configurable: true,
  });
}

function loadedField(value: unknown, instances: Map<DatabaseSync, number>): LoadedField {
  if (value === undefined) {
    return { kind: 'empty' };
  }
  if (!isDatabaseSync(value)) {
    return { kind: 'other' };
  }
  const instance = instances.get(value) ?? instances.size;
  instances.set(value, instance);
  return {
    kind: 'database',
    instance,
    open: value.isOpen,
    location: value.isOpen ? value.location() : null,
  };
}
