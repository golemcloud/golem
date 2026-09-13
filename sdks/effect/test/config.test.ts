import { describe, expect, it } from "@effect/vitest"
import { Effect, Layer, Option, Redacted, Schema } from "effect"
import type * as CoreTypes from "golem:core/types@2.0.0"
import { ConfigError, compileConfig, defineConfig, encodeOverrides } from "../src/Config.js"
import { ConfigClient } from "../src/host/ConfigClient.js"
import { SecretsClient } from "../src/host/SecretsClient.js"

const compile = <F extends Readonly<Record<string, Schema.Top>>>(fields: F) =>
  compileConfig(fields, "test")

const configLayer = (
  get: (path: ReadonlyArray<string>, expected: CoreTypes.SchemaGraph) => CoreTypes.SchemaValueTree,
) =>
  Layer.mergeAll(
    Layer.succeed(ConfigClient, ConfigClient.of({ getConfigValue: get })),
    secretsLayer(() => {
      throw new Error("unexpected secret reveal")
    }),
  )

const secretsLayer = (
  reveal: (secret: CoreTypes.Secret, expected: CoreTypes.SchemaGraph) => CoreTypes.SchemaValueTree,
) =>
  Layer.succeed(
    SecretsClient,
    SecretsClient.of({
      reveal,
      id: () => ({}) as never,
      metadata: () => ({}) as never,
    }),
  )

describe("Config 1.6", () => {
  it.effect("flattens nested fields and emits graph indices", () =>
    Effect.gen(function* () {
      const compiled = yield* compile({
        greeting: Schema.String,
        database: Schema.Struct({
          port: Schema.Number,
          password: Schema.Redacted(Schema.String),
        }),
      })
      expect(compiled.leaves.map((leaf) => [leaf.source, leaf.path])).toEqual([
        ["local", ["greeting"]],
        ["local", ["database", "port"]],
        ["secret", ["database", "password"]],
      ])
      expect(compiled.declarations([4, 7, 9])).toEqual([
        { source: "local", path: ["greeting"], valueType: 4 },
        { source: "local", path: ["database", "port"], valueType: 7 },
        { source: "secret", path: ["database", "password"], valueType: 9 },
      ])
    }),
  )

  it.effect("option-lifts required leaves below optional objects", () =>
    Effect.gen(function* () {
      const compiled = yield* compile({
        database: Schema.optional(
          Schema.Struct({ required: Schema.String, optional: Schema.optional(Schema.Number) }),
        ),
      })
      expect(compiled.leaves.map((leaf) => leaf.path)).toEqual([
        ["database", "required"],
        ["database", "optional"],
      ])
      expect(compiled.leaves[0]!.declarationGraph.root.body.tag).toBe("option")
      expect(compiled.leaves[1]!.declarationGraph.root.body.tag).toBe("option")
      expect(compiled.leaves[0]!.required).toBe(true)
      expect(compiled.leaves[1]!.required).toBe(false)
    }),
  )

  it.effect("exposes optional object descendants through the public config service", () =>
    Effect.gen(function* () {
      const PublicConfig = defineConfig("PublicConfig", {
        database: Schema.optional(
          Schema.Struct({
            host: Schema.String,
            password: Schema.Redacted(Schema.String),
          }),
        ),
      })
      const compiled = yield* PublicConfig.__compile()
      const hostLeaf = compiled.leaves.find((leaf) => leaf.path.join("/") === "database/host")!
      const passwordLeaf = compiled.leaves.find(
        (leaf) => leaf.path.join("/") === "database/password",
      )!
      expect(hostLeaf.declarationGraph.root.body.tag).toBe("option")
      expect(passwordLeaf.declarationGraph.root.body.tag).toBe("secret")
      if (passwordLeaf.declarationGraph.root.body.tag === "secret")
        expect(passwordLeaf.declarationGraph.root.body.inner.body.tag).toBe("option")

      const handle = {} as CoreTypes.Secret
      const secretTree: CoreTypes.SchemaValueTree = {
        root: 0,
        valueNodes: [{ tag: "secret-value", val: handle }],
      }
      let password: string | undefined
      let reveals = 0
      const shape = yield* compiled.buildShape().pipe(
        Effect.provide(
          Layer.mergeAll(
            configLayer((path) => {
              if (path.join("/") === "database/password") return secretTree
              return Effect.runSync(hostLeaf.codec.encode(undefined) as Effect.Effect<any, any>)
            }),
            secretsLayer(() => {
              reveals++
              return Effect.runSync(passwordLeaf.codec.encode(password) as Effect.Effect<any, any>)
            }),
          ),
        ),
      )
      yield* Effect.gen(function* () {
        const cfg = yield* PublicConfig
        expect(yield* cfg.database.host).toBeUndefined()
        expect(Redacted.value(yield* cfg.database.password.get)).toBeUndefined()
        password = "updated-value"
        expect(Redacted.value(yield* cfg.database.password.get)).toBe("updated-value")
      }).pipe(Effect.provideService(PublicConfig, shape as never))
      expect(reveals).toBe(2)
    }),
  )

  it.effect("memoizes local values per shape", () =>
    Effect.gen(function* () {
      const compiled = yield* compile({ greeting: Schema.String })
      let current = "first"
      let calls = 0
      const shape = (yield* compiled.buildShape().pipe(
        Effect.provide(
          configLayer(() => {
            calls++
            return Effect.runSync(
              compiled.leaves[0]!.codec.encode(current) as Effect.Effect<any, any>,
            )
          }),
        ),
      )) as { greeting: Effect.Effect<string, ConfigError> }
      expect(yield* shape.greeting).toBe("first")
      current = "second"
      expect(yield* shape.greeting).toBe("first")
      expect(calls).toBe(1)

      const nextShape = (yield* compiled.buildShape().pipe(
        Effect.provide(
          configLayer(() => {
            calls++
            return Effect.runSync(
              compiled.leaves[0]!.codec.encode(current) as Effect.Effect<any, any>,
            )
          }),
        ),
      )) as { greeting: Effect.Effect<string, ConfigError> }
      expect(yield* nextShape.greeting).toBe("second")
      expect(calls).toBe(2)
    }),
  )

  it.effect("reveals an opaque secret freshly and returns a redacted value", () =>
    Effect.gen(function* () {
      const compiled = yield* compile({ apiKey: Schema.Redacted(Schema.String) })
      const handle = {} as CoreTypes.Secret
      const secretTree: CoreTypes.SchemaValueTree = {
        root: 0,
        valueNodes: [{ tag: "secret-value", val: handle }],
      }
      let current = "one"
      let reveals = 0
      const layer = Layer.mergeAll(
        configLayer(() => secretTree),
        secretsLayer((_secret, _expected) => {
          reveals++
          return Effect.runSync(
            compiled.leaves[0]!.codec.encode(current) as Effect.Effect<any, any>,
          )
        }),
      )
      const shape = (yield* compiled.buildShape().pipe(Effect.provide(layer))) as {
        apiKey: { get: Effect.Effect<Redacted.Redacted<string>, ConfigError> }
      }
      expect(Redacted.value(yield* shape.apiKey.get)).toBe("one")
      current = "two"
      expect(Redacted.value(yield* shape.apiKey.get)).toBe("two")
      expect(reveals).toBe(2)
    }),
  )

  it.effect("reveals a structured redacted secret", () =>
    Effect.gen(function* () {
      const schema = Schema.Struct({ token: Schema.String, generation: Schema.Number })
      const compiled = yield* compile({ credentials: Schema.Redacted(schema) })
      const handle = {} as CoreTypes.Secret
      const layer = Layer.mergeAll(
        configLayer(() => ({ root: 0, valueNodes: [{ tag: "secret-value", val: handle }] })),
        secretsLayer(() =>
          Effect.runSync(
            compiled.leaves[0]!.codec.encode({ token: "abc", generation: 2 }) as Effect.Effect<
              any,
              any
            >,
          ),
        ),
      )
      const shape = (yield* compiled.buildShape().pipe(Effect.provide(layer))) as {
        credentials: {
          get: Effect.Effect<Redacted.Redacted<{ token: string; generation: number }>, ConfigError>
        }
      }
      expect(Redacted.value(yield* shape.credentials.get)).toEqual({ token: "abc", generation: 2 })
    }),
  )

  it.effect("round-trips Option none and some and declares an option graph", () =>
    Effect.gen(function* () {
      const compiled = yield* compile({ redisUrl: Schema.Option(Schema.String) })
      expect(compiled.leaves[0]!.declarationGraph.root.body.tag).toBe("option")
      for (const expected of [Option.none(), Option.some("redis://localhost")]) {
        const tree = yield* compiled.leaves[0]!.codec.encode(expected) as Effect.Effect<any, any>
        const shape = (yield* compiled
          .buildShape()
          .pipe(Effect.provide(configLayer(() => tree)))) as {
          redisUrl: Effect.Effect<Option.Option<string>, ConfigError>
        }
        const actual = yield* shape.redisUrl
        expect(Option.isSome(actual)).toBe(Option.isSome(expected))
        if (Option.isSome(actual)) expect(actual.value).toBe("redis://localhost")
      }
    }),
  )

  it.effect("materializes empty structs without reading the host", () =>
    Effect.gen(function* () {
      const compiled = yield* compile({ empty: Schema.Struct({}) })
      let calls = 0
      const shape = (yield* compiled.buildShape().pipe(
        Effect.provide(
          configLayer(() => {
            calls++
            throw new Error("unexpected")
          }),
        ),
      )) as { empty: Record<string, unknown> }
      expect(shape.empty).toEqual({})
      expect(calls).toBe(0)
    }),
  )

  it.effect("does not require the secrets host for local-only config", () =>
    Effect.gen(function* () {
      const compiled = yield* compile({ greeting: Schema.String })
      const tree = yield* compiled.leaves[0]!.codec.encode("hello") as Effect.Effect<
        CoreTypes.SchemaValueTree,
        unknown,
        never
      >
      const shape = (yield* compiled
        .buildShape()
        .pipe(Effect.provide(configLayer(() => tree)))) as {
        greeting: Effect.Effect<string, ConfigError>
      }
      expect(yield* shape.greeting).toBe("hello")
    }),
  )

  it.effect("reports host and decode failures with the leaf path", () =>
    Effect.gen(function* () {
      const compiled = yield* compile({ port: Schema.Number })
      const trapped = yield* Effect.result(
        Effect.gen(function* () {
          const shape = (yield* compiled.buildShape().pipe(
            Effect.provide(
              configLayer(() => {
                throw new Error("denied")
              }),
            ),
          )) as { port: Effect.Effect<number, ConfigError> }
          return yield* shape.port
        }),
      )
      expect(trapped._tag).toBe("Failure")
      if (trapped._tag === "Failure") expect(trapped.failure.reason._tag).toBe("HostTrap")

      const stringCompiled = yield* compile({ value: Schema.String })
      const wrong = yield* stringCompiled.leaves[0]!.codec.encode("no") as Effect.Effect<
        CoreTypes.SchemaValueTree,
        unknown,
        never
      >
      const shape = (yield* compiled
        .buildShape()
        .pipe(Effect.provide(configLayer(() => wrong)))) as {
        port: Effect.Effect<number, ConfigError>
      }
      const decoded = yield* Effect.result(shape.port)
      expect(decoded._tag).toBe("Failure")
      if (decoded._tag === "Failure") {
        expect(decoded.failure.path).toEqual(["port"])
        expect(decoded.failure.reason._tag).toBe("DecodeFailure")
      }
    }),
  )

  it.effect("encodes nested non-secret overrides and rejects secret or unknown paths", () =>
    Effect.gen(function* () {
      const compiled = yield* compile({
        database: Schema.Struct({
          host: Schema.String,
          password: Schema.Redacted(Schema.String),
        }),
      })
      const encoded = yield* encodeOverrides(compiled, { database: { host: "db" } })
      expect(encoded).toHaveLength(1)
      expect(encoded[0]!.path).toEqual(["database", "host"])
      for (const overrides of [
        { database: { password: "leak" } },
        { database: { unknown: true } },
        { database: { unknown: {} } },
        { unknown: {} },
      ]) {
        const result = yield* Effect.result(encodeOverrides(compiled, overrides))
        expect(result._tag).toBe("Failure")
        if (result._tag === "Failure" && result.failure instanceof ConfigError)
          expect(result.failure.reason._tag).toBe("Unsupported")
      }
    }),
  )

  it.effect("rejects unsupported leaves and memoizes config compilation", () =>
    Effect.gen(function* () {
      expect((yield* Effect.result(compileConfig({ value: Schema.Any }, "test")))._tag).toBe(
        "Failure",
      )
      const TestConfig = defineConfig("TestConfig", { greeting: Schema.String })
      expect(yield* TestConfig.__compile()).toBe(yield* TestConfig.__compile())
    }),
  )

  it.effect("keeps duplicate nested leaf paths distinct", () =>
    Effect.gen(function* () {
      const compiled = yield* compile({
        a: Schema.Struct({ value: Schema.String }),
        b: Schema.Struct({ value: Schema.Number }),
      })
      const paths = compiled.leaves.map((leaf) => leaf.path.join("/"))
      expect(new Set(paths).size).toBe(paths.length)
      expect(paths.sort()).toEqual(["a/value", "b/value"])
    }),
  )

  it("defines a Context service with config statics", () => {
    const TestConfig = defineConfig("Statics", { greeting: Schema.String })
    expect(TestConfig.fields).toEqual({ greeting: Schema.String })
    expect(typeof TestConfig.__compile).toBe("function")
  })
})
