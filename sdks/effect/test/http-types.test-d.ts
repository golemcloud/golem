/**
 * Type-only tests for the compile-time HTTP path validators in
 * `src/internal/httpTypes.ts`.
 *
 * Consumed via `tsc --noEmit` (the file is included by `tsconfig.json`'s
 * `test/**\/*` glob) and explicitly excluded from the vitest run by the
 * `.test-d.ts` suffix (vitest's default glob is `*.test.ts`).
 *
 * Negative cases use `// @ts-expect-error` so a regression — i.e. the
 * validator silently accepting an invalid path — fails `tsc` with
 * `TS2578: Unused '@ts-expect-error' directive`.
 *
 * @since 1.5.0
 */

import { Effect, Schema } from "effect"
import { defineAgent, method, Method } from "../src/index.js"
import * as Http from "../src/Http.js"
import type { BindableKeys } from "../src/internal/httpTypes.js"
import { multimodal } from "../src/Multimodal.js"
import { UnstructuredText } from "../src/Unstructured.js"

// ---------------------------------------------------------------------------
// Mount path: ValidMountPath
// ---------------------------------------------------------------------------

// Positive cases — valid mount paths must compile.
void Http.mount("/")
void Http.mount("/agents")
void Http.mount("/agents/v1")
void Http.mount("/counters/{name}")
void Http.mount("/c/{agent-type}/{name}")

// must start with '/'
// @ts-expect-error missing leading '/'
void Http.mount("agents")
// @ts-expect-error missing leading '/'
void Http.mount("")

// no trailing '/' (except the bare "/")
// @ts-expect-error trailing slash
void Http.mount("/agents/")
// @ts-expect-error trailing slash
void Http.mount("/a/b/")

// no '//'
// @ts-expect-error consecutive slashes
void Http.mount("//agents")
// @ts-expect-error consecutive slashes
void Http.mount("/a//b")

// mount paths may not contain '?'
// @ts-expect-error mount may not include a query string
void Http.mount("/agents?foo=1")

// ---------------------------------------------------------------------------
// MountOptions.webhookSuffix is validated
// ---------------------------------------------------------------------------

void Http.mount("/agents/{name}", { webhookSuffix: "/inbox" })
void Http.mount("/agents/{name}", { webhookSuffix: "/inbox/events" })

// @ts-expect-error webhookSuffix must start with '/'
void Http.mount("/agents/{name}", { webhookSuffix: "inbox" })
// @ts-expect-error webhookSuffix may not include a query string
void Http.mount("/agents/{name}", { webhookSuffix: "/inbox?q={x}" })
// @ts-expect-error webhookSuffix may not contain '//'
void Http.mount("/agents/{name}", { webhookSuffix: "//inbox" })

// ---------------------------------------------------------------------------
// withWebhookSuffix (pipeable form) is validated
// ---------------------------------------------------------------------------

void Http.mount("/agents/{name}").pipe(Http.withWebhookSuffix("/inbox"))
void Http.mount("/agents/{name}").pipe(Http.withWebhookSuffix("/inbox/events"))

// @ts-expect-error withWebhookSuffix must start with '/'
void Http.mount("/agents/{name}").pipe(Http.withWebhookSuffix("inbox"))
// @ts-expect-error withWebhookSuffix may not include '?'
void Http.mount("/agents/{name}").pipe(Http.withWebhookSuffix("/inbox?q={x}"))

// ---------------------------------------------------------------------------
// Endpoint path: ValidEndpointPath
// ---------------------------------------------------------------------------

// Positive cases — valid endpoint paths must compile.
void Http.get("/")
void Http.get("/items")
void Http.get("/items/{id}")
void Http.get("/items?q={q}")
void Http.get("/items?q={q}&p={p}")
void Http.post("/items")
void Http.put("/items/{id}")
void Http.del("/items/{id}")
void Http.patch("/items/{id}")
void Http.head("/items/{id}")
void Http.options("/items")
void Http.trace("/items")
void Http.connect("/items")
void Http.endpoint("GET", "/items/{id}")
void Http.custom("PURGE", "/items/{id}")

// must start with '/'
// @ts-expect-error missing leading '/'
void Http.get("items")

// no trailing '/'
// @ts-expect-error trailing slash
void Http.get("/items/")

// no '//'
// @ts-expect-error consecutive slashes
void Http.get("/items//list")

// at most one '?'
// @ts-expect-error multiple '?'
void Http.get("/items?a={a}?b={b}")

// no empty query parameter name
// @ts-expect-error empty query key after '?'
void Http.get("/items?={a}")
// @ts-expect-error empty query key after '&'
void Http.get("/items?a={a}&={b}")

// no empty '&' segment
// @ts-expect-error consecutive '&' producing empty pair
void Http.get("/items?a={a}&&b={b}")
// @ts-expect-error trailing '&' producing empty pair
void Http.get("/items?a={a}&")
// @ts-expect-error leading '&' producing empty pair
void Http.get("/items?&a={a}")

// ---------------------------------------------------------------------------
// All verb shorthands and `endpoint` / `custom` enforce the same rules
// ---------------------------------------------------------------------------

// @ts-expect-error post: missing leading '/'
void Http.post("items")
// @ts-expect-error put: trailing '/'
void Http.put("/items/")
// @ts-expect-error del: '//' in path
void Http.del("/items//x")
// @ts-expect-error patch: more than one '?'
void Http.patch("/items?a={a}?b={b}")
// @ts-expect-error head: empty query parameter name
void Http.head("/items?={a}")
// @ts-expect-error options: empty '&' segment
void Http.options("/items?a={a}&&b={b}")
// @ts-expect-error trace: trailing '&' segment
void Http.trace("/items?a={a}&")
// @ts-expect-error connect: leading '&' segment
void Http.connect("/items?&a={a}")
// @ts-expect-error endpoint: missing leading '/'
void Http.endpoint("GET", "items")
// @ts-expect-error custom: missing leading '/'
void Http.custom("PURGE", "items")

// ---------------------------------------------------------------------------
// every constructor parameter must be covered by the mount path
// ---------------------------------------------------------------------------

// Positive case — all params covered by the mount path.
defineAgent({
  name: "M4_AllCovered",
  constructorParams: { name: Schema.String, id: Schema.String },
  http: Http.mount("/agents/{name}/{id}"),
  methods: {
    op: method({ params: {}, success: Schema.String }),
  },
  impl: () => Effect.succeed({ op: () => Effect.succeed("ok") }),
})

// Positive case — agent without `http:` is OK regardless of the
// constructor params.
defineAgent({
  name: "M4_NoHttp",
  constructorParams: { name: Schema.String, id: Schema.String },
  methods: {
    op: method({ params: {}, success: Schema.String }),
  },
  impl: () => Effect.succeed({ op: () => Effect.succeed("ok") }),
})

// Positive case — agent with empty constructor params and a literal
// mount path. `keyof C & string` is `never`, so coverage is trivial.
defineAgent({
  name: "M4_NoParams",
  constructorParams: {},
  http: Http.mount("/agents"),
  methods: {
    op: method({ params: {}, success: Schema.String }),
  },
  impl: () => Effect.succeed({ op: () => Effect.succeed("ok") }),
})

// Negative case — mount path is missing `{id}`.
defineAgent({
  name: "M4_MissingId",
  constructorParams: { name: Schema.String, id: Schema.String },
  // @ts-expect-error mount path is missing the `{id}` constructor parameter
  http: Http.mount("/agents/{name}"),
  methods: {
    op: method({ params: {}, success: Schema.String }),
  },
  impl: () => Effect.succeed({ op: () => Effect.succeed("ok") }),
})

// Negative case — mount path covers neither constructor parameter.
defineAgent({
  name: "M4_None",
  constructorParams: { name: Schema.String, id: Schema.String },
  // @ts-expect-error mount path is missing `{name}` and `{id}`
  http: Http.mount("/agents"),
  methods: {
    op: method({ params: {}, success: Schema.String }),
  },
  impl: () => Effect.succeed({ op: () => Effect.succeed("ok") }),
})

// ---------------------------------------------------------------------------
// segment-level rules and parser-level checks
// (empty `{}` variable name, nested / malformed braces inside `{...}`).
// ---------------------------------------------------------------------------

// Positive cases — valid segment shapes must still compile.
void Http.mount("/{agent-type}")
void Http.mount("/{agent-type}/{agent-version}")
void Http.get("/files/{*rest}")
void Http.post("/items/{id}")
void Http.endpoint("GET", "/files/{*rest}")

// each segment must be a pure literal OR a single `{var}` /
// `{*rest}` (no mixing).
// @ts-expect-error literal text mixed with '{var}' in one segment
void Http.mount("/foo{bar}")
// @ts-expect-error '{var}' mixed with literal suffix
void Http.mount("/{name}suffix")
// @ts-expect-error literal text mixed with '{var}' in one segment
void Http.get("/items/{id}suffix")
// @ts-expect-error '{var}' embedded between literal characters
void Http.post("/x{y}z")
// @ts-expect-error dangling '{' in an otherwise-literal segment
void Http.get("/items/{id")
// @ts-expect-error dangling '}' in an otherwise-literal segment
void Http.get("/items/id}")

// catch-all `{*rest}` is only allowed as the LAST path segment
// (endpoints).
// @ts-expect-error catch-all is not the last segment
void Http.get("/{*rest}/x")
// @ts-expect-error catch-all is not the last segment
void Http.post("/files/{*rest}/etc")
// @ts-expect-error catch-all is not the last segment
void Http.endpoint("GET", "/files/{*rest}/x")

// catch-all variable name must be non-empty.
// @ts-expect-error empty catch-all name
void Http.get("/{*}")
// @ts-expect-error empty catch-all name
void Http.post("/api/{*}")

// mount paths may NOT contain `{*rest}` at all.
// @ts-expect-error catch-all is not allowed in mount paths
void Http.mount("/files/{*rest}")
// @ts-expect-error catch-all is not allowed in mount paths (even alone)
void Http.mount("/{*rest}")
// @ts-expect-error catch-all is not allowed in mount-suffix
void Http.mount("/c/{name}", { webhookSuffix: "/inbox/{*rest}" })
// @ts-expect-error catch-all is not allowed in mount-suffix
void Http.mount("/c/{name}").pipe(Http.withWebhookSuffix("/inbox/{*rest}"))

// Empty `{}` variable name.
// @ts-expect-error empty '{}' variable name
void Http.mount("/{}")
// @ts-expect-error empty '{}' variable name
void Http.get("/items/{}")
// @ts-expect-error empty '{}' variable name in webhook suffix
void Http.mount("/c/{name}").pipe(Http.withWebhookSuffix("/{}"))

// Nested / malformed braces inside `{...}`.
// @ts-expect-error nested braces inside '{...}'
void Http.get("/items/{a{b}}")
// @ts-expect-error nested braces inside '{...}'
void Http.mount("/items/{a{b}}")

// ---------------------------------------------------------------------------
// duplicate query-parameter keys
// ---------------------------------------------------------------------------

// Positive cases — distinct query keys must compile.
void Http.get("/x?a={a}")
void Http.get("/x?a={a}&b={b}")
void Http.get("/x?a={a}&b={b}&c={c}")
void Http.post("/items/{id}?q={q}&p={p}")
void Http.endpoint("POST", "/y?k={v}")
void Http.endpoint("GET", "/items?a={a}&b={b}")
void Http.custom("PURGE", "/items?a={a}&b={b}")

// Paths without query strings must still compile (UniqueQueryKeys is
// a no-op when there is no '?').
void Http.get("/x")
void Http.get("/items/{id}")
void Http.endpoint("GET", "/items/{id}")

// Negative cases — duplicate keys must be rejected with a message
// that mentions the offending key.
// @ts-expect-error duplicate query key 'a'
void Http.get("/x?a={a}&a={b}")
// @ts-expect-error duplicate query key 'a' (three pairs, last collides)
void Http.get("/x?a={a}&b={b}&a={c}")
// @ts-expect-error duplicate query key 'b' (middle vs last)
void Http.get("/x?a={a}&b={b}&b={c}")
// @ts-expect-error duplicate query key 'k'
void Http.endpoint("POST", "/y?k={a}&k={b}")
// @ts-expect-error duplicate query key 'a' on a verb shorthand
void Http.post("/items?a={a}&a={b}")
// @ts-expect-error duplicate query key 'q' on a custom verb
void Http.custom("PURGE", "/items?q={a}&q={b}")

// ---------------------------------------------------------------------------
// BindableKeys excludes Multimodal / ElementSpec at compile time
// ---------------------------------------------------------------------------

// Plain Schema params remain bindable.
type _Bind1 = BindableKeys<{ id: typeof Schema.String; n: typeof Schema.Number }>
declare const _b1: _Bind1
const _b1Ok: "id" | "n" = _b1
void _b1Ok

// Multimodal / ElementSpec params are filtered out at the type level.
type _Bind2 = BindableKeys<{
  id: typeof Schema.String
  text: ReturnType<typeof UnstructuredText>
  mm: ReturnType<typeof multimodal<{ chunk: ReturnType<typeof UnstructuredText> }>>
}>
declare const _b2: _Bind2
const _b2Ok: "id" = _b2
void _b2Ok

// `BindableKeys<any>` collapses to `string` (variance bypass) so that
// `T extends MethodSpec<any, any, any>` constraints in `withDescription`
// / `withPromptHint` continue to flow specific endpoint defs through.
type _BindAny = BindableKeys<any>
declare const _bAny: _BindAny
const _bAnyOk: string = _bAny
void _bAnyOk

// Positive compile-time test: plain string-bindable keys remain usable.
void method({
  params: { id: Schema.String },
  success: Schema.String,
  http: [Http.post("/items/{id}")],
})

// Negative compile-time test: ElementSpec param can't be bound from a path.
void method({
  params: { id: Schema.String, text: UnstructuredText() },
  success: Schema.String,
  // @ts-expect-error — forbids ElementSpec in path bindings
  http: [Http.post("/items/{text}")],
})

// Negative compile-time test: ElementSpec param can't be bound from a query.
void method({
  params: { text: UnstructuredText() },
  success: Schema.String,
  // @ts-expect-error — forbids ElementSpec in query bindings
  http: [Http.get("/items?t={text}")],
})

// `defineAgent` mount path — every literal var must be a constructor
// param that is itself bindable (i.e. not Multimodal / ElementSpec).
// (Type-only — the body is never executed.)
void (() =>
  defineAgent({
    name: "_Phase5Pos",
    constructorParams: { tenant: Schema.String },
    http: Http.mount("/api/{tenant}"),
    methods: {},
    impl: () => Effect.succeed({}),
  }))

// ---------------------------------------------------------------------------
// webhook-suffix vars validated against constructor params
// ---------------------------------------------------------------------------

// Positive — webhook var matches a bindable constructor key (literal options form).
void (() =>
  defineAgent({
    name: "_Phase6_Pos_Lit",
    constructorParams: { tenant: Schema.String },
    http: Http.mount("/api/{tenant}", { webhookSuffix: "/inbox/{tenant}" }),
    methods: {},
    impl: () => Effect.succeed({}),
  }))

// Positive — webhook var matches a bindable constructor key (pipeable form).
void (() =>
  defineAgent({
    name: "_Phase6_Pos_Pipe",
    constructorParams: { tenant: Schema.String },
    http: Http.mount("/api/{tenant}").pipe(Http.withWebhookSuffix("/inbox/{tenant}")),
    methods: {},
    impl: () => Effect.succeed({}),
  }))

// Positive — webhook suffix uses only system variables.
void (() =>
  defineAgent({
    name: "_Phase6_Pos_System",
    constructorParams: { tenant: Schema.String },
    http: Http.mount("/api/{tenant}", { webhookSuffix: "/inbox/{agent-type}" }),
    methods: {},
    impl: () => Effect.succeed({}),
  }))

// Positive — empty webhook suffix (no vars) is the no-op case.
void (() =>
  defineAgent({
    name: "_Phase6_Pos_Empty",
    constructorParams: { tenant: Schema.String },
    http: Http.mount("/api/{tenant}", { webhookSuffix: "/inbox" }),
    methods: {},
    impl: () => Effect.succeed({}),
  }))

// Negative — webhook var doesn't match any constructor key (literal form).
void (() =>
  defineAgent({
    name: "_Phase6_Neg_Unknown_Lit",
    constructorParams: { tenant: Schema.String },
    // @ts-expect-error — webhook-suffix var '{nope}' is not a constructor parameter
    http: Http.mount("/api/{tenant}", { webhookSuffix: "/inbox/{nope}" }),
    methods: {},
    impl: () => Effect.succeed({}),
  }))

// Negative — webhook var doesn't match any constructor key (pipeable form).
void (() =>
  defineAgent({
    name: "_Phase6_Neg_Unknown_Pipe",
    constructorParams: { tenant: Schema.String },
    // @ts-expect-error — webhook-suffix var '{nope}' is not a constructor parameter
    http: Http.mount("/api/{tenant}").pipe(Http.withWebhookSuffix("/inbox/{nope}")),
    methods: {},
    impl: () => Effect.succeed({}),
  }))

// Negative — webhook var refers to a Multimodal constructor param.
// All constructor params are covered by the mount path, so the rejection
// is unambiguously from WebhookVarsValid (not MountDefCovering).
void (() =>
  defineAgent({
    name: "_Phase6_Neg_Multimodal",
    constructorParams: {
      tenant: Schema.String,
      payload: multimodal({ chunk: UnstructuredText() }),
    },
    // @ts-expect-error — webhook-suffix var '{payload}' refers to a multimodal constructor param
    http: Http.mount("/api/{tenant}/{payload}", { webhookSuffix: "/inbox/{payload}" }),
    methods: {},
    impl: () => Effect.succeed({}),
  }))

// Negative — webhook var refers to an ElementSpec (UnstructuredText)
// constructor param. All constructor params are covered by the mount
// path, so the rejection is unambiguously from WebhookVarsValid.
void (() =>
  defineAgent({
    name: "_Phase6_Neg_Element",
    constructorParams: {
      tenant: Schema.String,
      text: UnstructuredText(),
    },
    // @ts-expect-error — webhook-suffix var '{text}' refers to an ElementSpec constructor param
    http: Http.mount("/api/{tenant}/{text}", { webhookSuffix: "/inbox/{text}" }),
    methods: {},
    impl: () => Effect.succeed({}),
  }))

// ---------------------------------------------------------------------------
// a method param can be bound at most once across
// path / query / header within a single endpoint.
// ---------------------------------------------------------------------------

// Positive — distinct names across path / query / header compile.
void method({
  params: { id: Schema.String, q: Schema.String },
  success: Schema.String,
  http: [Http.get("/items/{id}?q={q}")],
})

void method({
  params: { id: Schema.String, q: Schema.String, idem: Schema.String },
  success: Schema.String,
  http: [Http.post("/items/{id}?q={q}", { headers: { "X-Idem": "idem" } as const })],
})

// Positive — pipeable form, header bound to a fresh name.
void method({
  params: { id: Schema.String, idem: Schema.String },
  success: Schema.String,
  http: [Http.post("/items/{id}").pipe(Http.withHeader("X-Idem", "idem"))],
})

// Negative — path × path duplicate (catch-all + path var with same name
// is impossible because each segment carries one name; same name in two
// path segments is rejected at runtime, but we can craft path × query
// and path × header overlaps directly).

// Negative — path × query duplicate.
void method({
  params: { id: Schema.String },
  success: Schema.String,
  // @ts-expect-error — 'id' is bound from both path and query
  http: [Http.get("/items/{id}?id={id}")],
})

// Negative — path × query duplicate (verb shorthand: post).
void method({
  params: { id: Schema.String },
  success: Schema.String,
  // @ts-expect-error — 'id' is bound from both path and query
  http: [Http.post("/items/{id}?id={id}")],
})

// Negative — path × query duplicate (Http.endpoint generic form).
void method({
  params: { id: Schema.String },
  success: Schema.String,
  // @ts-expect-error — 'id' is bound from both path and query
  http: [Http.endpoint("PUT", "/items/{id}?id={id}")],
})

// Negative — path × query duplicate (Http.custom form).
void method({
  params: { id: Schema.String },
  success: Schema.String,
  // @ts-expect-error — 'id' is bound from both path and query
  http: [Http.custom("PURGE", "/items/{id}?id={id}")],
})

// Negative — query × query duplicate (already a path-level
// failure for paths like `?a={a}&a={b}`, but a parameter bound from
// two distinct query keys to the same name is a separate
// duplicate-binding concern). E.g. `?a={x}&b={x}` parses fine, but
// x is in the query tuple twice.
void method({
  params: { x: Schema.String },
  success: Schema.String,
  // @ts-expect-error — 'x' is bound from two query keys
  http: [Http.get("/items?a={x}&b={x}")],
})

// Negative — path × header duplicate via withHeader.
void method({
  params: { id: Schema.String },
  success: Schema.String,
  http: [
    // @ts-expect-error — 'id' is bound from both path and header
    Http.get("/items/{id}").pipe(Http.withHeader("X-Id", "id")),
  ],
})

// Negative — query × header duplicate via withHeader.
void method({
  params: { q: Schema.String },
  success: Schema.String,
  http: [
    // @ts-expect-error — 'q' is bound from both query and header
    Http.get("/items?q={q}").pipe(Http.withHeader("X-Q", "q")),
  ],
})

// Negative — header × header duplicate via two withHeader calls (same
// `varName`, distinct header names).
void method({
  params: { a: Schema.String },
  success: Schema.String,
  http: [
    // @ts-expect-error — 'a' is bound from two distinct headers
    Http.get("/items").pipe(Http.withHeader("X-A1", "a")).pipe(Http.withHeader("X-A2", "a")),
  ],
})

// Positive — path × header are different names.
void method({
  params: { id: Schema.String, idem: Schema.String },
  success: Schema.String,
  http: [Http.get("/items/{id}").pipe(Http.withHeader("X-Idem", "idem"))],
})

// Positive — endpoints in the same array are validated independently.
// (One endpoint binds `id` from path; another binds `id` from query —
// each in isolation is fine.)
void method({
  params: { id: Schema.String },
  success: Schema.String,
  http: [Http.get("/items/{id}"), Http.get("/items?id={id}")],
})

// Negative — when *any* element of the endpoints array fails the
// duplicate check, the whole `method({...})` call is rejected (not
// just the offending position). The first endpoint is fine (binds
// `id` once from path); the second collides on `id`.
//
// First endpoint binds `id` from the path so the bodyless-coverage
// check does not fire on it — keeping this test focused on the
// duplicate-source check.
void method({
  params: { id: Schema.String },
  success: Schema.String,
  http: [
    Http.get("/items/{id}"),
    // @ts-expect-error — second endpoint binds 'id' from path and query
    Http.get("/items/{id}?id={id}"),
  ],
})

// Positive — `withHeaders` (non-tuple appendation) defers to the
// runtime check. Even though the resulting header slot becomes a
// non-tuple array — short-circuiting the type-level dup detection —
// the call must still compile when names are distinct.
void method({
  params: { id: Schema.String, a: Schema.String, b: Schema.String },
  success: Schema.String,
  http: [Http.get("/items/{id}").pipe(Http.withHeaders({ "X-A": "a", "X-B": "b" }))],
})

// ---------------------------------------------------------------------------
// case-insensitive uniqueness of header names within one
// endpoint.
// ---------------------------------------------------------------------------

// Positive — chained `withHeader` calls with case-distinct names compile.
void method({
  params: { a: Schema.String, b: Schema.String },
  success: Schema.String,
  http: [Http.get("/items").pipe(Http.withHeader("X-A", "a"), Http.withHeader("X-B", "b"))],
})

// Positive — same header name, same casing, two different vars: this is
// a *runtime* error (duplicate-source binding for the var would be
// caught by NoDuplicateBindings if both varNames matched, but
// here distinct varNames means only the case-fold walker fires). The
// case-fold walker should NOT spuriously reject identical-case
// duplicates because they are case-fold-equal trivially — that path is
// caught by the same walker. So this MUST be rejected.
void method({
  params: { a: Schema.String, b: Schema.String },
  success: Schema.String,
  http: [
    // @ts-expect-error — 'X-A' is declared twice on the same endpoint
    Http.get("/items").pipe(Http.withHeader("X-A", "a"), Http.withHeader("X-A", "b")),
  ],
})

// Negative — chained `withHeader` calls with case-folded duplicate
// names (different casing of the same header) — must be rejected.
void method({
  params: { a: Schema.String, b: Schema.String },
  success: Schema.String,
  http: [
    // @ts-expect-error — 'X-A' and 'x-a' are case-fold duplicates
    Http.get("/items").pipe(Http.withHeader("X-A", "a"), Http.withHeader("x-a", "b")),
  ],
})

// Negative — three chained headers, case-fold collision between #1 and #3.
void method({
  params: { a: Schema.String, b: Schema.String, c: Schema.String },
  success: Schema.String,
  http: [
    // @ts-expect-error — 'Content-Type' and 'content-type' are case-fold duplicates
    Http.get("/items").pipe(
      Http.withHeader("Content-Type", "a"),
      Http.withHeader("X-Other", "b"),
      Http.withHeader("content-type", "c"),
    ),
  ],
})

// Positive — record-form `headers: { ... }` literal with case-distinct
// names compiles.
void method({
  params: { a: Schema.String, b: Schema.String },
  success: Schema.String,
  http: [Http.get("/items", { headers: { "X-A": "a", "X-B": "b" } as const })],
})

// Negative — record-form `headers: { ... }` literal with case-fold
// duplicates — must be rejected.
void method({
  params: { a: Schema.String, b: Schema.String },
  success: Schema.String,
  http: [
    // @ts-expect-error — 'X-A' and 'x-a' collide case-insensitively in the headers record
    Http.get("/items", { headers: { "X-A": "a", "x-a": "b" } as const }),
  ],
})

// Negative — record-form via `Http.endpoint` (generic verb).
void method({
  params: { a: Schema.String, b: Schema.String },
  success: Schema.String,
  http: [
    // @ts-expect-error — case-fold duplicate via Http.endpoint
    Http.endpoint("PUT", "/items", { headers: { "X-A": "a", "x-a": "b" } as const }),
  ],
})

// Negative — record-form via `Http.custom` (custom verb).
void method({
  params: { a: Schema.String, b: Schema.String },
  success: Schema.String,
  http: [
    // @ts-expect-error — case-fold duplicate via Http.custom
    Http.custom("PURGE", "/items", { headers: { "X-A": "a", "x-a": "b" } as const }),
  ],
})

// Negative — record-form via verb shorthand `Http.post`.
void method({
  params: { a: Schema.String, b: Schema.String },
  success: Schema.String,
  http: [
    // @ts-expect-error — case-fold duplicate via Http.post
    Http.post("/items", { headers: { "X-A": "a", "x-a": "b" } as const }),
  ],
})

// Positive — case-distinct headers via verb shorthand `Http.post`.
void method({
  params: { a: Schema.String, b: Schema.String },
  success: Schema.String,
  http: [Http.post("/items", { headers: { "X-A": "a", "X-B": "b" } as const })],
})

// Positive — chaining `withHeader` after a record-form header WITH a
// non-colliding name compiles.
void method({
  params: { a: Schema.String, b: Schema.String },
  success: Schema.String,
  http: [
    Http.get("/items", { headers: { "X-A": "a" } as const }).pipe(Http.withHeader("X-B", "b")),
  ],
})

// Positive — when the `header` argument widens to plain `string`
// (non-literal), the case-fold walker short-circuits and the call
// compiles even if the runtime value would clash. (The runtime check
// remains the canonical defence in this case.)
declare const dynamicHeader: string
void method({
  params: { a: Schema.String, b: Schema.String },
  success: Schema.String,
  http: [
    Http.get("/items").pipe(Http.withHeader(dynamicHeader, "a"), Http.withHeader("X-Other", "b")),
  ],
})

// ---------------------------------------------------------------------------
// bodyless verbs (GET / HEAD) cannot have unbound method
// parameters. Bodyful verbs (POST / PUT / DELETE / PATCH / OPTIONS /
// TRACE / CONNECT, plus `Http.endpoint(verb, …)` / `Http.custom(…)`)
// are NOT subject to this check — unbound params map to the JSON
// request body.
// ---------------------------------------------------------------------------

// Positive — `Http.get` / `Http.head` with no method parameters: nothing
// to bind, so the bodyless check is trivially satisfied.
void method({
  params: {},
  success: Schema.Number,
  http: [Http.get("/value"), Http.head("/value")],
})

// Positive — `Http.get` with a single method parameter bound from a
// path variable.
void method({
  params: { id: Schema.String },
  success: Schema.String,
  http: [Http.get("/items/{id}")],
})

// Positive — `Http.get` with the parameter bound from an inline query
// variable. This mirrors the integration-test `counter` agent's
// `Http.get("/add?by={by}")` endpoint and MUST keep compiling.
void method({
  params: { by: Schema.Number },
  success: Schema.Number,
  http: [Http.get("/add?by={by}")],
})

// Positive — `Http.get` with the parameter bound from a header (via
// the literal-options form of `headers`).
void method({
  params: { idem: Schema.String },
  success: Schema.String,
  http: [Http.get("/items", { headers: { "X-Idem": "idem" } as const })],
})

// Positive — `Http.get` with the parameter bound from a header (via
// the pipeable `withHeader` form).
void method({
  params: { idem: Schema.String },
  success: Schema.String,
  http: [Http.get("/items").pipe(Http.withHeader("X-Idem", "idem"))],
})

// Positive — multiple bodyless endpoints in the same array, each
// independently covering the (single) method parameter via a
// different binding source.
void method({
  params: { id: Schema.String },
  success: Schema.String,
  http: [Http.get("/items/{id}"), Http.head("/items?id={id}")],
})

// Positive — `Http.head` with the parameter bound from a path variable.
void method({
  params: { id: Schema.String },
  success: Schema.String,
  http: [Http.head("/items/{id}")],
})

// Negative — `Http.get` with an unbound method parameter (no path /
// query / header binding for `payload`).
void method({
  params: { payload: Schema.String },
  success: Schema.String,
  http: [
    // @ts-expect-error — bodyless GET cannot have unbound 'payload'
    Http.get("/op"),
  ],
})

// Negative — `Http.head` with an unbound method parameter.
void method({
  params: { payload: Schema.String },
  success: Schema.String,
  http: [
    // @ts-expect-error — bodyless HEAD cannot have unbound 'payload'
    Http.head("/op"),
  ],
})

// Negative — bodyless endpoint binds SOME but not all parameters.
void method({
  params: { id: Schema.String, name: Schema.String },
  success: Schema.String,
  http: [
    // @ts-expect-error — bodyless GET only binds 'id', leaves 'name' unbound
    Http.get("/items/{id}"),
  ],
})

// Negative — same array contains a bodyful endpoint (which is fine)
// AND a bodyless endpoint with an unbound parameter (which is not).
// The error fires on the bodyless element only.
void method({
  params: { payload: Schema.String },
  success: Schema.String,
  http: [
    Http.post("/op"),
    // @ts-expect-error — bodyless GET cannot have unbound 'payload'
    Http.get("/op"),
  ],
})

// Positive — bodyful verbs are NEVER subject to the check, regardless
// of binding coverage. `payload` reaches the handler via the JSON
// request body.
void method({
  params: { payload: Schema.String },
  success: Schema.String,
  http: [Http.post("/op")],
})
void method({
  params: { payload: Schema.String },
  success: Schema.String,
  http: [Http.put("/op")],
})
void method({
  params: { payload: Schema.String },
  success: Schema.String,
  http: [Http.del("/op")],
})
void method({
  params: { payload: Schema.String },
  success: Schema.String,
  http: [Http.patch("/op")],
})
void method({
  params: { payload: Schema.String },
  success: Schema.String,
  http: [Http.options("/op")],
})
void method({
  params: { payload: Schema.String },
  success: Schema.String,
  http: [Http.trace("/op")],
})
void method({
  params: { payload: Schema.String },
  success: Schema.String,
  http: [Http.connect("/op")],
})

// Positive — `Http.endpoint(verb, …)` is always tagged "bodyful" even
// when the verb string is "GET" / "HEAD", matching the runtime
// `isBodylessVerb` convention. The compile-time check therefore does
// NOT fire here, even with an unbound parameter.
void method({
  params: { payload: Schema.String },
  success: Schema.String,
  http: [Http.endpoint("GET", "/op")],
})
void method({
  params: { payload: Schema.String },
  success: Schema.String,
  http: [Http.endpoint("HEAD", "/op")],
})

// Positive — `Http.custom(verb, …)` is always tagged "bodyful". The
// runtime treats only the literal `"GET"` / `"HEAD"` shorthands as
// bodyless, so a custom `"PURGE"` is never subject to the check.
void method({
  params: { payload: Schema.String },
  success: Schema.String,
  http: [Http.custom("PURGE", "/op")],
})

// ---------------------------------------------------------------------------
// when any method declares HTTP endpoints, the agent's
// `http: Http.mount(...)` field is required.
// ---------------------------------------------------------------------------

// Positive — agent with NO HTTP methods may omit `http` entirely.
void defineAgent({
  name: "Phase10NoHttpMount",
  constructorParams: { name: Schema.String },
  methods: {
    value: method({ params: {}, success: Schema.Number }),
  },
  impl: () => Effect.succeed({ value: () => Effect.succeed(0) }),
})

// Positive — agent with NO HTTP methods may also omit `http` even when
// `withHttp` is NOT used. Confirms `HasHttp = false` is the default.
void defineAgent({
  name: "Phase10NoHttpMethodsAtAll",
  constructorParams: {},
  methods: {
    ping: method({ params: {}, success: Schema.Void }),
  },
  impl: () => Effect.succeed({ ping: () => Effect.void }),
})

// Positive — agent with HTTP methods AND a matching mount compiles.
void defineAgent({
  name: "Phase10WithHttpAndMount",
  constructorParams: { name: Schema.String },
  http: Http.mount("/agents/{name}"),
  methods: {
    value: method({
      params: {},
      success: Schema.Number,
      http: [Http.get("/value")],
    }),
  },
  impl: () => Effect.succeed({ value: () => Effect.succeed(0) }),
})

// Negative — agent with HTTP methods but NO mount is rejected at
// compile time. The `@ts-expect-error` directive asserts the type
// checker fires; the matching runtime check stays as defence-in-depth.
// @ts-expect-error — methods declare http, agent must declare http
void defineAgent({
  name: "Phase10HttpMethodsMissingMount",
  constructorParams: {},
  methods: {
    value: method({
      params: {},
      success: Schema.Number,
      http: [Http.get("/value")],
    }),
  },
  impl: () => Effect.succeed({ value: () => Effect.succeed(0) }),
})

// Negative — same as above but the http endpoint is added via the
// pipeable `withHttp` combinator. `withHttp` flips `HasHttp` to `true`,
// so the agent-level requirement still fires.
// @ts-expect-error — methods declare http via withHttp, agent must declare http
void defineAgent({
  name: "Phase10WithHttpPipeMissingMount",
  constructorParams: {},
  methods: {
    value: method({ params: {}, success: Schema.Number }).pipe(Method.withHttp(Http.get("/value"))),
  },
  impl: () => Effect.succeed({ value: () => Effect.succeed(0) }),
})

// Positive — agent with an EMPTY http array on a method does NOT
// require a mount. Matches the runtime check (`endpoints.length > 0`).
void defineAgent({
  name: "Phase10EmptyHttpArray",
  constructorParams: {},
  methods: {
    value: method({ params: {}, success: Schema.Number, http: [] }),
  },
  impl: () => Effect.succeed({ value: () => Effect.succeed(0) }),
})

// Positive — `withHttp` + matching mount also compiles.
void defineAgent({
  name: "Phase10WithHttpPipeAndMount",
  constructorParams: {},
  http: Http.mount("/agents"),
  methods: {
    value: method({ params: {}, success: Schema.Number }).pipe(Method.withHttp(Http.get("/value"))),
  },
  impl: () => Effect.succeed({ value: () => Effect.succeed(0) }),
})
