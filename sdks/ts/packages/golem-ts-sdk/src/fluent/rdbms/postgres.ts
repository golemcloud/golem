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

// Plain-async (no Effect) fluent wrapper around the
// `golem:rdbms/postgres@1.5.0` host interface. Ported from effect-golem's
// `Postgres/Pg.ts` + `Postgres/PgClient.ts` + `host/PostgresHostClient.ts`,
// de-Effect-ified: every operation returns a `Promise` and throws a typed
// {@link PostgresError} instead of failing an Effect. The db-value <-> JS codec
// lives in `./shared` (host-import-free, so it round-trips under node tests).

import {
  DbConnection,
  type DbResult,
  type DbValue,
  LazyDbValue,
} from 'golem:rdbms/postgres@1.5.0';
import {
  type DbColumnLike,
  type DbRowLike,
  decodeRows,
  decodeRowsValues,
  makeWrap,
  type PgBound,
  pgDecodeDbValue,
  pgEncodeDbValue,
  type PgRange,
  type PgRangeElementHint,
  pgParam,
  type PgSparseVec,
  RdbmsError,
  setPgLazyCtor,
  type TemporalDecodeMode,
  isReader as sqlIsReader,
} from './shared';

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/** Raised when any `golem:rdbms/postgres@1.5.0` host call traps or a parameter
 * cannot be encoded. See {@link RdbmsError} for `reason` / `trace`. */
export class PostgresError extends RdbmsError {
  override readonly name = 'PostgresError';
  constructor(cause: unknown, operation: string) {
    super(cause, operation, 'Postgres');
  }
}

const wrap = makeWrap('Postgres', PostgresError);

// Wire the host's `LazyDbValue` so nested array/composite/domain params are
// wrapped in the real resource (the codec falls back to a plain shim when this
// is unset, e.g. under node tests).
setPgLazyCtor(<T>(v: T) => new LazyDbValue(v as unknown as DbValue) as unknown as { get(): T });

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/** A decoded result set: column metadata + decoded rows (as records). */
export interface PgResultSet {
  /** Column metadata, in result order. */
  readonly columns: ReadonlyArray<{
    readonly ordinal: bigint;
    readonly name: string;
    readonly dbTypeName: string;
  }>;
  /** Rows decoded to `{ columnName: value }` records. */
  readonly rows: ReadonlyArray<Record<string, unknown>>;
}

const encodeParams = (params: ReadonlyArray<unknown>): DbValue[] =>
  params.map((p) => pgEncodeDbValue(p) as DbValue);

const toResultSet = (result: DbResult, mode: TemporalDecodeMode): PgResultSet => ({
  columns: result.columns.map((c) => ({
    ordinal: c.ordinal,
    name: c.name,
    dbTypeName: c.dbTypeName,
  })),
  rows: decodeRows(
    result.rows as ReadonlyArray<DbRowLike<DbValue>>,
    result.columns as ReadonlyArray<DbColumnLike>,
    pgDecodeDbValue,
    mode,
  ),
});

// ---------------------------------------------------------------------------
// Query target abstraction (connection vs transaction share the same surface)
// ---------------------------------------------------------------------------

interface QueryTarget {
  query(statement: string, params: DbValue[]): DbResult;
  execute(statement: string, params: DbValue[]): bigint;
}

const makeQueryApi = (target: QueryTarget, mode: TemporalDecodeMode) => ({
  query(sql: string, params: ReadonlyArray<unknown> = []): PgResultSet {
    return wrap('query', () => toResultSet(target.query(sql, encodeParams(params)), mode));
  },
  queryValues(sql: string, params: ReadonlyArray<unknown> = []): Array<Array<unknown>> {
    return wrap('queryValues', () => {
      const result = target.query(sql, encodeParams(params));
      return decodeRowsValues(
        result.rows as ReadonlyArray<DbRowLike<DbValue>>,
        pgDecodeDbValue,
        mode,
      );
    });
  },
  execute(sql: string, params: ReadonlyArray<unknown> = []): number {
    return wrap('execute', () => Number(target.execute(sql, encodeParams(params))));
  },
  /** Route by SQL shape: readers → `query`, writers → `execute` (rows affected). */
  run(sql: string, params: ReadonlyArray<unknown> = []): PgResultSet | number {
    return sqlIsReader(sql) ? this.query(sql, params) : this.execute(sql, params);
  },
});

// ---------------------------------------------------------------------------
// Transaction
// ---------------------------------------------------------------------------

/** An open postgres transaction. `commit()` / `rollback()` finalize it. */
export interface PgTransaction {
  query(sql: string, params?: ReadonlyArray<unknown>): PgResultSet;
  queryValues(sql: string, params?: ReadonlyArray<unknown>): Array<Array<unknown>>;
  execute(sql: string, params?: ReadonlyArray<unknown>): number;
  run(sql: string, params?: ReadonlyArray<unknown>): PgResultSet | number;
  commit(): Promise<void>;
  rollback(): Promise<void>;
}

// ---------------------------------------------------------------------------
// Connection
// ---------------------------------------------------------------------------

/** An open postgres connection. The underlying WIT `DbConnection` resource has
 * no explicit close; {@link PgConnection.close} drops the JS handle (GC-managed
 * lifetime) so callers have an explicit lifecycle hook. */
export interface PgConnection {
  /** Run a row-returning statement (`SELECT`, `RETURNING`, …). */
  query(sql: string, params?: ReadonlyArray<unknown>): Promise<PgResultSet>;
  /** Like {@link query} but returns positional value arrays (no column names). */
  queryValues(sql: string, params?: ReadonlyArray<unknown>): Promise<Array<Array<unknown>>>;
  /** Run a non-row statement and return the affected-row count. */
  execute(sql: string, params?: ReadonlyArray<unknown>): Promise<number>;
  /** Begin a transaction; returns a handle you must `commit()`/`rollback()`. */
  begin(): Promise<PgTransaction>;
  /**
   * Run `fn` inside a transaction: commits on success, rolls back if `fn`
   * throws (re-throwing the original error).
   */
  transaction<A>(fn: (tx: PgTransaction) => Promise<A> | A): Promise<A>;
  /** Drop the connection handle (GC-managed; provided for symmetry). */
  close(): void;
}

const makeTransaction = (
  tx: { query: QueryTarget['query']; execute: QueryTarget['execute']; commit(): void; rollback(): void },
  mode: TemporalDecodeMode,
): PgTransaction => {
  const api = makeQueryApi(tx, mode);
  return {
    query: (sql, params) => api.query(sql, params),
    queryValues: (sql, params) => api.queryValues(sql, params),
    execute: (sql, params) => api.execute(sql, params),
    run: (sql, params) => api.run(sql, params),
    async commit() {
      wrap('commit', () => tx.commit());
    },
    async rollback() {
      wrap('rollback', () => tx.rollback());
    },
  };
};

const makeConnection = (db: DbConnection, mode: TemporalDecodeMode): PgConnection => {
  const api = makeQueryApi(db, mode);
  const begin = (): PgTransaction => {
    const tx = wrap('beginTransaction', () => db.beginTransaction());
    return makeTransaction(tx, mode);
  };
  return {
    async query(sql, params) {
      return api.query(sql, params);
    },
    async queryValues(sql, params) {
      return api.queryValues(sql, params);
    },
    async execute(sql, params) {
      return api.execute(sql, params);
    },
    async begin() {
      return begin();
    },
    async transaction(fn) {
      const tx = begin();
      try {
        const out = await fn(tx);
        await tx.commit();
        return out;
      } catch (e) {
        try {
          await tx.rollback();
        } catch {
          /* prefer surfacing the original error */
        }
        throw e;
      }
    },
    close() {
      /* GC-managed: dropping the handle is enough. */
    },
  };
};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/** Options for {@link Postgres.open}. */
export interface PgOpenOptions {
  /** How temporal values decode from rows. Defaults to `"raw"`. */
  readonly decodeTemporal?: TemporalDecodeMode;
}

/**
 * Postgres entry point. `open` acquires a connection via the host's
 * `DbConnection.open(address)`; the remaining rich-type param helpers live on
 * the {@link Pg} namespace.
 */
export const Postgres = {
  /**
   * Open a connection to `url` (e.g. `postgres://user:pass@host:5432/db`).
   * Async for API ergonomics although the host call is synchronous. Failures
   * throw {@link PostgresError}.
   */
  async open(url: string, options: PgOpenOptions = {}): Promise<PgConnection> {
    const db = wrap('open', () => DbConnection.open(url));
    return makeConnection(db, options.decodeTemporal ?? 'raw');
  },
};

// ---------------------------------------------------------------------------
// Rich-type parameter helpers
// ---------------------------------------------------------------------------

/**
 * Explicit parameter wrappers for rich Postgres types. Use to override the
 * conservative default JS mapping, e.g. `conn.query(sql, [Pg.uuid(id),
 * Pg.jsonb({ a: 1 })])`.
 */
export const Pg = {
  json: (value: unknown) => pgParam('json', value),
  jsonb: (value: unknown) => pgParam('jsonb', value),
  jsonpath: (value: string) => pgParam('jsonpath', value),
  xml: (value: string) => pgParam('xml', value),
  uuid: (value: string | { highBits: bigint; lowBits: bigint }) => pgParam('uuid', value),
  array: (value: ReadonlyArray<unknown>) => pgParam('array', value),
  range: (value: PgRange<unknown>, hint: PgRangeElementHint) =>
    pgParam('range', { range: value, hint }),
  composite: (name: string, values: ReadonlyArray<unknown>) =>
    pgParam('composite', { name, values }),
  domain: (name: string, value: unknown) => pgParam('domain', { name, value }),
  enumeration: (name: string, value: string) => pgParam('enumeration', { name, value }),
  vector: (value: ReadonlyArray<number>) => pgParam('vector', value),
  halfvec: (value: ReadonlyArray<number>) => pgParam('halfvec', value),
  sparsevec: (value: PgSparseVec) => pgParam('sparsevec', value),
  numeric: (value: string) => pgParam('numeric', value),
  interval: (value: { months: number; days: number; microseconds: bigint }) =>
    pgParam('interval', value),
  inet: (value: unknown) => pgParam('inet', value),
  cidr: (value: unknown) => pgParam('cidr', value),
  macaddr: (value: unknown) => pgParam('macaddr', value),
  bit: (value: ReadonlyArray<boolean>) => pgParam('bit', value),
  varbit: (value: ReadonlyArray<boolean>) => pgParam('varbit', value),
  int2: (value: number) => pgParam('int2', value),
  int4: (value: number) => pgParam('int4', value),
  int8: (value: number | bigint) => pgParam('int8', value),
  float4: (value: number) => pgParam('float4', value),
  float8: (value: number) => pgParam('float8', value),
  text: (value: string) => pgParam('text', value),
  varchar: (value: string) => pgParam('varchar', value),
  bpchar: (value: string) => pgParam('bpchar', value),
  character: (value: number) => pgParam('character', value),
  oid: (value: number) => pgParam('oid', value),
  money: (value: bigint) => pgParam('money', value),
  bytea: (value: Uint8Array) => pgParam('bytea', value),
  timestamp: (value: unknown) => pgParam('timestamp', value),
  timestamptz: (value: unknown) => pgParam('timestamptz', value),
};

export type { PgBound, PgRange, PgRangeElementHint, PgSparseVec };
