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
 * Reads the databases of a loaded snapshot from its multipart parts and its `fileDatabases`
 * value. Throws a string, which starts with `description`, when `fileDatabases` does not map
 * field names to locations.
 */
export function decodeSnapshotDatabases(
  parts: readonly MultipartPart[],
  fileDatabases: unknown,
  description: string,
): SnapshotDatabases {
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

/**
 * Puts the snapshot databases into `state`. An entry whose field is empty gets a new database,
 * in memory or opened at its location, and the bytes of each in-memory entry are restored into
 * its database. Each open database then reads its schema and, when file-backed, every page, so
 * that the first recorded statements after the load match a live connection that holds its pages
 * in cache; otherwise snapshot recovery falls back to a full replay.
 */
export function restoreDatabases(state: Record<string, unknown>, databases: SnapshotDatabases) {
  for (const { name, bytes } of databases.inMemory) {
    restoreDatabaseSync(databaseAt(state, name, IN_MEMORY_LOCATION), bytes);
  }
  for (const [name, location] of Object.entries(databases.fileDatabases)) {
    databaseAt(state, name, location);
  }
  const warmed = new Set<DatabaseSync>();
  for (const value of Object.values(state)) {
    if (isDatabaseSync(value) && value.isOpen && !warmed.has(value)) {
      warmed.add(value);
      value.prepare('SELECT count(*) FROM sqlite_master').get();
      if (value.location() !== null) {
        serializeDatabaseSync(value);
      }
    }
  }
}

function databaseAt(state: Record<string, unknown>, name: string, location: string): DatabaseSync {
  let target = state[name];
  if (target === undefined) {
    target = new DatabaseSync(location, REOPENED_DATABASE_OPTIONS);
    state[name] = target;
  }
  if (!isDatabaseSync(target)) {
    throw new Error(`snapshot database field "${name}" is not a DatabaseSync`);
  }
  return target;
}
