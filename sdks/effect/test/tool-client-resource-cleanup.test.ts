import type * as Common from "golem:tool/common@0.1.0"
import { describe, expect, it, vi } from "vitest"
import { Deferred, Effect, Exit, Fiber, Schema, SchemaGetter } from "effect"
import * as ToolSchema from "../src/Schema.js"
import { client, ToolClientError, toolDefinition } from "../src/Tool.js"
import { compile } from "../src/WitCodec.js"
import { SchemaValueStream } from "golem:core/types@2.0.0"

const cases = [
  { tag: "secret-value", schema: ToolSchema.Secret(Schema.String) },
  { tag: "quota-token-handle", schema: ToolSchema.QuotaToken() },
  { tag: "permission-card-handle", schema: ToolSchema.PermissionCard({ polymorphic: false }) },
] as const

describe("tool client rejected payload ownership", () => {
  it("cancels both result streams before joining their shared producer", async () => {
    let release!: () => void
    const bothCancelled = new Promise<void>((resolve) => {
      release = resolve
    })
    let cancellations = 0
    const close = vi.fn(async () => {
      if (++cancellations === 2) release()
      await bothCancelled
      return { done: true as const, value: undefined }
    })
    const next = vi.fn()
    const dispose = vi.fn()
    const endpoints = await Promise.all(
      [0, 1].map(async () => {
        const raw = await SchemaValueStream.wrap({
          [Symbol.asyncIterator]: () => ({ next, return: close }),
        })
        return Object.assign(raw, { [Symbol.dispose]: dispose })
      }),
    )
    const schema = Schema.Struct({
      first: ToolSchema.AgentStream(Schema.Number),
      second: ToolSchema.AgentStream(Schema.Number),
    })
    const call = client(
      toolDefinition("probe").body((body) => body.returns(schema).output()),
      {
        transport: {
          start: () =>
            Effect.succeed({
              result: Effect.succeed({
                result: {
                  graph: Effect.runSync(compile(schema)).schemaGraph,
                  value: {
                    root: 2,
                    valueNodes: [
                      ...endpoints.map((val) => ({ tag: "stream-value" as const, val })),
                      { tag: "record-value", val: [0, 1] },
                    ],
                  },
                },
              }),
              stdout: (async function* () {})(),
              cancel: Effect.void,
            }),
        },
      },
    )
    const result = Effect.runPromiseExit(call({}, { stdout: () => Effect.fail("failed") }))
    try {
      await vi.waitFor(() => expect(close).toHaveBeenCalledTimes(2))
      expect(Exit.isFailure(await result)).toBe(true)
      expect(dispose).toHaveBeenCalledTimes(2)
      expect(next).not.toHaveBeenCalled()
    } finally {
      release()
      await result
    }
  })

  it.each([...cases, { tag: "stream-value", schema: ToolSchema.AgentStream(Schema.Number) }])(
    "releases a valid $tag result when stdout consumption fails",
    async ({ tag, schema }) => {
      const dispose = vi.fn()
      const next = vi.fn()
      const close = vi.fn(async () => ({ done: true as const, value: undefined }))
      const raw =
        tag === "stream-value"
          ? await SchemaValueStream.wrap({
              [Symbol.asyncIterator]: () => ({ next, return: close }),
            })
          : {}
      Object.assign(raw, { [Symbol.dispose]: dispose })
      const payload: Common.TypedSchemaValue = {
        graph: Effect.runSync(compile(schema)).schemaGraph,
        value: { root: 0, valueNodes: [{ tag, val: raw } as never] },
      }
      const failure = new Error("stdout callback failed")
      const call = client(
        toolDefinition("probe").body((body) => body.returns(schema).output()),
        {
          transport: {
            start: () =>
              Effect.succeed({
                result: Effect.succeed({ result: payload }),
                stdout: (async function* () {})(),
                cancel: Effect.void,
              }),
          },
        },
      )
      await expect(
        Effect.runPromise(call({}, { stdout: () => Effect.fail(failure) })),
      ).rejects.toMatchObject(new ToolClientError("stream", failure))
      expect(dispose).toHaveBeenCalledOnce()
      expect(payload.value.valueNodes[0].val).toBeUndefined()
      expect(next).not.toHaveBeenCalled()
      if (tag === "stream-value") expect(close).toHaveBeenCalledOnce()
    },
  )

  it.each(["success", "interrupt"])(
    "retains a result stream until output consumption completes: %s",
    async (mode) => {
      const dispose = vi.fn()
      const next = vi.fn()
      const close = vi.fn(async () => ({ done: true as const, value: undefined }))
      const raw = await SchemaValueStream.wrap({
        [Symbol.asyncIterator]: () => ({ next, return: close }),
      })
      Object.assign(raw, { [Symbol.dispose]: dispose })
      await Effect.runPromise(
        Effect.gen(function* () {
          const arrived = yield* Deferred.make<void>()
          const consumed = yield* Deferred.make<void>()
          const schema = ToolSchema.AgentStream(Schema.Number)
          const payload: Common.TypedSchemaValue = {
            graph: (yield* compile(schema)).schemaGraph,
            value: { root: 0, valueNodes: [{ tag: "stream-value", val: raw }] },
          }
          const call = client(
            toolDefinition("probe").body((body) => body.returns(schema).stderr()),
            {
              transport: {
                start: () =>
                  Effect.succeed({
                    result: Effect.succeed({ result: payload }).pipe(
                      Effect.tap(() => Deferred.succeed(arrived, undefined)),
                    ),
                    stderr: (async function* () {})(),
                    cancel: Effect.void,
                  }),
              },
            },
          )
          const fiber = yield* Effect.forkChild(
            call(
              {},
              {
                stderr: () => Deferred.await(consumed),
              },
            ),
          )
          yield* Deferred.await(arrived)
          yield* Effect.yieldNow
          expect(dispose).not.toHaveBeenCalled()
          expect(close).not.toHaveBeenCalled()
          if (mode === "interrupt") {
            yield* Fiber.interrupt(fiber)
            expect(dispose).toHaveBeenCalledOnce()
            expect(close).toHaveBeenCalledOnce()
          } else {
            yield* Deferred.succeed(consumed, undefined)
            expect(yield* Fiber.join(fiber)).toBeDefined()
            expect(dispose).not.toHaveBeenCalled()
            expect(close).not.toHaveBeenCalled()
          }
          expect(next).not.toHaveBeenCalled()
        }),
      )
    },
  )

  it.each(cases)(
    "releases a rejected $tag sibling in results and declared errors",
    async ({ tag, schema }) => {
      const shape = Schema.Struct({ resource: schema, count: Schema.Number })
      const graph = Effect.runSync(compile(shape)).schemaGraph
      for (const declared of [false, true]) {
        const dispose = vi.fn()
        const payload = {
          graph,
          value: {
            root: 2,
            valueNodes: [
              { tag, val: { [Symbol.dispose]: dispose } },
              { tag: "string-value", val: "not a number" },
              { tag: "record-value", val: [0, 1] },
            ],
          },
        } as Common.TypedSchemaValue
        const definition = toolDefinition("probe").body((body) =>
          body.returns(shape).error("rejected", shape),
        )
        const call = client(definition, {
          transport: {
            start: () =>
              Effect.succeed({
                result: declared
                  ? Effect.fail({ tag: "custom-error", val: { name: "rejected", payload } })
                  : Effect.succeed({ result: payload }),
                cancel: Effect.void,
              }),
          },
        })
        expect(Exit.isFailure(await Effect.runPromiseExit(call({})))).toBe(true)
        expect(dispose).toHaveBeenCalledOnce()
        expect(payload.value.valueNodes[0].val).toBeUndefined()
      }
    },
  )

  it.each(["graph", "unit", "malformed", "attachments"])(
    "discards resources on pre-decode rejection: %s",
    async (mode) => {
      const dispose = vi.fn(() => {
        throw new Error("drop failed")
      })
      const sibling = vi.fn()
      const payload: Common.TypedSchemaValue = {
        graph: Effect.runSync(
          compile(
            Schema.Struct({
              first: ToolSchema.Secret(Schema.String),
              second: ToolSchema.Secret(Schema.String),
            }),
          ),
        ).schemaGraph,
        value: {
          root: mode === "malformed" ? 999 : 2,
          valueNodes: [
            { tag: "secret-value", val: { [Symbol.dispose]: dispose } as never },
            { tag: "secret-value", val: { [Symbol.dispose]: sibling } as never },
            { tag: "record-value", val: [0, 1] },
          ],
        },
      }
      const definition = toolDefinition("probe").body((body) =>
        mode === "unit" ? body : body.returns(Schema.String).error("rejected", Schema.String),
      )
      const call = client(definition, {
        transport: {
          start: () =>
            Effect.succeed({
              result:
                mode === "graph"
                  ? Effect.fail({ tag: "custom-error", val: { name: "rejected", payload } })
                  : Effect.succeed({
                      result: payload,
                      ...(mode === "attachments" ? { stdout: (async function* () {})() } : {}),
                    }),
              cancel: Effect.void,
            }),
        },
      })
      expect(Exit.isFailure(await Effect.runPromiseExit(call({})))).toBe(true)
      expect(dispose).toHaveBeenCalledOnce()
      expect(sibling).toHaveBeenCalledOnce()
    },
  )

  it("closes a rejected schema stream without pulling its contents", async () => {
    const next = vi.fn()
    const close = vi.fn(async () => ({ done: true as const, value: undefined }))
    const raw = await SchemaValueStream.wrap({
      [Symbol.asyncIterator]: () => ({ next, return: close }),
    })
    const dispose = vi.fn()
    Object.assign(raw, { [Symbol.dispose]: dispose })
    const payload: Common.TypedSchemaValue = {
      graph: Effect.runSync(compile(ToolSchema.AgentStream(Schema.Number))).schemaGraph,
      value: { root: 0, valueNodes: [{ tag: "stream-value", val: raw }] },
    }
    const call = client(
      toolDefinition("probe").body((body) => body),
      {
        transport: {
          start: () =>
            Effect.succeed({ result: Effect.succeed({ result: payload }), cancel: Effect.void }),
        },
      },
    )
    expect(Exit.isFailure(await Effect.runPromiseExit(call({})))).toBe(true)
    expect(close).toHaveBeenCalledOnce()
    expect(dispose).toHaveBeenCalledOnce()
    expect(next).not.toHaveBeenCalled()
  })

  it.each(["result", "declared", "unknown"])(
    "retains ownership transferred through a valid %s",
    async (mode) => {
      const dispose = vi.fn()
      const schema = ToolSchema.Secret(Schema.String)
      const payload: Common.TypedSchemaValue = {
        graph: Effect.runSync(compile(schema)).schemaGraph,
        value: {
          root: 0,
          valueNodes: [{ tag: "secret-value", val: { [Symbol.dispose]: dispose } as never }],
        },
      }
      const definition = toolDefinition("probe").body((body) =>
        body.returns(schema).error("rejected", schema),
      )
      const call = client(definition, {
        transport: {
          start: () =>
            Effect.succeed({
              result:
                mode === "result"
                  ? Effect.succeed({ result: payload })
                  : Effect.fail({
                      tag: "custom-error",
                      val: { name: mode === "unknown" ? "future" : "rejected", payload },
                    }),
              cancel: Effect.void,
            }),
        },
      })
      const exit = await Effect.runPromiseExit(call({}))
      expect(Exit.isSuccess(exit)).toBe(mode === "result")
      expect(dispose).not.toHaveBeenCalled()
      expect(payload.value.valueNodes[0].val !== undefined).toBe(mode === "unknown")
    },
  )

  it.each(["quota-token-handle", "permission-card-handle"] as const)(
    "releases owned siblings after rejecting a malformed %s",
    async (tag) => {
      const dispose = vi.fn()
      const payload = {
        graph: Effect.runSync(compile(Schema.String)).schemaGraph,
        value: {
          root: 0,
          valueNodes: [
            { tag, val: 1 },
            { tag: "secret-value", val: { [Symbol.dispose]: dispose } },
          ],
        },
      } as unknown as Common.TypedSchemaValue
      const call = client(
        toolDefinition("probe").body((body) => body),
        {
          transport: {
            start: () =>
              Effect.succeed({ result: Effect.succeed({ result: payload }), cancel: Effect.void }),
          },
        },
      )
      expect(Exit.isFailure(await Effect.runPromiseExit(call({})))).toBe(true)
      expect(dispose).toHaveBeenCalledOnce()
      expect(payload.value.valueNodes[1].val).toBeUndefined()
    },
  )

  it("discards restored capabilities when suspended decoding is interrupted", async () => {
    const dispose = vi.fn()
    await Effect.runPromise(
      Effect.gen(function* () {
        const entered = yield* Deferred.make<void>()
        const schema = ToolSchema.Secret(Schema.String).pipe(
          Schema.decodeTo(ToolSchema.Secret(Schema.String), {
            decode: SchemaGetter.transformEffect(() =>
              Deferred.succeed(entered, undefined).pipe(Effect.andThen(Effect.never)),
            ),
            encode: SchemaGetter.transformEffect(Effect.succeed),
          }),
        )
        const payload: Common.TypedSchemaValue = {
          graph: (yield* compile(schema)).schemaGraph,
          value: {
            root: 0,
            valueNodes: [{ tag: "secret-value", val: { [Symbol.dispose]: dispose } as never }],
          },
        }
        const call = client(
          toolDefinition("probe").body((body) => body.returns(schema)),
          {
            transport: {
              start: () =>
                Effect.succeed({
                  result: Effect.succeed({ result: payload }),
                  cancel: Effect.void,
                }),
            },
          },
        )
        const fiber = yield* Effect.forkChild(call({}))
        yield* Deferred.await(entered)
        yield* Fiber.interrupt(fiber)
        expect(dispose).toHaveBeenCalledOnce()
        expect(payload.value.valueNodes[0].val).toBeUndefined()
      }),
    )
  })
})
