package cloud.golem.annotations

/**
 * Declares an HTTP router: an ephemeral agent with no identity whose [mount] is a literal path. The host serves [staticBindings]
 * (immutable component files) itself, and merges the OpenAPI fragment returned by an [OpenApiProvider] method. Routers take no
 * constructor parameters (a `Config<T>` is allowed) and have no snapshots.
 *
 * An optional [HttpHandler] method takes `request: cloud.golem.http.HttpRequest` and returns `HttpResponse`; both bodies are streams.
 */
@Target(AnnotationTarget.CLASS)
@Retention(AnnotationRetention.RUNTIME)
annotation class HttpRouter(
    /** The agent type name (a router has no class-name convention). */
    val typeName: String,
    /** A literal mount path such as `/web` (no `{variables}`). */
    val mount: String,
    /** Files served from the component, e.g. `FileMapping("/", "/site/index.html")` and `FileMapping("/assets/<star>", "/site/<dollar>1")`. */
    val staticBindings: Array<FileMapping> = [],
    val auth: Boolean = false,
    val cors: Array<String> = [],
    /** Extra response headers for the served files, in declaration order. */
    val fileResponseHeaders: Array<ResponseHeader> = [],
)

/**
 * The router's request handler: exactly one method `fun serve(request: HttpRequest): HttpResponse` (any name, optionally combined with an
 * [OpenApiProvider]). It is bound to every method and path below the router mount; configure auth and CORS on the router, not here.
 */
@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class HttpHandler

/** A parameterless router method returning an OpenAPI 3.1.0 JSON fragment as a string. The host validates, mounts and merges it. */
@Target(AnnotationTarget.FUNCTION)
@Retention(AnnotationRetention.RUNTIME)
annotation class OpenApiProvider
