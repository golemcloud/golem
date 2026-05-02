import { afterEach, beforeEach, describe, expect, it } from "@effect/vitest"
import { Effect, Layer, Schema } from "effect"
import * as Webhook from "../src/Webhook.js"
import * as Http from "../src/Http.js"
import { AgentHostLive } from "../src/host/AgentHostClient.js"
import { PromiseLive } from "../src/host/PromiseClient.js"
import * as ApiHostMock from "./mocks/golem-api-host.js"
import * as AgentHostMock from "./mocks/golem-agent-host.js"

/**
 * Layer-based replacement for the deleted `__setX/__resetX`
 * indirection in `src/webhook.ts`. Production layers go through the
 * vitest-aliased mock modules — the per-test `AgentHostMock.__set...`
 * setters drive the host's `createWebhook` response unchanged.
 */
const HostLayer = Layer.mergeAll(AgentHostLive, PromiseLive)

describe("Webhook.create", () => {
  beforeEach(() => {
    ApiHostMock.__resetAll()
    AgentHostMock.__resetCreateWebhookImpl()
  })
  afterEach(() => {
    ApiHostMock.__resetAll()
    AgentHostMock.__resetCreateWebhookImpl()
  })

  it.effect("allocates a promise then mints a webhook URL bound to it", () =>
    Effect.gen(function* () {
      let observedPromiseId: ApiHostMock.PromiseId | undefined
      AgentHostMock.__setCreateWebhookImpl((id) => {
        observedPromiseId = id
        return "https://hooks.example/abc"
      })

      const hook = yield* Webhook.create

      expect(hook.url).toBe("https://hooks.example/abc")
      expect(observedPromiseId).toBeDefined()
      expect(hook.promiseId).toEqual(observedPromiseId)
    }).pipe(Effect.provide(HostLayer)),
  )

  it.effect("surfaces a thrown create-webhook as WebhookHostError", () =>
    Effect.gen(function* () {
      AgentHostMock.__setCreateWebhookImpl(() => {
        throw new Error("agent not deployed via http api")
      })
      const exit = yield* Effect.exit(Webhook.create)
      expect(exit._tag).toBe("Failure")
      if (exit._tag === "Failure") {
        const failure = exit.cause as unknown as { _tag?: string }
        // The cause should carry our typed error.
        const json = JSON.stringify(exit.cause)
        expect(json).toContain("WebhookHostError")
        expect(json).toContain("agent not deployed via http api")
        void failure
      }
    }).pipe(Effect.provide(HostLayer)),
  )
})

describe("Webhook handle — await / poll", () => {
  beforeEach(() => {
    ApiHostMock.__resetAll()
    AgentHostMock.__resetCreateWebhookImpl()
  })
  afterEach(() => {
    ApiHostMock.__resetAll()
    AgentHostMock.__resetCreateWebhookImpl()
  })

  it.effect("await resolves to a WebhookPayload once the underlying promise completes", () =>
    Effect.gen(function* () {
      AgentHostMock.__setCreateWebhookImpl(() => "https://hooks.example/x")
      const hook = yield* Webhook.create

      const polled1 = yield* hook.poll
      expect(polled1).toBeUndefined()

      setTimeout(() => {
        ApiHostMock.completePromise(hook.promiseId, new TextEncoder().encode("hello"))
      }, 5)

      const payload = yield* hook.await
      expect(payload).toBeInstanceOf(Webhook.WebhookPayload)
      expect(payload.text()).toBe("hello")

      const polled2 = yield* hook.poll
      expect(polled2).toBeInstanceOf(Webhook.WebhookPayload)
    }).pipe(Effect.provide(HostLayer)),
  )

  it.effect("await fast-paths when the promise is already completed", () =>
    Effect.gen(function* () {
      AgentHostMock.__setCreateWebhookImpl(() => "https://hooks.example/y")
      const hook = yield* Webhook.create
      ApiHostMock.completePromise(hook.promiseId, new TextEncoder().encode("ready"))
      const payload = yield* hook.await
      expect(payload.text()).toBe("ready")
    }).pipe(Effect.provide(HostLayer)),
  )
})

describe("WebhookPayload", () => {
  it.effect("text() / json() / decode(schema) round-trip", () =>
    Effect.gen(function* () {
      const body = JSON.stringify({ id: "evt-1", status: "ok" })
      const payload = new Webhook.WebhookPayload(new TextEncoder().encode(body))

      expect(payload.text()).toBe(body)
      expect(payload.json<{ id: string; status: string }>().id).toBe("evt-1")

      const Event = Schema.Struct({ id: Schema.String, status: Schema.String })
      const decoded = yield* payload.decode(Event)
      expect(decoded.status).toBe("ok")
    }),
  )

  it.effect("decode surfaces invalid JSON as Schema.SchemaError", () =>
    Effect.gen(function* () {
      const payload = new Webhook.WebhookPayload(new TextEncoder().encode("not json"))
      const Event = Schema.Struct({ id: Schema.String })
      const exit = yield* Effect.exit(payload.decode(Event))
      expect(exit._tag).toBe("Failure")
    }),
  )

  it.effect("decode surfaces invalid UTF-8 as WebhookDecodeError", () =>
    Effect.gen(function* () {
      // 0xff is not a valid leading UTF-8 byte; strict TextDecoder rejects it.
      const payload = new Webhook.WebhookPayload(new Uint8Array([0xff, 0xff, 0xff]))
      const Event = Schema.Struct({ id: Schema.String })
      const exit = yield* Effect.exit(payload.decode(Event))
      expect(exit._tag).toBe("Failure")
      if (exit._tag === "Failure") {
        const json = JSON.stringify(exit.cause)
        expect(json).toContain("WebhookDecodeError")
      }
    }),
  )

  it.effect("decode surfaces schema mismatches as Schema.SchemaError", () =>
    Effect.gen(function* () {
      const payload = new Webhook.WebhookPayload(new TextEncoder().encode(JSON.stringify({})))
      const Event = Schema.Struct({ id: Schema.String })
      const exit = yield* Effect.exit(payload.decode(Event))
      expect(exit._tag).toBe("Failure")
    }),
  )

  it("json() throws synchronously on invalid JSON", () => {
    const payload = new Webhook.WebhookPayload(new TextEncoder().encode("not json"))
    expect(() => payload.json()).toThrow()
  })
})

describe("Http.mount({ webhookSuffix }) — validation", () => {
  it("accepts a literal-only suffix", () => {
    const m = Http.mount("/agents/{name}", { webhookSuffix: "/inbox" })
    expect(m.webhookSuffix).toEqual([{ _tag: "Literal", value: "inbox" }])
  })

  it("accepts a {constructor-param} suffix variable", () => {
    const m = Http.mount("/agents/{name}", { webhookSuffix: "/{name}/events" })
    expect(m.webhookSuffix.length).toBe(2)
    expect(m.webhookSuffix[0]).toEqual({ _tag: "PathVar", name: "name" })
    expect(m.webhookSuffix[1]).toEqual({ _tag: "Literal", value: "events" })
  })

  it("rejects a query string in webhookSuffix at parse time", () => {
    expect(() => Http.mount("/agents/{name}", { webhookSuffix: "/inbox?q={x}" })).toThrow(
      Http.HttpRouteError,
    )
  })

  it("rejects a catch-all in webhookSuffix at parse time", () => {
    expect(() => Http.mount("/agents/{name}", { webhookSuffix: "/{*rest}" })).toThrow(
      Http.HttpRouteError,
    )
  })

  it.effect("rejects a webhookSuffix path variable that is not a constructor param", () =>
    Effect.gen(function* () {
      const m = Http.mount("/agents/{name}", { webhookSuffix: "/{unknown}/events" })
      const exit = yield* Effect.exit(
        Http.validateAgentHttp({
          agentName: "Test",
          mount: m,
          constructorParamNames: ["name"],
          nonStringBindableConstructorParams: new Set(),
          stringBindableConstructorParams: new Set(["name"]),
          methods: [],
        }),
      )
      expect(exit._tag).toBe("Failure")
      if (exit._tag === "Failure") {
        const json = JSON.stringify(exit.cause)
        expect(json).toContain("webhook-suffix path variable 'unknown'")
      }
    }),
  )
})
