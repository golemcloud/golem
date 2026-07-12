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

// Plain-async fluent wrapper around the `golem:rdbms/mysql@1.5.0` host
// interface. Every operation returns a `Promise` and throws a typed error on
// failure. The db-value <-> JS codec lives in `./shared`.

import { DbConnection, type DbResult, type DbValue } from 'golem:rdbms/mysql@1.5.0';
import {
  type DbColumnLike,
  type DbRowLike,
  decodeRows,
  decodeRowsValues,
  makeWrap,
  mysqlDecodeDbValue,
  mysqlEncodeDbValue,
  mysqlParam,
  RdbmsError,
  type TemporalDecodeMode,
  isReader as sqlIsReader,
} from './shared';

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/** Raised when any `golem:rdbms/mysql@1.5.0` host call traps or a parameter
 * cannot be encoded. See {@link RdbmsError} for `reason` / `trace`. */
export class MySqlError extends RdbmsError {
  override readonly name = 'MySqlError';
  constructor(cause: unknown, operation: string) {
    super(cause, operation, 'MySql');
  }
}

const wrap = makeWrap('MySql', MySqlError);

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

/** A decoded result set: column metadata + decoded rows (as records). */
export interface MySqlResultSet {
  readonly columns: ReadonlyArray<{
    readonly ordinal: bigint;
    readonly name: string;
    readonly dbTypeName: string;
  }>;
  readonly rows: ReadonlyArray<Record<string, unknown>>;
}

const encodeParams = (params: ReadonlyArray<unknown>): DbValue[] =>
  params.map((p) => mysqlEncodeDbValue(p) as DbValue);

const toResultSet = (result: DbResult, mode: TemporalDecodeMode): MySqlResultSet => ({
  columns: result.columns.map((c) => ({
    ordinal: c.ordinal,
    name: c.name,
    dbTypeName: c.dbTypeName,
  })),
  rows: decodeRows(
    result.rows as ReadonlyArray<DbRowLike<DbValue>>,
    result.columns as ReadonlyArray<DbColumnLike>,
    mysqlDecodeDbValue,
    mode,
  ),
});

interface QueryTarget {
  query(statement: string, params: DbValue[]): DbResult;
  execute(statement: string, params: DbValue[]): bigint;
}

const makeQueryApi = (target: QueryTarget, mode: TemporalDecodeMode) => ({
  query(sql: string, params: ReadonlyArray<unknown> = []): MySqlResultSet {
    return wrap('query', () => toResultSet(target.query(sql, encodeParams(params)), mode));
  },
  queryValues(sql: string, params: ReadonlyArray<unknown> = []): Array<Array<unknown>> {
    return wrap('queryValues', () =>
      decodeRowsValues(
        target.query(sql, encodeParams(params)).rows as ReadonlyArray<DbRowLike<DbValue>>,
        mysqlDecodeDbValue,
        mode,
      ),
    );
  },
  execute(sql: string, params: ReadonlyArray<unknown> = []): number {
    return wrap('execute', () => Number(target.execute(sql, encodeParams(params))));
  },
  run(sql: string, params: ReadonlyArray<unknown> = []): MySqlResultSet | number {
    return sqlIsReader(sql) ? this.query(sql, params) : this.execute(sql, params);
  },
});

// ---------------------------------------------------------------------------
// Transaction + connection
// ---------------------------------------------------------------------------

/** An open mysql transaction. */
export interface MySqlTransaction {
  query(sql: string, params?: ReadonlyArray<unknown>): MySqlResultSet;
  queryValues(sql: string, params?: ReadonlyArray<unknown>): Array<Array<unknown>>;
  execute(sql: string, params?: ReadonlyArray<unknown>): number;
  run(sql: string, params?: ReadonlyArray<unknown>): MySqlResultSet | number;
  commit(): Promise<void>;
  rollback(): Promise<void>;
}

/** An open mysql connection. */
export interface MySqlConnection {
  query(sql: string, params?: ReadonlyArray<unknown>): Promise<MySqlResultSet>;
  queryValues(sql: string, params?: ReadonlyArray<unknown>): Promise<Array<Array<unknown>>>;
  execute(sql: string, params?: ReadonlyArray<unknown>): Promise<number>;
  begin(): Promise<MySqlTransaction>;
  transaction<A>(fn: (tx: MySqlTransaction) => Promise<A> | A): Promise<A>;
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
): MySqlTransaction => {
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

const makeConnection = (db: DbConnection, mode: TemporalDecodeMode): MySqlConnection => {
  const api = makeQueryApi(db, mode);
  const begin = (): MySqlTransaction =>
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

/** Options for {@link MySql.open}. */
export interface MySqlOpenOptions {
  readonly decodeTemporal?: TemporalDecodeMode;
}

/** MySQL entry point + rich-type param helpers. */
export const MySql = {
  /**
   * Open a connection to `url` (e.g. `mysql://user:pass@host:3306/db`). Async
   * for ergonomics although the host call is synchronous. Throws
   * {@link MySqlError}.
   */
  async open(url: string, options: MySqlOpenOptions = {}): Promise<MySqlConnection> {
    const db = wrap('open', () => DbConnection.open(url));
    return makeConnection(db, options.decodeTemporal ?? 'raw');
  },

  json: (value: unknown) => mysqlParam('json', value),
  enumeration: (value: string) => mysqlParam('enumeration', value),
  set: (value: string) => mysqlParam('set', value),
  bit: (value: ReadonlyArray<boolean>) => mysqlParam('bit', value),
  decimal: (value: string) => mysqlParam('decimal', value),
  year: (value: number) => mysqlParam('year', value),
  tinyint: (value: number) => mysqlParam('tinyint', value),
  smallint: (value: number) => mysqlParam('smallint', value),
  mediumint: (value: number) => mysqlParam('mediumint', value),
  int: (value: number) => mysqlParam('int', value),
  bigint: (value: number | bigint) => mysqlParam('bigint', value),
  tinyintUnsigned: (value: number) => mysqlParam('tinyint-unsigned', value),
  smallintUnsigned: (value: number) => mysqlParam('smallint-unsigned', value),
  mediumintUnsigned: (value: number) => mysqlParam('mediumint-unsigned', value),
  intUnsigned: (value: number) => mysqlParam('int-unsigned', value),
  bigintUnsigned: (value: number | bigint) => mysqlParam('bigint-unsigned', value),
  float: (value: number) => mysqlParam('float', value),
  double: (value: number) => mysqlParam('double', value),
  fixchar: (value: string) => mysqlParam('fixchar', value),
  varchar: (value: string) => mysqlParam('varchar', value),
  tinytext: (value: string) => mysqlParam('tinytext', value),
  text: (value: string) => mysqlParam('text', value),
  mediumtext: (value: string) => mysqlParam('mediumtext', value),
  longtext: (value: string) => mysqlParam('longtext', value),
  binary: (value: Uint8Array) => mysqlParam('binary', value),
  varbinary: (value: Uint8Array) => mysqlParam('varbinary', value),
  tinyblob: (value: Uint8Array) => mysqlParam('tinyblob', value),
  blob: (value: Uint8Array) => mysqlParam('blob', value),
  mediumblob: (value: Uint8Array) => mysqlParam('mediumblob', value),
  longblob: (value: Uint8Array) => mysqlParam('longblob', value),
  date: (value: unknown) => mysqlParam('date', value),
  time: (value: unknown) => mysqlParam('time', value),
  datetime: (value: unknown) => mysqlParam('datetime', value),
  timestamp: (value: unknown) => mysqlParam('timestamp', value),
};
