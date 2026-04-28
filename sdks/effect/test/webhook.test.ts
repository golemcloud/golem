import { Effect, Exit, Schema } from "effect"
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import * as Webhook from "../src/webhook.js"
import * as Http from "../src/http.js"
import * as ApiHostMock from "./mocks/golem-api-host.js"
import * as AgentHostMock from "./mocks/golem-agent-host.js"

const runP = <A, E>(eff: Effect.Effect<A, E, never>): Promise<A> => Effect.runPromise(eff)
const runExit = <A, E>(eff: Effect.Effect<A, E, never>): Promise<Exit.Exit<A, E>> =>
  Effect.runPromiseExit(eff)

describe("Webhook.create", () => {
  beforeEach(() => {
    ApiHostMock.__resetAll()
    AgentHostMock.__resetCreateWebhookImpl()
  })
  afterEach(() => {
    ApiHostMock.__resetAll()
    AgentHostMock.__resetCreateWebhookImpl()
  })

  it("allocates a promise then mints a webhook URL bound to it", async () => {
    let observedPromiseId: ApiHostMock.PromiseId | undefined
    AgentHostMock.__setCreateWebhookImpl((id) => {
      observedPromiseId = id
      return "https://hooks.example/abc"
    })

    const hook = await runP(Webhook.create)

    expect(hook.url).toBe("https://hooks.example/abc")
    expect(observedPromiseId).toBeDefined()
    expect(hook.promiseId).toEqual(observedPromiseId)
  })

  it("surfaces a thrown create-webhook as WebhookHostError", async () => {
    AgentHostMock.__setCreateWebhookImpl(() => {
      throw new Error("agent not deployed via http api")
    })
    const exit = await runExit(Webhook.create)
    expect(exit._tag).toBe("Failure")
    if (exit._tag === "Failure") {
      const failure = exit.cause as unknown as { _tag?: string }
      // The cause should carry our typed error.
      const json = JSON.stringify(exit.cause)
      expect(json).toContain("WebhookHostError")
      expect(json).toContain("agent not deployed via http api")
      void failure
    }
  })
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

  it("await resolves to a WebhookPayload once the underlying promise completes", async () => {
    AgentHostMock.__setCreateWebhookImpl(() => "https://hooks.example/x")
    const hook = await runP(Webhook.create)

    const polled1 = await runP(hook.poll)
    expect(polled1).toBeUndefined()

    setTimeout(() => {
      ApiHostMock.completePromise(hook.promiseId, new TextEncoder().encode("hello"))
    }, 5)

    const payload = await runP(hook.await)
    expect(payload).toBeInstanceOf(Webhook.WebhookPayload)
    expect(payload.text()).toBe("hello")

    const polled2 = await runP(hook.poll)
    expect(polled2).toBeInstanceOf(Webhook.WebhookPayload)
  })

  it("await fast-paths when the promise is already completed", async () => {
    AgentHostMock.__setCreateWebhookImpl(() => "https://hooks.example/y")
    const hook = await runP(Webhook.create)
    ApiHostMock.completePromise(hook.promiseId, new TextEncoder().encode("ready"))
    const payload = await runP(hook.await)
    expect(payload.text()).toBe("ready")
  })
})

describe("WebhookPayload", () => {
  it("text() / json() / decode(schema) round-trip", async () => {
    const body = JSON.stringify({ id: "evt-1", status: "ok" })
    const payload = new Webhook.WebhookPayload(new TextEncoder().encode(body))

    expect(payload.text()).toBe(body)
    expect(payload.json<{ id: string; status: string }>().id).toBe("evt-1")

    const Event = Schema.Struct({ id: Schema.String, status: Schema.String })
    const decoded = await runP(payload.decode(Event))
    expect(decoded.status).toBe("ok")
  })

  it("decode surfaces invalid JSON as WebhookDecodeError", async () => {
    const payload = new Webhook.WebhookPayload(new TextEncoder().encode("not json"))
    const Event = Schema.Struct({ id: Schema.String })
    const exit = await runExit(payload.decode(Event))
    expect(exit._tag).toBe("Failure")
    if (exit._tag === "Failure") {
      const json = JSON.stringify(exit.cause)
      expect(json).toContain("WebhookDecodeError")
    }
  })

  it("decode surfaces schema mismatches as Schema.SchemaError", async () => {
    const payload = new Webhook.WebhookPayload(new TextEncoder().encode(JSON.stringify({})))
    const Event = Schema.Struct({ id: Schema.String })
    const exit = await runExit(payload.decode(Event))
    expect(exit._tag).toBe("Failure")
  })

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

  it("rejects a webhookSuffix path variable that is not a constructor param", async () => {
    const m = Http.mount("/agents/{name}", { webhookSuffix: "/{unknown}/events" })
    const exit = await runExit(
      Http.validateAgentHttp({
        agentName: "Test",
        mount: m,
        constructorParamNames: ["name"],
        nonStringBindableConstructorParams: new Set(),
        methods: [],
      }),
    )
    expect(exit._tag).toBe("Failure")
    if (exit._tag === "Failure") {
      const json = JSON.stringify(exit.cause)
      expect(json).toContain("webhook-suffix path variable 'unknown'")
    }
  })
})
