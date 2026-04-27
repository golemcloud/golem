import { describe, it, expect } from "vitest"
import { Effect, Schema } from "effect"
import { compileMethod, defineMethod, invoke, invokeDataValue } from "../src/method.js"
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
