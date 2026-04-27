import { describe, it, expect, beforeEach } from "vitest"
import { Cause, Effect, Option, Schema } from "effect"
import { defineAgent, __resetAgents } from "../src/agent.js"
import { method } from "../src/method.js"
import { guest } from "../src/exports.js"
import {
  agentType,
  agentVersion,
  compileEndpoint,
  compileMount,
  custom,
  del,
  endpoint,
  get,
  head,
  HttpRouteError,
  isStringBindableSchema,
  literal,
  mount,
  options as opt,
  parseEndpointPath,
  parseMountPath,
  patch,
  pathVar,
  post,
  put,
  restVar,
  trace,
  connect,
} from "../src/http.js"
import { multimodal } from "../src/multimodal.js"
import { UnstructuredText } from "../src/unstructured.js"

// Run an Effect to a sync result; throw on failure.
const run = <A, E>(eff: Effect.Effect<A, E>) => Effect.runPromise(eff)

const runFail = async <A, E>(eff: Effect.Effect<A, E>): Promise<E> => {
  const exit = await Effect.runPromiseExit(eff)
  if (exit._tag !== "Failure") {
    throw new Error(`expected failure, got success: ${JSON.stringify(exit.value)}`)
  }
  const opt = Cause.findErrorOption(exit.cause)
  if (Option.isNone(opt)) {
    throw new Error(`failure cause without typed errors: ${JSON.stringify(exit.cause)}`)
  }
  return opt.value as E
}

/**
 * Run `defineAgent` (which calls `Effect.runSync(registerAgent(...))`)
 * and expect it to fail with an `HttpRouteError` whose `reason` matches
 * the given pattern. The plain `expect(...).toThrow(/.../)` matcher
 * cannot peek inside the wrapped Effect failure (HttpRouteError does
 * not extend `Error`), so we extract the reason ourselves.
 */
const expectRouteError = (thunk: () => unknown, pattern: RegExp): void => {
  let caught: unknown
  try {
    thunk()
  } catch (e) {
    caught = e
  }
  if (caught === undefined) {
    throw new Error(`expected HttpRouteError matching ${pattern}, got success`)
  }
  // The wrapper's string form embeds the cause; that's enough for
  // robust matching across Effect versions.
  const text =
    (caught as { reason?: string }).reason ??
    (caught as { message?: string }).message ??
    String(caught)
  if (!pattern.test(text)) {
    throw new Error(
      `expected HttpRouteError matching ${pattern}, got: ${text} (full: ${String(caught)})`,
    )
  }
}

describe("Http parser — parseMountPath", () => {
  it("accepts a single slash", async () => {
    expect(await run(parseMountPath("/"))).toEqual([])
  })

  it("parses literal segments", async () => {
    expect(await run(parseMountPath("/api"))).toEqual([{ _tag: "Literal", value: "api" }])
  })

  it("parses literal + path-variable segments", async () => {
    expect(await run(parseMountPath("/api/{tenant}"))).toEqual([
      { _tag: "Literal", value: "api" },
      { _tag: "PathVar", name: "tenant" },
    ])
  })

  it("parses system variables", async () => {
    expect(await run(parseMountPath("/{agent-type}/x"))).toEqual([
      { _tag: "SystemVar", name: "agent-type" },
      { _tag: "Literal", value: "x" },
    ])
  })

  it("parses multiple variables in order", async () => {
    expect(await run(parseMountPath("/a/b/{c}/{agent-version}"))).toEqual([
      { _tag: "Literal", value: "a" },
      { _tag: "Literal", value: "b" },
      { _tag: "PathVar", name: "c" },
      { _tag: "SystemVar", name: "agent-version" },
    ])
  })

  it("rejects query strings in mount paths", async () => {
    const err = await runFail(parseMountPath("/x?q={q}"))
    expect((err as HttpRouteError).reason).toMatch(/may not include a query string/)
  })

  it("rejects catch-all segments in mount paths", async () => {
    const err = await runFail(parseMountPath("/x/{*rest}"))
    expect((err as HttpRouteError).reason).toMatch(/catch-all/)
  })

  it("rejects trailing slashes", async () => {
    const err = await runFail(parseMountPath("/x/"))
    expect((err as HttpRouteError).reason).toMatch(/must not end with '\/'/)
  })

  it("rejects paths without a leading slash", async () => {
    const err = await runFail(parseMountPath("x"))
    expect((err as HttpRouteError).reason).toMatch(/must start with '\/'/)
  })

  it("rejects empty variable names", async () => {
    const err = await runFail(parseMountPath("/{}"))
    expect((err as HttpRouteError).reason).toMatch(/empty variable name/)
  })

  it("rejects mixed literal+var segments", async () => {
    // `a-{b}` starts with 'a' and ends with '}' — the parser rejects
    // it as "unbalanced" (which is the first symptom). Either label
    // describes the same malformed segment.
    const err = await runFail(parseMountPath("/a-{b}"))
    expect((err as HttpRouteError).reason).toMatch(/unbalanced|mixes literal text with/)
  })

  it("rejects literal-prefixed-var segments after a brace open", async () => {
    // A segment that *starts* with '{' but contains additional '{' or
    // '}' chars hits the "mixes literal text with" rule.
    const err = await runFail(parseMountPath("/{a}b"))
    expect((err as HttpRouteError).reason).toMatch(/unbalanced|mixes literal text with/)
  })

  it("rejects unbalanced braces", async () => {
    const err = await runFail(parseMountPath("/a/{b/c"))
    expect((err as HttpRouteError).reason).toMatch(/unbalanced/)
  })
})

describe("Http parser — parseEndpointPath", () => {
  it("accepts a single slash", async () => {
    expect(await run(parseEndpointPath("/"))).toEqual({ path: [], query: [] })
  })

  it("parses path variables", async () => {
    const r = await run(parseEndpointPath("/items/{id}"))
    expect(r.path).toEqual([
      { _tag: "Literal", value: "items" },
      { _tag: "PathVar", name: "id" },
    ])
    expect(r.query).toEqual([])
  })

  it("parses catch-all (last segment)", async () => {
    const r = await run(parseEndpointPath("/items/{id}/{*sub}"))
    expect(r.path).toEqual([
      { _tag: "Literal", value: "items" },
      { _tag: "PathVar", name: "id" },
      { _tag: "RestVar", name: "sub" },
    ])
  })

  it("parses inline query bindings", async () => {
    const r = await run(parseEndpointPath("/search?q={query}&n={n}"))
    expect(r.path).toEqual([{ _tag: "Literal", value: "search" }])
    expect(r.query).toEqual([
      { queryParam: "q", varName: "query" },
      { queryParam: "n", varName: "n" },
    ])
  })

  it("rejects catch-all in non-last position", async () => {
    const err = await runFail(parseEndpointPath("/{*rest}/x"))
    expect((err as HttpRouteError).reason).toMatch(/LAST path segment/)
  })

  it("rejects duplicate query keys", async () => {
    const err = await runFail(parseEndpointPath("/x?a={x}&a={y}"))
    expect((err as HttpRouteError).reason).toMatch(/duplicate query parameter/)
  })

  it("rejects malformed query (no braces around value)", async () => {
    const err = await runFail(parseEndpointPath("/x?key=value"))
    expect((err as HttpRouteError).reason).toMatch(/value must be '\{varName\}'/)
  })

  it("rejects empty query parameter name", async () => {
    const err = await runFail(parseEndpointPath("/x?={y}"))
    expect((err as HttpRouteError).reason).toMatch(/empty query parameter name/)
  })

  it("rejects multiple '?' separators", async () => {
    const err = await runFail(parseEndpointPath("/x?a={b}?c={d}"))
    expect((err as HttpRouteError).reason).toMatch(/more than one '\?'/)
  })
})

describe("Http compileMount / compileEndpoint", () => {
  it("compiles a fully-featured mount", () => {
    const m = mount("/api/{tenant}", {
      auth: true,
      cors: ["*", "https://acme.com"],
      phantomAgent: true,
      webhookSuffix: "/hooks/{tenant}",
    })
    const w = compileMount(m)
    expect(w.pathPrefix).toEqual([
      { tag: "literal", val: "api" },
      { tag: "path-variable", val: { variableName: "tenant" } },
    ])
    expect(w.authDetails).toEqual({ required: true })
    expect(w.phantomAgent).toBe(true)
    expect(w.corsOptions).toEqual({ allowedPatterns: ["*", "https://acme.com"] })
    expect(w.webhookSuffix).toEqual([
      { tag: "literal", val: "hooks" },
      { tag: "path-variable", val: { variableName: "tenant" } },
    ])
  })

  it("omits authDetails when auth is not requested", () => {
    const w = compileMount(mount("/x/{a}", {}))
    expect(w.authDetails).toBeUndefined()
    expect(w.phantomAgent).toBe(false)
  })

  it("compiles a fully-featured endpoint", () => {
    const e = get("/items/{id}?q={query}", {
      headers: { "X-Tenant": "tenant" } as const,
      auth: false,
      cors: ["https://x"],
    })
    const w = compileEndpoint(e)
    expect(w.httpMethod).toEqual({ tag: "get" })
    expect(w.pathSuffix).toEqual([
      { tag: "literal", val: "items" },
      { tag: "path-variable", val: { variableName: "id" } },
    ])
    expect(w.queryVars).toEqual([{ queryParamName: "q", variableName: "query" }])
    expect(w.headerVars).toEqual([{ headerName: "X-Tenant", variableName: "tenant" }])
    expect(w.authDetails).toEqual({ required: false })
    expect(w.corsOptions).toEqual({ allowedPatterns: ["https://x"] })
  })

  it("compiles system-variable segments", () => {
    const w = compileEndpoint(endpoint("GET", "/{agent-type}/{agent-version}/x"))
    expect(w.pathSuffix).toEqual([
      { tag: "system-variable", val: "agent-type" },
      { tag: "system-variable", val: "agent-version" },
      { tag: "literal", val: "x" },
    ])
  })

  it("compiles catch-all segments", () => {
    const w = compileEndpoint(endpoint("GET", "/files/{*path}"))
    expect(w.pathSuffix).toEqual([
      { tag: "literal", val: "files" },
      { tag: "remaining-path-variable", val: { variableName: "path" } },
    ])
  })

  it("compiles a custom HTTP verb", () => {
    const w = compileEndpoint(custom("PROPFIND", "/x"))
    expect(w.httpMethod).toEqual({ tag: "custom", val: "PROPFIND" })
  })

  it("maps each verb shorthand to the correct WIT tag", () => {
    expect(compileEndpoint(get("/x")).httpMethod).toEqual({ tag: "get" })
    expect(compileEndpoint(head("/x")).httpMethod).toEqual({ tag: "head" })
    expect(compileEndpoint(post("/x")).httpMethod).toEqual({ tag: "post" })
    expect(compileEndpoint(put("/x")).httpMethod).toEqual({ tag: "put" })
    expect(compileEndpoint(del("/x")).httpMethod).toEqual({ tag: "delete" })
    expect(compileEndpoint(patch("/x")).httpMethod).toEqual({ tag: "patch" })
    expect(compileEndpoint(opt("/x")).httpMethod).toEqual({ tag: "options" })
    expect(compileEndpoint(trace("/x")).httpMethod).toEqual({ tag: "trace" })
    expect(compileEndpoint(connect("/x")).httpMethod).toEqual({ tag: "connect" })
  })

  it("escape-hatch segment constructors round-trip", () => {
    expect(literal("a")).toEqual({ _tag: "Literal", value: "a" })
    expect(pathVar("x")).toEqual({ _tag: "PathVar", name: "x" })
    expect(restVar("y")).toEqual({ _tag: "RestVar", name: "y" })
    expect(agentType()).toEqual({ _tag: "SystemVar", name: "agent-type" })
    expect(agentVersion()).toEqual({ _tag: "SystemVar", name: "agent-version" })
  })

  it("endpoint with no auth option emits undefined authDetails (inherits)", () => {
    const w = compileEndpoint(get("/x"))
    expect(w.authDetails).toBeUndefined()
  })
})

describe("Http isStringBindableSchema", () => {
  it("accepts primitive scalars", () => {
    expect(isStringBindableSchema(Schema.String)).toBe(true)
    expect(isStringBindableSchema(Schema.Number)).toBe(true)
    expect(isStringBindableSchema(Schema.BigInt)).toBe(true)
    expect(isStringBindableSchema(Schema.Boolean)).toBe(true)
  })

  it("accepts literal schemas", () => {
    expect(isStringBindableSchema(Schema.Literal("a"))).toBe(true)
    expect(isStringBindableSchema(Schema.Literal(1))).toBe(true)
  })

  it("accepts refined/branded scalars", () => {
    const Refined = Schema.String.pipe(Schema.brand("UserId"))
    expect(isStringBindableSchema(Refined)).toBe(true)
  })

  it("rejects structs and arrays", () => {
    expect(isStringBindableSchema(Schema.Struct({ a: Schema.String }))).toBe(false)
    expect(isStringBindableSchema(Schema.Array(Schema.String))).toBe(false)
  })
})

describe("Http defineAgent integration", () => {
  beforeEach(async () => {
    await __resetAgents()
  })

  it("populates httpMount and httpEndpoint on the registered AgentType", async () => {
    defineAgent({
      name: "Counter1",
      description: "test counter",
      promptHint: "use this counter",
      constructorParams: { name: Schema.String },
      http: mount("/counters/{name}", { cors: ["*"], auth: true }),
      methods: {
        value: method({
          params: {},
          success: Schema.Number,
          description: "current value",
          http: [get("/value")],
        }),
        add: method({
          params: { by: Schema.Number },
          success: Schema.Number,
          http: [post("/add"), get("/add?by={by}")],
        }),
      },
      impl: () =>
        Effect.succeed({
          value: () => Effect.succeed(0),
          add: ({ by }) => Effect.succeed(by),
        }),
    })

    const types = await guest.discoverAgentTypes()
    const a = types.find((t) => t.typeName === "Counter1")!
    expect(a.description).toBe("test counter")
    expect(a.constructor.promptHint).toBe("use this counter")
    expect(a.httpMount).toBeDefined()
    expect(a.httpMount!.pathPrefix).toEqual([
      { tag: "literal", val: "counters" },
      { tag: "path-variable", val: { variableName: "name" } },
    ])
    expect(a.httpMount!.corsOptions.allowedPatterns).toEqual(["*"])
    expect(a.httpMount!.authDetails).toEqual({ required: true })

    const valueMethod = a.methods.find((m) => m.name === "value")!
    expect(valueMethod.description).toBe("current value")
    expect(valueMethod.httpEndpoint).toHaveLength(1)
    expect(valueMethod.httpEndpoint[0]!.httpMethod).toEqual({ tag: "get" })
    expect(valueMethod.httpEndpoint[0]!.pathSuffix).toEqual([{ tag: "literal", val: "value" }])

    const addMethod = a.methods.find((m) => m.name === "add")!
    expect(addMethod.httpEndpoint).toHaveLength(2)
    expect(addMethod.httpEndpoint.map((e) => e.httpMethod.tag)).toEqual(["post", "get"])
    expect(addMethod.httpEndpoint[1]!.queryVars).toEqual([
      { queryParamName: "by", variableName: "by" },
    ])
  })

  it("agents without HTTP routes have httpMount: undefined and empty httpEndpoint", async () => {
    defineAgent({
      name: "NoHttp",
      constructorParams: {},
      methods: {
        ping: method({ params: {}, success: Schema.Void }),
      },
      impl: () => Effect.succeed({ ping: () => Effect.void }),
    })
    const types = await guest.discoverAgentTypes()
    const a = types.find((t) => t.typeName === "NoHttp")!
    expect(a.httpMount).toBeUndefined()
    expect(a.methods[0]!.httpEndpoint).toEqual([])
  })

  it("rejects an agent that declares method endpoints without a mount", async () => {
    expectRouteError(
      () =>
        defineAgent({
          name: "MissingMount",
          constructorParams: {},
          methods: {
            value: method({ params: {}, success: Schema.Number, http: [get("/v")] }),
          },
          impl: () => Effect.succeed({ value: () => Effect.succeed(0) }),
        }),
      /declares HTTP endpoints but no mount/,
    )
  })

  it("rejects a mount path-var that does not match a constructor param", async () => {
    expectRouteError(
      () =>
        defineAgent({
          name: "BadMountVar",
          constructorParams: { name: Schema.String },
          http: mount("/x/{nonExistent}") as never,
          methods: {},
          impl: () => Effect.succeed({}),
        }),
      /does not match any constructor parameter/,
    )
  })

  it("rejects when a constructor param is not covered by the mount path", async () => {
    expectRouteError(
      () =>
        defineAgent({
          name: "UncoveredCtor",
          constructorParams: { name: Schema.String, region: Schema.String },
          http: mount("/x/{name}") as never,
          methods: {},
          impl: () => Effect.succeed({}),
        }),
      /'region' is not covered by a mount path variable/,
    )
  })

  it("rejects an endpoint path-var that does not match a method param", async () => {
    expectRouteError(
      () =>
        defineAgent({
          name: "BadEndpointVar",
          constructorParams: {},
          http: mount("/x"),
          methods: {
            getOne: method({
              params: {} as Record<string, never>,
              success: Schema.String,
              http: [get("/items/{nope}")] as never,
            }),
          },
          impl: () => Effect.succeed({ getOne: () => Effect.succeed("") } as never),
        }),
      /does not match any method parameter/,
    )
  })

  it("rejects a parameter bound from BOTH path and query", async () => {
    expectRouteError(
      () =>
        defineAgent({
          name: "DualBind",
          constructorParams: {},
          http: mount("/x"),
          methods: {
            op: method({
              params: { id: Schema.String },
              success: Schema.String,
              http: [get("/items/{id}?id={id}")],
            }),
          },
          impl: () => Effect.succeed({ op: ({ id }) => Effect.succeed(id) }),
        }),
      /bound from both/,
    )
  })

  it("rejects duplicate headers (case-insensitive)", async () => {
    expectRouteError(
      () =>
        defineAgent({
          name: "DupHeaders",
          constructorParams: {},
          http: mount("/x"),
          methods: {
            op: method({
              params: { a: Schema.String, b: Schema.String },
              success: Schema.String,
              http: [get("/items", { headers: { "X-Foo": "a", "x-foo": "b" } as const })],
            }),
          },
          impl: () => Effect.succeed({ op: ({ a, b }) => Effect.succeed(`${a}/${b}`) }),
        }),
      /duplicate header/,
    )
  })

  it("rejects a GET endpoint with an unbound (body) parameter", async () => {
    expectRouteError(
      () =>
        defineAgent({
          name: "GetWithBody",
          constructorParams: {},
          http: mount("/x"),
          methods: {
            op: method({
              params: { payload: Schema.String },
              success: Schema.String,
              http: [get("/op")],
            }),
          },
          impl: () => Effect.succeed({ op: ({ payload }) => Effect.succeed(payload) }),
        }),
      /may not have unbound parameters/,
    )
  })

  it("rejects a non-string-bindable param bound from a path variable", async () => {
    expectRouteError(
      () =>
        defineAgent({
          name: "BadBindShape",
          constructorParams: {},
          http: mount("/x"),
          methods: {
            op: method({
              params: { obj: Schema.Struct({ a: Schema.String }) },
              success: Schema.String,
              http: [post("/items/{obj}")],
            }),
          },
          impl: () => Effect.succeed({ op: ({ obj }) => Effect.succeed(JSON.stringify(obj)) }),
        }),
      /not bindable from a path variable/,
    )
  })

  it("rejects an unstructured param bound from a path variable", async () => {
    expectRouteError(
      () =>
        defineAgent({
          name: "UnstructuredBind",
          constructorParams: {},
          http: mount("/x"),
          methods: {
            op: method({
              params: { text: UnstructuredText() },
              success: Schema.String,
              http: [post("/items/{text}")],
            }),
          },
          impl: () => Effect.succeed({ op: () => Effect.succeed("") }),
        }),
      /multimodal\/unstructured/,
    )
  })

  it("rejects a multimodal param bound from a path variable", async () => {
    expectRouteError(
      () =>
        defineAgent({
          name: "MultimodalBind",
          constructorParams: {},
          http: mount("/x"),
          methods: {
            op: method({
              params: { mm: multimodal({ chunk: UnstructuredText() }) },
              success: Schema.String,
              http: [post("/items/{mm}")],
            }),
          },
          impl: () => Effect.succeed({ op: () => Effect.succeed("") }),
        }),
      /multimodal\/unstructured/,
    )
  })

  it("accepts string-bindable params bound from path / query / header", async () => {
    defineAgent({
      name: "GoodBindings",
      constructorParams: { tenant: Schema.String },
      http: mount("/api/{tenant}"),
      methods: {
        find: method({
          params: { id: Schema.String, q: Schema.String, traceId: Schema.String },
          success: Schema.String,
          http: [get("/items/{id}?q={q}", { headers: { "X-Trace": "traceId" } as const })],
        }),
      },
      impl: () =>
        Effect.succeed({
          find: ({ id, q, traceId }) => Effect.succeed(`${id}/${q}/${traceId}`),
        }),
    })
    const types = await guest.discoverAgentTypes()
    const a = types.find((t) => t.typeName === "GoodBindings")!
    const find = a.methods[0]!
    expect(find.httpEndpoint).toHaveLength(1)
    expect(find.httpEndpoint[0]!.headerVars).toEqual([
      { headerName: "X-Trace", variableName: "traceId" },
    ])
    expect(find.httpEndpoint[0]!.queryVars).toEqual([{ queryParamName: "q", variableName: "q" }])
  })
})
