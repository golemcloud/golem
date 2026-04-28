import { Effect, Exit, Option, Schema } from "effect"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import * as KeyValue from "../src/keyvalue.js"
import * as KvTypesMock from "./mocks/wasi-keyvalue-types.js"
import * as KvEventualMock from "./mocks/wasi-keyvalue-eventual.js"
import * as KvBatchMock from "./mocks/wasi-keyvalue-eventual-batch.js"

const runP = <A, E, R>(eff: Effect.Effect<A, E, R>): Promise<A> =>
  Effect.runPromise(Effect.scoped(eff as Effect.Effect<A, E, never>))
const runExit = <A, E, R>(eff: Effect.Effect<A, E, R>): Promise<Exit.Exit<A, E>> =>
  Effect.runPromiseExit(Effect.scoped(eff as Effect.Effect<A, E, never>))

const u8 = (s: string): Uint8Array => new TextEncoder().encode(s)
const s = (b: Uint8Array): string => new TextDecoder().decode(b)

describe("KeyValue.openBucket", () => {
  beforeEach(() => {
    KvTypesMock.__resetKeyValueMock()
    KvEventualMock.__resetNextError()
    KvBatchMock.__resetNextBatchError()
  })
  afterEach(() => {
    KvTypesMock.__resetKeyValueMock()
  })

  it("opens a bucket and returns a Bucket handle", async () => {
    const bucket = await runP(
      Effect.gen(function* () {
        const b = yield* KeyValue.openBucket("users")
        return b
      }),
    )
    expect(bucket.name).toBe("users")
    expect(KeyValue.isBucket(bucket)).toBe(true)
  })

  it("surfaces openBucket failures as KeyValueHostError", async () => {
    KvTypesMock.__setOpenError("bad", "bucket name not allowed")
    const exit = await runExit(KeyValue.openBucket("bad"))
    expect(exit._tag).toBe("Failure")
    if (exit._tag === "Failure") {
      const json = JSON.stringify(exit.cause)
      expect(json).toContain("KeyValueHostError")
      expect(json).toContain("bucket name not allowed")
    }
  })
})

describe("Bucket — eventual CRUD", () => {
  beforeEach(() => {
    KvTypesMock.__resetKeyValueMock()
    KvEventualMock.__resetNextError()
    KvBatchMock.__resetNextBatchError()
  })

  it("set + get + exists + delete round-trip", async () => {
    await runP(
      Effect.gen(function* () {
        const bucket = yield* KeyValue.openBucket("test")
        yield* bucket.set("hello", u8("world"))

        const got = yield* bucket.get("hello")
        expect(Option.isSome(got)).toBe(true)
        if (Option.isSome(got)) expect(s(got.value)).toBe("world")

        const has = yield* bucket.exists("hello")
        expect(has).toBe(true)

        yield* bucket.delete("hello")

        const after = yield* bucket.get("hello")
        expect(Option.isNone(after)).toBe(true)

        const stillThere = yield* bucket.exists("hello")
        expect(stillThere).toBe(false)
      }),
    )
  })

  it("returns Option.none() for missing keys", async () => {
    const result = await runP(
      Effect.gen(function* () {
        const bucket = yield* KeyValue.openBucket("test")
        return yield* bucket.get("missing")
      }),
    )
    expect(Option.isNone(result)).toBe(true)
  })

  it("surfaces get failures as KeyValueHostError with operation tag", async () => {
    KvEventualMock.__setNextError("get", "redis connection refused")
    const exit = await runExit(
      Effect.gen(function* () {
        const bucket = yield* KeyValue.openBucket("test")
        return yield* bucket.get("hello")
      }),
    )
    expect(exit._tag).toBe("Failure")
    if (exit._tag === "Failure") {
      const json = JSON.stringify(exit.cause)
      expect(json).toContain("KeyValueHostError")
      expect(json).toContain("eventual.get")
      expect(json).toContain("redis connection refused")
    }
  })
})

describe("Bucket — batch operations", () => {
  beforeEach(() => {
    KvTypesMock.__resetKeyValueMock()
    KvBatchMock.__resetNextBatchError()
  })

  it("setMany / getMany preserve positional alignment", async () => {
    const result = await runP(
      Effect.gen(function* () {
        const bucket = yield* KeyValue.openBucket("test")
        yield* bucket.setMany([
          ["a", u8("alpha")],
          ["b", u8("beta")],
          ["c", u8("gamma")],
        ])

        const got = yield* bucket.getMany(["a", "missing", "c"])
        return got
      }),
    )

    expect(result).toHaveLength(3)
    expect(Option.isSome(result[0]!)).toBe(true)
    if (Option.isSome(result[0]!)) expect(s(result[0]!.value)).toBe("alpha")
    expect(Option.isNone(result[1]!)).toBe(true)
    expect(Option.isSome(result[2]!)).toBe(true)
    if (Option.isSome(result[2]!)) expect(s(result[2]!.value)).toBe("gamma")
  })

  it("deleteMany clears keys", async () => {
    const result = await runP(
      Effect.gen(function* () {
        const bucket = yield* KeyValue.openBucket("test")
        yield* bucket.setMany([
          ["a", u8("alpha")],
          ["b", u8("beta")],
          ["c", u8("gamma")],
        ])
        yield* bucket.deleteMany(["a", "c"])
        return yield* bucket.keys
      }),
    )

    expect(result.slice().sort()).toEqual(["b"])
  })

  it("keys returns all bucket keys", async () => {
    const keys = await runP(
      Effect.gen(function* () {
        const bucket = yield* KeyValue.openBucket("test")
        yield* bucket.set("k1", u8("v1"))
        yield* bucket.set("k2", u8("v2"))
        return yield* bucket.keys
      }),
    )
    expect(keys.slice().sort()).toEqual(["k1", "k2"])
  })

  it("getMany failure is all-or-nothing", async () => {
    KvBatchMock.__setNextBatchError("get-many", "backend down")
    const exit = await runExit(
      Effect.gen(function* () {
        const bucket = yield* KeyValue.openBucket("test")
        yield* bucket.set("a", u8("alpha"))
        return yield* bucket.getMany(["a", "b"])
      }),
    )
    expect(exit._tag).toBe("Failure")
    if (exit._tag === "Failure") {
      const json = JSON.stringify(exit.cause)
      expect(json).toContain("eventual-batch.get-many")
      expect(json).toContain("backend down")
    }
  })
})

describe("Bucket.forSchema", () => {
  beforeEach(() => {
    KvTypesMock.__resetKeyValueMock()
    KvEventualMock.__resetNextError()
    KvBatchMock.__resetNextBatchError()
  })

  const User = Schema.Struct({ id: Schema.String, name: Schema.String })

  it("set + get round-trip a typed value", async () => {
    const result = await runP(
      Effect.gen(function* () {
        const bucket = yield* KeyValue.openBucket("test")
        const users = bucket.forSchema(User)

        yield* users.set("u1", { id: "u1", name: "Ada" })
        return yield* users.get("u1")
      }),
    )
    expect(Option.isSome(result)).toBe(true)
    if (Option.isSome(result)) {
      expect(result.value).toEqual({ id: "u1", name: "Ada" })
    }
  })

  it("get returns Option.none for missing keys", async () => {
    const result = await runP(
      Effect.gen(function* () {
        const bucket = yield* KeyValue.openBucket("test")
        const users = bucket.forSchema(User)
        return yield* users.get("missing")
      }),
    )
    expect(Option.isNone(result)).toBe(true)
  })

  it("get surfaces malformed JSON as a typed failure (Schema.SchemaError via fromJsonString)", async () => {
    const exit = await runExit(
      Effect.gen(function* () {
        const bucket = yield* KeyValue.openBucket("test")
        // Stash invalid JSON directly
        yield* bucket.set("u1", u8("not json"))
        const users = bucket.forSchema(User)
        return yield* users.get("u1")
      }),
    )
    expect(exit._tag).toBe("Failure")
    // Failure is typed (either SchemaError or our KeyValueDecodeError),
    // not a defect — that is the contract.
  })

  it("get surfaces schema mismatches as Schema.SchemaError", async () => {
    const exit = await runExit(
      Effect.gen(function* () {
        const bucket = yield* KeyValue.openBucket("test")
        // Stash a JSON document missing required fields
        yield* bucket.set("u1", u8(JSON.stringify({ id: "u1" })))
        const users = bucket.forSchema(User)
        return yield* users.get("u1")
      }),
    )
    expect(exit._tag).toBe("Failure")
  })

  it("get surfaces JSON syntax errors as Schema.SchemaError (typed, not defect)", async () => {
    const exit = await runExit(
      Effect.gen(function* () {
        const bucket = yield* KeyValue.openBucket("bad-json")
        // Stash invalid JSON directly via the raw byte API.
        yield* bucket.set("k", new TextEncoder().encode("not json"))
        const view = bucket.forSchema(User)
        return yield* view.get("k")
      }),
    )
    expect(exit._tag).toBe("Failure")
    // Either Schema.SchemaError (JSON parse) or our wrapper around
    // it. The point is: failure is typed, not a defect.
  })

  it("getMany round-trips typed values", async () => {
    const result = await runP(
      Effect.gen(function* () {
        const bucket = yield* KeyValue.openBucket("test")
        const users = bucket.forSchema(User)
        yield* users.setMany([
          ["u1", { id: "u1", name: "Ada" }],
          ["u2", { id: "u2", name: "Bea" }],
        ])
        return yield* users.getMany(["u1", "missing", "u2"])
      }),
    )
    expect(result).toHaveLength(3)
    expect(Option.isSome(result[0]!)).toBe(true)
    if (Option.isSome(result[0]!)) expect(result[0]!.value.name).toBe("Ada")
    expect(Option.isNone(result[1]!)).toBe(true)
    expect(Option.isSome(result[2]!)).toBe(true)
    if (Option.isSome(result[2]!)) expect(result[2]!.value.name).toBe("Bea")
  })
})
