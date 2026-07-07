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

// Plain-async fluent wrapper around the `golem:rdbms/ignite2@1.5.0` host
// interface. Every operation returns a `Promise` and throws a typed error on
// failure. The db-value <-> JS codec lives in `./shared`.

import { DbConnection, type DbResult, type DbValue } from 'golem:rdbms/ignite2@1.5.0';
import {
  type DbColumnLike,
  type DbRowLike,
  decodeRows,
  decodeRowsValues,
  igniteDecodeDbValue,
  igniteEncodeDbValue,
  igniteParam,
  type IgniteUuid,
  makeWrap,
  RdbmsError,
  type TemporalDecodeMode,
  isReader as sqlIsReader,
} from './shared';

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/** Raised when any `golem:rdbms/ignite2@1.5.0` host call traps or a parameter
 * cannot be encoded. See {@link RdbmsError} for `reason` / `trace`. */
export class IgniteError extends RdbmsError {
  override readonly name = 'IgniteError';
  constructor(cause: unknown, operation: string) {
    super(cause, operation, 'Ignite');
  }
}

const wrap = makeWrap('Ignite', IgniteError);

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/** A decoded result set. Ignite columns carry only `ordinal` + `name`. */
export interface IgniteResultSet {
  readonly columns: ReadonlyArray<{ readonly ordinal: bigint; readonly name: string }>;
  readonly rows: ReadonlyArray<Record<string, unknown>>;
}

const encodeParams = (params: ReadonlyArray<unknown>): DbValue[] =>
  params.map((p) => igniteEncodeDbValue(p) as DbValue);

const toResultSet = (result: DbResult, mode: TemporalDecodeMode): IgniteResultSet => ({
  columns: result.columns.map((c) => ({ ordinal: c.ordinal, name: c.name })),
  rows: decodeRows(
    result.rows as ReadonlyArray<DbRowLike<DbValue>>,
    result.columns as ReadonlyArray<DbColumnLike>,
    igniteDecodeDbValue,
    mode,
  ),
});

interface QueryTarget {
  query(statement: string, params: DbValue[]): DbResult;
  execute(statement: string, params: DbValue[]): bigint;
}

const makeQueryApi = (target: QueryTarget, mode: TemporalDecodeMode) => ({
  query(sql: string, params: ReadonlyArray<unknown> = []): IgniteResultSet {
    return wrap('query', () => toResultSet(target.query(sql, encodeParams(params)), mode));
  },
  queryValues(sql: string, params: ReadonlyArray<unknown> = []): Array<Array<unknown>> {
    return wrap('queryValues', () =>
      decodeRowsValues(
        target.query(sql, encodeParams(params)).rows as ReadonlyArray<DbRowLike<DbValue>>,
        igniteDecodeDbValue,
        mode,
      ),
    );
  },
  execute(sql: string, params: ReadonlyArray<unknown> = []): number {
    return wrap('execute', () => Number(target.execute(sql, encodeParams(params))));
  },
  run(sql: string, params: ReadonlyArray<unknown> = []): IgniteResultSet | number {
    return sqlIsReader(sql) ? this.query(sql, params) : this.execute(sql, params);
  },
});

// ---------------------------------------------------------------------------
// Transaction + connection
// ---------------------------------------------------------------------------

/** An open ignite transaction. */
export interface IgniteTransaction {
  query(sql: string, params?: ReadonlyArray<unknown>): IgniteResultSet;
  queryValues(sql: string, params?: ReadonlyArray<unknown>): Array<Array<unknown>>;
  execute(sql: string, params?: ReadonlyArray<unknown>): number;
  run(sql: string, params?: ReadonlyArray<unknown>): IgniteResultSet | number;
  commit(): Promise<void>;
  rollback(): Promise<void>;
}

/** An open ignite connection. */
export interface IgniteConnection {
  query(sql: string, params?: ReadonlyArray<unknown>): Promise<IgniteResultSet>;
  queryValues(sql: string, params?: ReadonlyArray<unknown>): Promise<Array<Array<unknown>>>;
  execute(sql: string, params?: ReadonlyArray<unknown>): Promise<number>;
  begin(): Promise<IgniteTransaction>;
  transaction<A>(fn: (tx: IgniteTransaction) => Promise<A> | A): Promise<A>;
  close(): void;
}

const makeTransaction = (
  tx: {
    query: QueryTarget['query'];
    execute: QueryTarget['execute'];
    commit(): void;
    rollback(): void;
  },
  mode: TemporalDecodeMode,
): IgniteTransaction => {
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

const makeConnection = (db: DbConnection, mode: TemporalDecodeMode): IgniteConnection => {
  const api = makeQueryApi(db, mode);
  const begin = (): IgniteTransaction =>
    makeTransaction(
      wrap('beginTransaction', () => db.beginTransaction()),
      mode,
    );
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
      /* GC-managed. */
    },
  };
};

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/** Options for {@link Ignite.open}. */
export interface IgniteOpenOptions {
  readonly decodeTemporal?: TemporalDecodeMode;
}

/** Apache Ignite 2.x entry point + rich-type param helpers. */
export const Ignite = {
  /**
   * Open a connection to `url`
   * (`ignite://[user:pass@]host:port[?pool_size=N&tls=true]`, default port
   * 10800). Async for ergonomics although the host call is synchronous. Throws
   * {@link IgniteError}.
   */
  async open(url: string, options: IgniteOpenOptions = {}): Promise<IgniteConnection> {
    const db = wrap('open', () => DbConnection.open(url));
    return makeConnection(db, options.decodeTemporal ?? 'raw');
  },

  uuid: (value: IgniteUuid) => igniteParam('uuid', value),
  decimal: (value: string) => igniteParam('decimal', value),
  date: (value: bigint | number | Date) => igniteParam('date', value),
  timestamp: (millis: bigint | number, subMilliNanos: number) =>
    igniteParam('timestamp', { millis, subMilliNanos }),
  time: (nanos: bigint | number) => igniteParam('time', nanos),
  char: (codeUnit: number) => igniteParam('char', codeUnit),
  byteArray: (value: Uint8Array) => igniteParam('byte-array', value),
  byte: (value: number) => igniteParam('byte', value),
  short: (value: number) => igniteParam('short', value),
  int: (value: number) => igniteParam('int', value),
  long: (value: number | bigint) => igniteParam('long', value),
  float: (value: number) => igniteParam('float', value),
  double: (value: number) => igniteParam('double', value),
  string: (value: string) => igniteParam('string', value),
  boolean: (value: boolean) => igniteParam('boolean', value),
};

export type { IgniteUuid };
