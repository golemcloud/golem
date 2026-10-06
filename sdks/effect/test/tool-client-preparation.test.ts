import { beforeEach, describe, expect, it, vi } from "vitest"
import { Context, Effect, Exit, Fiber, Schema, SchemaGetter, SchemaIssue } from "effect"
import { client, toolDefinition, type ToolTransport } from "../src/Tool.js"
import type { UnsupportedSchemaError } from "../src/WitCodec.js"

const preparation = vi.hoisted(() => ({
  schemas: [] as Schema.Top[],
  before: undefined as ((schema: Schema.Top) => Effect.Effect<void>) | undefined,
}))

vi.mock("../src/WitCodec.js", async (importOriginal) => {
  const original = await importOriginal<typeof import("../src/WitCodec.js")>()
  return {
    ...original,
    compile: (schema: Schema.Top) =>
      Effect.suspend(() => {
        preparation.schemas.push(schema)
        return Effect.andThen(preparation.before?.(schema) ?? Effect.void, original.compile(schema))
      }),
  }
})

const { compile: realCompile } =
  await vi.importActual<typeof import("../src/WitCodec.js")>("../src/WitCodec.js")
const wire = (text: string) => {
  const codec = Effect.runSync(realCompile(Schema.String))
  return { graph: codec.schemaGraph, value: Effect.runSync(codec.encode(text)) }
}
const success = wire("accepted")
const rejected = { tag: "custom-error", val: { name: "rejected", payload: wire("denied") } }
const transport: ToolTransport = {
  start: (_name, _path, input) =>
    Effect.succeed({
      result: input.value.valueNodes.some(
        (node) => node.tag === "string-value" && node.val === "error",
      )
        ? Effect.fail(rejected)
        : Effect.succeed({ result: success }),
      cancel: Effect.void,
    }),
}
const definition = () =>
  toolDefinition("prepared-tool").body((body) =>
    body.positional("value", Schema.String).returns(Schema.String).error("rejected", Schema.String),
  )
const gate = () => {
  let release!: () => void
  let started!: () => void
  const waiting = new Promise<void>((resolve) => (release = resolve))
  const entered = new Promise<void>((resolve) => (started = resolve))
  return {
    release,
    entered,
    effect: Effect.sync(started).pipe(Effect.andThen(Effect.promise(() => waiting))),
  }
}

describe("tool client codec preparation", () => {
  beforeEach(() => {
    preparation.schemas = []
    preparation.before = undefined
  })

  it("lazily caches input, output and declared errors per client and per command", async () => {
    const def = definition().command("other", (command) =>
      command.body((body) => body.positional("value", Schema.String).returns(Schema.String)),
    )
    const remote = client(def, { transport })
    expect(preparation.schemas).toHaveLength(0)
    for (let i = 0; i < 3; i++) {
      expect(await Effect.runPromise(remote({ value: "ok" }))).toBe("accepted")
      const error = await Effect.runPromise(Effect.flip(remote({ value: "error" })))
      expect(error).toMatchObject({ _tag: "ToolFailure", name: "rejected", value: "denied" })
    }
    expect(preparation.schemas).toHaveLength(3)
    await Effect.runPromise(remote.other({ value: "ok" }))
    await Effect.runPromise(remote.other({ value: "ok" }))
    expect(preparation.schemas).toHaveLength(5)
    await Effect.runPromise(client(def, { transport })({ value: "ok" }))
    expect(preparation.schemas).toHaveLength(7)
  })

  it("shares concurrent first preparations for success and declared-error calls", async () => {
    const input = gate()
    const result = gate()
    const error = gate()
    preparation.before = () => [input, result, error][preparation.schemas.length - 1]!.effect
    const remote = client(definition(), { transport })
    const running = Effect.runPromise(
      Effect.all(
        Array.from({ length: 8 }, (_, i) => Effect.exit(remote({ value: i % 2 ? "error" : "ok" }))),
        { concurrency: "unbounded" },
      ),
    )
    await input.entered
    expect(preparation.schemas).toHaveLength(1)
    input.release()
    await Promise.all([result.entered, error.entered])
    expect(preparation.schemas).toHaveLength(3)
    result.release()
    error.release()
    const exits = await running
    expect(exits.filter(Exit.isSuccess)).toHaveLength(4)
    expect(exits.filter(Exit.isFailure)).toHaveLength(4)
    expect(preparation.schemas).toHaveLength(3)
  })

  it.each(["input", "result", "declared-error"] as const)(
    "retains typed %s preparation failures without recompiling",
    async (phase) => {
      const def = toolDefinition("unsupported-preparation").body((body) =>
        body
          .positional("value", phase === "input" ? Schema.Symbol : Schema.String)
          .returns(phase === "result" ? Schema.Symbol : Schema.String)
          .error("rejected", phase === "declared-error" ? Schema.Symbol : Schema.String),
      )
      const remote = client(def, { transport })
      for (let i = 0; i < 3; i++) {
        const error = await Effect.runPromise(
          Effect.flip(remote({ value: phase === "declared-error" ? "error" : "ok" })),
        )
        expect(error).toMatchObject({
          _tag: "ToolClientError",
          phase,
          cause: { _tag: "UnsupportedSchemaError" },
        })
        expect((error as { cause: UnsupportedSchemaError }).cause.reason).toBeTruthy()
      }
      expect(preparation.schemas).toHaveLength(phase === "input" ? 1 : 2)
    },
  )

  it.each(["input", "result", "declared-error"] as const)(
    "allows fresh %s preparation after the owning caller is interrupted",
    async (phase) => {
      const waiting = gate()
      const at = phase === "input" ? 1 : 2
      let finalized = 0
      preparation.before = () =>
        preparation.schemas.length === at
          ? waiting.effect.pipe(Effect.ensuring(Effect.sync(() => finalized++)))
          : Effect.void
      const remote = client(definition(), { transport })
      const call = () => remote({ value: phase === "declared-error" ? "error" : "ok" })
      const fiber = Effect.runFork(call())
      await waiting.entered
      await Effect.runPromise(Fiber.interrupt(fiber))
      const interrupted = await Effect.runPromise(Fiber.await(fiber))
      expect(Exit.hasInterrupts(interrupted)).toBe(true)
      expect(finalized).toBe(1)
      preparation.before = undefined
      const retried = await Effect.runPromise(Effect.exit(call()))
      if (phase === "declared-error") expect(Exit.isFailure(retried)).toBe(true)
      else expect(retried).toEqual(Exit.succeed("accepted"))
      expect(preparation.schemas).toHaveLength(at + (phase === "input" ? 2 : 1))
      await Effect.runPromise(Effect.exit(call()))
      expect(preparation.schemas).toHaveLength(3)
    },
  )

  it.each(["owner", "waiter"] as const)(
    "does not poison the shared cache when an in-flight %s is interrupted",
    async (interrupted) => {
      const preparing = gate()
      let markWaiterStarted!: () => void
      const waiterStarted = new Promise<void>((resolve) => (markWaiterStarted = resolve))
      preparation.before = () => preparing.effect
      const remote = client(definition(), { transport })
      const owner = Effect.runFork(remote({ value: "ok" }))
      await preparing.entered
      const waiter = Effect.runFork(
        Effect.sync(markWaiterStarted).pipe(Effect.andThen(remote({ value: "ok" }))),
      )
      await waiterStarted
      expect(preparation.schemas).toHaveLength(1)
      await Effect.runPromise(Fiber.interrupt(interrupted === "owner" ? owner : waiter))
      if (interrupted === "owner") {
        expect(Exit.hasInterrupts(await Effect.runPromise(Fiber.await(waiter)))).toBe(true)
      }
      preparation.before = undefined
      preparing.release()
      if (interrupted === "waiter") {
        expect(await Effect.runPromise(Fiber.await(owner))).toEqual(Exit.succeed("accepted"))
      }
      expect(await Effect.runPromise(remote({ value: "ok" }))).toBe("accepted")
      expect(preparation.schemas).toHaveLength(interrupted === "owner" ? 3 : 2)
    },
  )

  it("uses current invocation services for validation and transforms after preparation", async () => {
    class Expected extends Context.Service<Expected, { readonly value: string }>()(
      "codec/Expected",
    ) {}
    let transforms = 0
    const serviceful = Schema.String.pipe(
      Schema.decodeTo(Schema.String, {
        decode: SchemaGetter.transformEffect((value) =>
          Effect.gen(function* () {
            const expected = yield* Expected
            transforms++
            return `${value}:${expected.value}`
          }),
        ),
        encode: SchemaGetter.transformEffect((value) =>
          Effect.gen(function* () {
            const expected = yield* Expected
            return value === expected.value
              ? value
              : yield* Effect.fail(
                  new SchemaIssue.InvalidValue({ message: "wrong service value" }, value),
                )
          }),
        ),
      }),
    )
    const def = toolDefinition("serviceful-preparation").body((body) =>
      body.positional("value", serviceful).returns(serviceful).error("rejected", serviceful),
    )
    const remote = client(def, { transport })
    for (const value of ["one", "two"]) {
      const context = Context.make(Expected, { value })
      expect(await Effect.runPromise(remote({ value }).pipe(Effect.provide(context)))).toBe(
        `accepted:${value}`,
      )
      expect(
        await Effect.runPromise(
          Effect.flip(remote({ value: "wrong" }).pipe(Effect.provide(context))),
        ),
      ).toMatchObject({ phase: "input" })
    }
    for (let i = 0; i < 2; i++) {
      const error = await Effect.runPromise(
        Effect.flip(
          remote({ value: "error" }).pipe(
            Effect.provide(Context.make(Expected, { value: "error" })),
          ),
        ),
      )
      expect(error).toMatchObject({ _tag: "ToolFailure", value: "denied:error" })
    }
    expect(transforms).toBe(4)
    expect(preparation.schemas).toHaveLength(3)
  })
})
