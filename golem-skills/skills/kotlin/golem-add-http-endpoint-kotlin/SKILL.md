---
name: golem-add-http-endpoint-kotlin
description: "Exposing Kotlin agent methods over HTTP. Use when the user asks to add a REST endpoint, mount an agent on a path, bind path/query/header values to parameters, enable auth or CORS, or serve files or a custom handler from an agent."
---

# Adding HTTP Endpoints (Kotlin)

## Mount and routes

`@Agent(mount = ...)` gives the agent a base path; `{var}` segments bind to constructor parameters (the variable name must equal the Kotlin parameter name exactly, so `{listName}` and not `{list-name}`, and every constructor parameter must be bound). `@Endpoint(get/post/put/delete/patch = "path")` exposes a method relative to the mount:

```kotlin
import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.Header

@Agent(mount = "/items/{owner}", auth = false, cors = ["*"])
class Items(val owner: String) : BaseAgent() {

    @Endpoint(post = "/add")
    fun add(name: String): Int = TODO()                    // body field "name"

    @Endpoint(get = "/find/{kind}?q={query}&limit={max}")
    fun find(kind: String, query: String, max: Int, @Header("X-Tenant") tenant: String): String = TODO()

    @Endpoint(patch = "/rename")
    fun rename(to: String): String = TODO()
}
```

- `{param}` in the route is a path capture (`{+rest}` captures the remaining path, endpoint routes only, never the mount), `?name={param}` binds query parameter `name` to method parameter `param`, `@Header("Name")` binds a header. Every binding must name a method parameter and each parameter may be bound once; violations are build errors.
- An `@Endpoint` with a verb needs a mounted agent; a bare `@Endpoint` is RPC-only.
- Parameters not bound to path, query or header come from the request body.
- Setting several verbs on one `@Endpoint` exposes the method under each.
- `auth = true` (on `@Agent` or `@Endpoint`) marks the mount / endpoint as requiring authentication and `cors = ["https://example.com"]` sets allowed origins. These are metadata; the host enforces them, and authentication needs a `securityScheme` for the agent in the `httpApi` deployment (see `golem-configure-api-domain`).
- **`auth` can only tighten, never loosen.** An annotation parameter cannot be nullable in Kotlin, so `@Endpoint(auth = false)` is indistinguishable from omitting it, and the SDK lowers both to `option::none`, which the host reads as "inherit the mount". An endpoint under a mount with `auth = true` therefore cannot be made public. Put public endpoints on a separate agent whose mount has no `auth`.
- Inside a method, `principal` (from `BaseAgent`) tells who called.

## Phantom agents

`@Agent(phantom = true, phantomIdPath = "instance")` (or `phantomIdQuery = "..."`, optionally `phantomIdOptional = true`; setting a selector implies `phantom`) lets the host address extra instances of the same identity per request. Requires a durable, mounted agent without live files. A path selector must name exactly one mount capture that is not a constructor parameter:

```kotlin
@Agent(mount = "/pp/{id}/{instance}", phantomIdPath = "instance")
class PathPhantom(val id: String) : BaseAgent() {
    @Endpoint(get = "/who")
    fun who(): String = "pathphantom:$id"
}
```

## Serving files

```kotlin
import cloud.golem.annotations.FileMapping

@Agent(mount = "/documents/{owner}",
       exposeFiles = [FileMapping("/latest", "/public/latest.txt"), FileMapping("/*", "/public/\$1")])
class Documents(val owner: String) : BaseAgent()
```

Sources are exact paths or end in a final `*` segment; subtree targets end in `$1`. Order matters and a repeated source with another target is an ordered fallback. Constructor parameters must each be scalar and bound exactly once by a mount capture. `fileResponseHeaders = [ResponseHeader(name, value)]` adds response headers.

## Routers and handlers

`@HttpRouter(typeName, mount, staticBindings, auth, cors, fileResponseHeaders)` declares an ephemeral, identity-less agent (no constructor parameters except an optional `Config<T>`; literal mount without `{variables}`) that serves static files and/or a catch-all handler: one `@HttpHandler` method taking `cloud.golem.http.HttpRequest` and returning `HttpResponse`, and one `@OpenApiProvider fun api(): String` (OpenAPI 3.1.0 JSON). Request and response bodies are `AgentStream<ByteArray>`; `HttpResponse.bytes(status, body, headers)` builds a one-chunk response and `HttpHandlers.withMiddleware` / `ensuring` compose handlers.

```kotlin
import cloud.golem.BaseAgent
import cloud.golem.annotations.HttpHandler
import cloud.golem.annotations.HttpRouter
import cloud.golem.annotations.OpenApiProvider
import cloud.golem.http.HttpHeader
import cloud.golem.http.HttpRequest
import cloud.golem.http.HttpResponse

@HttpRouter(typeName = "Echo", mount = "/echo")
class EchoRouter : BaseAgent() {
    @HttpHandler
    fun serve(request: HttpRequest): HttpResponse =
        HttpResponse.bytes(200, request.path.encodeToByteArray(), listOf(HttpHeader.ascii("content-type", "text/plain")))

    @OpenApiProvider
    fun api(): String = """{"openapi":"3.1.0","info":{"title":"echo","version":"1"},"paths":{}}"""
}
```

Add the router's `typeName` under the deployment's `agents:` like any agent. See `sdks/kotlin/docs/api/http-routers.md`.

## Deployment

The HTTP API is deployed from `golem.yaml` (`httpApi.deployments`) — add the agent type name under a deployment (`domain:` or `subdomain:`), then `golem deploy`:

```yaml
httpApi:
  deployments:
    local:
    - subdomain: app-name   # resolves to app-name.localhost:9006 locally
      agents:
        Items: {}
```

The scaffolded templates already contain such an entry for their agents; add yours to it. See `golem-configure-api-domain` for security schemes and custom domains.

## Related skills

- `golem-add-agent-kotlin`
- `golem-streaming-agent-kotlin` — streaming request/response bodies
- `golem-durable-streams-kotlin` — HTTP routes over Durable Streams
