# Auth / CORS Middleware

> Declaring authentication and CORS metadata on an agent's HTTP mount (`@Agent`) and its
> individual endpoints (`@Endpoint`), threaded into the `golem:agent/common@2.0.0` agent-type.
> **Status:** 🟡 Partial — the metadata threads through to the agent-type (jco-verified) and
> route templates are validated at compile time; request-time HTTP enforcement of auth/CORS is
> host-side. The host reads `auth` from the agent type: deploying an `@Agent(auth = true)` mount without a configured security scheme is rejected with `NO_SECURITY_SCHEME_CONFIGURED`. With a `testSessionHeaderName` on the deployment (the host's way to test auth without an identity provider), a real executor rejects a request to an `@Endpoint(auth = true)` method that carries no session with 401, and hands the session's identity to the agent as an `OIDC` principal. An endpoint without `auth` does not resolve the session: its principal stays `Anonymous`. CORS has not been exercised.

## Overview

An [`@Agent`](agent-model.md) can expose an HTTP mount, and each method can expose one or more
[`@Endpoint`](agent-model.md)s under it. Both carry two pieces of middleware metadata:

- **`auth`** — a `Boolean`. When true, the corresponding `auth-details.required` flag is set in
  the agent-type.
- **`cors`** — an array/list of allowed origin patterns (e.g. `["*"]`). Empty means no CORS
  headers are declared.

At build time the KSP-generated descriptor carries these fields, and
`AgentTypeModel.lowerAgentType` lowers them into the canonical-ABI `agent-type` record that
Golem reads via `get-definition()`. The KSP processor also **validates the route templates at
compile time** (see [Compile-time route validation](#compile-time-route-validation)) so
malformed mounts/endpoints fail the build rather than surfacing at deploy time. Request-time
enforcement belongs to Golem's HTTP gateway, not the SDK: the auth/CORS values are emitted into
the agent-type (and round-trip through jco), and the gateway enforces the `auth` requirement on
each request (verified on a real executor with a test session header, see the status line).
Applying the CORS policy has not been exercised.

## Compile-time route validation

The KSP processor (`cloud.golem.ksp.HttpValidation`) checks every `@Agent(mount=...)` /
`@Endpoint` route template against the agent's constructor and method parameters, failing the
build (via `logger.error`) on a violation. Ported from the Scala SDK's `HttpValidation`,
restricted to the path-variable subset the Kotlin surface exposes and using the runtime's
segment convention (`{name}` = path variable, `{+name}` = catch-all). The rules:

- a mount path may not contain a catch-all (`{+name}`) variable;
- every mount path variable must name a constructor parameter;
- every constructor parameter must be provided by the mount path (a mounted agent's identity
  must be fully addressable from its URL);
- a method may not declare HTTP endpoints on an agent with no mount;
- every endpoint path variable (and header / query variable) must name a parameter of that method, and no parameter may be bound more than once;
- a phantom path selector (`@Agent(phantomIdPath = ...)`) must name exactly one mount capture that is not a constructor parameter.

Beyond path variables, the validator also checks header and query bindings (see
[HTTP routers](http-routers.md#endpoint-bindings)): every `@Header` / `?name={param}` binding must name a method
parameter, and a parameter may be bound only once across path, query and header variables. The `Principal`-rejection check
from the Scala version is intentionally not ported: while a caller
[`Principal`](agent-model.md#baseagent) now exists (read via `BaseAgent.principal`), it is **not**
an agent-surface *parameter* type — so there is no Principal-typed path/mount variable for those
checks to reject.

For the SDK overview see [`../../README.md`](../../README.md).

## `@Agent` — mount-level auth / CORS

`cloud.golem.annotations.Agent`. `auth` and `cors` apply to the agent's HTTP mount
(`http-mount-details`):

```kotlin
@Target(AnnotationTarget.CLASS)
@Retention(AnnotationRetention.RUNTIME)
annotation class Agent(
    val mount: String = "",
    val description: String = "",
    /** If true, the mount's `http-mount-details.auth-details` requires authentication. */
    val auth: Boolean = false,
    /** Allowed CORS origin patterns for the mount, e.g. `["*"]`. Empty = no CORS headers. */
    val cors: Array<String> = [],
    val mode: String = "durable",
    val snapshotting: String = "disabled",
    val exposeFiles: Array<FileMapping> = [],
    val fileResponseHeaders: Array<ResponseHeader> = [],
    val phantom: Boolean = false,
    val phantomIdPath: String = "",
    val phantomIdQuery: String = "",
    val phantomIdOptional: Boolean = false,
)
```

(`exposeFiles` / `fileResponseHeaders` / `phantom*` are covered in [HTTP routers](http-routers.md).) `@HttpRouter` also takes `auth` and `cors` for its mount.

The mount's auth/CORS are only emitted when the agent declares a non-empty `mount`.

## `@Endpoint` — endpoint-level auth / CORS

`cloud.golem.annotations.Endpoint`. `auth` and `cors` apply to that single endpoint
(`http-endpoint-details`):

```kotlin
@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class Endpoint(
    val post: String = "",
    val get: String = "",
    val put: String = "",
    val delete: String = "",
    val patch: String = "",
    val path: String = "",
    /** If true, the endpoint's `http-endpoint-details.auth-details` requires authentication. */
    val auth: Boolean = false,
    /** Allowed CORS origin patterns for the endpoint, e.g. `["*"]`. Empty = no CORS headers. */
    val cors: Array<String> = []
)
```

## How it threads into the agent-type

The annotations feed the native descriptor (`cloud.golem.runtime.NativeAgentDescriptor`), whose
relevant fields are:

```kotlin
data class NativeAgentDescriptor(
    // ...
    val mountPath: String,
    /** From `@Agent(auth=..., cors=...)` — the mount's auth-details / cors-options. */
    val mountAuth: Boolean = false,
    val mountCors: List<String> = emptyList(),
    // ...
)

data class NativeHttpEndpoint(
    val verb: String,
    val path: String,
    val auth: Boolean = false,       // from @Endpoint(auth = ...)
    val cors: List<String> = emptyList(),  // from @Endpoint(cors = ...)
    val headerVars: List<Pair<String, String>> = emptyList(),  // from @Header
    val queryVars: List<Pair<String, String>> = emptyList(),   // from ?name={param}
    val durableStreams: NativeDurableStreams? = null,
)
```

`AgentTypeModel.lowerAgentType` then lowers these into the canonical ABI:

- **auth** → `option<auth-details>`: `none` when `auth = false`, otherwise
  `some({ required: true })`. Applied to both the mount (`http-mount-details.auth-details`) and
  each endpoint (`http-endpoint-details.auth-details`).
- **cors** → `cors-options { allowed-patterns: list<string> }`: an empty list when no patterns
  are declared, otherwise the declared origin patterns. Applied to the mount
  (`http-mount-details.cors-options`) and each endpoint (`http-endpoint-details.cors-options`).

## Examples

An agent whose mount requires auth and allows one CORS origin, with a mix of protected and
public endpoints:

```kotlin
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.BaseAgent

@Agent(
    mount = "/counter",
    description = "A durable counter",
    auth = true,
    cors = ["https://app.example.com"]
)
class CounterAgent : BaseAgent() {
    private var count: Int = 0

    // Inherits the mount's auth requirement; adds a wildcard CORS policy for this endpoint.
    @Endpoint(post = "/increment", auth = true, cors = ["*"])
    fun increment(by: Int): Int {
        count += by
        return count
    }

    // A public read endpoint: no auth required, no CORS headers declared.
    @Endpoint(get = "/value")
    fun value(): Int = count
}
```

## Notes

- **The SDK only declares.** Auth/CORS are lowered into the agent-type and verified via jco. The
  HTTP gateway enforces `auth` per request (verified with a test session header). Its CORS
  handling has not been exercised.
- **`auth` is a plain boolean** — it maps to `auth-details.required`. There is no scope /
  scheme / audience modelling yet.
- **`cors` is a list of origin patterns**, lowered verbatim into `cors-options.allowed-patterns`.
  An empty array means no CORS headers are declared (not "deny all" enforcement).
- Mount-level auth/CORS are only emitted when `@Agent(mount = ...)` is non-empty.

See also: [Agent model](agent-model.md) · [Tools](tools.md) · [WASI](wasi.md) ·
[SDK README](../../README.md).
