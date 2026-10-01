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
} from './sqlite';

/** The SQLite databases of a typed snapshot, keyed by the agent field that holds each one. */
export type SnapshotDatabases = {
  /** The bytes of each in-memory or temporary database. */
  inMemory: Array<{ name: string; bytes: Uint8Array }>;
  /** The location of each file-backed database, as `DatabaseSync.location()` reports it. */
  files: Record<string, string>;
};

/** What a save reads from one `DatabaseSync` field. */
export type DatabaseField = {
  name: string;
  /** False when the connection has an open transaction. */
  autocommit: boolean;
  /** `DatabaseSync.location()`: null for an in-memory or temporary database. */
  location: string | null;
};

/** Which databases go into the snapshot as bytes, and which by their location. */
export type DatabasePlan = {
  inMemory: string[];
  files: Record<string, string>;
};

/**
 * The open options of a database that a load creates. These are the options that the
 * `node:sqlite` builtin reads, each with the value that the builtin uses when it is not given.
 */
export const REOPENED_DATABASE_OPTIONS = {
  open: true,
  readOnly: false,
  enableForeignKeyConstraints: true,
  enableDoubleQuotedStringLiterals: false,
  timeout: 0,
  defensive: true,
  readBigInts: false,
  returnArrays: false,
  allowBareNamedParameters: true,
  allowUnknownNamedParameters: false,
} as const;

const IN_MEMORY_LOCATION = ':memory:';

export function isDatabaseSync(val: unknown): val is DatabaseSync {
  return val instanceof DatabaseSync;
}

/**
 * Decides how each database goes into the snapshot. A database with an open transaction fails
 * the save, also when it is file-backed. A database without a location goes in with its bytes.
 * A database with a location goes in with its location only.
 */
export function planDatabases(
  fields: readonly DatabaseField[],
): { tag: 'ok'; val: DatabasePlan } | { tag: 'err'; val: string } {
  const plan: DatabasePlan = { inMemory: [], files: {} };
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
      plan.files[field.name] = field.location;
    }
  }
  return { tag: 'ok', val: plan };
}

/**
 * Reads the databases of a save and returns what the snapshot holds for them. Throws a string
 * when a database has an open transaction. Of a file-backed database it reads only the
 * transaction state and the location.
 */
export function takeDatabases(
  databases: ReadonlyArray<readonly [string, DatabaseSync]>,
): SnapshotDatabases {
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
    inMemory: plan.val.inMemory.map((name) => ({
      name,
      bytes: serializeDatabaseSync(byName.get(name)!),
    })),
    files: plan.val.files,
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
  for (const [name, location] of Object.entries(databases.files)) {
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
