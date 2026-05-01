/**
 * Param encoding + row decoding for the MySQL adapter.
 *
 * Translates between JS values / {@link MySqlParam} envelopes and the
 * `golem:rdbms/mysql@1.5.0` `DbValue` discriminated union, both
 * directions. Consumed only by `src/Mysql/MySqlClient.ts`.
 *
 * @internal
 * @since 0.1.0
 */
import {
  type Date as MyDate,
  type DbColumn,
  type DbRow,
  type DbValue,
  type Time,
  type Timestamp,
} from "golem:rdbms/mysql@1.5.0"
import { ParamEncodingError, toBigIntChecked } from "../../internal/rdbmsShared.js"
import { isMySqlParam, type MySqlParam } from "../MySql.js"

// ---------------------------------------------------------------------------
// Numeric / null constants
// ---------------------------------------------------------------------------

/** @internal */
export const NULL_DB_VALUE: DbValue = { tag: "null" }

/** @internal */
export const I32_MIN = -2_147_483_648
/** @internal */
export const I32_MAX = 2_147_483_647
/** @internal */
export const I64_MIN = BigInt("-9223372036854775808")
/** @internal */
export const I64_MAX = BigInt("9223372036854775807")
/** @internal */
export const U64_MAX = BigInt("18446744073709551615")

/** @internal */
export const isSafeInt32 = (n: number): boolean =>
  Number.isInteger(n) && n >= I32_MIN && n <= I32_MAX

/** @internal */
export const checkInt64 = (b: bigint): void => {
  if (b < I64_MIN || b > I64_MAX) {
    throw new ParamEncodingError(`bigint ${b.toString()} is out of bigint range`)
  }
}

/** @internal */
export const checkUint64 = (b: bigint): void => {
  if (b < 0n || b > U64_MAX) {
    throw new ParamEncodingError(`bigint ${b.toString()} is out of bigint-unsigned range`)
  }
}

/** @internal */
export const dateToTimestamp = (d: Date): Timestamp => {
  const ms = d.getTime()
  if (!Number.isFinite(ms)) {
    throw new ParamEncodingError("Date is not a valid timestamp")
  }
  return {
    date: { year: d.getUTCFullYear(), month: d.getUTCMonth() + 1, day: d.getUTCDate() },
    time: {
      hour: d.getUTCHours(),
      minute: d.getUTCMinutes(),
      second: d.getUTCSeconds(),
      nanosecond: d.getUTCMilliseconds() * 1_000_000,
    },
  }
}

// ---------------------------------------------------------------------------
// Param encoding
// ---------------------------------------------------------------------------

/**
 * Convert a single template-literal parameter to a `DbValue`. Throws
 * a `ParamEncodingError` for unsupported shapes; the caller wraps it
 * into a `SqlError`.
 *
 * @internal
 */
export const encodeDbValue = (value: unknown): DbValue => {
  if (value === null || value === undefined) return NULL_DB_VALUE
  if (isMySqlParam(value)) {
    return encodeMySqlParam(value)
  }
  switch (typeof value) {
    case "string":
      return { tag: "varchar", val: value }
    case "boolean":
      return { tag: "boolean", val: value }
    case "bigint":
      checkInt64(value)
      return { tag: "bigint", val: value }
    case "number": {
      if (Number.isNaN(value) || !Number.isFinite(value)) {
        throw new ParamEncodingError(
          "NaN / Infinity cannot be sent to MySQL; use MySql.decimal(string) for non-finite values",
        )
      }
      if (isSafeInt32(value)) {
        return { tag: "int", val: value }
      }
      if (Number.isInteger(value)) {
        // `Number.isInteger` is true even for non-safe-integer values
        // — `toBigIntChecked` rejects those with a typed
        // `ParamEncodingError` instead of silently dropping precision.
        return { tag: "bigint", val: toBigIntChecked(value, "integer number") }
      }
      return { tag: "double", val: value }
    }
  }
  if (value instanceof Uint8Array) {
    return { tag: "blob", val: value }
  }
  if (value instanceof Date) {
    return { tag: "datetime", val: dateToTimestamp(value) }
  }
  throw new ParamEncodingError(
    `unsupported parameter type ${Object.prototype.toString.call(value)} — use a MySql.<helper>(...) wrapper`,
  )
}

/** @internal */
export const encodeMySqlParam = (param: MySqlParam<string, unknown>): DbValue => {
  switch (param.kind) {
    case "json":
      return { tag: "json", val: JSON.stringify(param.value) }
    case "enumeration":
      return { tag: "enumeration", val: param.value as string }
    case "set":
      return { tag: "set", val: param.value as string }
    case "bit":
      return { tag: "bit", val: (param.value as ReadonlyArray<boolean>).slice() }
    case "decimal":
      return { tag: "decimal", val: param.value as string }
    case "year":
      return { tag: "year", val: param.value as number }
    case "tinyint":
      return { tag: "tinyint", val: param.value as number }
    case "smallint":
      return { tag: "smallint", val: param.value as number }
    case "mediumint":
      return { tag: "mediumint", val: param.value as number }
    case "int":
      return { tag: "int", val: param.value as number }
    case "bigint": {
      const b = toBigIntChecked(param.value as number | bigint, "MySql.bigint")
      checkInt64(b)
      return { tag: "bigint", val: b }
    }
    case "tinyint-unsigned":
      return { tag: "tinyint-unsigned", val: param.value as number }
    case "smallint-unsigned":
      return { tag: "smallint-unsigned", val: param.value as number }
    case "mediumint-unsigned":
      return { tag: "mediumint-unsigned", val: param.value as number }
    case "int-unsigned":
      return { tag: "int-unsigned", val: param.value as number }
    case "bigint-unsigned": {
      const b = toBigIntChecked(param.value as number | bigint, "MySql.bigintUnsigned")
      checkUint64(b)
      return { tag: "bigint-unsigned", val: b }
    }
    case "float":
      return { tag: "float", val: param.value as number }
    case "double":
      return { tag: "double", val: param.value as number }
    case "fixchar":
      return { tag: "fixchar", val: param.value as string }
    case "varchar":
      return { tag: "varchar", val: param.value as string }
    case "tinytext":
      return { tag: "tinytext", val: param.value as string }
    case "text":
      return { tag: "text", val: param.value as string }
    case "mediumtext":
      return { tag: "mediumtext", val: param.value as string }
    case "longtext":
      return { tag: "longtext", val: param.value as string }
    case "binary":
      return { tag: "binary", val: param.value as Uint8Array }
    case "varbinary":
      return { tag: "varbinary", val: param.value as Uint8Array }
    case "tinyblob":
      return { tag: "tinyblob", val: param.value as Uint8Array }
    case "blob":
      return { tag: "blob", val: param.value as Uint8Array }
    case "mediumblob":
      return { tag: "mediumblob", val: param.value as Uint8Array }
    case "longblob":
      return { tag: "longblob", val: param.value as Uint8Array }
    case "date":
      return { tag: "date", val: param.value as MyDate }
    case "time":
      return { tag: "time", val: param.value as Time }
    case "datetime":
      return { tag: "datetime", val: param.value as Timestamp }
    case "timestamp":
      return { tag: "timestamp", val: param.value as Timestamp }
    default:
      throw new ParamEncodingError(`unknown MySql helper: ${param.kind}`)
  }
}

/** @internal */
export const encodeAllParams = (params: ReadonlyArray<unknown>): Array<DbValue> =>
  params.map((p) => encodeDbValue(p))

// ---------------------------------------------------------------------------
// Row decoding
// ---------------------------------------------------------------------------

/** @internal */
export const timestampToDate = (ts: Timestamp): Date => {
  const ms = Math.floor(ts.time.nanosecond / 1_000_000)
  return new Date(
    Date.UTC(
      ts.date.year,
      ts.date.month - 1,
      ts.date.day,
      ts.time.hour,
      ts.time.minute,
      ts.time.second,
      ms,
    ),
  )
}

/** @internal */
export const dateOnlyToDate = (d: { year: number; month: number; day: number }): Date =>
  new Date(Date.UTC(d.year, d.month - 1, d.day))

/** @internal */
export const decodeDbValue = (value: DbValue, decodeTemporal: "raw" | "date"): unknown => {
  switch (value.tag) {
    case "null":
      return null
    case "boolean":
      return value.val
    case "tinyint":
    case "smallint":
    case "mediumint":
    case "int":
    case "tinyint-unsigned":
    case "smallint-unsigned":
    case "mediumint-unsigned":
    case "int-unsigned":
    case "year":
    case "float":
    case "double":
      return value.val
    case "bigint":
    case "bigint-unsigned":
      return value.val
    case "decimal":
      return value.val
    case "fixchar":
    case "varchar":
    case "tinytext":
    case "text":
    case "mediumtext":
    case "longtext":
    case "json":
    case "enumeration":
    case "set":
      return value.val
    case "binary":
    case "varbinary":
    case "tinyblob":
    case "blob":
    case "mediumblob":
    case "longblob":
      return value.val
    case "bit":
      return value.val
    case "datetime":
    case "timestamp":
      return decodeTemporal === "date" ? timestampToDate(value.val) : value.val
    case "date":
      return decodeTemporal === "date" ? dateOnlyToDate(value.val) : value.val
    case "time":
      return value.val
  }
}

/** @internal */
export const decodeRows = (
  rows: ReadonlyArray<DbRow>,
  columns: ReadonlyArray<DbColumn>,
  decodeTemporal: "raw" | "date",
): Array<Record<string, unknown>> => {
  const colNames = columns.map((c) => c.name)
  return rows.map((row) => {
    const obj: Record<string, unknown> = {}
    for (let i = 0; i < colNames.length; i++) {
      obj[colNames[i]!] = decodeDbValue(row.values[i]!, decodeTemporal)
    }
    return obj
  })
}

/** @internal */
export const decodeRowsValues = (
  rows: ReadonlyArray<DbRow>,
  decodeTemporal: "raw" | "date",
): Array<Array<unknown>> =>
  rows.map((row) => row.values.map((v) => decodeDbValue(v, decodeTemporal)))
