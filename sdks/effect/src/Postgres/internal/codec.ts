/**
 * Param encoding + row decoding for the Postgres adapter.
 *
 * Translates between JS values / {@link PgParam} envelopes and the
 * `golem:rdbms/postgres@1.5.0` `DbValue` discriminated union, both
 * directions. Consumed only by `src/Postgres/PgClient.ts`.
 *
 * @internal
 * @since 0.1.0
 */
import {
  type DbColumn,
  type DbRow,
  type DbValue,
  type Interval,
  LazyDbValue,
  type Range,
  type Timestamp,
  type Timestamptz,
  type Uuid,
  type ValuesRange,
} from "golem:rdbms/postgres@1.5.0"
import type { IpAddress, MacAddress } from "golem:rdbms/types@1.5.0"
import { ParamEncodingError, toBigIntChecked } from "../../internal/rdbmsShared.js"
import {
  isPgParam,
  type PgBound,
  type PgIp,
  type PgParam,
  type PgRange,
  type PgRangeElementHint,
  type PgSparseVec,
} from "../Pg.js"

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
export const isSafeInt32 = (n: number): boolean =>
  Number.isInteger(n) && n >= I32_MIN && n <= I32_MAX

/** @internal */
export const checkInt64 = (b: bigint): void => {
  if (b < I64_MIN || b > I64_MAX) {
    throw new ParamEncodingError(`bigint ${b.toString()} is out of int8 range`)
  }
}

/** @internal */
export const encodeUuid = (input: string | Uuid): Uuid => {
  if (typeof input !== "string") return input
  const normalized = input.replace(/-/g, "").toLowerCase()
  if (!/^[0-9a-f]{32}$/.test(normalized)) {
    throw new ParamEncodingError(`invalid uuid: ${input}`)
  }
  const hi = BigInt("0x" + normalized.slice(0, 16))
  const lo = BigInt("0x" + normalized.slice(16, 32))
  return { highBits: hi, lowBits: lo }
}

/** @internal */
export const encodeMacAddr = (
  input: MacAddress | readonly [number, number, number, number, number, number],
): MacAddress => {
  if (Array.isArray(input)) {
    return {
      octets: [input[0], input[1], input[2], input[3], input[4], input[5]] as MacAddress["octets"],
    }
  }
  return input as MacAddress
}

/** @internal */
export const dateToTimestamptz = (d: Date): Timestamptz => {
  const ms = d.getTime()
  if (!Number.isFinite(ms)) {
    throw new ParamEncodingError("Date is not a valid timestamp")
  }
  return {
    timestamp: {
      date: { year: d.getUTCFullYear(), month: d.getUTCMonth() + 1, day: d.getUTCDate() },
      time: {
        hour: d.getUTCHours(),
        minute: d.getUTCMinutes(),
        second: d.getUTCSeconds(),
        nanosecond: d.getUTCMilliseconds() * 1_000_000,
      },
    },
    offset: 0,
  }
}

/** @internal */
export const encodeIp = (input: PgIp | IpAddress): IpAddress => input as IpAddress

interface Int4Bound {
  tag: "included" | "excluded" | "unbounded"
  val?: number
}
interface Int8Bound {
  tag: "included" | "excluded" | "unbounded"
  val?: bigint
}
interface NumBound {
  tag: "included" | "excluded" | "unbounded"
  val?: string
}
interface TsBound {
  tag: "included" | "excluded" | "unbounded"
  val?: Timestamp
}
interface TstzBound {
  tag: "included" | "excluded" | "unbounded"
  val?: Timestamptz
}
interface DateBound {
  tag: "included" | "excluded" | "unbounded"
  val?: { year: number; month: number; day: number }
}

/** @internal */
export const encodeRangeBoundTyped = <T>(
  bound: PgBound<unknown>,
  cast: (v: unknown) => T,
): { tag: "included" | "excluded"; val: T } | { tag: "unbounded" } =>
  bound.tag === "unbounded"
    ? { tag: "unbounded" as const }
    : { tag: bound.tag, val: cast(bound.val) }

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
  if (isPgParam(value)) {
    return encodePgParam(value)
  }
  switch (typeof value) {
    case "string":
      return { tag: "text", val: value }
    case "boolean":
      return { tag: "boolean", val: value }
    case "bigint":
      checkInt64(value)
      return { tag: "int8", val: value }
    case "number": {
      if (Number.isNaN(value) || !Number.isFinite(value)) {
        throw new ParamEncodingError(
          "NaN / Infinity cannot be sent to postgres; use Pg.numeric(string) for non-finite values",
        )
      }
      if (isSafeInt32(value)) {
        return { tag: "int4", val: value }
      }
      if (Number.isInteger(value)) {
        return { tag: "int8", val: toBigIntChecked(value, "integer number") }
      }
      return { tag: "float8", val: value }
    }
  }
  if (value instanceof Uint8Array) {
    return { tag: "bytea", val: value }
  }
  if (value instanceof Date) {
    return { tag: "timestamptz", val: dateToTimestamptz(value) }
  }
  throw new ParamEncodingError(
    `unsupported parameter type ${Object.prototype.toString.call(value)} — use a Pg.<helper>(...) wrapper`,
  )
}

/** @internal */
export const encodePgParam = (param: PgParam<string, unknown>): DbValue => {
  switch (param.kind) {
    case "json":
      return { tag: "json", val: JSON.stringify(param.value) }
    case "jsonb":
      return { tag: "jsonb", val: JSON.stringify(param.value) }
    case "jsonpath":
      return { tag: "jsonpath", val: param.value as string }
    case "xml":
      return { tag: "xml", val: param.value as string }
    case "uuid":
      return { tag: "uuid", val: encodeUuid(param.value as string | Uuid) }
    case "array": {
      const arr = param.value as ReadonlyArray<unknown>
      return { tag: "array", val: arr.map((v) => new LazyDbValue(encodeDbValue(v))) }
    }
    case "range": {
      const { range, hint } = param.value as {
        range: PgRange<unknown>
        hint: PgRangeElementHint
      }
      switch (hint) {
        case "int4": {
          const start = encodeRangeBoundTyped(range.start, (v) => v as number) as Int4Bound
          const end = encodeRangeBoundTyped(range.end, (v) => v as number) as Int4Bound
          return { tag: "int4range", val: { start: start as never, end: end as never } }
        }
        case "int8": {
          const start = encodeRangeBoundTyped(range.start, (v) => v as bigint) as Int8Bound
          const end = encodeRangeBoundTyped(range.end, (v) => v as bigint) as Int8Bound
          return { tag: "int8range", val: { start: start as never, end: end as never } }
        }
        case "num": {
          const start = encodeRangeBoundTyped(range.start, (v) => v as string) as NumBound
          const end = encodeRangeBoundTyped(range.end, (v) => v as string) as NumBound
          return { tag: "numrange", val: { start: start as never, end: end as never } }
        }
        case "ts": {
          const start = encodeRangeBoundTyped(range.start, (v) => v as Timestamp) as TsBound
          const end = encodeRangeBoundTyped(range.end, (v) => v as Timestamp) as TsBound
          return { tag: "tsrange", val: { start: start as never, end: end as never } }
        }
        case "tstz": {
          const start = encodeRangeBoundTyped(range.start, (v) => v as Timestamptz) as TstzBound
          const end = encodeRangeBoundTyped(range.end, (v) => v as Timestamptz) as TstzBound
          return { tag: "tstzrange", val: { start: start as never, end: end as never } }
        }
        case "date": {
          const start = encodeRangeBoundTyped(
            range.start,
            (v) => v as { year: number; month: number; day: number },
          ) as DateBound
          const end = encodeRangeBoundTyped(
            range.end,
            (v) => v as { year: number; month: number; day: number },
          ) as DateBound
          return { tag: "daterange", val: { start: start as never, end: end as never } }
        }
      }
      break
    }
    case "composite": {
      const { name, values } = param.value as {
        name: string
        values: ReadonlyArray<unknown>
      }
      return {
        tag: "composite",
        val: {
          name,
          values: values.map((v) => new LazyDbValue(encodeDbValue(v))),
        },
      }
    }
    case "domain": {
      const { name, value } = param.value as { name: string; value: unknown }
      return {
        tag: "domain",
        val: { name, value: new LazyDbValue(encodeDbValue(value)) },
      }
    }
    case "enumeration": {
      const { name, value } = param.value as { name: string; value: string }
      return { tag: "enumeration", val: { name, value } }
    }
    case "vector":
      return { tag: "vector", val: (param.value as ReadonlyArray<number>).slice() }
    case "halfvec":
      return { tag: "halfvec", val: (param.value as ReadonlyArray<number>).slice() }
    case "sparsevec": {
      const sv = param.value as PgSparseVec
      return {
        tag: "sparsevec",
        val: {
          dim: sv.dim,
          indices: sv.indices.slice(),
          values: sv.values.slice(),
        },
      }
    }
    case "numeric":
      return { tag: "numeric", val: param.value as string }
    case "interval":
      return { tag: "interval", val: param.value as Interval }
    case "inet":
      return { tag: "inet", val: encodeIp(param.value as PgIp | IpAddress) }
    case "cidr":
      return { tag: "cidr", val: encodeIp(param.value as PgIp | IpAddress) }
    case "macaddr":
      return {
        tag: "macaddr",
        val: encodeMacAddr(
          param.value as MacAddress | readonly [number, number, number, number, number, number],
        ),
      }
    case "bit":
      return { tag: "bit", val: (param.value as ReadonlyArray<boolean>).slice() }
    case "varbit":
      return { tag: "varbit", val: (param.value as ReadonlyArray<boolean>).slice() }
    case "int2":
      return { tag: "int2", val: param.value as number }
    case "int4":
      return { tag: "int4", val: param.value as number }
    case "int8": {
      const b = toBigIntChecked(param.value as number | bigint, "Pg.int8")
      checkInt64(b)
      return { tag: "int8", val: b }
    }
    case "float4":
      return { tag: "float4", val: param.value as number }
    case "float8":
      return { tag: "float8", val: param.value as number }
    case "text":
      return { tag: "text", val: param.value as string }
    case "varchar":
      return { tag: "varchar", val: param.value as string }
    case "bpchar":
      return { tag: "bpchar", val: param.value as string }
    case "character":
      return { tag: "character", val: param.value as number }
    case "oid":
      return { tag: "oid", val: param.value as number }
    case "money":
      return { tag: "money", val: param.value as bigint }
    case "bytea":
      return { tag: "bytea", val: param.value as Uint8Array }
    case "timestamp":
      return { tag: "timestamp", val: param.value as Timestamp }
    case "timestamptz":
      return { tag: "timestamptz", val: param.value as Timestamptz }
    default:
      throw new ParamEncodingError(`unknown Pg helper: ${param.kind}`)
  }
  throw new ParamEncodingError(`unhandled Pg helper kind: ${param.kind}`)
}

/** @internal */
export const encodeAllParams = (params: ReadonlyArray<unknown>): Array<DbValue> =>
  params.map((p) => encodeDbValue(p))

// ---------------------------------------------------------------------------
// Row decoding
// ---------------------------------------------------------------------------

/** @internal */
export const uuidToString = (uuid: Uuid): string => {
  const hi = uuid.highBits.toString(16).padStart(16, "0")
  const lo = uuid.lowBits.toString(16).padStart(16, "0")
  return `${hi.slice(0, 8)}-${hi.slice(8, 12)}-${hi.slice(12, 16)}-${lo.slice(0, 4)}-${lo.slice(4, 16)}`
}

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
export const timestamptzToDate = (tstz: Timestamptz): Date => {
  // Offset is seconds east of UTC. To get the UTC instant we subtract.
  const baseMs = timestampToDate(tstz.timestamp).getTime()
  return new Date(baseMs - tstz.offset * 1000)
}

/** @internal */
export const dateOnlyToDate = (d: { year: number; month: number; day: number }): Date =>
  new Date(Date.UTC(d.year, d.month - 1, d.day))

const decodeBound = (
  bound: ValuesRange["start"] | ValuesRange["end"],
  decodeTemporal: "raw" | "date",
): unknown => {
  if (bound.tag === "unbounded") return { tag: "unbounded" as const }
  const inner = bound.val.get()
  return { tag: bound.tag, val: decodeDbValue(inner, decodeTemporal) }
}

const decodeRange = (range: Range, decodeTemporal: "raw" | "date") => ({
  name: range.name,
  value: {
    start: decodeBound(range.value.start, decodeTemporal),
    end: decodeBound(range.value.end, decodeTemporal),
  },
})

/** @internal */
export const decodeDbValue = (value: DbValue, decodeTemporal: "raw" | "date"): unknown => {
  switch (value.tag) {
    case "null":
      return null
    case "character":
    case "int2":
    case "int4":
    case "float4":
    case "float8":
    case "oid":
      return value.val
    case "int8":
    case "money":
      return value.val
    case "numeric":
      return value.val
    case "boolean":
      return value.val
    case "text":
    case "varchar":
    case "bpchar":
    case "json":
    case "jsonb":
    case "jsonpath":
    case "xml":
      return value.val
    case "bytea":
      return value.val
    case "uuid":
      return uuidToString(value.val)
    case "timestamp":
      return decodeTemporal === "date" ? timestampToDate(value.val) : value.val
    case "timestamptz":
      return decodeTemporal === "date" ? timestamptzToDate(value.val) : value.val
    case "date":
      return decodeTemporal === "date" ? dateOnlyToDate(value.val) : value.val
    case "time":
    case "timetz":
    case "interval":
    case "inet":
    case "cidr":
    case "macaddr":
      return value.val
    case "bit":
    case "varbit":
      return value.val
    case "int4range":
    case "int8range":
    case "numrange":
    case "tsrange":
    case "tstzrange":
    case "daterange":
      return value.val
    case "enumeration":
      return value.val
    case "composite":
      return {
        name: value.val.name,
        values: value.val.values.map((lv) => decodeDbValue(lv.get(), decodeTemporal)),
      }
    case "domain":
      return {
        name: value.val.name,
        value: decodeDbValue(value.val.value.get(), decodeTemporal),
      }
    case "array":
      return value.val.map((lv) => decodeDbValue(lv.get(), decodeTemporal))
    case "range":
      return decodeRange(value.val, decodeTemporal)
    case "vector":
    case "halfvec":
      return value.val
    case "sparsevec":
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
