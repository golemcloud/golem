declare module 'golem:rdbms/ignite2@1.5.0' {
  export class DbResultStream {
    /**
     * @throws Error
     */
    getColumns(): DbColumn[];
    /**
     * @throws Error
     */
    getNext(): DbRow[] | undefined;
  }
  export class DbTransaction {
    /**
     * @throws Error
     */
    query(statement: string, params: DbValue[]): DbResult;
    /**
     * @throws Error
     */
    queryStream(statement: string, params: DbValue[]): DbResultStream;
    /**
     * @throws Error
     */
    execute(statement: string, params: DbValue[]): bigint;
    /**
     * @throws Error
     */
    commit(): void;
    /**
     * @throws Error
     */
    rollback(): void;
  }
  export class DbConnection {
    /**
     * Open a connection to an Apache Ignite 2.x node.
     * Address format: `ignite://[user:pass@]host:port[?pool_size=N&tls=true]`
     * Default port: 10800.
     * @throws Error
     */
    static open(address: string): DbConnection;
    /**
     * @throws Error
     */
    query(statement: string, params: DbValue[]): DbResult;
    /**
     * @throws Error
     */
    queryStream(statement: string, params: DbValue[]): DbResultStream;
    /**
     * @throws Error
     */
    execute(statement: string, params: DbValue[]): bigint;
    /**
     * @throws Error
     */
    beginTransaction(): DbTransaction;
  }
  /**
   * ── Error ──────────────────────────────────────────────────────────────────
   */
  export type Error = 
  {
    tag: 'connection-failure'
    val: string
  } |
  {
    tag: 'query-parameter-failure'
    val: string
  } |
  {
    tag: 'query-execution-failure'
    val: string
  } |
  {
    tag: 'query-response-failure'
    val: string
  } |
  {
    tag: 'other'
    val: string
  };
  /**
   * ── Value types (maps 1-to-1 onto ignite_client::IgniteValue) ─────────────
   */
  export type DbValue = 
  {
    tag: 'db-null'
  } |
  {
    tag: 'db-boolean'
    val: boolean
  } |
  {
    tag: 'db-byte'
    val: number
  } |
  {
    tag: 'db-short'
    val: number
  } |
  {
    tag: 'db-int'
    val: number
  } |
  {
    tag: 'db-long'
    val: bigint
  } |
  {
    tag: 'db-float'
    val: number
  } |
  {
    tag: 'db-double'
    val: number
  } |
  /** 16-bit Unicode code unit (Java char). */
  {
    tag: 'db-char'
    val: number
  } |
  {
    tag: 'db-string'
    val: string
  } |
  {
    tag: 'db-uuid'
    val: [bigint, bigint]
  } |
  /** Milliseconds since Unix epoch (UTC). */
  {
    tag: 'db-date'
    val: bigint
  } |
  /** (milliseconds since epoch, sub-millisecond nanoseconds 0..999_999). */
  {
    tag: 'db-timestamp'
    val: [bigint, number]
  } |
  /** Nanoseconds since midnight. */
  {
    tag: 'db-time'
    val: bigint
  } |
  {
    tag: 'db-decimal'
    val: string
  } |
  {
    tag: 'db-byte-array'
    val: Uint8Array
  };
  /**
   * ── Metadata ───────────────────────────────────────────────────────────────
   */
  export type DbColumn = {
    ordinal: bigint;
    name: string;
    dbTypeName: string;
  };
  export type DbRow = {
    values: DbValue[];
  };
  export type DbResult = {
    columns: DbColumn[];
    rows: DbRow[];
  };
  export type Result<T, E> = { tag: 'ok', val: T } | { tag: 'err', val: E };
}
