# HTTP routers and live files

Use a router for custom streaming HTTP handling, immutable component files, or a custom
OpenAPI fragment. No HTTP server or OpenAPI framework dependency is required.

```scala
import golem.BaseAgent
import golem.runtime.annotations.*
import golem.runtime.http.*
import scala.concurrent.Future

@httpRouter("Website", "/web", staticBindings = Array(
  ("/", "/site/index.html"),
  ("/assets/*", "/site/$1")
))
trait Website extends BaseAgent {
  @httpHandler def serve(request: HttpRequest): Future[HttpResponse]
  @openApiProvider def describe(): String
}
```

Implement the trait with `@agentImplementation()`, as for ordinary agents. Routers have
no identity parameters and are always ephemeral, with snapshots disabled. Configuration
injection through `AgentConfig[A]`/`Config[A]` and calls to generated ordinary-agent clients
work normally. Routers themselves do not receive ordinary generated clients. The handler,
provider, and static mappings are independently optional; even a static-only router needs
an implementation for registration.

The sole handler takes `request: HttpRequest` and returns `HttpResponse` or
`Future[HttpResponse]`. An optional injected `Principal` uses ordinary method injection.
The provider takes no arguments and returns `String` or `Future[String]` containing an
OpenAPI 3.1.0 JSON fragment. Method names are arbitrary. Configure authentication and CORS
on the router mount, not with endpoint overrides. The host validates, mounts, merges, and
caches provider output; the SDK does not parse or rewrite JSON.

`HttpRequest` retains the full public path and raw optional query. `HttpHeader` preserves
ordered duplicate names and opaque byte values; `HttpHeader.ascii` is a convenience for
ASCII text. `HttpResponse` uses `golem.UShort` for status. Both bodies are
`AgentStream[Array[Byte]]`; their HTTP codecs encode bytes as `u8`, including values above
127. Head-only helpers attach a stream with `withBody`.

`HttpHandler.withMiddleware(handler, wrappers)` composes functional middleware, first
wrapper outermost. `HttpHandler.ensuring(handler)(cleanup)` keeps cleanup attached to the
response body until EOF, failure, or disposal, including when echoing the request body.
It does not wrap host-served files or the OpenAPI provider.

Streams are affine: returning, encoding, mapping, or decorating one transfers ownership.
Do not pull or close the old reference afterward. `AgentStream.ensuring` preserves this
ownership while adding cleanup. Local disposal releases a pending reader, but does not
interrupt arbitrary producer operations. Host stream disposal and active invocation
cancellation are different operations. Output EOF alone is not invocation success.
For HEAD and status 204/205/304, router dispatch disposes the body without pulling it,
before the component bridge can start its producer. These envelope semantics also apply
to direct invocation of the annotated handler. Status and headers remain unchanged for
host validation. The host owns framing, origin trust, and public commitment: failures
before commitment may become an error response; later failures terminate the body.

## Live files belong to ordinary durable agents

```scala
@agentDefinition(mount = "/documents/{owner}", exposeFiles = Array(
  ("/latest", "/public/latest.txt"),
  ("/*", "/public/$1")
))
trait Documents extends BaseAgent {
  class Id(val owner: String)
  def update(text: String): Future[Unit]
}
```

The constructor may create the files and ordinary methods may update them. All identity
fields must be scalar, bound exactly once by mount captures; caller-dependent `Principal`
identity/constructor injection and phantom or ephemeral owners are rejected. These agents
remain ordinary callable agents. `filesystemBindings` is internal metadata; the public
declaration is `exposeFiles`.

Both mapping arrays retain order. Repeating a source with a different target provides
ordered fallback; duplicate identical mappings are rejected. Sources are exact URI paths
or a final `/*`; subtree targets end in `/$1`. Sources decode percent escapes exactly once,
after splitting segments. Targets are absolute filesystem text, never URI-decoded. Empty,
dot, traversal, encoded separators, controls, malformed UTF-8, and ambiguous placeholders
are rejected.

## Examples and verification boundaries

`test-agents/HttpRouterExample.scala` (under `src/main/scala/example/integrationtests`)
demonstrates a combined router, configuration, a generated dependency client, constructor-created
live files, and JSON-text OpenAPI. `cli/golem-cli/tests/app/scala_http_router.rs` deploys it
and generates an additional Scala router from shared corpus response inputs.

- `HttpCorpusSpec` executes all `metadata-*` and `mapping-*` cases through actual Scala
  metadata validation and mapping compilation (27 cases).
- `HttpExchangeSpec` exercises byte/stream adapters on JVM and JS; invocation ownership
  is checked by `InvocationStreamOwnershipSpec` at the method-return boundary.
- The deployed CLI test executes the nine envelope `action: response` cases, checks
  incremental duplex echo, immutable/live files, configuration/dependencies, OpenAPI,
  early request disposal, and errors before and after public commitment.
- Source-discovery/codegen tests execute `tooling-router-provisioning`,
  `tooling-router-clients`, and `tooling-regular-files-still-callable`. The deployed
  configuration/dependency example exercises the inclusion in `tooling-router-config`.
  Platform forwarding trust, raw request framing, generic catalogs, REPL/MCP,
  and host active-invocation cancellation are not established by these SDK tests.
- The Bun corpus checker verifies fixture integrity only, not Scala runtime conformance.

Regression commands from `sdks/scala`:

```sh
sbt golemTestAll
sbt '++3.8.2; codegen/test; ++2.12.21!; codegen/test; sbtPlugin/test'
```

After publishing the SDK locally and building debug Golem binaries, from the repository root:

```sh
GOLEM_CLI_TEST_BIN_PROFILE=debug cargo test -p golem-cli --test integration -- test_scala_http_router_e2e --report-time
```
