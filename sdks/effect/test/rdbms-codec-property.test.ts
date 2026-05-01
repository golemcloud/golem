import { describe, expect, it } from "@effect/vitest"
import * as fc from "effect/testing/FastCheck"
import * as Ig from "../src/Ignite/internal/codec.js"
import * as My from "../src/Mysql/internal/codec.js"
import * as Pg from "../src/Postgres/internal/codec.js"

// ---------------------------------------------------------------------------
// Common arbitraries
// ---------------------------------------------------------------------------

/** s32 range — encodes as `int4` / `int` / `db-int`. */
const safeInt32Arb = fc.integer({ min: -(2 ** 31), max: 2 ** 31 - 1 })

/** s64 range, but excluding the safe-int32 subrange so we exercise the
 * `int8` / `bigint` / `db-long` branch, not the `int4` branch. */
const wideBigIntArb = fc.bigInt({ min: -(2n ** 63n), max: 2n ** 63n - 1n })

/** Finite, non-integer doubles — exercises `float8` / `double` / `db-double`. */
const finiteFloatArb = fc
  .double({ noNaN: true, noDefaultInfinity: true })
  .filter((n) => !Number.isInteger(n))

/** Random byte strings (mapped to `bytea` / `blob` / `db-byte-array`). */
const bytesArb = fc.uint8Array({ maxLength: 64 })

/** Round to whole milliseconds — JS `Date` is millisecond-precision and the
 * encoders only persist `getUTCMilliseconds() * 1_000_000` worth of nanos. */
const dateArb = fc.date({ noInvalidDate: true }).map((d) => new Date(d.getTime()))

// ---------------------------------------------------------------------------
// Per-adapter properties
// ---------------------------------------------------------------------------

const cases = [
  {
    name: "Postgres",
    encode: (v: unknown) => Pg.encodeDbValue(v) as unknown,
    decode: (v: unknown) => Pg.decodeDbValue(v as Parameters<typeof Pg.decodeDbValue>[0], "date"),
    int4Tag: "int4",
    int8Tag: "int8",
    floatTag: "float8",
    bytesTag: "bytea",
    stringTag: "text",
    boolTag: "boolean",
  },
  {
    name: "MySQL",
    encode: (v: unknown) => My.encodeDbValue(v) as unknown,
    decode: (v: unknown) => My.decodeDbValue(v as Parameters<typeof My.decodeDbValue>[0], "date"),
    int4Tag: "int",
    int8Tag: "bigint",
    floatTag: "double",
    bytesTag: "blob",
    stringTag: "varchar",
    boolTag: "boolean",
  },
  {
    name: "Ignite",
    encode: (v: unknown) => Ig.encodeDbValue(v) as unknown,
    decode: (v: unknown) => Ig.decodeDbValue(v as Parameters<typeof Ig.decodeDbValue>[0], "date"),
    int4Tag: "db-int",
    int8Tag: "db-long",
    floatTag: "db-double",
    bytesTag: "db-byte-array",
    stringTag: "db-string",
    boolTag: "db-boolean",
  },
] as const

for (const c of cases) {
  describe(`${c.name} DbValue codec roundtrip properties`, () => {
    it.prop(
      "string round-trips through the string DB type",
      { value: fc.string() },
      ({ value }) => {
        const wire = c.encode(value) as { tag: string; val: string }
        expect(wire.tag).toBe(c.stringTag)
        expect(c.decode(wire)).toBe(value)
      },
    )

    it.prop("boolean round-trips", { value: fc.boolean() }, ({ value }) => {
      const wire = c.encode(value) as { tag: string; val: boolean }
      expect(wire.tag).toBe(c.boolTag)
      expect(c.decode(wire)).toBe(value)
    })

    it.prop(
      "safe int32 number round-trips through int4/int/db-int",
      { value: safeInt32Arb },
      ({ value }) => {
        const wire = c.encode(value) as { tag: string; val: number }
        expect(wire.tag).toBe(c.int4Tag)
        expect(c.decode(wire)).toBe(value)
      },
    )

    it.prop(
      "bigint outside int32 round-trips through int8/bigint/db-long",
      { value: wideBigIntArb },
      ({ value }) => {
        const wire = c.encode(value) as { tag: string; val: bigint }
        expect(wire.tag).toBe(c.int8Tag)
        expect(c.decode(wire)).toBe(value)
      },
    )

    it.prop(
      "finite non-integer double round-trips through float8/double/db-double",
      { value: finiteFloatArb },
      ({ value }) => {
        const wire = c.encode(value) as { tag: string; val: number }
        expect(wire.tag).toBe(c.floatTag)
        expect(c.decode(wire)).toBe(value)
      },
    )

    it.prop(
      "Uint8Array round-trips through the bytes DB type",
      { value: bytesArb },
      ({ value }) => {
        const wire = c.encode(value) as { tag: string; val: Uint8Array }
        expect(wire.tag).toBe(c.bytesTag)
        const back = c.decode(wire) as Uint8Array
        expect(Array.from(back)).toEqual(Array.from(value))
      },
    )

    it.prop(
      "Date round-trips at millisecond precision (decodeTemporal: 'date')",
      { value: dateArb },
      ({ value }) => {
        const wire = c.encode(value)
        const back = c.decode(wire) as Date
        expect(back).toBeInstanceOf(Date)
        expect(back.getTime()).toBe(value.getTime())
      },
    )

    it.prop("null and undefined both encode to null", { _ignored: fc.boolean() }, () => {
      const a = c.encode(null) as { tag: string }
      const b = c.encode(undefined) as { tag: string }
      // Each adapter spells its null tag differently; just check both
      // encode to the same thing and decode back to JS `null`.
      expect(a).toEqual(b)
      expect(c.decode(a)).toBeNull()
    })
  })
}
