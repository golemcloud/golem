import type * as Common from "golem:tool/common@0.1.0"
import type { PermissionCard as RawPermissionCard } from "golem:core/types@2.0.0"
import { beforeEach, describe, expect, it, vi } from "vitest"
import { Effect, Exit, Result, Schema } from "effect"
import * as ToolSchema from "../src/Schema.js"
import { client, resetTools, toolDefinition, type ToolTransport } from "../src/Tool.js"
import { resetMiddlewares, toolMiddlewareGuest, typed } from "../src/Middleware.js"
import { compile } from "../src/WitCodec.js"
import {
  createGuestPermissionCardHandle,
  peekGuestPermissionCardHandle,
  type GuestPermissionCardHandle,
} from "../src/internal/schema-model/permissionCardHandle.js"
import { PERMISSION_CARD_INTERNAL } from "../src/internal/schema-model/permissionCardInternal.js"
import { invokeRegistered } from "../src/internal/tool/runtime.js"

const Card = ToolSchema.PermissionCard({ polymorphic: false })
const Payload = Schema.Struct({
  card: Card,
  outcome: Schema.Result(
    Schema.Struct({ marker: Schema.Literal("canonical") }),
    Schema.Struct({ reason: Schema.String, retryable: Schema.Boolean }),
  ),
})

type Payload = typeof Payload.Type

const rawCard = (): RawPermissionCard => Object.freeze({}) as RawPermissionCard
const handle = (raw: RawPermissionCard) =>
  createGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, raw)
const success = (card: GuestPermissionCardHandle): Payload => ({
  card,
  outcome: Result.succeed({ marker: "canonical" }),
})
const rawIn = (value: Common.SchemaValueTree): RawPermissionCard => {
  const node = value.valueNodes.find((candidate) => candidate.tag === "permission-card-handle")
  if (!node || node.tag !== "permission-card-handle") throw new Error("missing permission card")
  return node.val
}

describe("permission-card ownership across tool boundaries", () => {
  beforeEach(() => {
    resetTools()
    resetMiddlewares()
  })

  it("moves the exact card through a custom client transport and rejects either owner twice", async () => {
    const definition = toolDefinition("card-client").body((body) =>
      body.positional("payload", Payload).returns(Payload),
    )
    const raw = rawCard()
    const sender = handle(raw)
    let transportedRaw: RawPermissionCard | undefined
    const transport: ToolTransport = {
      start: (_tool, _path, input) =>
        Effect.gen(function* () {
          transportedRaw = rawIn(input.value)
          const inputCodec = yield* compile(Schema.Struct({ payload: Payload }))
          const decoded = yield* inputCodec.decode(input.value)
          const outputCodec = yield* compile(Payload)
          const output = yield* outputCodec.encode(decoded.payload)
          return {
            result: Effect.succeed({
              result: { graph: outputCodec.schemaGraph, value: output },
            }),
            cancel: Effect.void,
          }
        }),
    }

    const received = (await Effect.runPromise(
      client(definition, { transport })({ payload: success(sender) }),
    )) as unknown as Payload
    expect(transportedRaw).toBe(raw)
    expect(peekGuestPermissionCardHandle(PERMISSION_CARD_INTERNAL, received.card)).toBe(raw)

    const codec = Effect.runSync(compile(Payload))
    expect(
      Exit.isFailure(await Effect.runPromise(Effect.exit(codec.encode(success(sender))))),
    ).toBe(true)
    await Effect.runPromise(codec.encode(success(received.card)))
    expect(
      Exit.isFailure(await Effect.runPromise(Effect.exit(codec.encode(success(received.card))))),
    ).toBe(true)
  })

  it("rolls guest input and output ownership back when an asymmetric sibling is invalid", async () => {
    const captured: GuestPermissionCardHandle[] = []
    let invalidOutput = true
    toolDefinition("card-guest")
      .body((body) => body.positional("payload", Payload).returns(Payload))
      .implement({
        cardGuest: ({ payload }) => {
          captured.push(payload.card)
          return Effect.succeed({
            card: payload.card,
            outcome: Result.succeed({
              marker: invalidOutput ? "not-canonical" : "canonical",
            }),
          } as never)
        },
      })
    const codec = Effect.runSync(compile(Schema.Struct({ payload: Payload })))
    const raw = rawCard()
    const wire = await Effect.runPromise(codec.encode({ payload: success(handle(raw)) }))

    await expect(
      invokeRegistered(
        "card-guest",
        [],
        { graph: codec.schemaGraph, value: wire },
        undefined,
        undefined,
        {},
      ),
    ).rejects.toBeDefined()
    expect(captured).toHaveLength(1)
    const retry = Effect.runSync(compile(Payload))
    const retried = await Effect.runPromise(retry.encode(success(captured[0]!)))
    expect(rawIn(retried)).toBe(raw)

    invalidOutput = false
    const successfulRaw = rawCard()
    const successfulInput = await Effect.runPromise(
      codec.encode({ payload: success(handle(successfulRaw)) }),
    )
    const result = await invokeRegistered(
      "card-guest",
      [],
      { graph: codec.schemaGraph, value: successfulInput },
      undefined,
      undefined,
      {},
    )
    expect(rawIn(result.result!.value)).toBe(successfulRaw)
    const received = await Effect.runPromise(retry.decode(result.result!.value))
    await Effect.runPromise(retry.encode(received))
    expect(Exit.isFailure(await Effect.runPromise(Effect.exit(retry.encode(received))))).toBe(true)
  })

  it("preserves identity through typed middleware and leaves a rejected input wire retryable", async () => {
    const definition = toolDefinition("card-middleware").body((body) =>
      body.positional("payload", Payload).returns(Payload),
    )
    typed({
      name: "card-pass",
      presented: definition,
      handler: {
        cardMiddleware: ({ payload }, { underlying }) => underlying({ payload }),
      },
    })
    const codec = Effect.runSync(compile(Schema.Struct({ payload: Payload })))
    const raw = rawCard()
    const malformed = await Effect.runPromise(codec.encode({ payload: success(handle(raw)) }))
    const literal = malformed.valueNodes.find((node) => node.tag === "string-value")
    if (!literal || literal.tag !== "string-value") throw new Error("missing marker")
    literal.val = "not-canonical"
    const wrapped = {
      invoke: vi.fn(async (_path, input: Common.TypedSchemaValue) => {
        const decoded = await Effect.runPromise(codec.decode(input.value))
        const outputCodec = Effect.runSync(compile(Payload))
        const value = await Effect.runPromise(outputCodec.encode(decoded.payload))
        return { result: { graph: outputCodec.schemaGraph, value } }
      }),
    }

    await expect(
      toolMiddlewareGuest.invokeToolMiddleware(
        "card-pass",
        "card-middleware",
        toolMiddlewareGuest.getToolMiddleware("card-pass") as never,
        [],
        { graph: codec.schemaGraph, value: malformed },
        undefined,
        { tag: "anonymous" },
        wrapped as never,
      ),
    ).rejects.toBeDefined()
    literal.val = "canonical"
    const result = await toolMiddlewareGuest.invokeToolMiddleware(
      "card-pass",
      "card-middleware",
      toolMiddlewareGuest.getToolMiddleware("card-pass") as never,
      [],
      { graph: codec.schemaGraph, value: malformed },
      undefined,
      { tag: "anonymous" },
      wrapped as never,
    )
    expect(rawIn(result.result!.value)).toBe(raw)
    expect(wrapped.invoke).toHaveBeenCalledOnce()
  })
})
