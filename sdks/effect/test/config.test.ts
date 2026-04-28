import { describe, expect, it, beforeEach } from "@effect/vitest"
import { Effect, Option, Redacted, Schema } from "effect"
import {
  __resetGetConfigValueForTest,
  __setGetConfigValueForTest,
  ConfigError,
  compileConfig,
  defineConfig,
  encodeOverrides,
} from "../src/config.js"
import type { CompiledConfig } from "../src/config.js"
import { toWitCodec } from "../src/wit-codec.js"
import type * as CoreTypes from "golem:core/types@1.5.0"

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

describe("compileConfig", () => {
  beforeEach(() => {
    __resetGetConfigValueForTest()
  })

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
      __setGetConfigValueForTest((_path, _type) => {
        calls++
        return greetingWv
      })

      // One invocation: build shape, read field twice → 1 host call.
      const shape1 = (yield* cc.buildShape()) as {
        greeting: Effect.Effect<string, ConfigError>
      }
      const v1 = yield* shape1.greeting
      const v2 = yield* shape1.greeting
      expect(v1).toBe("hi")
      expect(v2).toBe("hi")
      expect(calls).toBe(1)

      // A *different* shape (next invocation) re-fetches.
      const shape2 = (yield* cc.buildShape()) as {
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
      __setGetConfigValueForTest(() => {
        calls++
        return wv
      })
      const shape = (yield* cc.buildShape()) as {
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

  it.effect("surfaces host traps as ConfigError(_, HostTrap)", () =>
    Effect.gen(function* () {
      const cc = yield* compile({ greeting: Schema.String })
      __setGetConfigValueForTest(() => {
        throw new Error("nope")
      })
      const shape = (yield* cc.buildShape()) as {
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
      __setGetConfigValueForTest(() => stringWv)

      const shape = (yield* cc.buildShape()) as {
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
  beforeEach(() => {
    __resetGetConfigValueForTest()
  })

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
  beforeEach(() => {
    __resetGetConfigValueForTest()
  })

  it.effect("materialises an empty Schema.Struct({}) as `{}` in the shape", () =>
    Effect.gen(function* () {
      const cc = yield* compile({ empty: Schema.Struct({}) })
      const shape = (yield* cc.buildShape()) as {
        empty: Record<string, unknown>
      }
      expect(shape.empty).toBeDefined()
      expect(typeof shape.empty).toBe("object")
      expect(Object.keys(shape.empty).length).toBe(0)
    }),
  )
})

describe("Schema.Option leaves", () => {
  beforeEach(() => {
    __resetGetConfigValueForTest()
  })

  it.effect("a Schema.Option leaf round-trips Option.none() returned by the host", () =>
    Effect.gen(function* () {
      const cc = yield* compile({ redisUrl: Schema.Option(Schema.String) })
      // Pre-encode Option.none() through the same WitCodec the runtime
      // uses, so the mock returns a WitValue the decoder will accept.
      const noneWv = yield* encode(Schema.Option(Schema.String), Option.none())
      __setGetConfigValueForTest(() => noneWv)

      const shape = (yield* cc.buildShape()) as {
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
      __setGetConfigValueForTest(() => someWv)

      const shape = (yield* cc.buildShape()) as {
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
  beforeEach(() => {
    __resetGetConfigValueForTest()
  })

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
  beforeEach(() => {
    __resetGetConfigValueForTest()
  })

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
