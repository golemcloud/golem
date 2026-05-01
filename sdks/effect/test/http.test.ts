import { describe, it, expect, beforeEach } from "@effect/vitest"
import { Cause, Effect, Option, Schema } from "effect"
import { defineAgent, __resetAgents } from "../src/Agent.js"
import { method } from "../src/Method.js"
import { guest } from "../src/internal/guest.js"
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
  withAuth,
  withCors,
  withHeader,
  withHeaders,
  withPhantomAgent,
  withWebhookSuffix,
} from "../src/Http.js"
import { multimodal } from "../src/Multimodal.js"
import { UnstructuredText } from "../src/Unstructured.js"

/**
 * Run an Effect that is expected to fail and extract its typed error from
 * the cause. Misuse (success exit, or a cause without a typed failure)
 * surfaces as an `Error` in the typed-failure channel so the surrounding
 * `it.effect(...)` test fails cleanly.
 */
const runFail = <A, E>(eff: Effect.Effect<A, E>): Effect.Effect<E, Error> =>
  Effect.gen(function* () {
    const exit = yield* Effect.exit(eff)
    if (exit._tag === "Success") {
      return yield* Effect.fail(
        new Error(`expected failure, got success: ${JSON.stringify(exit.value)}`),
      )
    }
    const opt = Cause.findErrorOption(exit.cause)
    if (Option.isNone(opt)) {
      return yield* Effect.fail(
        new Error(`failure cause without typed errors: ${JSON.stringify(exit.cause)}`),
      )
    }
    return opt.value as E
  })

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
  it.effect("accepts a single slash", () =>
    Effect.gen(function* () {
      expect(yield* parseMountPath("/")).toEqual([])
    }),
  )

  it.effect("parses literal segments", () =>
    Effect.gen(function* () {
      expect(yield* parseMountPath("/api")).toEqual([{ _tag: "Literal", value: "api" }])
    }),
  )

  it.effect("parses literal + path-variable segments", () =>
    Effect.gen(function* () {
      expect(yield* parseMountPath("/api/{tenant}")).toEqual([
        { _tag: "Literal", value: "api" },
        { _tag: "PathVar", name: "tenant" },
      ])
    }),
  )

  it.effect("parses system variables", () =>
    Effect.gen(function* () {
      expect(yield* parseMountPath("/{agent-type}/x")).toEqual([
        { _tag: "SystemVar", name: "agent-type" },
        { _tag: "Literal", value: "x" },
      ])
    }),
  )

  it.effect("parses multiple variables in order", () =>
    Effect.gen(function* () {
      expect(yield* parseMountPath("/a/b/{c}/{agent-version}")).toEqual([
        { _tag: "Literal", value: "a" },
        { _tag: "Literal", value: "b" },
        { _tag: "PathVar", name: "c" },
        { _tag: "SystemVar", name: "agent-version" },
      ])
    }),
  )

  it.effect("rejects query strings in mount paths", () =>
    Effect.gen(function* () {
      const err = yield* runFail(parseMountPath("/x?q={q}"))
      expect((err as HttpRouteError).reason).toMatch(/may not include a query string/)
    }),
  )

  it.effect("rejects catch-all segments in mount paths", () =>
    Effect.gen(function* () {
      const err = yield* runFail(parseMountPath("/x/{*rest}"))
      expect((err as HttpRouteError).reason).toMatch(/catch-all/)
    }),
  )

  it.effect("rejects trailing slashes", () =>
    Effect.gen(function* () {
      const err = yield* runFail(parseMountPath("/x/"))
      expect((err as HttpRouteError).reason).toMatch(/must not end with '\/'/)
    }),
  )

  it.effect("rejects paths without a leading slash", () =>
    Effect.gen(function* () {
      const err = yield* runFail(parseMountPath("x"))
      expect((err as HttpRouteError).reason).toMatch(/must start with '\/'/)
    }),
  )

  it.effect("rejects empty variable names", () =>
    Effect.gen(function* () {
      const err = yield* runFail(parseMountPath("/{}"))
      expect((err as HttpRouteError).reason).toMatch(/empty variable name/)
    }),
  )

  it.effect("rejects mixed literal+var segments", () =>
    Effect.gen(function* () {
      // `a-{b}` starts with 'a' and ends with '}' — the parser rejects
      // it as "unbalanced" (which is the first symptom). Either label
      // describes the same malformed segment.
      const err = yield* runFail(parseMountPath("/a-{b}"))
      expect((err as HttpRouteError).reason).toMatch(/unbalanced|mixes literal text with/)
    }),
  )

  it.effect("rejects literal-prefixed-var segments after a brace open", () =>
    Effect.gen(function* () {
      // A segment that *starts* with '{' but contains additional '{' or
      // '}' chars hits the "mixes literal text with" rule.
      const err = yield* runFail(parseMountPath("/{a}b"))
      expect((err as HttpRouteError).reason).toMatch(/unbalanced|mixes literal text with/)
    }),
  )

  it.effect("rejects unbalanced braces", () =>
    Effect.gen(function* () {
      const err = yield* runFail(parseMountPath("/a/{b/c"))
      expect((err as HttpRouteError).reason).toMatch(/unbalanced/)
    }),
  )
})

describe("Http parser — parseEndpointPath", () => {
  it.effect("accepts a single slash", () =>
    Effect.gen(function* () {
      expect(yield* parseEndpointPath("/")).toEqual({ path: [], query: [] })
    }),
  )

  it.effect("parses path variables", () =>
    Effect.gen(function* () {
      const r = yield* parseEndpointPath("/items/{id}")
      expect(r.path).toEqual([
        { _tag: "Literal", value: "items" },
        { _tag: "PathVar", name: "id" },
      ])
      expect(r.query).toEqual([])
    }),
  )

  it.effect("parses catch-all (last segment)", () =>
    Effect.gen(function* () {
      const r = yield* parseEndpointPath("/items/{id}/{*sub}")
      expect(r.path).toEqual([
        { _tag: "Literal", value: "items" },
        { _tag: "PathVar", name: "id" },
        { _tag: "RestVar", name: "sub" },
      ])
    }),
  )

  it.effect("parses inline query bindings", () =>
    Effect.gen(function* () {
      const r = yield* parseEndpointPath("/search?q={query}&n={n}")
      expect(r.path).toEqual([{ _tag: "Literal", value: "search" }])
      expect(r.query).toEqual([
        { queryParam: "q", varName: "query" },
        { queryParam: "n", varName: "n" },
      ])
    }),
  )

  it.effect("rejects catch-all in non-last position", () =>
    Effect.gen(function* () {
      const err = yield* runFail(parseEndpointPath("/{*rest}/x"))
      expect((err as HttpRouteError).reason).toMatch(/LAST path segment/)
    }),
  )

  it.effect("rejects duplicate query keys", () =>
    Effect.gen(function* () {
      const err = yield* runFail(parseEndpointPath("/x?a={x}&a={y}"))
      expect((err as HttpRouteError).reason).toMatch(/duplicate query parameter/)
    }),
  )

  it.effect("rejects malformed query (no braces around value)", () =>
    Effect.gen(function* () {
      const err = yield* runFail(parseEndpointPath("/x?key=value"))
      expect((err as HttpRouteError).reason).toMatch(/value must be '\{varName\}'/)
    }),
  )

  it.effect("rejects empty query parameter name", () =>
    Effect.gen(function* () {
      const err = yield* runFail(parseEndpointPath("/x?={y}"))
      expect((err as HttpRouteError).reason).toMatch(/empty query parameter name/)
    }),
  )

  it.effect("rejects multiple '?' separators", () =>
    Effect.gen(function* () {
      const err = yield* runFail(parseEndpointPath("/x?a={b}?c={d}"))
      expect((err as HttpRouteError).reason).toMatch(/more than one '\?'/)
    }),
  )
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

  it.effect("populates httpMount and httpEndpoint on the registered AgentType", () =>
    Effect.gen(function* () {
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

      const types = yield* Effect.promise(() => guest.discoverAgentTypes())
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
    }),
  )

  it.effect("agents without HTTP routes have httpMount: undefined and empty httpEndpoint", () =>
    Effect.gen(function* () {
      defineAgent({
        name: "NoHttp",
        constructorParams: {},
        methods: {
          ping: method({ params: {}, success: Schema.Void }),
        },
        impl: () => Effect.succeed({ ping: () => Effect.void }),
      })
      const types = yield* Effect.promise(() => guest.discoverAgentTypes())
      const a = types.find((t) => t.typeName === "NoHttp")!
      expect(a.httpMount).toBeUndefined()
      expect(a.methods[0]!.httpEndpoint).toEqual([])
    }),
  )

  it("rejects an agent that declares method endpoints without a mount", () => {
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

  it("rejects a mount path-var that does not match a constructor param", () => {
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

  it("rejects when a constructor param is not covered by the mount path", () => {
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

  it("rejects an endpoint path-var that does not match a method param", () => {
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

  it("rejects a parameter bound from BOTH path and query", () => {
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

  it("rejects duplicate headers (case-insensitive)", () => {
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

  it("rejects a GET endpoint with an unbound (body) parameter", () => {
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

  it("rejects a non-string-bindable param bound from a path variable", () => {
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

  it("rejects an unstructured param bound from a path variable", () => {
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

  it("rejects a multimodal param bound from a path variable", () => {
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

  it.effect("accepts string-bindable params bound from path / query / header", () =>
    Effect.gen(function* () {
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
      const types = yield* Effect.promise(() => guest.discoverAgentTypes())
      const a = types.find((t) => t.typeName === "GoodBindings")!
      const find = a.methods[0]!
      expect(find.httpEndpoint).toHaveLength(1)
      expect(find.httpEndpoint[0]!.headerVars).toEqual([
        { headerName: "X-Trace", variableName: "traceId" },
      ])
      expect(find.httpEndpoint[0]!.queryVars).toEqual([{ queryParamName: "q", variableName: "q" }])
    }),
  )
})

describe("Http pipeable combinators — endpoints", () => {
  it("Http.get(...) is pipeable (has a `.pipe` method)", () => {
    const ep = get("/x")
    expect(typeof ep.pipe).toBe("function")
  })

  it("`.pipe(withAuth(true))` overrides authRequired without mutating the input", () => {
    const base = get("/x")
    const withAuthEp = base.pipe(withAuth(true))
    expect(base.authRequired).toBeUndefined()
    expect(withAuthEp.authRequired).toBe(true)
  })

  it("`.pipe(withAuth(false))` matches `endpoint(verb, path, { auth: false })`", () => {
    const piped = get("/x").pipe(withAuth(false))
    const literal = get("/x", { auth: false })
    expect(compileEndpoint(piped)).toEqual(compileEndpoint(literal))
  })

  it("`.pipe(withCors(...))` replaces (does not append) the cors list", () => {
    const piped = get("/x", { cors: ["a"] }).pipe(withCors("b", "c"))
    expect(piped.cors).toEqual(["b", "c"])
    const literal = get("/x", { cors: ["b", "c"] })
    expect(compileEndpoint(piped)).toEqual(compileEndpoint(literal))
  })

  it("`.pipe(withHeader(name, varName))` appends one header binding", () => {
    const piped = get("/x").pipe(withHeader("X-Trace", "traceId"))
    expect(piped.headerVars).toEqual([{ header: "X-Trace", varName: "traceId" }])
  })

  it("`.pipe(withHeader(...))` widens the EndpointVars phantom (compile-time check)", () => {
    // Build with one path var and pipe in two extra header bindings;
    // the resulting spec must accept all three names as `keyof Params`
    // when wired into a method. This exercises the type-level union
    // widening in `withHeader` / `withHeaders`.
    method({
      params: {
        id: Schema.String,
        traceId: Schema.String,
        idempotencyKey: Schema.String,
      },
      success: Schema.String,
      http: [
        get("/items/{id}").pipe(
          withHeader("X-Trace", "traceId"),
          withHeader("X-Idem", "idempotencyKey"),
        ),
      ],
    })
  })

  it("`.pipe(withHeaders({...}))` appends multiple header bindings", () => {
    const piped = get("/x").pipe(
      withHeaders({ "X-Trace": "traceId", "X-Idem": "idempotencyKey" } as const),
    )
    expect(piped.headerVars).toEqual([
      { header: "X-Trace", varName: "traceId" },
      { header: "X-Idem", varName: "idempotencyKey" },
    ])
  })

  it("multiple combinators chain in `.pipe(...)` and produce the same WIT output as the literal form", () => {
    const piped = post("/items/{id}").pipe(
      withHeader("X-Trace", "traceId"),
      withCors("https://x.com"),
      withAuth(true),
    )
    const literal = post("/items/{id}", {
      headers: { "X-Trace": "traceId" } as const,
      cors: ["https://x.com"],
      auth: true,
    })
    expect(compileEndpoint(piped)).toEqual(compileEndpoint(literal))
  })

  it.effect("piped endpoints work end-to-end inside defineAgent.methods[].http", () =>
    Effect.gen(function* () {
      defineAgent({
        name: "PipedEndpoints",
        constructorParams: { tenant: Schema.String },
        http: mount("/api/{tenant}"),
        methods: {
          find: method({
            params: { id: Schema.String, traceId: Schema.String },
            success: Schema.String,
            http: [get("/items/{id}").pipe(withHeader("X-Trace", "traceId"), withAuth(true))],
          }),
        },
        impl: () =>
          Effect.succeed({
            find: ({ id, traceId }) => Effect.succeed(`${id}/${traceId}`),
          }),
      })
      const types = yield* Effect.promise(() => guest.discoverAgentTypes())
      const a = types.find((t) => t.typeName === "PipedEndpoints")!
      const find = a.methods[0]!
      expect(find.httpEndpoint).toHaveLength(1)
      expect(find.httpEndpoint[0]!.authDetails).toEqual({ required: true })
      expect(find.httpEndpoint[0]!.headerVars).toEqual([
        { headerName: "X-Trace", variableName: "traceId" },
      ])
    }),
  )
})

describe("Http pipeable combinators — mounts", () => {
  it("Http.mount(...) is pipeable (has a `.pipe` method)", () => {
    const m = mount("/api/{tenant}")
    expect(typeof m.pipe).toBe("function")
  })

  it("`.pipe(withAuth(true))` overrides authRequired without mutating the input", () => {
    const base = mount("/api/{tenant}")
    const withAuthMount = base.pipe(withAuth(true))
    expect(base.authRequired).toBe(false)
    expect(withAuthMount.authRequired).toBe(true)
  })

  it("`.pipe(withCors(...))` replaces the cors list", () => {
    const piped = mount("/api/{tenant}", { cors: ["a"] }).pipe(withCors("b", "c"))
    expect(piped.cors).toEqual(["b", "c"])
  })

  it("`.pipe(withPhantomAgent(true))` flips the phantom-agent flag", () => {
    const piped = mount("/api/{tenant}").pipe(withPhantomAgent(true))
    expect(piped.phantomAgent).toBe(true)
  })

  it("`.pipe(withWebhookSuffix('/x'))` parses the suffix the same as the literal form", () => {
    const piped = mount("/api/{tenant}").pipe(withWebhookSuffix("/inbox"))
    const literal = mount("/api/{tenant}", { webhookSuffix: "/inbox" })
    expect(compileMount(piped)).toEqual(compileMount(literal))
  })

  it("multi-combinator chain produces the same WIT output as the literal form", () => {
    const piped = mount("/api/{tenant}").pipe(
      withAuth(true),
      withCors("https://x.com"),
      withPhantomAgent(true),
      withWebhookSuffix("/inbox"),
    )
    const literal = mount("/api/{tenant}", {
      auth: true,
      cors: ["https://x.com"],
      phantomAgent: true,
      webhookSuffix: "/inbox",
    })
    expect(compileMount(piped)).toEqual(compileMount(literal))
  })

  it("pipeable webhook-suffix path variables are still validated against constructor params", () => {
    expectRouteError(
      () =>
        defineAgent({
          name: "PipedBadWebhook",
          constructorParams: { tenant: Schema.String },
          http: mount("/api/{tenant}").pipe(withWebhookSuffix("/{nope}")),
          methods: {},
          impl: () => Effect.succeed({}),
        }),
      /webhook-suffix path variable 'nope'/,
    )
  })

  it.effect("piped mounts work end-to-end inside defineAgent.http", () =>
    Effect.gen(function* () {
      defineAgent({
        name: "PipedMount",
        constructorParams: { tenant: Schema.String },
        http: mount("/api/{tenant}").pipe(withAuth(true), withCors("https://x.com")),
        methods: {
          find: method({
            params: {},
            success: Schema.String,
            http: [get("/items")],
          }),
        },
        impl: () => Effect.succeed({ find: () => Effect.succeed("ok") }),
      })
      const types = yield* Effect.promise(() => guest.discoverAgentTypes())
      const a = types.find((t) => t.typeName === "PipedMount")!
      expect(a.httpMount?.authDetails).toEqual({ required: true })
      expect(a.httpMount?.corsOptions).toEqual({ allowedPatterns: ["https://x.com"] })
    }),
  )
})
