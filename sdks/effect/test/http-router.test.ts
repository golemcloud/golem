import { readFileSync } from "node:fs"
import { beforeEach, describe, expect, it } from "vitest"
import { Context, Effect, Exit, Fiber, Layer, Schema, Scope, Stream } from "effect"
import {
  HttpEffect,
  HttpRouter,
  HttpServerRequest,
  HttpServerRespondable,
  HttpServerResponse,
} from "effect/unstable/http"
import { __resetAgents, defineAgent } from "../src/Agent.js"
import * as GolemRouter from "../src/HttpRouter.js"
import * as Http from "../src/Http.js"
import { guest } from "../src/internal/guest.js"
import { CanonicalRequest, handleApplication, type Body } from "../src/internal/httpResponse.js"
import { ServerRequest, localUrl } from "../src/internal/httpRequest.js"
import { agentStreamFromHandle, agentStreamToHandle } from "../src/internal/agentStream.js"
import { toWitCodec } from "../src/WitCodec.js"
import { Uint8ArraySchema } from "../src/WitTypes.js"
import { PreparedStream } from "../src/internal/schema-model/preparedStream.js"
import { GuestSchemaValueStreamHandle } from "../src/internal/schema-model/schemaValueStreamHandle.js"
import { STREAM_INTERNAL } from "../src/internal/schema-model/streamInternal.js"
import { schemaValueToWit } from "../src/internal/schema-model/wit.js"
import { v } from "../src/internal/schema-model/model.js"
import type { HttpRequest } from "@golemcloud/golem-ts-sdk/http-router"

const corpus = JSON.parse(
  readFileSync(
    new URL(
      "../../../golem-service-base/tests/fixtures/http-handlers/corpus.json",
      import.meta.url,
    ),
    "utf8",
  ),
)
const fixture = (id: string) => {
  const found = corpus.cases.find((c: { id: string }) => c.id === id)
  if (!found) throw new Error(`Missing corpus case ${id}`)
  return found
}
const bytes = (s: string) => new Uint8Array(Buffer.from(s, "hex"))
const request = (overrides: Partial<HttpRequest<Body>> = {}): HttpRequest<Body> => ({
  method: "GET",
  scheme: "https",
  authority: "example.test",
  path: "/web",
  query: undefined,
  headers: [],
  body: Stream.empty,
  ...overrides,
})
type App = Effect.Effect<
  HttpServerResponse.HttpServerResponse,
  unknown,
  Scope.Scope | HttpServerRequest.HttpServerRequest | HttpRequest<Body>
>
async function exchange(make: Effect.Effect<App, unknown, Scope.Scope>, req = request()) {
  const scope = Scope.makeUnsafe()
  try {
    const response = await Effect.runPromise(
      make.pipe(
        Effect.flatMap((app) => handleApplication(req, app, "/web")),
        Scope.provide(scope),
      ),
    )
    return { ...response, close: () => Effect.runPromise(Scope.close(scope, Exit.void)) }
  } catch (error) {
    await Effect.runPromise(Scope.close(scope, Exit.void))
    throw error
  }
}
const collect = (body: Body) => Effect.runPromise(Stream.mkUint8Array(body))
const codec = Effect.runSync(toWitCodec(Uint8ArraySchema)).codec

describe("real Effect HTTP application adapter", () => {
  beforeEach(() => __resetAgents())

  it.each(["fail", "die"])("preserves a Respondable %s response", async (kind) => {
    const error = {
      [HttpServerRespondable.symbol]: () =>
        Effect.succeed(HttpServerResponse.text("rejected", { status: 422 })),
    }
    const response = await exchange(
      Effect.succeed(kind === "fail" ? Effect.fail(error) : Effect.die(error)),
    )
    expect(response.status).toBe(422)
    expect(new TextDecoder().decode(await collect(response.body))).toBe("rejected")
    await response.close()
  })

  it("envelope-extension-and-bytes: preserves the canonical request alongside its local URL", async () => {
    const { input, expect: wanted } = fixture("envelope-extension-and-bytes")
    const [path, query] = input.target.split("?")
    const original = request({
      method: input.method,
      scheme: input.scheme,
      authority: input.authority,
      path,
      query,
      headers: input.headers.map((h: { name: string; value_hex: string }) => ({
        name: h.name.toLowerCase(),
        value: bytes(h.value_hex),
      })),
      body: Stream.fromIterable(input.chunks_hex.map(bytes)),
    })
    const app = Effect.gen(function* () {
      const raw = yield* CanonicalRequest
      const view = yield* HttpServerRequest.HttpServerRequest
      expect(raw.method).toBe(wanted.method)
      expect(raw.path).toBe(wanted.path)
      expect(raw.query).toBe(wanted.query)
      expect(view.url).toBe("/%61?q=+&q=%20")
      expect(view.method).toBe(wanted.method)
      expect(
        raw.headers.map((h) => ({ name: h.name, value_hex: Buffer.from(h.value).toString("hex") })),
      ).toEqual(wanted.headers)
      return HttpServerResponse.stream(view.stream)
    })
    const response = await exchange(Effect.succeed(app), original)
    expect(Buffer.from(await collect(response.body)).toString("hex")).toBe(wanted.body_hex)
    await response.close()
  })

  it("routes GET, POST, HEAD fallback and misses using the real HttpRouter", async () => {
    const routes = Layer.mergeAll(
      HttpRouter.add("GET", "/hello", HttpServerResponse.text("get-value")),
      HttpRouter.add("POST", "/hello", HttpServerResponse.text("post-value")),
    )
    for (const [method, path, status, text] of [
      ["GET", "/web/hello", 200, "get-value"],
      ["POST", "/web/hello", 200, "post-value"],
      ["HEAD", "/web/hello", 200, ""],
      ["GET", "/web/missing", 404, ""],
    ] as const) {
      const response = await exchange(HttpRouter.toHttpEffect(routes), request({ method, path }))
      expect(response.status).toBe(status)
      expect(new TextDecoder().decode(await collect(response.body))).toBe(text)
      await response.close()
    }
  })

  it("envelope-cookie-order: preserves repeated fields through the raw escape hatch", async () => {
    const { input, expect: wanted } = fixture("envelope-cookie-order")
    const response = await exchange(
      Effect.succeed(
        Effect.succeed(
          GolemRouter.withRawHeaders(
            HttpServerResponse.empty({ status: input.status }),
            input.headers.map(([name, value]: [string, string]) => ({
              name,
              value: new TextEncoder().encode(value),
            })),
          ),
        ),
      ),
    )
    for (const name of new Set(wanted.headers.map(([name]: [string, string]) => name))) {
      expect(
        response.headers
          .filter((h) => h.name === name)
          .map((h) => [h.name, new TextDecoder().decode(h.value)]),
      ).toEqual(wanted.headers.filter(([n]: [string, string]) => n === name))
    }
    await collect(response.body)
    await response.close()
  })

  it.each([
    "envelope-head-unpolled",
    "envelope-204-removes-length",
    "envelope-205-zero-length",
    "envelope-304-retains-declared-length",
  ])("%s", async (id) => {
    const { input, expect: wanted } = fixture(id)
    let released = 0,
      polled = 0
    const app = Effect.gen(function* () {
      yield* Effect.addFinalizer(() =>
        Effect.sync(() => {
          released++
        }),
      )
      return GolemRouter.withRawHeaders(
        HttpServerResponse.stream(
          Stream.fromEffect(
            Effect.sync(() => {
              polled++
              throw new Error("body must not be pulled")
            }),
          ),
          { status: input.status },
        ),
        input.headers.map(([name, value]: [string, string]) => ({
          name,
          value: new TextEncoder().encode(value),
        })),
      )
    })
    const response = await exchange(Effect.succeed(app), request({ method: input.method }))
    expect(response.status).toBe(wanted.status)
    expect(await collect(response.body)).toEqual(new Uint8Array())
    expect(polled).toBe(0)
    expect(released).toBe(1)
    for (const [name, value] of wanted.headers ?? [])
      expect(
        response.headers.some(
          (h) => h.name === name && new TextDecoder().decode(h.value) === value,
        ),
      ).toBe(true)
    for (const name of wanted.absent_headers ?? [])
      expect(response.headers.some((h) => h.name === name)).toBe(false)
    await response.close()
  })

  it("HEAD does not start a lazy stream or register its stream-local finalizers", async () => {
    let pulled = 0
    let released = 0
    const body = Stream.suspend(() => {
      pulled++
      return Stream.succeed(bytes("01"))
    }).pipe(
      Stream.ensuring(
        Effect.sync(() => {
          released++
        }),
      ),
    )

    const response = await exchange(
      Effect.succeed(Effect.succeed(HttpServerResponse.stream(body))),
      request({ method: "HEAD" }),
    )

    expect(pulled).toBe(0)
    expect(released).toBe(0)
    await response.close()
  })

  it.each(["HEAD", "GET", "204", "205", "304"])(
    "disposes acquired transformed endpoints on %s without pulling",
    async (kind) => {
      let pulls = 0,
        releases = 0
      const app = Effect.gen(function* () {
        const context = yield* Effect.context<never>()
        const endpoint = new GuestSchemaValueStreamHandle(STREAM_INTERNAL, {
          kind: "native",
          value: {
            [Symbol.asyncIterator]() {
              return {
                async next() {
                  pulls++
                  throw new Error("must not pull")
                },
                async return() {
                  releases++
                  return { done: true as const, value: undefined }
                },
              }
            },
          },
        })
        const body = agentStreamFromHandle(endpoint, codec, context as Context.Context<any>).pipe(
          Stream.map((chunk) => chunk.slice()),
        )
        return HttpServerResponse.stream(body, { status: Number(kind) || 200 })
      })
      const response = await exchange(
        Effect.succeed(app),
        request({ method: kind === "HEAD" ? "HEAD" : "GET" }),
      )
      if (kind === "GET") {
        expect(releases).toBe(0)
        const endpoint = agentStreamToHandle(response.body, codec).take()!
        if (endpoint.kind !== "native") throw new Error("expected producer")
        const prepared = new PreparedStream(endpoint.value)
        prepared.commit()
        await prepared.return()
      }
      expect(pulls).toBe(0)
      expect(releases).toBe(1)
      await response.close()
      expect(releases).toBe(1)
    },
  )

  it.each(["take-one", "upstream-failure"])("joins one active reader close on %s", async (mode) => {
    let returned = 0,
      finished = 0
    const app = Effect.gen(function* () {
      const context = yield* Effect.context<never>()
      const endpoint = new GuestSchemaValueStreamHandle(STREAM_INTERNAL, {
        kind: "native",
        value: {
          [Symbol.asyncIterator]() {
            return {
              async next() {
                if (mode === "upstream-failure") throw new Error("upstream-failed")
                return { done: false as const, value: schemaValueToWit(v.list([v.u8(90)])) }
              },
              async return() {
                returned++
                await new Promise((resolve) => setTimeout(resolve, 1))
                finished++
                return { done: true as const, value: undefined }
              },
            }
          },
        },
      })
      return HttpServerResponse.stream(
        agentStreamFromHandle(endpoint, codec, context as Context.Context<any>).pipe(
          Stream.take(1),
        ),
      )
    })
    const response = await exchange(Effect.succeed(app))
    const output = agentStreamFromHandle(agentStreamToHandle(response.body, codec), codec)
    if (mode === "take-one") expect(await collect(output)).toEqual(bytes("5a"))
    else {
      const failure = await collect(output).then(
        () => {
          throw new Error("expected failure")
        },
        (error: unknown) => String(error),
      )
      expect(failure).toContain("upstream-failed")
      expect(failure).not.toContain("already transferred or closed")
    }
    expect(returned).toBe(1)
    expect(finished).toBe(1)
    await response.close()
  })

  it.each(["success", "failure", "drop"])("passes %s to request finalizers", async (outcome) => {
    const exits: Exit.Exit<unknown, unknown>[] = []
    const app = Effect.gen(function* () {
      yield* Effect.addFinalizer((exit) =>
        Effect.sync(() => {
          exits.push(exit)
        }),
      )
      return HttpServerResponse.stream(
        outcome === "failure"
          ? Stream.fail(new Error("late failure"))
          : Stream.succeed(bytes("6162")),
      )
    })
    const response = await exchange(Effect.succeed(app))
    const endpoint = agentStreamToHandle(response.body, codec).take()!
    if (endpoint.kind !== "native") throw new Error("expected producer")
    const prepared = new PreparedStream(endpoint.value)
    prepared.commit()
    if (outcome === "drop") await prepared.return()
    else if (outcome === "failure") await expect(prepared.next()).rejects.toThrow("late failure")
    else {
      await prepared.next()
      await prepared.next()
    }
    expect(exits).toHaveLength(1)
    expect(Exit.isSuccess(exits[0]!)).toBe(outcome === "success")
    await response.close()
  })

  it("lifecycle-output-backpressure: pulls only on demand and releases an unpolled committed output", async () => {
    fixture("lifecycle-output-backpressure")
    let pulled = 0,
      released = 0
    const make = Effect.succeed(
      Effect.gen(function* () {
        yield* Effect.addFinalizer(() =>
          Effect.sync(() => {
            released++
          }),
        )
        return HttpServerResponse.stream(
          Stream.fromIterable([bytes("01"), bytes("0304")]).pipe(
            Stream.tap(() =>
              Effect.sync(() => {
                pulled++
              }),
            ),
          ),
        )
      }),
    )
    const response = await exchange(make)
    const handle = agentStreamToHandle(response.body, codec).take()!
    if (handle.kind !== "native") throw new Error("expected producer")
    const prepared = new PreparedStream(handle.value)
    prepared.commit()
    expect(pulled).toBe(0)
    await prepared.return()
    expect(pulled).toBe(0)
    expect(released).toBe(1)
    await response.close()
  })

  it("keeps request resources open across pulls and joins asynchronous finalizers", async () => {
    let released = 0,
      pulled = 0
    const response = await exchange(
      Effect.succeed(
        Effect.gen(function* () {
          yield* Effect.addFinalizer(() =>
            Effect.promise(async () => {
              await Promise.resolve()
              released++
            }),
          )
          return HttpServerResponse.stream(
            Stream.fromIterable([bytes("12"), bytes("3456")]).pipe(
              Stream.tap(() =>
                Effect.sync(() => {
                  pulled++
                }),
              ),
            ),
          )
        }),
      ),
    )
    const output = agentStreamFromHandle(agentStreamToHandle(response.body, codec), codec)
    expect(released).toBe(0)
    expect(pulled).toBe(0)
    expect(await collect(output)).toEqual(bytes("123456"))
    expect(released).toBe(1)
    await response.close()
  })

  it("lifecycle-early-response: never waits for an abandoned upload", async () => {
    fixture("lifecycle-early-response")
    let reads = 0
    const response = await exchange(
      Effect.succeed(Effect.succeed(HttpServerResponse.text("early"))),
      request({
        body: Stream.fromEffect(
          Effect.sync(() => {
            reads++
          }).pipe(Effect.andThen(Effect.never)),
        ),
      }),
    )
    expect(new TextDecoder().decode(await collect(response.body))).toBe("early")
    expect(reads).toBe(0)
    await response.close()
  })

  it("propagates application defects and late body failure instead of successful EOF", async () => {
    fixture("lifecycle-before-commit-failure")
    await expect(exchange(Effect.succeed(Effect.die(new Error("early failure"))))).rejects.toThrow(
      "early failure",
    )
    const response = await exchange(
      Effect.succeed(
        Effect.succeed(
          HttpServerResponse.stream(
            Stream.concat(Stream.succeed(bytes("1234")), Stream.fail(new Error("late failure"))),
          ),
        ),
      ),
    )
    await expect(collect(response.body)).rejects.toThrow("late failure")
    await response.close()
  })

  it("preserves raw header occurrences across Effect pre-response hooks", async () => {
    const raw = [
      { name: "x-repeat", value: bytes("ff") },
      { name: "x-repeat", value: bytes("61") },
    ]
    const response = await exchange(
      Effect.succeed(
        HttpEffect.withPreResponseHandler(
          Effect.succeed(GolemRouter.withRawHeaders(HttpServerResponse.text("hook"), raw)),
          (_, response) => Effect.succeed(HttpServerResponse.setHeader(response, "x-hook", "yes")),
        ),
      ),
    )
    expect(response.headers.some((h) => h.name === "x-hook")).toBe(true)
    expect(response.headers.filter((h) => h.name === "x-repeat")).toEqual(raw)
    await collect(response.body)
    await response.close()
  })

  it("propagates a pre-response hook failure and finalizes its request", async () => {
    let finalized = 0
    await expect(
      exchange(
        Effect.succeed(
          HttpEffect.withPreResponseHandler(
            Effect.addFinalizer(() =>
              Effect.sync(() => {
                finalized++
              }),
            ).pipe(Effect.as(HttpServerResponse.text("not sent"))),
            () => Effect.die(new Error("hook failed")),
          ),
        ),
      ),
    ).rejects.toThrow("hook failed")
    expect(finalized).toBe(1)
  })

  it("shares one body claim between canonical and Effect request views", async () => {
    const app = Effect.gen(function* () {
      const raw = yield* CanonicalRequest
      const view = yield* HttpServerRequest.HttpServerRequest
      expect(yield* view.text).toBe("first")
      expect(yield* view.text).toBe("first")
      const second = yield* Stream.runDrain(raw.body).pipe(Effect.exit)
      expect(Exit.isFailure(second)).toBe(true)
      return HttpServerResponse.empty()
    })
    const response = await exchange(
      Effect.succeed(app),
      request({ body: Stream.succeed(new TextEncoder().encode("first")) }),
    )
    await collect(response.body)
    await response.close()
  })

  it("local Effect interruption joins pre-head scope finalization (not a host cancellation claim)", async () => {
    let ready!: () => void,
      finalized = 0
    const started = new Promise<void>((resolve) => {
      ready = resolve
    })
    const scope = Scope.makeUnsafe()
    const fiber = Effect.runFork(
      handleApplication(
        request(),
        Effect.gen(function* () {
          yield* Effect.addFinalizer(() =>
            Effect.sync(() => {
              finalized++
            }),
          )
          ready()
          return yield* Effect.never
        }),
        "/web",
      ).pipe(
        Scope.provide(scope),
        Effect.onExit((exit) => Scope.close(scope, exit)),
      ),
    )
    await started
    await Effect.runPromise(Fiber.interrupt(fiber))
    expect(finalized).toBe(1)
  })

  it("does not inspect bodies and distinguishes absent/empty queries", async () => {
    let reads = 0
    const req = request({
      body: Stream.fromEffect(
        Effect.sync(() => {
          reads++
          return bytes("736563726574")
        }),
      ),
    })
    const view = new ServerRequest(req, "/")
    expect(JSON.stringify(view)).not.toContain("secret")
    expect(reads).toBe(0)
    expect(localUrl(req, "/web")).toBe("/")
    expect(localUrl({ ...req, query: "" }, "/web")).toBe("/?")
    expect(Context.isContext(Context.empty())).toBe(true)
  })

  it("metadata-static-only and metadata-provider-only require no fake handler", () => {
    const { input } = fixture("metadata-static-only")
    GolemRouter.define("Files", {
      mount: Http.mount(input.mounts[0]),
      static: input.static_bindings.map(([route, path]: [string, string]) => ({ route, path })),
    }).register()
    GolemRouter.define("Docs", {
      mount: Http.mount("/docs"),
      openApi: Effect.succeed({}),
    }).register()
    const [files, docs] = guest.discoverAgentTypes()
    expect(files!.kind).toBe("http-router")
    expect(files!.mode).toBe("ephemeral")
    expect(files!.snapshotting.tag).toBe("disabled")
    expect(files!.methods).toEqual([])
    expect(files!.httpMount!.staticBindings).toEqual([
      { tag: "exact", val: { publicPath: [], filePath: "/index.html" } },
    ])
    expect(docs!.methods.map((m) => m.name)).toEqual(["openApi"])
    expect(docs!.methods[0]!.httpEndpoint).toEqual([])
  })

  it("metadata-arbitrary-handler-name: emits an ordinary Any binding and canonical schemas", () => {
    const { expect: wanted } = fixture("metadata-arbitrary-handler-name")
    GolemRouter.define("Web", {
      mount: Http.mount("/api"),
      handlerMethod: wanted.handler,
    }).implement(Effect.succeed(Effect.succeed(HttpServerResponse.empty())))
    const metadata = guest.discoverAgentTypes().find((agent) => agent.typeName === "Web")!
    expect(metadata.methods[0]!.name).toBe(wanted.handler)
    expect(metadata.methods[0]!.httpEndpoint[0]!.httpMethod).toEqual({ tag: "any" })
    expect(metadata.constructor.inputSchema).toEqual({ tag: "parameters", val: [] })
    const serialized = JSON.stringify(metadata.schema)
    expect(serialized).toContain("stream")
    expect(serialized).toContain("u16")
  })

  it("ordinary-agent exposeFiles uses shared mapping compilation and rejects ephemeral owners", () => {
    defineAgent({
      name: "LiveFiles",
      id: { id: Schema.String },
      http: Http.mount("/files/{id}", { exposeFiles: [{ route: "/*", path: "/data/$1" }] }),
      methods: {},
    }).implement({ init: () => Effect.void, methods: () => ({}) })
    expect(
      guest.discoverAgentTypes().find((agent) => agent.typeName === "LiveFiles")!.httpMount!
        .filesystemBindings,
    ).toEqual([{ tag: "subtree", val: { publicPrefix: [], filesystemRoot: "/data" } }])
    defineAgent({
      name: "BadFiles",
      mode: "ephemeral",
      id: {},
      http: Http.mount("/bad", { exposeFiles: [{ route: "/", path: "/data.txt" }] }),
      methods: {},
    }).implement({ init: () => Effect.void, methods: () => ({}) })
    expect(() => guest.discoverAgentTypes()).toThrow()
  })
})
