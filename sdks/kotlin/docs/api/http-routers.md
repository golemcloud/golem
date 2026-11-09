# HTTP routers and live files

> `@HttpRouter` agents (the host serves their files, merges their OpenAPI fragment, and sends everything else to an optional
> **handler**) and live files on ordinary durable agents (`@Agent(exposeFiles = ...)`). **Status:** ✅ Complete for the 1.6.0 API.
> The handler's HTTP envelope, streaming bodies and middleware are verified on a real 1.6.0 executor; real HTTP serving is not (see
> *Verification*).

Annotations (`HttpRouter`, `HttpHandler`, `OpenApiProvider`, `FileMapping`, `ResponseHeader`, `Header`) are in `cloud.golem.annotations`;
`HttpRequest`, `HttpResponse`, `HttpHeader`, `HttpHandlers` and the `HttpHandlerFunction` / `HttpMiddleware` aliases are in `cloud.golem.http`.

```kotlin
annotation class HttpRouter(
    val typeName: String,
    val mount: String,
    val staticBindings: Array<FileMapping> = [],
    val auth: Boolean = false,
    val cors: Array<String> = [],
    val fileResponseHeaders: Array<ResponseHeader> = [],
)
```

## Handlers

```kotlin
@HttpRouter(typeName = "Echo", mount = "/echo")
class EchoRouter : BaseAgent() {
    private val handler = HttpHandlers.ensuring(HttpHandlers.withMiddleware(::core, listOf(addHeader))) { cleanup() }

    @HttpHandler
    fun serve(request: HttpRequest): HttpResponse = handler(request)

    @OpenApiProvider
    fun api(): String = """{"openapi":"3.1.0","info":{"title":"echo","version":"1"},"paths":{}}"""
}

fun core(request: HttpRequest): HttpResponse {
    val body = request.body.toList().fold(ByteArray(0)) { a, b -> a + b }          // the request body is a stream
    return HttpResponse(
        200,
        listOf(HttpHeader.ascii("content-type", "text/plain")),
        AgentStream.produce { sink -> sink.write("echo: ".encodeToByteArray()); sink.write(body) },   // keeps producing after serve returned
    )
}
```

- The handler is **one** method taking `request: HttpRequest` and returning `HttpResponse` (any name; at most one handler and one provider,
  nothing else). It is registered with the catch-all `any` binding at the router root, so it receives every method and path below the mount.
  Configure auth and CORS on the router mount, not on the handler.
- `HttpRequest` keeps the original public `path` and the raw optional `query` (an empty query is kept), `method`, `scheme`, `authority`, and
  the `headers`. `HttpHeader` preserves ordered duplicate names and opaque byte values (`HttpHeader.ascii(name, text)`, `.text`).
- `HttpResponse(status, headers, body)`; the status is a `u16` (checked). Keep repeated headers as separate entries (including `set-cookie`).
  `HttpResponse.text(...)` / `.bytes(...)` build single-chunk responses. Both bodies are `AgentStream<ByteArray>`; chunk boundaries are not
  meaningful. Bodies are streams of `list<u8>`, so each byte is one schema element: fine for ordinary pages and APIs, not tuned for
  multi-megabyte payloads. Measured on a local debug stack, a 1 MiB response body streams in about 21 s and a 4 MiB request body is read in about 22 s (about 50 KB per second), and the agent's memory stays flat.
- **Streams are affine** (see [Streams](streams.md)): returning a response moves its body, and the body's producer **runs after the handler
  returned** (the host reads concurrently). Do not read or close a stream you handed on.
- `HttpHandlerFunction` is `(HttpRequest) -> HttpResponse` and `HttpMiddleware` is `(HttpHandlerFunction) -> HttpHandlerFunction`. `HttpRequest.head` / `HttpResponse.head` give a body-less `HttpRequestHead` / `HttpResponseHead` (with `withBody(...)`), handy in middleware.
- `HttpHandlers.withMiddleware(handler, listOf(m1, m2))` composes functional middleware (the first is outermost; middleware runs inside the
  handler, not around host-served files or the OpenAPI provider). `HttpHandlers.ensuring(handler) { cleanup }` runs `cleanup` exactly once when
  the response body ends, fails or is dropped (also when the handler echoes the request body), or when producing the response fails.
- The host owns framing, origin trust and public commitment: a failure before commitment may become an error response, a later one ends the body.

## Routers

```kotlin
@HttpRouter(
    typeName = "Website",
    mount = "/web",
    staticBindings = [FileMapping("/", "/site/index.html"), FileMapping("/assets/*", "/site/\$1")],
    fileResponseHeaders = [ResponseHeader("Content-Security-Policy", "default-src 'self'"), ResponseHeader("Referrer-Policy", "same-origin")],
)
class Website(val config: Config<SiteConfig>) : BaseAgent() {
    @OpenApiProvider
    fun describe(): String = """{"openapi":"3.1.0","info":{"title":"${config.value.title}","version":"1"},"paths":{}}"""
}
```

- A router is **always ephemeral**, has **no identity** (no constructor parameters; a `Config<T>` is allowed, see
  [Agent configuration](config.md)), no snapshots, and a **literal mount** (no `{variables}`). Its agent type name is `typeName`.
- `staticBindings` are files from the component that the host serves itself. `fileResponseHeaders` are added, in declaration order, to
  responses for served files. Configure `auth` / `cors` on the router mount.
- `@OpenApiProvider` marks a parameterless method returning an OpenAPI 3.1.0 JSON fragment as a `String`. The host validates, mounts,
  merges and caches it; the SDK does not parse or rewrite it. The static-only and provider-only forms each work on their own, but a
  router still needs its class for registration.
- Routers get no generated RPC clients, and the agent can be called directly like any agent (the provider method is an ordinary
  invocation). The host only attaches streams between **durable** agents, so a router (ephemeral) is driven by the host's HTTP layer, not by
  another agent's RPC call with a stream.

## Live files on ordinary agents

```kotlin
@Agent(
    mount = "/documents/{owner}",
    exposeFiles = [FileMapping("/latest", "/public/latest.txt"), FileMapping("/*", "/public/\$1")],
    fileResponseHeaders = [ResponseHeader("Referrer-Policy", "no-referrer")],
)
class Documents(val owner: String) : BaseAgent() {
    @Endpoint(post = "/update")
    fun update(text: String): String { /* write /public/latest.txt */ return "ok" }
}
```

The agent's own files below the mapped roots are served live. The agent must be durable with a non-phantom mount, every constructor
parameter must be a scalar bound **exactly once** by a mount capture, and these agents stay ordinary callable agents.

## File mappings

`FileMapping(source, target)`: a source is an exact URI path or ends in a final `/*`; a subtree target ends in `/$1`. Order is
kept; repeating a source with a different target gives an ordered fallback, and an identical duplicate is rejected. Sources are
split into segments and percent-decoded exactly once; targets are absolute filesystem text and are never URI-decoded. Empty, dot,
traversal, encoded-separator, control-character, malformed-UTF-8 and ambiguous-placeholder forms are rejected at build time with a
code (`source-path`, `source-wildcard`, `target-path`, `target-placeholder`, `duplicate-mapping`).

## Endpoint bindings

`@Endpoint` paths can bind more than path captures:

```kotlin
@Agent(mount = "/items/{owner}")
class Items(val owner: String) : BaseAgent() {
    @Endpoint(get = "/find/{kind}?q={query}&limit={max}")
    fun find(kind: String, query: String, max: Int, @Header("X-Tenant") tenant: String): String = ...

    @Endpoint(patch = "/rename")
    fun rename(to: String): String = ...
}
```

- `?name={param}` binds a query parameter to a method parameter; `@Header("Name")` binds a request header. Every binding must name a
  parameter and each parameter is bound once (`invalid query binding`, `bound more than once` are build errors).
- `@Endpoint` takes `post`, `get`, `put`, `delete`, `patch` (each an HTTP path template), plus `path`, `auth` and `cors`.
- **Phantom agents:** `@Agent(phantom = true, phantomIdPath = "instance")` (or `phantomIdQuery = ...`, optionally
  `phantomIdOptional = true`) lets the host create a fresh agent per request. A path selector must name exactly one mount capture that is not a
  constructor parameter; path and query selectors are mutually exclusive, names must not be empty and endpoints must not reuse the selector
  name (`phantom-owner`, `phantom-conflict`).

Verified on a real 1.6.0 executor (`scripts/http-bindings/`): the host read back `header_vars`, `query_vars`, the PATCH method, `phantom_agent`
and `phantom_id_binding` (`Query { optional }` and `Path`) for each fixture agent. Actual HTTP serving is not verified (see below).

## Validation

The processor applies the same rules as the Scala SDK and reports them as build errors (`router-mode`, `router-constructor`,
`router-mount`, `router-method-role`, `provider-schema`, `filesystem-owner`, `unbound-constructor`, `static-bindings-on-regular-agent`,
`ephemeral-snapshotting`, …). They are executed against the shared corpus
`golem-service-base/tests/fixtures/http-handlers/corpus.json` (all 11 `mapping-*` and 16 `metadata-*` cases) in `ksp`'s
`HttpCorpusTest`; the other suites in that corpus (routes, envelopes, files, OpenAPI, lifecycle) describe host behaviour.

## Verification

On a real 1.6.0 executor the fixtures (`scripts/http-routers/`) registered the router agent types and the host read back `kind = HttpRouter`,
`mode = Ephemeral`, the compiled static mappings, the response headers, the OpenAPI provider method and the handler's `any` binding with an
empty path suffix; a durable agent's `filesystem-bindings` were read back too, and the provider returned its JSON with config resolved. The
handler logic was exercised through a durable agent exposing the same method (`EchoAgent`, called by `HttpCaller`): method, path and query,
duplicate request headers, a streamed request body, duplicate `set-cookie` response headers, middleware, response bodies of 2 and 50 chunks
(50 000 bytes), and `ensuring` cleanup once per response.

**Not verified:** actual HTTP serving (it needs the worker service, and a local `golem server` cannot start here without the built-in tool
components), the static-file / OpenAPI behaviour, and full-duplex echo (a handler that reads and writes the same stream concurrently relies on a
concurrent peer such as the host's HTTP server; two blocking Kotlin agents can deadlock, see [Streams](streams.md)).

See also: [Agent model](agent-model.md) · [Agent configuration](config.md) · [SDK README](../../README.md).
