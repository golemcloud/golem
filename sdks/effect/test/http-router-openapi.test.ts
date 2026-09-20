import { beforeEach, describe, expect, it, vi } from "vitest"
import { Effect, Exit, FileSystem, Layer, Path, Schema, Scope, Stream } from "effect"
import { Etag, HttpPlatform, HttpRouter } from "effect/unstable/http"
import {
  HttpApi,
  HttpApiBuilder,
  HttpApiEndpoint,
  HttpApiGroup,
  OpenApi,
} from "effect/unstable/httpapi"
import * as GolemRouter from "../src/HttpRouter.js"
import { mount } from "../src/Http.js"
import { __resetAgents } from "../src/Agent.js"
import { guest } from "../src/internal/guest.js"
import { handleApplication } from "../src/internal/httpResponse.js"
import { schemaValueFromWit, schemaValueToWit } from "../src/internal/schema-model/wit.js"
import { v } from "../src/internal/schema-model/model.js"
import { GuestSchemaValueStreamHandle } from "../src/internal/schema-model/schemaValueStreamHandle.js"
import { STREAM_INTERNAL } from "../src/internal/schema-model/streamInternal.js"
import { schemaValueToWitAsync } from "../src/internal/schema-model/wit.js"
import { HttpServerResponse } from "effect/unstable/http"

const transport = vi.hoisted(() => ({
  wrap: vi.fn(async (source: AsyncIterable<unknown>) => ({ source })),
  unwrap: vi.fn(async (stream: { source: AsyncIterable<unknown> }) => stream.source),
}))
vi.mock("golem:core/types@2.0.0", async (original) => ({
  ...(await original<object>()),
  SchemaValueStream: transport,
}))

const Api = HttpApi.make("Catalog").add(
  HttpApiGroup.make("items").add(
    HttpApiEndpoint.get("getItem", "/items/:id", {
      params: Schema.Struct({ id: Schema.String }),
      success: Schema.Struct({ id: Schema.String, count: Schema.Int }),
    }),
  ),
)
const Handlers = HttpApiBuilder.group(Api, "items", (handlers) =>
  handlers.handle("getItem", ({ params }) => Effect.succeed({ id: params.id, count: 7 })),
)
// File access is deliberately unsupported here; HTTP static files are host-owned.
const Platform = Layer.mergeAll(
  Path.layer,
  Etag.layerWeak,
  FileSystem.layerNoop({}),
  HttpPlatform.layer.pipe(Layer.provide(FileSystem.layerNoop({}))),
)
const Routes = HttpApiBuilder.layer(Api).pipe(Layer.provide(Handlers), Layer.provide(Platform))
const principal = { tag: "anonymous" } as const
const emptyInput = () => schemaValueToWit(v.record([]))

describe("real Effect HttpApi and generated provider", () => {
  beforeEach(() => __resetAgents())

  it.each(["complete", "drop", "encode-failure"])(
    "registered handler retains its factory scope through %s",
    async (mode) => {
      let released = 0,
        initialized = 0,
        uploadReads = 0,
        uploadReleases = 0
      const name = `WireRouter-${mode}`
      GolemRouter.define(name, { mount: mount("/wire") }).implement(
        Effect.gen(function* () {
          initialized++
          yield* Effect.addFinalizer(() =>
            Effect.sync(() => {
              released++
            }),
          )
          return Effect.succeed(
            HttpServerResponse.stream(Stream.succeed(new TextEncoder().encode("wire-body"))),
          )
        }),
      )
      await guest.initialize(name, emptyInput(), principal)
      expect(initialized).toBe(0)
      const body = new GuestSchemaValueStreamHandle(STREAM_INTERNAL, {
        kind: "native",
        value: {
          [Symbol.asyncIterator]() {
            return {
              async next() {
                uploadReads++
                throw new Error("unused upload")
              },
              async return() {
                uploadReleases++
                return { done: true as const, value: undefined }
              },
            }
          },
        },
      })
      const input = await schemaValueToWitAsync(
        v.record([
          v.record([
            v.string("GET"),
            v.string("https"),
            v.string("example.test"),
            v.string("/wire"),
            v.option(),
            v.list([]),
            v.stream(body),
          ]),
        ]),
      )
      if (mode === "encode-failure") {
        transport.wrap.mockRejectedValueOnce(new Error("transport rejected output"))
        await expect(guest.invoke("handle", input, principal)).rejects.toThrow(
          "transport rejected output",
        )
      } else {
        const wire = await guest.invoke("handle", input, principal)
        expect(released).toBe(0)
        const output = schemaValueFromWit(wire!)
        if (
          output.tag !== "record" ||
          output.fields[0]!.tag !== "u16" ||
          output.fields[2]!.tag !== "stream"
        )
          throw new Error("invalid HTTP response envelope")
        expect(output.fields[0]!.value).toBe(200)
        const endpoint = output.fields[2]!.handle.take()!
        if (endpoint.kind !== "wrapped") throw new Error("expected wrapped output")
        const source = (await transport.unwrap(endpoint.value as never)) as AsyncIterable<
          import("golem:core/types@2.0.0").SchemaValueTree
        >
        const iterator = source[Symbol.asyncIterator]()
        if (mode === "complete") {
          const first = schemaValueFromWit((await iterator.next()).value!)
          expect(first).toEqual(v.list([...new TextEncoder().encode("wire-body")].map(v.u8)))
          expect((await iterator.next()).done).toBe(true)
        } else await iterator.return?.()
      }
      expect(initialized).toBe(1)
      expect(released).toBe(1)
      expect(uploadReads).toBe(0)
      expect(uploadReleases).toBe(1)
    },
  )

  it("runs the generated application and emits an independently invoked mount-relative provider", async () => {
    let generations = 0
    GolemRouter.define("CatalogRouter", {
      mount: mount("/catalog"),
      openapiProviderMethod: "describeCatalog",
      openApi: Effect.sync(() => {
        generations++
        return OpenApi.fromApi(Api)
      }),
    }).implement(HttpRouter.toHttpEffect(Routes))
    expect(generations).toBe(0)
    const metadata = guest.discoverAgentTypes().find((agent) => agent.typeName === "CatalogRouter")!
    expect(metadata.httpMount!.openapiProviderMethod).toBe("describeCatalog")
    expect(
      metadata.methods.find((method) => method.name === "describeCatalog")!.inputSchema,
    ).toEqual({ tag: "parameters", val: [] })

    const scope = Scope.makeUnsafe()
    const response = await Effect.runPromise(
      HttpRouter.toHttpEffect(Routes).pipe(
        Effect.flatMap((app) =>
          handleApplication(
            {
              method: "GET",
              scheme: "https",
              authority: "example.test",
              path: "/catalog/items/abc",
              query: undefined,
              headers: [],
              body: Stream.empty,
            },
            app,
            "/catalog",
          ),
        ),
        Scope.provide(scope),
      ),
    )
    expect(response.status).toBe(200)
    expect(
      JSON.parse(
        new TextDecoder().decode(await Effect.runPromise(Stream.mkUint8Array(response.body))),
      ),
    ).toEqual({ id: "abc", count: 7 })
    await Effect.runPromise(Scope.close(scope, Exit.void))
    expect(generations).toBe(0)

    await guest.initialize("CatalogRouter", emptyInput(), principal)
    const wire = await guest.invoke("describeCatalog", emptyInput(), principal)
    const value = schemaValueFromWit(wire!)
    if (value.tag !== "string") throw new Error("provider must return one string")
    const document = JSON.parse(value.value)
    expect(document.openapi).toBe("3.1.0")
    expect(Object.keys(document.paths)).toEqual(["/items/{id}"])
    expect(document.paths["/items/{id}"].get.parameters).toEqual(
      expect.arrayContaining([expect.objectContaining({ name: "id", in: "path", required: true })]),
    )
    expect(
      document.paths["/items/{id}"].get.responses["200"].content["application/json"].schema
        .properties.count.type,
    ).toBe("integer")
    expect(generations).toBe(1)
  })

  it("provider-only registration is lazy and serialization failures are invocation failures", async () => {
    let generations = 0
    GolemRouter.define("BadProvider", {
      mount: mount("/bad-provider"),
      openApi: Effect.sync(() => {
        generations++
        return { openapi: "3.0.0", info: { title: "invalid", version: "1" }, paths: {} }
      }),
    }).register()
    expect(generations).toBe(0)
    await guest.initialize("BadProvider", emptyInput(), principal)
    await expect(guest.invoke("openApi", emptyInput(), principal)).rejects.toThrow(
      "openapi-document",
    )
    expect(generations).toBe(1)
  })
})
