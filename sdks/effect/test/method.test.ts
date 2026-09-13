import { describe, expect, it } from "@effect/vitest"
import { Effect, Result, Schema } from "effect"
import { get, post } from "../src/Http.js"
import {
  compileMethod,
  compileMethodSpec,
  defineMethod,
  invoke,
  invokeMethod,
  method,
  withDescription,
  withHttp,
  withPromptHint,
} from "../src/Method.js"
import { schemaValueFromWit } from "../src/internal/schema-model/wit.js"

const Person = Schema.Struct({ name: Schema.String, age: Schema.Number })
const greet = defineMethod({
  name: "greet",
  input: { person: Person, greeting: Schema.String },
  success: Schema.String,
  body: ({ person, greeting }) => Effect.succeed(`${greeting}, ${person.name} (${person.age})!`),
})

describe("Method", () => {
  it.effect("invokes decoded methods", () =>
    Effect.gen(function* () {
      expect(yield* invoke(greet, { person: { name: "Ada", age: 36 }, greeting: "Hello" })).toBe(
        "Hello, Ada (36)!",
      )
    }),
  )

  it.effect("compiles one input record graph and round-trips it transactionally", () =>
    Effect.gen(function* () {
      const compiled = yield* compileMethod(greet)
      expect(compiled.inputSchema.tag).toBe("parameters")
      expect(compiled.inputSchema.val.map((f) => f.name)).toEqual(["person", "greeting"])
      const value = { person: { name: "Ada", age: 36 }, greeting: "Hello" }
      const wire = yield* compiled.inputCodec.encode(value)
      expect(yield* compiled.inputCodec.decode(wire)).toEqual(value)
      expect(compiled.schemaGraph.typeNodes.length).toBeGreaterThan(0)
    }),
  )

  it.effect("uses unit output metadata and no output codec for void", () =>
    Effect.gen(function* () {
      const ping = defineMethod({
        name: "ping",
        input: {},
        success: Schema.Void,
        body: () => Effect.void,
      })
      const compiled = yield* compileMethod(ping)
      expect(compiled.outputSchema).toEqual({ tag: "unit" })
      expect(compiled.outputCodec).toBeUndefined()
      const input = yield* compiled.inputCodec.encode({})
      expect(yield* invokeMethod(compiled, ping.body, input)).toBeUndefined()
    }),
  )

  it.effect("folds typed failures into Schema.Result", () =>
    Effect.gen(function* () {
      const ErrorSchema = Schema.Struct({ _tag: Schema.Literal("Missing"), id: Schema.String })
      const lookup = defineMethod({
        name: "lookup",
        input: { id: Schema.String },
        success: Schema.Number,
        error: ErrorSchema,
        body: ({ id }) => Effect.fail({ _tag: "Missing" as const, id }),
      })
      const compiled = yield* compileMethod(lookup)
      const input = yield* compiled.inputCodec.encode({ id: "x" })
      const output = yield* invokeMethod(compiled, lookup.body, input)
      const decoded = yield* Schema.decodeEffect(compiled.outputCodec!.codec)(
        schemaValueFromWit(output!),
      )
      const result = decoded as Result.Result<unknown, unknown>
      expect(Result.isFailure(result)).toBe(true)
      if (Result.isFailure(result)) expect(result.failure).toEqual({ _tag: "Missing", id: "x" })
    }),
  )

  it.effect("compiles canonical read-only policies", () =>
    Effect.gen(function* () {
      const defaultPolicy = yield* compileMethodSpec(
        "default",
        method({ input: {}, success: Schema.String, readOnly: true }),
      )
      const noCache = yield* compileMethodSpec(
        "no-cache",
        method({
          input: {},
          success: Schema.String,
          readOnly: { cache: "no-cache", usesPrincipal: true },
        }),
      )
      const ttl = yield* compileMethodSpec(
        "ttl",
        method({ input: {}, success: Schema.String, readOnly: { cache: { ttlNanos: 42n } } }),
      )
      expect(defaultPolicy.readOnly).toEqual({
        cachePolicy: { tag: "until-write" },
        usesPrincipal: false,
      })
      expect(noCache.readOnly).toEqual({
        cachePolicy: { tag: "no-cache" },
        usesPrincipal: true,
      })
      expect(ttl.readOnly).toEqual({ cachePolicy: { tag: "ttl", val: 42n }, usesPrincipal: false })
    }),
  )

  it("preserves the pipeable DSL and HTTP validation surface", () => {
    const base = method({ input: { by: Schema.Number }, success: Schema.Number })
    const piped = base.pipe(
      withHttp(post("/add"), get("/add?by={by}")),
      withDescription("Add by"),
      withPromptHint("Increment"),
    )
    expect(piped.http?.length).toBe(2)
    expect(piped.description).toBe("Add by")
    expect(piped.promptHint).toBe("Increment")
    expect(base.description).toBeUndefined()

    method({
      input: { id: Schema.String },
      success: Schema.String,
      // @ts-expect-error endpoint refers to a missing input field
    }).pipe(withHttp(get("/items/{id}/{missing}")))
  })
})
