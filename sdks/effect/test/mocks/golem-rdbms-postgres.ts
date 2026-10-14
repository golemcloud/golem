/**
 * In-memory mock for `golem:rdbms/postgres@1.5.0` host bindings used
 * by Vitest. Stores rows in a tiny per-table map keyed by the SQL
 * string literal — enough for round-trip + transaction assertions.
 *
 * Behaviour:
 * - `DbConnection.open(addr)` succeeds unless the address starts with
 *   `bad:` (in which case it throws a tagged `connection-failure`
 *   shape — both Error-instance and plain `{tag,val}` variants are
 *   exposed via `__setOpenErrorMode`).
 * - Inserts of the form `INSERT INTO <t> (...) VALUES (...) RETURNING *`
 *   parse the columns from the SQL string and store the row in
 *   `__rowsByTable.get(t)`.
 * - `SELECT ... FROM <t>` returns whatever has been stashed.
 * - DDL is recorded but otherwise no-op.
 *
 * The mock is intentionally shallow: postgres-specific syntax is not
 * understood. Tests that exercise SQL semantics use direct host calls
 * (insertRow, listRows) rather than the SQL parser.
 *
 * Each test that uses the mock should call `__resetPostgresMock()` at
 * the start to clear shared state.
 */

import type {
  Date as PgDate,
  IpAddress,
  MacAddress,
  Time,
  Timestamp,
  Timestamptz,
  Timetz,
  Uuid,
} from "./golem-rdbms-types.js"

export type { PgDate as Date, Time, Timestamp, Timestamptz, Timetz, Uuid, IpAddress, MacAddress }

// ---------------------------------------------------------------------------
// LazyDbValue / LazyDbColumnType
// ---------------------------------------------------------------------------

export class LazyDbValue {
  constructor(private readonly _value: DbValue) {}
  get(): DbValue {
    return this._value
  }
}

export class LazyDbColumnType {
  constructor(private readonly _value: DbColumnType) {}
  get(): DbColumnType {
    return this._value
  }
}

// ---------------------------------------------------------------------------
// DbValue / DbColumnType definitions (copy of the real WIT shapes)
// ---------------------------------------------------------------------------

export type Interval = { months: number; days: number; microseconds: bigint }
export type Int4bound =
  | { tag: "included"; val: number }
  | { tag: "excluded"; val: number }
  | { tag: "unbounded" }
export type Int8bound =
  | { tag: "included"; val: bigint }
  | { tag: "excluded"; val: bigint }
  | { tag: "unbounded" }
export type Numbound =
  | { tag: "included"; val: string }
  | { tag: "excluded"; val: string }
  | { tag: "unbounded" }
export type Tsbound =
  | { tag: "included"; val: Timestamp }
  | { tag: "excluded"; val: Timestamp }
  | { tag: "unbounded" }
export type Tstzbound =
  | { tag: "included"; val: Timestamptz }
  | { tag: "excluded"; val: Timestamptz }
  | { tag: "unbounded" }
export type Datebound =
  | { tag: "included"; val: PgDate }
  | { tag: "excluded"; val: PgDate }
  | { tag: "unbounded" }
export interface Int4range {
  start: Int4bound
  end: Int4bound
}
export interface Int8range {
  start: Int8bound
  end: Int8bound
}
export interface Numrange {
  start: Numbound
  end: Numbound
}
export interface Tsrange {
  start: Tsbound
  end: Tsbound
}
export interface Tstzrange {
  start: Tstzbound
  end: Tstzbound
}
export interface Daterange {
  start: Datebound
  end: Datebound
}
export interface Enumeration {
  name: string
  value: string
}
export interface EnumerationType {
  name: string
}
export interface SparseVec {
  dim: number
  indices: number[]
  values: number[]
}
export interface Composite {
  name: string
  values: LazyDbValue[]
}
export interface Domain {
  name: string
  value: LazyDbValue
}
export type ValueBound =
  | { tag: "included"; val: LazyDbValue }
  | { tag: "excluded"; val: LazyDbValue }
  | { tag: "unbounded" }
export interface ValuesRange {
  start: ValueBound
  end: ValueBound
}
export interface Range {
  name: string
  value: ValuesRange
}

export type DbValue =
  | { tag: "character"; val: number }
  | { tag: "int2"; val: number }
  | { tag: "int4"; val: number }
  | { tag: "int8"; val: bigint }
  | { tag: "float4"; val: number }
  | { tag: "float8"; val: number }
  | { tag: "numeric"; val: string }
  | { tag: "boolean"; val: boolean }
  | { tag: "text"; val: string }
  | { tag: "varchar"; val: string }
  | { tag: "bpchar"; val: string }
  | { tag: "timestamp"; val: Timestamp }
  | { tag: "timestamptz"; val: Timestamptz }
  | { tag: "date"; val: PgDate }
  | { tag: "time"; val: Time }
  | { tag: "timetz"; val: Timetz }
  | { tag: "interval"; val: Interval }
  | { tag: "bytea"; val: Uint8Array }
  | { tag: "json"; val: string }
  | { tag: "jsonb"; val: string }
  | { tag: "jsonpath"; val: string }
  | { tag: "xml"; val: string }
  | { tag: "uuid"; val: Uuid }
  | { tag: "inet"; val: IpAddress }
  | { tag: "cidr"; val: IpAddress }
  | { tag: "macaddr"; val: MacAddress }
  | { tag: "bit"; val: boolean[] }
  | { tag: "varbit"; val: boolean[] }
  | { tag: "int4range"; val: Int4range }
  | { tag: "int8range"; val: Int8range }
  | { tag: "numrange"; val: Numrange }
  | { tag: "tsrange"; val: Tsrange }
  | { tag: "tstzrange"; val: Tstzrange }
  | { tag: "daterange"; val: Daterange }
  | { tag: "money"; val: bigint }
  | { tag: "oid"; val: number }
  | { tag: "enumeration"; val: Enumeration }
  | { tag: "composite"; val: Composite }
  | { tag: "domain"; val: Domain }
  | { tag: "array"; val: LazyDbValue[] }
  | { tag: "range"; val: Range }
  | { tag: "null" }
  | { tag: "vector"; val: number[] }
  | { tag: "halfvec"; val: number[] }
  | { tag: "sparsevec"; val: SparseVec }

export type DbColumnType =
  | { tag: "character" }
  | { tag: "int2" }
  | { tag: "int4" }
  | { tag: "int8" }
  | { tag: "float4" }
  | { tag: "float8" }
  | { tag: "numeric" }
  | { tag: "boolean" }
  | { tag: "text" }
  | { tag: "varchar" }
  | { tag: "bpchar" }
  | { tag: "timestamp" }
  | { tag: "timestamptz" }
  | { tag: "date" }
  | { tag: "time" }
  | { tag: "timetz" }
  | { tag: "interval" }
  | { tag: "bytea" }
  | { tag: "uuid" }
  | { tag: "xml" }
  | { tag: "json" }
  | { tag: "jsonb" }
  | { tag: "jsonpath" }
  | { tag: "inet" }
  | { tag: "cidr" }
  | { tag: "macaddr" }
  | { tag: "bit" }
  | { tag: "varbit" }
  | { tag: "int4range" }
  | { tag: "int8range" }
  | { tag: "numrange" }
  | { tag: "tsrange" }
  | { tag: "tstzrange" }
  | { tag: "daterange" }
  | { tag: "money" }
  | { tag: "oid" }
  | { tag: "enumeration"; val: EnumerationType }
  | { tag: "composite"; val: { name: string; attributes: [string, LazyDbColumnType][] } }
  | { tag: "domain"; val: { name: string; baseType: LazyDbColumnType } }
  | { tag: "array"; val: LazyDbColumnType }
  | { tag: "range"; val: { name: string; baseType: LazyDbColumnType } }
  | { tag: "vector" }
  | { tag: "halfvec" }
  | { tag: "sparsevec" }

export interface DbColumn {
  ordinal: bigint
  name: string
  dbType: DbColumnType
  dbTypeName: string
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
// Mock state — module-level, reset by tests
// ---------------------------------------------------------------------------

interface RowStore {
  readonly columns: ReadonlyArray<{ name: string; dbType: DbColumnType }>
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

export const __resetPostgresMock = (): void => {
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

/** Seed a table with rows. */
export const __seedTable = (
  name: string,
  columns: ReadonlyArray<{ name: string; dbType: DbColumnType }>,
  rows: Array<DbRow>,
): void => {
  __rowsByTable.set(name, { columns, rows })
}

const __getTable = (name: string): RowStore | undefined => __rowsByTable.get(name)

const __upsertTable = (
  name: string,
  columns: ReadonlyArray<{ name: string; dbType: DbColumnType }>,
): RowStore => {
  let store = __rowsByTable.get(name)
  if (!store) {
    store = { columns, rows: [] }
    __rowsByTable.set(name, store)
  }
  return store
}

const buildColumns = (cols: ReadonlyArray<{ name: string; dbType: DbColumnType }>): DbColumn[] =>
  cols.map((c, i) => ({
    ordinal: BigInt(i),
    name: c.name,
    dbType: c.dbType,
    dbTypeName: c.dbType.tag,
  }))

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

/** Best-effort table extraction. Used for the in-memory row store. */
const tableFromSelect = (sql: string): string | undefined => {
  const m = /\bFROM\s+([A-Za-z_][A-Za-z0-9_]*)/i.exec(sql)
  return m?.[1]
}

const tableFromInsert = (sql: string): string | undefined => {
  const m = /\bINSERT\s+INTO\s+([A-Za-z_][A-Za-z0-9_]*)/i.exec(sql)
  return m?.[1]
}

const tableFromUpdate = (sql: string): string | undefined => {
  const m = /\bUPDATE\s+([A-Za-z_][A-Za-z0-9_]*)/i.exec(sql)
  return m?.[1]
}

/** Extract column list from `INSERT INTO t (a, b, c) VALUES ...`. */
const insertColumns = (sql: string): ReadonlyArray<string> => {
  const m = /\bINSERT\s+INTO\s+\w+\s*\(([^)]+)\)/i.exec(sql)
  if (!m) return []
  return m[1]!.split(",").map((s) => s.trim().replace(/^"(.*)"$/, "$1"))
}

const isReturning = (sql: string): boolean => /\bRETURNING\b/i.test(sql)

const inferColumnType = (v: DbValue): DbColumnType => {
  switch (v.tag) {
    case "null":
      return { tag: "text" }
    case "character":
    case "int2":
    case "int4":
    case "int8":
    case "float4":
    case "float8":
    case "numeric":
    case "boolean":
    case "text":
    case "varchar":
    case "bpchar":
    case "json":
    case "jsonb":
    case "jsonpath":
    case "xml":
    case "bytea":
    case "uuid":
    case "timestamp":
    case "timestamptz":
    case "date":
    case "time":
    case "timetz":
    case "interval":
    case "inet":
    case "cidr":
    case "macaddr":
    case "bit":
    case "varbit":
    case "int4range":
    case "int8range":
    case "numrange":
    case "tsrange":
    case "tstzrange":
    case "daterange":
    case "money":
    case "oid":
    case "vector":
    case "halfvec":
    case "sparsevec":
      return { tag: v.tag } as DbColumnType
    case "enumeration":
      return { tag: "enumeration", val: { name: v.val.name } }
    case "composite":
      return {
        tag: "composite",
        val: {
          name: v.val.name,
          attributes: v.val.values.map((_, i) => [
            String(i),
            new LazyDbColumnType({ tag: "text" }),
          ]),
        },
      }
    case "domain":
      return {
        tag: "domain",
        val: { name: v.val.name, baseType: new LazyDbColumnType({ tag: "text" }) },
      }
    case "array":
      return {
        tag: "array",
        val: new LazyDbColumnType({ tag: "text" }),
      }
    case "range":
      return {
        tag: "range",
        val: { name: v.val.name, baseType: new LazyDbColumnType({ tag: "text" }) },
      }
  }
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
  if (/\bINSERT\b/i.test(sql) && isReturning(sql)) {
    const tbl = tableFromInsert(sql)
    if (!tbl) return { columns: [], rows: [] }
    const cols = insertColumns(sql)
    const inferred: ReadonlyArray<{ name: string; dbType: DbColumnType }> = cols.map((c, i) => ({
      name: c,
      dbType: inferColumnType(params[i] ?? { tag: "null" }),
    }))
    const store = __upsertTable(tbl, inferred)
    const row: DbRow = { values: params.slice() }
    store.rows.push(row)
    return { columns: buildColumns(store.columns), rows: [row] }
  }
  if (/\bUPDATE\b/i.test(sql) && isReturning(sql)) {
    const tbl = tableFromUpdate(sql)
    if (!tbl) return { columns: [], rows: [] }
    const store = __getTable(tbl)
    if (!store) return { columns: [], rows: [] }
    return { columns: buildColumns(store.columns), rows: store.rows.slice() }
  }
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
    const inferred = cols.map((c, i) => ({
      name: c,
      dbType: inferColumnType(params[i] ?? { tag: "null" }),
    }))
    const store = __upsertTable(tbl, inferred)
    store.rows.push({ values: params.slice() })
    return 1n
  }
  if (/^\s*(?:UPDATE|DELETE)\b/i.test(sql)) {
    return 1n
  }
  if (/^\s*(?:BEGIN|COMMIT|ROLLBACK|SAVEPOINT|RELEASE\s+SAVEPOINT)\b/i.test(sql)) {
    return 0n
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
      throw new Error(`authentication failed for role 'pgrole'`)
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
