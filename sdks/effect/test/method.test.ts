import { describe, expect, it } from "@effect/vitest"
import { Effect, Exit, Result, Schema, SchemaGetter } from "effect"
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
import { schemaValueFromWit, schemaValueToWit } from "../src/internal/schema-model/wit.js"
import { v } from "../src/internal/schema-model/model.js"
import { Principal, PrincipalSchema, type PrincipalValue } from "../src/Principal.js"

const Person = Schema.Struct({ name: Schema.String, age: Schema.Number })
const greet = defineMethod({
  name: "greet",
  input: { person: Person, greeting: Schema.String },
  success: Schema.String,
  body: ({ person, greeting }) => Effect.succeed(`${greeting}, ${person.name} (${person.age})!`),
})

describe("Method", () => {
  for (const mixed of [false, true]) {
    it.effect(
      `injects the current principal through invokeMethod (${mixed ? "mixed" : "principal-only"})`,
      () =>
        Effect.gen(function* () {
          const compiled = yield* compileMethodSpec(
            "caller",
            method({
              input: mixed
                ? { user: Schema.String, caller: PrincipalSchema }
                : { caller: PrincipalSchema },
              success: Schema.String,
            }),
          )
          expect(compiled.inputSchema.val.map((field) => field.name)).toEqual(
            mixed ? ["user", "caller"] : ["caller"],
          )
          const wire = schemaValueToWit(v.record(mixed ? [v.string("Ada")] : []))
          const decoded = yield* compiled.inputCodec.decode(wire)
          expect(decoded).toEqual(mixed ? { user: "Ada" } : {})
          expect(decoded.caller).toBeUndefined()
          const principals: PrincipalValue[] = [
            { tag: "anonymous" },
            {
              tag: "agent",
              val: {
                agentId: {
                  componentId: { uuid: { highBits: 1n, lowBits: 2n } },
                  agentId: "current",
                },
              },
            },
          ]
          for (const principal of principals) {
            const output = yield* invokeMethod(
              compiled,
              (input) => {
                expect(input).toEqual(
                  mixed ? { user: "Ada", caller: principal } : { caller: principal },
                )
                return Effect.succeed(input.caller.tag)
              },
              wire,
            ).pipe(Effect.provideService(Principal, principal))
            expect(schemaValueFromWit(output!)).toEqual(v.string(principal.tag))
          }
        }),
    )
  }

  it.effect("projects caller codec types while retaining full handler input", () =>
    Effect.gen(function* () {
      const compiled = yield* compileMethodSpec(
        "mixed",
        method({ input: { user: Schema.String, caller: PrincipalSchema }, success: Schema.String }),
      )
      const wire = yield* compiled.inputCodec.encode({ user: "Ada" })
      const decoded = yield* compiled.inputCodec.decode(wire)
      // @ts-expect-error caller codecs do not decode host-injected fields
      expect(decoded.caller).toBeUndefined()
      expect(decoded.user).toBe("Ada")
      const principal: PrincipalValue = { tag: "anonymous" }
      const output = yield* invokeMethod(
        compiled,
        ({ user, caller }) => Effect.succeed(`${user}:${caller.tag}`),
        wire,
      ).pipe(Effect.provideService(Principal, principal))
      expect(schemaValueFromWit(output!)).toEqual(v.string("Ada:anonymous"))
    }),
  )

  it.effect("reuses prepared codecs without caching transformed values or validation results", () =>
    Effect.gen(function* () {
      const pattern = new RegExp("^item-[0-9]+$")
      let decodes = 0
      let encodes = 0
      const transformed = Schema.String.check(Schema.isPattern(pattern)).pipe(
        Schema.decodeTo(Schema.Number, {
          decode: SchemaGetter.transform((value) => {
            decodes++
            return Number(value.slice(5))
          }),
          encode: SchemaGetter.transform((value) => {
            encodes++
            return `item-${value}`
          }),
        }),
      )
      const compiled = yield* compileMethodSpec(
        "transform",
        method({ input: { value: transformed }, success: transformed }),
      )
      expect([decodes, encodes]).toEqual([0, 0])
      for (const n of [17, 42, 17]) {
        const input = schemaValueToWit(v.record([v.string(`item-${n}`)]))
        const output = yield* invokeMethod(
          compiled,
          ({ value }) => Effect.succeed(value + 3),
          input,
        )
        expect(schemaValueFromWit(output!)).toEqual(v.string(`item-${n + 3}`))
      }
      expect([decodes, encodes]).toEqual([3, 3])
      const invalid = schemaValueToWit(v.record([v.string("not-an-item")]))
      expect(Exit.isFailure(yield* Effect.exit(compiled.inputCodec.decode(invalid)))).toBe(true)
      expect(Exit.isFailure(yield* Effect.exit(compiled.encodeOutput!(-1)))).toBe(true)
    }),
  )

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
          input: { principal: PrincipalSchema },
          success: Schema.String,
          readOnly: { cache: "no-cache" },
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
