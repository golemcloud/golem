import { describe, it, expect } from "vitest"
import { Effect, Schema } from "effect"
import {
  compileMethod,
  defineMethod,
  invoke,
  invokeDataValue,
  method,
  withDescription,
  withHttp,
  withPromptHint,
} from "../src/method.js"
import { get, post } from "../src/http.js"
import { toWitCodec } from "../src/wit-codec.js"

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
  it("invokes with a decoded input record", async () => {
    const result = await Effect.runPromise(
      invoke(greet, { person: { name: "Ada", age: 36 }, greeting: "Hello" }),
    )
    expect(result).toBe("Hello, Ada (36)!")
  })

  it("supports unit-returning, no-arg methods", async () => {
    const result = await Effect.runPromise(invoke(ping, {}))
    expect(result).toBeUndefined()
  })
})

describe("MethodCodec", () => {
  it("invokes a method through a Golem DataValue (tuple) and gets a tuple back", async () => {
    const mc = await Effect.runPromise(compileMethod(greet))

    // Build the input DataValue by encoding each parameter through its WitCodec.
    const personCodec = await Effect.runPromise(toWitCodec(Person))
    const stringCodec = await Effect.runPromise(toWitCodec(Schema.String))
    const personWv = await Effect.runPromise(
      Schema.encodeEffect(personCodec.codec)({ name: "Ada", age: 36 }),
    )
    const greetingWv = await Effect.runPromise(Schema.encodeEffect(stringCodec.codec)("Hello"))
    const input = {
      tag: "tuple" as const,
      val: [
        { tag: "component-model" as const, val: personWv },
        { tag: "component-model" as const, val: greetingWv },
      ],
    }

    const out = await Effect.runPromise(invokeDataValue(mc, greet.body, input))
    expect(out.tag).toBe("tuple")
    if (out.tag !== "tuple") throw new Error()
    expect(out.val.length).toBe(1)
    const elem = out.val[0]!
    expect(elem.tag).toBe("component-model")
    if (elem.tag !== "component-model") throw new Error()

    // Decode the result back through the success WitCodec.
    const decoded = await Effect.runPromise(Schema.decodeEffect(stringCodec.codec)(elem.val))
    expect(decoded).toBe("Hello, Ada (36)!")
  })

  it("returns an empty tuple DataValue for unit-returning methods", async () => {
    const mc = await Effect.runPromise(compileMethod(ping))
    const out = await Effect.runPromise(invokeDataValue(mc, ping.body, { tag: "tuple", val: [] }))
    expect(out).toEqual({ tag: "tuple", val: [] })
  })

  it("exposes the method's input/output DataSchemas", async () => {
    const mc = await Effect.runPromise(compileMethod(greet))
    expect(mc.inputSchema.tag).toBe("tuple")
    if (mc.inputSchema.tag !== "tuple") throw new Error()
    expect(mc.inputSchema.val.map(([k]) => k)).toEqual(["person", "greeting"])

    expect(mc.outputSchema.tag).toBe("tuple")
    if (mc.outputSchema.tag !== "tuple") throw new Error()
    expect(mc.outputSchema.val.length).toBe(1)

    const pingMc = await Effect.runPromise(compileMethod(ping))
    if (pingMc.outputSchema.tag !== "tuple") throw new Error()
    expect(pingMc.outputSchema.val.length).toBe(0)
  })
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
