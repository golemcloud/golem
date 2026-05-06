import { describe, expect, it } from "@effect/vitest"
import { Effect, Layer, Option, Redacted, Schema } from "effect"
import { ConfigError, compileConfig, defineConfig, encodeOverrides } from "../src/Config.js"
import type { CompiledConfig } from "../src/Config.js"
import { ConfigClient } from "../src/host/ConfigClient.js"
import { toWitCodec } from "../src/WitCodec.js"
import type * as CoreTypes from "golem:core/types@1.5.0"

type WitType = CoreTypes.WitType
type WitValue = CoreTypes.WitValue

const encode = <S extends Schema.Top>(
  schema: S,
  value: S["Type"],
): Effect.Effect<WitValue, unknown> =>
  Effect.gen(function* () {
    const codec = yield* toWitCodec(schema)
    return yield* Schema.encodeEffect(codec.codec)(value) as Effect.Effect<WitValue, unknown, never>
  })

const compile = <F extends Record<string, Schema.Top>>(
  fields: F,
): Effect.Effect<CompiledConfig, unknown> => compileConfig(fields, "test")

/**
 * Build a `ConfigClient` test layer over a synchronous responder. The
 * layer is local to each `it.effect` body so per-test state never
 * leaks across tests (replaces the previous module-level
 * `__setGetConfigValueForTest` / `__resetGetConfigValueForTest`
 * indirection).
 */
const ConfigStub = (
  responder: (key: ReadonlyArray<string>, expectedType: WitType) => WitValue,
): Layer.Layer<ConfigClient> =>
  Layer.succeed(
    ConfigClient,
    ConfigClient.of({
      getConfigValue: responder,
    }),
  )

/** Default stub — fails any unexpected call (used when a test never reads a leaf). */
const ConfigStubFail: Layer.Layer<ConfigClient> = ConfigStub(() => {
  throw new Error("ConfigClient: unexpected getConfigValue call in test")
})

describe("compileConfig", () => {
  it.effect("emits one local declaration per primitive field", () =>
    Effect.gen(function* () {
      const cc = yield* compile({
        greeting: Schema.String,
        port: Schema.Number,
      })
      expect(cc.declarations.length).toBe(2)
      const greet = cc.declarations.find((d) => d.path[0] === "greeting")!
      expect(greet.source).toBe("local")
      expect(greet.path).toEqual(["greeting"])
      expect(greet.valueType.nodes[0]!.type.tag).toBe("prim-string-type")
      const port = cc.declarations.find((d) => d.path[0] === "port")!
      expect(port.source).toBe("local")
      expect(port.valueType.nodes[0]!.type.tag).toBe("prim-f64-type")
    }),
  )

  it.effect("Schema.Redacted leaves are emitted as `secret` source", () =>
    Effect.gen(function* () {
      const cc = yield* compile({
        apiKey: Schema.Redacted(Schema.String),
      })
      expect(cc.declarations.length).toBe(1)
      const decl = cc.declarations[0]!
      expect(decl.source).toBe("secret")
      expect(decl.path).toEqual(["apiKey"])
      // valueType uses the inner schema's WIT representation.
      expect(decl.valueType.nodes[0]!.type.tag).toBe("prim-string-type")
    }),
  )

  it.effect("recurses into nested Struct, prefixing the path", () =>
    Effect.gen(function* () {
      const cc = yield* compile({
        database: Schema.Struct({
          host: Schema.String,
          password: Schema.Redacted(Schema.String),
        }),
      })
      expect(cc.declarations.length).toBe(2)
      const host = cc.declarations.find((d) => d.path.join(".") === "database.host")!
      expect(host.source).toBe("local")
      const pwd = cc.declarations.find((d) => d.path.join(".") === "database.password")!
      expect(pwd.source).toBe("secret")
    }),
  )

  it.effect("memoizes regular fields per buildShape (per-invocation cache)", () =>
    Effect.gen(function* () {
      const cc = yield* compile({ greeting: Schema.String })
      const greetingWv = yield* encode(Schema.String, "hi")
      let calls = 0
      const stub = ConfigStub((_path, _type) => {
        calls++
        return greetingWv
      })

      // One invocation: build shape, read field twice → 1 host call.
      const shape1 = (yield* cc.buildShape().pipe(Effect.provide(stub))) as {
        greeting: Effect.Effect<string, ConfigError>
      }
      const v1 = yield* shape1.greeting
      const v2 = yield* shape1.greeting
      expect(v1).toBe("hi")
      expect(v2).toBe("hi")
      expect(calls).toBe(1)

      // A *different* shape (next invocation) re-fetches.
      const shape2 = (yield* cc.buildShape().pipe(Effect.provide(stub))) as {
        greeting: Effect.Effect<string, ConfigError>
      }
      yield* shape2.greeting
      expect(calls).toBe(2)
    }),
  )

  it.effect("never caches secret leaves", () =>
    Effect.gen(function* () {
      const cc = yield* compile({ apiKey: Schema.Redacted(Schema.String) })
      const wv = yield* encode(Schema.String, "sk-1234")
      let calls = 0
      const stub = ConfigStub(() => {
        calls++
        return wv
      })
      const shape = (yield* cc.buildShape().pipe(Effect.provide(stub))) as {
        apiKey: { get: Effect.Effect<Redacted.Redacted<string>, ConfigError> }
      }
      const r1 = yield* shape.apiKey.get
      const r2 = yield* shape.apiKey.get
      expect(Redacted.value(r1)).toBe("sk-1234")
      expect(Redacted.value(r2)).toBe("sk-1234")
      // N reads = N host calls for secret leaves.
      expect(calls).toBe(2)
    }),
  )

  it.effect("Schema.Redacted(Schema.Struct(...)) — structured secret round-trip", () =>
    Effect.gen(function* () {
      // Mirrors `golem-ts-sdk` PR #3329: `Secret<T>` accepts structured
      // record types, not just strings. Round-trips a redacted struct
      // value through the host.
      const InnerStruct = Schema.Struct({ foo: Schema.String, bar: Schema.Number })
      const cc = yield* compile({ apiToken: Schema.Redacted(InnerStruct) })
      const wv = yield* encode(InnerStruct, { foo: "abc", bar: 42 })
      const stub = ConfigStub(() => wv)
      const shape = (yield* cc.buildShape().pipe(Effect.provide(stub))) as {
        apiToken: {
          get: Effect.Effect<Redacted.Redacted<{ foo: string; bar: number }>, ConfigError>
        }
      }
      const r = yield* shape.apiToken.get
      const v = Redacted.value(r)
      expect(v.foo).toBe("abc")
      expect(v.bar).toBe(42)
    }),
  )

  it.effect("surfaces host traps as ConfigError(_, HostTrap)", () =>
    Effect.gen(function* () {
      const cc = yield* compile({ greeting: Schema.String })
      const stub = ConfigStub(() => {
        throw new Error("nope")
      })
      const shape = (yield* cc.buildShape().pipe(Effect.provide(stub))) as {
        greeting: Effect.Effect<string, ConfigError>
      }
      const result = yield* Effect.result(shape.greeting)
      expect(result._tag).toBe("Failure")
      if (result._tag !== "Failure") return
      const err = result.failure as ConfigError
      expect(err).toBeInstanceOf(ConfigError)
      expect(err.path).toEqual(["greeting"])
      expect(err.reason._tag).toBe("HostTrap")
    }),
  )

  it.effect("surfaces decode mismatches as ConfigError(_, DecodeFailure)", () =>
    Effect.gen(function* () {
      const cc = yield* compile({ port: Schema.Number })
      // Return a string WitValue when a number is expected.
      const stringWv = yield* encode(Schema.String, "not a number")
      const stub = ConfigStub(() => stringWv)

      const shape = (yield* cc.buildShape().pipe(Effect.provide(stub))) as {
        port: Effect.Effect<number, ConfigError>
      }
      const result = yield* Effect.result(shape.port)
      expect(result._tag).toBe("Failure")
      if (result._tag !== "Failure") return
      const err = result.failure as ConfigError
      expect(err.path).toEqual(["port"])
      expect(err.reason._tag).toBe("DecodeFailure")
    }),
  )

  it.effect("rejects unsupported leaves (e.g. Schema.Any) with UnsupportedSchemaError", () =>
    Effect.gen(function* () {
      const result = yield* Effect.result(
        compileConfig({ blob: Schema.Any }, "t") as Effect.Effect<unknown, unknown, never>,
      )
      expect(result._tag).toBe("Failure")
    }),
  )
})

describe("encodeOverrides", () => {
  it.effect("encodes simple non-secret leaves", () =>
    Effect.gen(function* () {
      const cc = yield* compile({ greeting: Schema.String, port: Schema.Number })
      const out = yield* encodeOverrides(cc, { greeting: "hello" }) as Effect.Effect<
        ReadonlyArray<unknown>,
        unknown,
        never
      >
      expect(out.length).toBe(1)
      expect((out[0] as { path: Array<string> }).path).toEqual(["greeting"])
    }),
  )

  it.effect("rejects overriding a secret leaf", () =>
    Effect.gen(function* () {
      const cc = yield* compile({
        apiKey: Schema.Redacted(Schema.String),
      })
      const result = yield* Effect.result(
        encodeOverrides(cc, { apiKey: "leak" }) as Effect.Effect<unknown, ConfigError, never>,
      )
      expect(result._tag).toBe("Failure")
      if (result._tag !== "Failure") return
      const err = result.failure as ConfigError
      expect(err.reason._tag).toBe("Unsupported")
    }),
  )

  it.effect("rejects unknown override paths", () =>
    Effect.gen(function* () {
      const cc = yield* compile({ greeting: Schema.String })
      const result = yield* Effect.result(
        encodeOverrides(cc, { unknownField: 1 }) as Effect.Effect<unknown, ConfigError, never>,
      )
      expect(result._tag).toBe("Failure")
    }),
  )

  it.effect("can encode nested struct overrides", () =>
    Effect.gen(function* () {
      const cc = yield* compile({
        database: Schema.Struct({
          host: Schema.String,
          password: Schema.Redacted(Schema.String),
        }),
      })
      const out = yield* encodeOverrides(cc, { database: { host: "db.example" } }) as Effect.Effect<
        ReadonlyArray<unknown>,
        unknown,
        never
      >
      expect(out.length).toBe(1)
      expect((out[0] as { path: Array<string> }).path).toEqual(["database", "host"])
    }),
  )

  it.effect("rejects an unknown top-level branch even when its value is an empty object", () =>
    Effect.gen(function* () {
      const cc = yield* compile({ greeting: Schema.String })
      const result = yield* Effect.result(
        encodeOverrides(cc, { bogus: {} }) as Effect.Effect<unknown, ConfigError, never>,
      )
      expect(result._tag).toBe("Failure")
    }),
  )

  it.effect("rejects an unknown nested branch under a real struct", () =>
    Effect.gen(function* () {
      const cc = yield* compile({
        database: Schema.Struct({
          host: Schema.String,
        }),
      })
      const result = yield* Effect.result(
        encodeOverrides(cc, { database: { bogus: {} } }) as Effect.Effect<
          unknown,
          ConfigError,
          never
        >,
      )
      expect(result._tag).toBe("Failure")
    }),
  )
})

describe("buildShape — empty struct branches", () => {
  it.effect("materialises an empty Schema.Struct({}) as `{}` in the shape", () =>
    Effect.gen(function* () {
      const cc = yield* compile({ empty: Schema.Struct({}) })
      const shape = (yield* cc.buildShape().pipe(Effect.provide(ConfigStubFail))) as {
        empty: Record<string, unknown>
      }
      expect(shape.empty).toBeDefined()
      expect(typeof shape.empty).toBe("object")
      expect(Object.keys(shape.empty).length).toBe(0)
    }),
  )
})

describe("Schema.Option leaves", () => {
  it.effect("a Schema.Option leaf round-trips Option.none() returned by the host", () =>
    Effect.gen(function* () {
      const cc = yield* compile({ redisUrl: Schema.Option(Schema.String) })
      // Pre-encode Option.none() through the same WitCodec the runtime
      // uses, so the mock returns a WitValue the decoder will accept.
      const noneWv = yield* encode(Schema.Option(Schema.String), Option.none())
      const stub = ConfigStub(() => noneWv)

      const shape = (yield* cc.buildShape().pipe(Effect.provide(stub))) as {
        redisUrl: Effect.Effect<Option.Option<string>, ConfigError>
      }
      const v = yield* shape.redisUrl
      expect(Option.isNone(v)).toBe(true)
    }),
  )

  it.effect("a Schema.Option leaf round-trips Option.some(x) returned by the host", () =>
    Effect.gen(function* () {
      const cc = yield* compile({ redisUrl: Schema.Option(Schema.String) })
      const someWv = yield* encode(Schema.Option(Schema.String), Option.some("redis://localhost"))
      const stub = ConfigStub(() => someWv)

      const shape = (yield* cc.buildShape().pipe(Effect.provide(stub))) as {
        redisUrl: Effect.Effect<Option.Option<string>, ConfigError>
      }
      const v = yield* shape.redisUrl
      expect(Option.isSome(v)).toBe(true)
      if (Option.isSome(v)) expect(v.value).toBe("redis://localhost")
    }),
  )

  it.effect("emits an option-type WitType in the AgentConfigDeclaration valueType", () =>
    Effect.gen(function* () {
      const cc = yield* compile({ redisUrl: Schema.Option(Schema.String) })
      expect(cc.declarations.length).toBe(1)
      expect(cc.declarations[0]!.valueType.nodes[0]!.type.tag).toBe("option-type")
    }),
  )
})

describe("compileConfig — duplicate-path guard", () => {
  it.effect("the schema walker cannot naturally produce duplicate paths", () =>
    Effect.gen(function* () {
      // JS object semantics already guarantee unique keys at each level —
      // this just sanity-checks that nested struct paths stay distinct.
      const cc = yield* compile({
        a: Schema.Struct({ x: Schema.String }),
        b: Schema.Struct({ x: Schema.Number }),
      })
      const paths = cc.declarations.map((d) => d.path.join("/"))
      expect(new Set(paths).size).toBe(paths.length)
      expect(paths.sort()).toEqual(["a/x", "b/x"])
    }),
  )
})

describe("defineConfig", () => {
  it("returns a Context.Service-compatible class with static fields", () => {
    const MyConfig = defineConfig("MyConfig", {
      greeting: Schema.String,
    })
    expect((MyConfig as unknown as { fields: unknown }).fields).toBeDefined()
    expect(typeof (MyConfig as unknown as { __compile: unknown }).__compile).toBe("function")
  })

  it.effect("static __compile returns the same compile bundle on repeat calls", () =>
    Effect.gen(function* () {
      const MyConfig = defineConfig("Repeat", {
        greeting: Schema.String,
      })
      const a = yield* MyConfig.__compile()
      const b = yield* MyConfig.__compile()
      // The cached compile cell returns the same bundle.
      expect(a).toBe(b)
    }),
  )
})
