/**
 * In-memory mock for `golem:rdbms/ignite2@1.5.0` host bindings used by
 * Vitest. Mirrors the postgres mock — stores rows in a tiny per-table
 * map keyed by the SQL string literal.
 */

// ---------------------------------------------------------------------------
// DbValue
// ---------------------------------------------------------------------------

export type DbValue =
  | { tag: "db-null" }
  | { tag: "db-boolean"; val: boolean }
  | { tag: "db-byte"; val: number }
  | { tag: "db-short"; val: number }
  | { tag: "db-int"; val: number }
  | { tag: "db-long"; val: bigint }
  | { tag: "db-float"; val: number }
  | { tag: "db-double"; val: number }
  | { tag: "db-char"; val: number }
  | { tag: "db-string"; val: string }
  | { tag: "db-uuid"; val: [bigint, bigint] }
  | { tag: "db-date"; val: bigint }
  | { tag: "db-timestamp"; val: [bigint, number] }
  | { tag: "db-time"; val: bigint }
  | { tag: "db-decimal"; val: string }
  | { tag: "db-byte-array"; val: Uint8Array }

export interface DbColumn {
  ordinal: bigint
  name: string
}

export interface DbRow {
  values: DbValue[]
}

export interface DbResult {
  columns: DbColumn[]
  rows: DbRow[]
}

export type Error =
  | { tag: "connection-failure"; val: string }
  | { tag: "query-parameter-failure"; val: string }
  | { tag: "query-execution-failure"; val: string }
  | { tag: "query-response-failure"; val: string }
  | { tag: "other"; val: string }

export type Result<T, E> = { tag: "ok"; val: T } | { tag: "err"; val: E }

// ---------------------------------------------------------------------------
// Mock state
// ---------------------------------------------------------------------------

interface RowStore {
  readonly columns: ReadonlyArray<{ name: string }>
  readonly rows: Array<DbRow>
}

const __rowsByTable: Map<string, RowStore> = new Map()
const __ddlLog: Array<string> = []
const __executeLog: Array<{ sql: string; params: ReadonlyArray<DbValue> }> = []
const __queryLog: Array<{ sql: string; params: ReadonlyArray<DbValue> }> = []
let __openMode: "ok" | "tagged-err" | "tagged-error-instance" | "auth-err" | "plain-err" = "ok"
let __nextQueryError:
  | { mode: "tagged" | "tagged-error-instance" | "plain"; tag: Error["tag"]; val: string }
  | undefined

export const __resetIgniteMock = (): void => {
  __rowsByTable.clear()
  __ddlLog.length = 0
  __executeLog.length = 0
  __queryLog.length = 0
  __openMode = "ok"
  __nextQueryError = undefined
}

export const __setOpenMode = (
  mode: "ok" | "tagged-err" | "tagged-error-instance" | "auth-err" | "plain-err",
): void => {
  __openMode = mode
}

export const __setNextQueryError = (
  err:
    | { mode: "tagged" | "tagged-error-instance" | "plain"; tag: Error["tag"]; val: string }
    | undefined,
): void => {
  __nextQueryError = err
}

export const __getDdlLog = (): ReadonlyArray<string> => __ddlLog
export const __getExecuteLog = () => __executeLog
export const __getQueryLog = () => __queryLog

export const __seedTable = (
  name: string,
  columns: ReadonlyArray<{ name: string }>,
  rows: Array<DbRow>,
): void => {
  __rowsByTable.set(name, { columns, rows })
}

const __getTable = (name: string): RowStore | undefined => __rowsByTable.get(name)

const __upsertTable = (name: string, columns: ReadonlyArray<{ name: string }>): RowStore => {
  let store = __rowsByTable.get(name)
  if (!store) {
    store = { columns, rows: [] }
    __rowsByTable.set(name, store)
  }
  return store
}

const buildColumns = (cols: ReadonlyArray<{ name: string }>): DbColumn[] =>
  cols.map((c, i) => ({ ordinal: BigInt(i), name: c.name }))

const throwTagged = (tag: Error["tag"], val: string): never => {
  throw { tag, val }
}

const throwTaggedErrorInstance = (tag: Error["tag"], val: string): never => {
  const err = new Error(`${tag}: ${val}`)
  ;(err as unknown as { payload: Error }).payload = { tag, val } as Error
  throw err
}

const maybeThrowQueryError = (): void => {
  const err = __nextQueryError
  if (!err) return
  __nextQueryError = undefined
  if (err.mode === "tagged") throwTagged(err.tag, err.val)
  if (err.mode === "tagged-error-instance") throwTaggedErrorInstance(err.tag, err.val)
  throw new Error(err.val)
}

// ---------------------------------------------------------------------------
// Tiny SQL parsers
// ---------------------------------------------------------------------------

const tableFromSelect = (sql: string): string | undefined => {
  const m = /\bFROM\s+"?([A-Za-z_][A-Za-z0-9_]*)"?/i.exec(sql)
  return m?.[1]
}

const tableFromInsert = (sql: string): string | undefined => {
  const m = /\bINSERT\s+INTO\s+"?([A-Za-z_][A-Za-z0-9_]*)"?/i.exec(sql)
  return m?.[1]
}

const insertColumns = (sql: string): ReadonlyArray<string> => {
  const m = /\bINSERT\s+INTO\s+"?\w+"?\s*\(([^)]+)\)/i.exec(sql)
  if (!m) return []
  return m[1]!.split(",").map((s) => s.trim().replace(/^"(.*)"$/, "$1"))
}

// ---------------------------------------------------------------------------
// Resource classes
// ---------------------------------------------------------------------------

export class DbResultStream {
  private _exhausted = false
  private _idx = 0
  constructor(
    private readonly _columns: DbColumn[],
    private readonly _rows: DbRow[],
    private readonly _batchSize = 1,
  ) {}
  getColumns(): DbColumn[] {
    return this._columns
  }
  getNext(): DbRow[] | undefined {
    if (this._exhausted) return undefined
    if (this._idx >= this._rows.length) {
      this._exhausted = true
      return undefined
    }
    const slice = this._rows.slice(this._idx, this._idx + this._batchSize)
    this._idx += this._batchSize
    return slice
  }
}

const handleQueryLike = (sql: string, params: ReadonlyArray<DbValue>): DbResult => {
  __queryLog.push({ sql, params })
  maybeThrowQueryError()
  if (/^\s*(?:SELECT|WITH|VALUES|SHOW|EXPLAIN|TABLE)\b/i.test(sql)) {
    const tbl = tableFromSelect(sql)
    if (!tbl) return { columns: [], rows: [] }
    const store = __getTable(tbl)
    if (!store) return { columns: [], rows: [] }
    return { columns: buildColumns(store.columns), rows: store.rows.slice() }
  }
  return { columns: [], rows: [] }
}

const handleExecute = (sql: string, params: ReadonlyArray<DbValue>): bigint => {
  __executeLog.push({ sql, params })
  maybeThrowQueryError()
  if (/^\s*(?:CREATE|DROP|ALTER|TRUNCATE)\b/i.test(sql)) {
    __ddlLog.push(sql)
    return 0n
  }
  if (/\bINSERT\b/i.test(sql)) {
    const tbl = tableFromInsert(sql)
    if (!tbl) return 1n
    const cols = insertColumns(sql)
    const store = __upsertTable(
      tbl,
      cols.map((name) => ({ name })),
    )
    store.rows.push({ values: params.slice() })
    return 1n
  }
  if (/^\s*(?:UPDATE|DELETE|MERGE)\b/i.test(sql)) {
    return 1n
  }
  return 0n
}

export class DbTransaction {
  private _completed = false
  constructor(private readonly _conn: DbConnection) {
    void this._conn
  }
  query(sql: string, params: DbValue[]): DbResult {
    if (this._completed) throw { tag: "query-execution-failure", val: "tx already completed" }
    return handleQueryLike(sql, params)
  }
  queryStream(sql: string, params: DbValue[]): DbResultStream {
    if (this._completed) throw { tag: "query-execution-failure", val: "tx already completed" }
    const res = handleQueryLike(sql, params)
    return new DbResultStream(res.columns, res.rows)
  }
  execute(sql: string, params: DbValue[]): bigint {
    if (this._completed) throw { tag: "query-execution-failure", val: "tx already completed" }
    return handleExecute(sql, params)
  }
  commit(): void {
    if (this._completed) throw { tag: "other", val: "tx already completed" }
    this._completed = true
  }
  rollback(): void {
    if (this._completed) throw { tag: "other", val: "tx already completed" }
    this._completed = true
  }
}

export class DbConnection {
  private constructor(public readonly address: string) {}
  static open(address: string): DbConnection {
    if (__openMode === "tagged-err") {
      throw { tag: "connection-failure", val: `cannot open ${address}` } as Error
    }
    if (__openMode === "tagged-error-instance") {
      const err = new Error(`connection-failure: cannot open ${address}`)
      ;(err as unknown as { payload: Error }).payload = {
        tag: "connection-failure",
        val: `cannot open ${address}`,
      } as Error
      throw err
    }
    if (__openMode === "auth-err") {
      throw new Error(`authentication failed for user 'igniteuser'`)
    }
    if (__openMode === "plain-err") {
      throw new Error(`generic open failure for ${address}`)
    }
    return new DbConnection(address)
  }
  query(sql: string, params: DbValue[]): DbResult {
    return handleQueryLike(sql, params)
  }
  queryStream(sql: string, params: DbValue[]): DbResultStream {
    const res = handleQueryLike(sql, params)
    return new DbResultStream(res.columns, res.rows)
  }
  execute(sql: string, params: DbValue[]): bigint {
    return handleExecute(sql, params)
  }
  beginTransaction(): DbTransaction {
    return new DbTransaction(this)
  }
}
