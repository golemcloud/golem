/**
 * Param encoding + row decoding for the Ignite (Apache Ignite 2.x)
 * adapter.
 *
 * Translates between JS values / {@link IgniteParam} envelopes and the
 * `golem:rdbms/ignite2@1.5.0` `DbValue` discriminated union, both
 * directions. Consumed only by `src/Ignite/IgniteClient.ts`.
 *
 * @internal
 * @since 1.5.0
 */
import { type DbColumn, type DbRow, type DbValue } from "golem:rdbms/ignite2@1.5.0"
import { ParamEncodingError, toBigIntChecked } from "../../internal/rdbmsShared.js"
import { type IgniteParam, type IgniteUuid, isIgniteParam } from "../Ignite.js"

// ---------------------------------------------------------------------------
// Numeric / null constants
// ---------------------------------------------------------------------------

/** @internal */
export const NULL_DB_VALUE: DbValue = { tag: "db-null" }

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
    throw new ParamEncodingError(`bigint ${b.toString()} is out of db-long range`)
  }
}

/** @internal */
export const checkUint64 = (b: bigint, label: string): void => {
  if (b < 0n || b > U64_MAX) {
    throw new ParamEncodingError(`${label} ${b.toString()} is out of u64 range`)
  }
}

/** @internal */
export const encodeUuid = (input: IgniteUuid): [bigint, bigint] => {
  if (typeof input === "string") {
    const normalized = input.replace(/-/g, "").toLowerCase()
    if (!/^[0-9a-f]{32}$/.test(normalized)) {
      throw new ParamEncodingError(`invalid uuid: ${input}`)
    }
    const hi = BigInt("0x" + normalized.slice(0, 16))
    const lo = BigInt("0x" + normalized.slice(16, 32))
    return [hi, lo]
  }
  // WIT spec: db-uuid is `tuple<u64, u64>`. Validate both halves.
  const hi = Array.isArray(input) ? input[0] : input.hi
  const lo = Array.isArray(input) ? input[1] : input.lo
  if (typeof hi !== "bigint" || typeof lo !== "bigint") {
    throw new ParamEncodingError("Ignite.uuid hi/lo must both be bigint")
  }
  checkUint64(hi, "Ignite.uuid hi")
  checkUint64(lo, "Ignite.uuid lo")
  return [hi, lo]
}

/** @internal */
export const dateToEpochMillis = (d: Date): bigint => {
  const ms = d.getTime()
  if (!Number.isFinite(ms)) {
    throw new ParamEncodingError("Date is not a valid timestamp")
  }
  return BigInt(ms)
}

// ---------------------------------------------------------------------------
// Param encoding
// ---------------------------------------------------------------------------

/**
 * Convert a single template-literal parameter to a `DbValue`.
 *
 * @internal
 */
export const encodeDbValue = (value: unknown): DbValue => {
  if (value === null || value === undefined) return NULL_DB_VALUE
  if (isIgniteParam(value)) {
    return encodeIgniteParam(value)
  }
  switch (typeof value) {
    case "string":
      return { tag: "db-string", val: value }
    case "boolean":
      return { tag: "db-boolean", val: value }
    case "bigint":
      checkInt64(value)
      return { tag: "db-long", val: value }
    case "number": {
      if (Number.isNaN(value) || !Number.isFinite(value)) {
        throw new ParamEncodingError(
          "NaN / Infinity cannot be sent to Ignite; use Ignite.decimal(string) for non-finite values",
        )
      }
      if (isSafeInt32(value)) {
        return { tag: "db-int", val: value }
      }
      if (Number.isInteger(value)) {
        return { tag: "db-long", val: toBigIntChecked(value, "integer number") }
      }
      return { tag: "db-double", val: value }
    }
  }
  if (value instanceof Uint8Array) {
    return { tag: "db-byte-array", val: value }
  }
  if (value instanceof Date) {
    return { tag: "db-date", val: dateToEpochMillis(value) }
  }
  throw new ParamEncodingError(
    `unsupported parameter type ${Object.prototype.toString.call(value)} — use an Ignite.<helper>(...) wrapper`,
  )
}

/** @internal */
export const encodeIgniteParam = (param: IgniteParam<string, unknown>): DbValue => {
  switch (param.kind) {
    case "uuid":
      return { tag: "db-uuid", val: encodeUuid(param.value as IgniteUuid) }
    case "decimal":
      return { tag: "db-decimal", val: param.value as string }
    case "date": {
      const v = param.value as bigint | number | Date
      const millis =
        v instanceof Date ? dateToEpochMillis(v) : toBigIntChecked(v, "Ignite.date millis")
      return { tag: "db-date", val: millis }
    }
    case "timestamp": {
      const { millis, subMilliNanos } = param.value as {
        millis: bigint | number
        subMilliNanos: number
      }
      const m = toBigIntChecked(millis, "Ignite.timestamp millis")
      if (!Number.isInteger(subMilliNanos) || subMilliNanos < 0 || subMilliNanos > 999_999) {
        throw new ParamEncodingError(
          `Ignite.timestamp sub-ms-nanos must be an integer in 0..999_999; got ${String(subMilliNanos)}`,
        )
      }
      return { tag: "db-timestamp", val: [m, subMilliNanos] }
    }
    case "time": {
      const v = param.value as bigint | number
      return { tag: "db-time", val: toBigIntChecked(v, "Ignite.time nanos") }
    }
    case "char":
      return { tag: "db-char", val: param.value as number }
    case "byte-array":
      return { tag: "db-byte-array", val: param.value as Uint8Array }
    case "byte":
      return { tag: "db-byte", val: param.value as number }
    case "short":
      return { tag: "db-short", val: param.value as number }
    case "int":
      return { tag: "db-int", val: param.value as number }
    case "long": {
      const b = toBigIntChecked(param.value as number | bigint, "Ignite.long")
      checkInt64(b)
      return { tag: "db-long", val: b }
    }
    case "float":
      return { tag: "db-float", val: param.value as number }
    case "double":
      return { tag: "db-double", val: param.value as number }
    case "string":
      return { tag: "db-string", val: param.value as string }
    case "boolean":
      return { tag: "db-boolean", val: param.value as boolean }
    default:
      throw new ParamEncodingError(`unknown Ignite helper: ${param.kind}`)
  }
}

/** @internal */
export const encodeAllParams = (params: ReadonlyArray<unknown>): Array<DbValue> =>
  params.map((p) => encodeDbValue(p))

// ---------------------------------------------------------------------------
// Row decoding
// ---------------------------------------------------------------------------

/** @internal */
export const uuidToString = (u: [bigint, bigint]): string => {
  const hi = u[0].toString(16).padStart(16, "0")
  const lo = u[1].toString(16).padStart(16, "0")
  return `${hi.slice(0, 8)}-${hi.slice(8, 12)}-${hi.slice(12, 16)}-${lo.slice(0, 4)}-${lo.slice(4, 16)}`
}

/** @internal */
export const decodeDbValue = (value: DbValue, decodeTemporal: "raw" | "date"): unknown => {
  switch (value.tag) {
    case "db-null":
      return null
    case "db-boolean":
      return value.val
    case "db-byte":
    case "db-short":
    case "db-int":
    case "db-float":
    case "db-double":
    case "db-char":
      return value.val
    case "db-long":
      return value.val
    case "db-string":
    case "db-decimal":
      return value.val
    case "db-uuid":
      return uuidToString(value.val)
    case "db-date":
      return decodeTemporal === "date" ? new Date(Number(value.val)) : value.val
    case "db-timestamp": {
      if (decodeTemporal !== "date") return value.val
      const [millis] = value.val
      return new Date(Number(millis))
    }
    case "db-time":
      return value.val
    case "db-byte-array":
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
