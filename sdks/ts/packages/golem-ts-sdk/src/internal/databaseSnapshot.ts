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
import { existsSync } from './fileSystem';
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
 * the save, also when it is file-backed. A database without a location goes in with its bytes.
 * A database with a location goes in with its location only.
 */
export function planDatabases(
  fields: readonly DatabaseField[],
): { tag: 'ok'; val: DatabasePlan } | { tag: 'err'; val: string } {
  const plan: DatabasePlan = { inMemory: [], fileDatabases: {} };
  for (const field of fields) {
    if (!field.autocommit) {
      return {
        tag: 'err',
        val: `Cannot snapshot database "${field.name}": an open transaction exists. Commit or rollback before saving.`,
      };
    }
    if (field.location === null) {
      plan.inMemory.push(field.name);
    } else {
      plan.fileDatabases[field.name] = field.location;
    }
  }
  return { tag: 'ok', val: plan };
}

/**
 * Takes the SQLite fields out of the state fields of a save. Gives the other fields, a
 * `db:<field>` multipart part for each in-memory database, and the location of each file-backed
 * database. Throws a string when a field holds a `StatementSync`, `Session` or `SQLTagStore`,
 * when two fields hold the same database, or when a database has an open transaction. Of a
 * file-backed database it reads only the transaction state and the location.
 */
export function takeDatabases(fields: ReadonlyArray<readonly [string, unknown]>): {
  ordinary: Record<string, unknown>;
  databaseParts: MultipartPart[];
  fileDatabases: Record<string, string>;
} {
  const ordinary: Record<string, unknown> = {};
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
      ordinary[name] = val;
    }
  }
  const plan = planDatabases(
    databases.map(([name, db]) => ({
      name,
      autocommit: isAutocommitDatabaseSync(db),
      location: db.location(),
    })),
  );
  if (plan.tag === 'err') {
    throw plan.val;
  }
  const byName = new Map(databases);
  return {
    ordinary,
    databaseParts: plan.val.inMemory.map((name) => ({
      name: `${DATABASE_PART_PREFIX}${name}`,
      contentType: DATABASE_PART_CONTENT_TYPE,
      body: serializeDatabaseSync(byName.get(name)!),
    })),
    fileDatabases: plan.val.fileDatabases,
  };
}

/**
 * Reads the databases of a loaded snapshot from its multipart parts and the `fileDatabases`
 * field of its JSON envelope. Throws a string, which starts with `description`, when the
 * envelope has no `fileDatabases` field or when the field does not map field names to locations.
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
  return {
    inMemory: parts
      .filter((part) => part.name.startsWith(DATABASE_PART_PREFIX))
      .map((part) => ({ name: part.name.slice(DATABASE_PART_PREFIX.length), bytes: part.body })),
    fileDatabases: fileDatabases as Record<string, string>,
  };
}

/** What a load finds in one field of the restored state. */
type LoadedField =
  | { kind: 'empty' }
  | { kind: 'database'; instance: number; open: boolean; location: string | null }
  | { kind: 'other' };

/**
 * The steps of a load, by field name: the locations whose files must exist, the databases to
 * open (in memory when `location` is null), and the databases to warm.
 */
type RestorePlan = {
  check: string[];
  open: Array<{ name: string; location: string | null }>;
  warm: Array<{ name: string; allPages: boolean }>;
};

/**
 * Decides how a load puts the snapshot databases into the restored state. A field that holds a
 * database keeps it. An empty field gets a new database: in memory for an in-memory entry, at
 * its location for a file entry, whose file must exist. A field that holds any other value fails
 * the load. Each open database is then warmed once: it reads its schema and, when file-backed,
 * every page.
 */
export function planRestore(
  fields: ReadonlyArray<readonly [string, LoadedField]>,
  databases: SnapshotDatabases,
): { tag: 'ok'; val: RestorePlan } | { tag: 'err'; val: string } {
  const byName = new Map<string, LoadedField>(fields);
  const names = fields.map(([name]) => name);
  const plan: RestorePlan = { check: [], open: [], warm: [] };
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
      if (location !== null) {
        plan.check.push(location);
      }
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
 * given the locations of `plan.check` that exist; null when every such file exists.
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
 * then warms the databases of the plan. The warm-up makes the first
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
    new Set(plan.val.check.filter((location) => existsSync(location))),
  );
  if (missing !== null) {
    throw new Error(missing);
  }
  for (const { name, location } of plan.val.open) {
    state[name] = new DatabaseSync(location ?? IN_MEMORY_LOCATION, REOPENED_DATABASE_OPTIONS);
  }
  for (const { name, bytes } of databases.inMemory) {
    restoreDatabaseSync(state[name] as DatabaseSync, bytes);
  }
  for (const { name, allPages } of plan.val.warm) {
    const database = state[name] as DatabaseSync;
    database.prepare('SELECT count(*) FROM sqlite_master').get();
    if (allPages) {
      serializeDatabaseSync(database);
    }
  }
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
