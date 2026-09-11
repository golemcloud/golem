import { describe, expect, it } from "@effect/vitest"
import { Effect, Exit, Result, Schema } from "effect"
import {
  compileMethod,
  compileMethodSpec,
  defineMethod,
  invoke,
  invokeDataValue,
  method,
  withDescription,
  withHttp,
  withPromptHint,
} from "../src/Method.js"
import { get, post } from "../src/Http.js"
import { toWitCodec } from "../src/WitCodec.js"

const Person = Schema.Struct({
  name: Schema.String,
  age: Schema.Number,
})

const greet = defineMethod({
  name: "greet",
  params: { person: Person, greeting: Schema.String },
  success: Schema.String,
  body: ({ person, greeting }) => Effect.succeed(`${greeting}, ${person.name} (${person.age})!`),
})

const ping = defineMethod({
  name: "ping",
  params: {},
  success: Schema.Void,
  body: () => Effect.void,
})

describe("Method (decoded invoke)", () => {
  it.effect("invokes with a decoded input record", () =>
    Effect.gen(function* () {
      const result = yield* invoke(greet, {
        person: { name: "Ada", age: 36 },
        greeting: "Hello",
      })
      expect(result).toBe("Hello, Ada (36)!")
    }),
  )

  it.effect("supports unit-returning, no-arg methods", () =>
    Effect.gen(function* () {
      const result = yield* invoke(ping, {})
      expect(result).toBeUndefined()
    }),
  )
})

describe("MethodCodec", () => {
  it.effect("invokes a method through a Golem DataValue (tuple) and gets a tuple back", () =>
    Effect.gen(function* () {
      const mc = yield* compileMethod(greet)

      // Build the input DataValue by encoding each parameter through its WitCodec.
      const personCodec = yield* toWitCodec(Person)
      const stringCodec = yield* toWitCodec(Schema.String)
      const personWv = yield* Schema.encodeEffect(personCodec.codec)({ name: "Ada", age: 36 })
      const greetingWv = yield* Schema.encodeEffect(stringCodec.codec)("Hello")
      const input = {
        tag: "tuple" as const,
        val: [
          { tag: "component-model" as const, val: personWv },
          { tag: "component-model" as const, val: greetingWv },
        ],
      }

      const out = yield* invokeDataValue(mc, greet.body, input)
      expect(out.tag).toBe("tuple")
      if (out.tag !== "tuple") throw new Error()
      expect(out.val.length).toBe(1)
      const elem = out.val[0]!
      expect(elem.tag).toBe("component-model")
      if (elem.tag !== "component-model") throw new Error()

      // Decode the result back through the success WitCodec.
      const decoded = yield* Schema.decodeEffect(stringCodec.codec)(elem.val)
      expect(decoded).toBe("Hello, Ada (36)!")
    }),
  )

  it.effect("returns an empty tuple DataValue for unit-returning methods", () =>
    Effect.gen(function* () {
      const mc = yield* compileMethod(ping)
      const out = yield* invokeDataValue(mc, ping.body, { tag: "tuple", val: [] })
      expect(out).toEqual({ tag: "tuple", val: [] })
    }),
  )

  it.effect("exposes the method's input/output DataSchemas", () =>
    Effect.gen(function* () {
      const mc = yield* compileMethod(greet)
      expect(mc.inputSchema.tag).toBe("tuple")
      if (mc.inputSchema.tag !== "tuple") throw new Error()
      expect(mc.inputSchema.val.map(([k]) => k)).toEqual(["person", "greeting"])

      expect(mc.outputSchema.tag).toBe("tuple")
      if (mc.outputSchema.tag !== "tuple") throw new Error()
      expect(mc.outputSchema.val.length).toBe(1)

      const pingMc = yield* compileMethod(ping)
      if (pingMc.outputSchema.tag !== "tuple") throw new Error()
      expect(pingMc.outputSchema.val.length).toBe(0)
    }),
  )
})

describe("Method pipeable combinators", () => {
  it("`method({...})` is pipeable (has a `.pipe` method)", () => {
    const spec = method({ params: { by: Schema.Number }, success: Schema.Number })
    expect(typeof spec.pipe).toBe("function")
  })

  it("`defineMethod({...})` is pipeable (has a `.pipe` method)", () => {
    const m = defineMethod({
      name: "addOne",
      params: { by: Schema.Number },
      success: Schema.Number,
      body: ({ by }) => Effect.succeed(by + 1),
    })
    expect(typeof m.pipe).toBe("function")
    // …and the chained pipe yields a usable spec without losing the body.
    const piped = m.pipe(withDescription("Add one"))
    expect(piped.description).toBe("Add one")
    expect(piped.body).toBe(m.body)
  })

  it("`.pipe(withDescription(...))` sets description without mutating the input", () => {
    const base = method({ params: { by: Schema.Number }, success: Schema.Number })
    const piped = base.pipe(withDescription("Add by"))
    expect(base.description).toBeUndefined()
    expect(piped.description).toBe("Add by")
  })

  it("`.pipe(withPromptHint(...))` sets promptHint without mutating the input", () => {
    const base = method({ params: { by: Schema.Number }, success: Schema.Number })
    const piped = base.pipe(withPromptHint("Increment by `by`"))
    expect(base.promptHint).toBeUndefined()
    expect(piped.promptHint).toBe("Increment by `by`")
  })

  it("`.pipe(withHttp(...))` appends endpoints, preserving any pre-existing ones", () => {
    const base = method({
      params: { by: Schema.Number },
      success: Schema.Number,
      http: [post("/add")],
    })
    const piped = base.pipe(withHttp(get("/add?by={by}")))
    expect(piped.http?.length).toBe(2)
    expect(piped.http?.[0]?.verb).toBe("POST")
    expect(piped.http?.[1]?.verb).toBe("GET")
  })

  it("`.pipe(withHttp(...))` enforces that endpoint bindings reference existing params", () => {
    // Happy path: every endpoint binding name (`id`, `q`) appears in
    // the method's `params`, so this typechecks AND runs.
    const piped = method({
      params: { id: Schema.String, q: Schema.String },
      success: Schema.String,
    }).pipe(withHttp(get("/items/{id}?q={q}")))
    expect(piped.http?.length).toBe(1)
    expect(piped.http?.[0]?.queryVars).toEqual([{ queryParam: "q", varName: "q" }])

    // Negative type-level case: piping an endpoint whose binding refers
    // to a name NOT in `params` is a compile-time error. The
    // `@ts-expect-error` directive asserts the type checker rejects it.
    method({
      params: { id: Schema.String },
      success: Schema.String,
      // @ts-expect-error — endpoint binds `nope`, which is not a param
    }).pipe(withHttp(get("/items/{id}/{nope}")))
  })

  it("multi-combinator chain produces the same MethodSpec as the literal form", () => {
    const piped = method({
      params: { by: Schema.Number },
      success: Schema.Number,
    }).pipe(
      withHttp(post("/add"), get("/add?by={by}")),
      withDescription("Add by"),
      withPromptHint("Increment by `by`"),
    )
    const literal = method({
      params: { by: Schema.Number },
      success: Schema.Number,
      description: "Add by",
      promptHint: "Increment by `by`",
      http: [post("/add"), get("/add?by={by}")],
    })
    // Compare relevant fields verbatim. (The `pipe` member lives on
    // the prototype, not as an own property, so it does not show up
    // in `Object.keys` / spread / `JSON.stringify` and does not
    // affect own-property equality.)
    expect(piped.description).toBe(literal.description)
    expect(piped.promptHint).toBe(literal.promptHint)
    expect(piped.http?.length).toBe(literal.http?.length)
    expect(piped.http?.[0]?.verb).toBe(literal.http?.[0]?.verb)
    expect(piped.http?.[1]?.verb).toBe(literal.http?.[1]?.verb)
  })
})

// ---------------------------------------------------------------------------
// Typed errors — folded into a component-model `result<S, E>` carried on the
// success DataValue. Verifies the server-side encoding path of the
// "RemoteMethod over-promises typed remote failures" fix: when a method
// declares a non-Void `error:` schema, `compileMethodSpec` flips
// `errorWrapped = true` and `invokeDataValue` folds typed `Effect.fail`
// into `Result.fail(e)` before encoding through the wrapped output codec.
// ---------------------------------------------------------------------------

const NotFoundError = Schema.Struct({
  _tag: Schema.Literal("NotFoundError"),
  resource: Schema.String,
})

describe("MethodCodec — typed errors", () => {
  it("compileMethodSpec flips errorWrapped when a non-Void error is declared", () =>
    Effect.gen(function* () {
      const noErr = yield* compileMethodSpec(
        "noErr",
        method({ params: {}, success: Schema.Number }),
      )
      const withErr = yield* compileMethodSpec(
        "withErr",
        method({ params: {}, success: Schema.Number, error: NotFoundError }),
      )
      expect(noErr.errorWrapped).toBe(false)
      expect(withErr.errorWrapped).toBe(true)
      expect(withErr.outputElement).not.toBeNull()
      expect(withErr.outputCodec).not.toBeNull()
    }).pipe(Effect.runPromise))

  it.effect(
    "invokeDataValue folds Effect.fail<E> into Result.fail and encodes through `result<S, E>`",
    () =>
      Effect.gen(function* () {
        const lookup = defineMethod({
          name: "lookup",
          params: { id: Schema.String },
          success: Schema.Number,
          error: NotFoundError,
          body: ({ id }) =>
            id === "ok"
              ? Effect.succeed(7)
              : Effect.fail({ _tag: "NotFoundError" as const, resource: id }),
        })
        const mc = yield* compileMethod(lookup)
        expect(mc.errorWrapped).toBe(true)

        // Build the input DataValue for `id = "ok"`.
        const stringCodec = yield* toWitCodec(Schema.String)
        const idWvOk = yield* Schema.encodeEffect(stringCodec.codec)("ok")
        const okOut = yield* invokeDataValue(mc, lookup.body, {
          tag: "tuple",
          val: [{ tag: "component-model", val: idWvOk }],
        })
        expect(okOut.tag).toBe("tuple")
        if (okOut.tag !== "tuple") throw new Error()
        const okElem = okOut.val[0]!
        if (okElem.tag !== "component-model") throw new Error()
        // Decode through the wrapped success-codec to confirm it is a
        // component-model `result<u32, NotFoundError>` carrying success.
        const resultCodec = yield* toWitCodec(Schema.Result(Schema.Number, NotFoundError))
        const decodedOk = yield* Schema.decodeEffect(resultCodec.codec)(okElem.val)
        expect(Result.isSuccess(decodedOk)).toBe(true)
        if (!Result.isSuccess(decodedOk)) throw new Error()
        expect(decodedOk.success).toBe(7)

        // And now the typed-failure path.
        const idWvMiss = yield* Schema.encodeEffect(stringCodec.codec)("nope")
        const missOut = yield* invokeDataValue(mc, lookup.body, {
          tag: "tuple",
          val: [{ tag: "component-model", val: idWvMiss }],
        })
        expect(missOut.tag).toBe("tuple")
        if (missOut.tag !== "tuple") throw new Error()
        const missElem = missOut.val[0]!
        if (missElem.tag !== "component-model") throw new Error()
        const decodedMiss = yield* Schema.decodeEffect(resultCodec.codec)(missElem.val)
        expect(Result.isFailure(decodedMiss)).toBe(true)
        if (!Result.isFailure(decodedMiss)) throw new Error()
        expect(decodedMiss.failure).toEqual({ _tag: "NotFoundError", resource: "nope" })
      }),
  )

  it.effect("Schema.Void success + typed error → result<{}, E>; both arms round-trip", () =>
    Effect.gen(function* () {
      const cmd = defineMethod({
        name: "cmd",
        params: { fail: Schema.Boolean },
        success: Schema.Void,
        error: NotFoundError,
        body: ({ fail }) =>
          fail ? Effect.fail({ _tag: "NotFoundError" as const, resource: "always" }) : Effect.void,
      })
      const mc = yield* compileMethod(cmd)
      expect(mc.errorWrapped).toBe(true)
      expect(mc.successVoid).toBe(true)
      // Crucially: outputElement is non-null even though success is Void,
      // because the wrapped Result needs an element to carry the err tag
      // (the success arm uses an empty-record stand-in).
      expect(mc.outputElement).not.toBeNull()

      const boolCodec = yield* toWitCodec(Schema.Boolean)
      const failWv = yield* Schema.encodeEffect(boolCodec.codec)(true)
      const out = yield* invokeDataValue(mc, cmd.body, {
        tag: "tuple",
        val: [{ tag: "component-model", val: failWv }],
      })
      expect(out.tag).toBe("tuple")
      if (out.tag !== "tuple") throw new Error()
      expect(out.val.length).toBe(1)
      const elem = out.val[0]!
      if (elem.tag !== "component-model") throw new Error()
      // The success arm's stand-in is `Schema.Struct({})`.
      const resultCodec = yield* toWitCodec(Schema.Result(Schema.Struct({}), NotFoundError))
      const decoded = yield* Schema.decodeEffect(resultCodec.codec)(elem.val)
      expect(Result.isFailure(decoded)).toBe(true)
      if (!Result.isFailure(decoded)) throw new Error()
      expect(decoded.failure).toEqual({ _tag: "NotFoundError", resource: "always" })

      // And the success path encodes Result.succeed({}) on the wire.
      const okWv = yield* Schema.encodeEffect(boolCodec.codec)(false)
      const okOut = yield* invokeDataValue(mc, cmd.body, {
        tag: "tuple",
        val: [{ tag: "component-model", val: okWv }],
      })
      if (okOut.tag !== "tuple") throw new Error()
      const okElem = okOut.val[0]!
      if (okElem.tag !== "component-model") throw new Error()
      const decodedOk = yield* Schema.decodeEffect(resultCodec.codec)(okElem.val)
      expect(Result.isSuccess(decodedOk)).toBe(true)
      if (!Result.isSuccess(decodedOk)) throw new Error()
      expect(decodedOk.success).toEqual({})
    }),
  )

  it.effect("defects are NOT folded into Result — they propagate", () =>
    Effect.gen(function* () {
      const boom = defineMethod({
        name: "boom",
        params: {},
        success: Schema.Number,
        error: NotFoundError,
        body: () => Effect.die(new Error("kaboom")),
      })
      const mc = yield* compileMethod(boom)
      const exit = yield* Effect.exit(invokeDataValue(mc, boom.body, { tag: "tuple", val: [] }))
      expect(Exit.isFailure(exit)).toBe(true)
      if (!Exit.isFailure(exit)) throw new Error()
      // A defect surfaces with at least one Die reason in the cause.
      const dies = exit.cause.reasons.filter((r) => r._tag === "Die")
      expect(dies.length).toBeGreaterThan(0)
    }),
  )
})
