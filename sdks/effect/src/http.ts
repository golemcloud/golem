import { Effect, Pipeable, Schema, SchemaAST } from "effect"
import type * as AgentCommon from "golem:agent/common@1.5.0"
import { withPipe } from "./pipeable.js"

/**
 * HTTP route metadata for Golem agents.
 *
 * `effect-golem` is a *metadata-only* SDK on the HTTP side: agent code
 * never sees raw HTTP requests. The Golem host owns the HTTP server —
 * it matches the request, enforces auth/CORS, decodes path/query/header
 * values, and ultimately calls `invoke(methodName, dataValue, principal)`
 * with the resulting `DataValue`. The SDK's job is to *advertise* the
 * routing structure via `discoverAgentTypes()` so the host knows how to
 * route incoming requests.
 *
 * Authoring model:
 *
 * ```ts
 * import { Http, defineAgent, method } from "effect-golem"
 *
 * defineAgent({
 *   name: "Counter",
 *   constructorParams: { name: Schema.String },
 *   http: Http.mount("/counters/{agent-type}/{name}", { cors: ["*"] }),
 *   methods: {
 *     value: method({ params: {}, success: Schema.Number, http: [Http.get("/value")] }),
 *     add:   method({
 *       params: { by: Schema.Number },
 *       success: Schema.Number,
 *       http: [
 *         Http.post("/add"),               // by ← JSON body
 *         Http.get("/add?by={by}"),        // by ← query param
 *       ],
 *     }),
 *   },
 *   impl: ...
 * })
 * ```
 */

// ---------------------------------------------------------------------------
// Path-segment IR (also exposed as escape hatches)
// ---------------------------------------------------------------------------

/**
 * Internal representation of a single URL path segment, mirroring the
 * Golem WIT `path-segment` variant.
 */
export type PathSegment =
  | { readonly _tag: "Literal"; readonly value: string }
  | { readonly _tag: "PathVar"; readonly name: string }
  | { readonly _tag: "RestVar"; readonly name: string }
  | { readonly _tag: "SystemVar"; readonly name: SystemVariableName }

/** System-variable names the Golem host injects into routes. */
export type SystemVariableName = "agent-type" | "agent-version"

/** Build a literal path segment, e.g. `Http.literal("api")`. */
export const literal = (value: string): PathSegment => ({ _tag: "Literal", value })

/** Build a path-variable segment, e.g. `Http.pathVar("id")`. */
export const pathVar = (name: string): PathSegment => ({ _tag: "PathVar", name })

/** Build a catch-all (remaining-path) variable segment. Only valid as the last segment. */
export const restVar = (name: string): PathSegment => ({ _tag: "RestVar", name })

/** `{agent-type}` — runtime-injected by the host. */
export const agentType = (): PathSegment => ({ _tag: "SystemVar", name: "agent-type" })

/** `{agent-version}` — runtime-injected by the host. */
export const agentVersion = (): PathSegment => ({ _tag: "SystemVar", name: "agent-version" })

/** Bind one HTTP query parameter to a method parameter. */
export interface QueryVariable {
  readonly queryParam: string
  readonly varName: string
}

/** Bind one HTTP header to a method parameter. */
export interface HeaderVariable {
  readonly header: string
  readonly varName: string
}

// ---------------------------------------------------------------------------
// HTTP verbs
// ---------------------------------------------------------------------------

/**
 * The set of HTTP verbs supported by the Golem host. Standard verbs are
 * uppercased string literals; `{ custom: "VERB" }` carries non-standard
 * verbs verbatim through to `http-method.custom(string)`.
 */
export type HttpVerb =
  | "GET"
  | "HEAD"
  | "POST"
  | "PUT"
  | "DELETE"
  | "CONNECT"
  | "OPTIONS"
  | "TRACE"
  | "PATCH"
  | { readonly custom: string }

const verbToWit = (v: HttpVerb): AgentCommon.HttpMethod => {
  if (typeof v !== "string") return { tag: "custom", val: v.custom }
  switch (v) {
    case "GET":
      return { tag: "get" }
    case "HEAD":
      return { tag: "head" }
    case "POST":
      return { tag: "post" }
    case "PUT":
      return { tag: "put" }
    case "DELETE":
      return { tag: "delete" }
    case "CONNECT":
      return { tag: "connect" }
    case "OPTIONS":
      return { tag: "options" }
    case "TRACE":
      return { tag: "trace" }
    case "PATCH":
      return { tag: "patch" }
  }
}

/** Verbs that may not carry a request body (per the SDK convention). */
const isBodylessVerb = (v: HttpVerb): boolean =>
  typeof v === "string" && (v === "GET" || v === "HEAD")

// ---------------------------------------------------------------------------
// Typed failures
// ---------------------------------------------------------------------------

/**
 * Typed failure surfaced by the parser and validator. Threaded into
 * `registerAgent`'s Effect alongside `UnsupportedSchemaError`.
 */
export class HttpRouteError {
  readonly _tag = "HttpRouteError"
  constructor(readonly reason: string) {}
}

// ---------------------------------------------------------------------------
// Type-level path-variable extraction
// ---------------------------------------------------------------------------

// Splits a path string into its variable names.
//
// - Mixes paths and inline query strings.
// - Recognises `{var}`, `{*rest}`, and `{agent-type}` / `{agent-version}`.
// - System variables are intentionally NOT excluded here; callers compose
//   `Exclude<..., SystemVariableName>` when they want to drop them.

/** Extract all path-variable names from a literal path template (system vars included). */
export type PathVarsOf<S extends string> = ExtractVarsFromPath<SplitPathQuery<S>["path"]>

/** Extract all query-variable values from a literal path template's `?…` portion. */
export type QueryVarsOf<S extends string> = ExtractVarsFromQuery<SplitPathQuery<S>["query"]>

type SplitPathQuery<S extends string> = S extends `${infer P}?${infer Q}`
  ? { path: P; query: Q }
  : { path: S; query: "" }

type ExtractVarsFromPath<S extends string> = S extends `${string}{${infer V}}${infer Rest}`
  ? CleanVarName<V> | ExtractVarsFromPath<Rest>
  : never

type CleanVarName<V extends string> = V extends `*${infer N}` ? N : V

type ExtractVarsFromQuery<S extends string> = S extends `${string}={${infer V}}${infer Rest}`
  ? V | ExtractVarsFromQuery<Rest>
  : never

/**
 * Extract the union of values from a `Record<string, string>` literal.
 * Returns `never` for empty records (`{}` should not widen the binding
 * union to `string`).
 */
export type ValuesOf<R> = [keyof R] extends [never]
  ? never
  : R extends Readonly<Record<string, infer V>>
    ? V & string
    : never

// ---------------------------------------------------------------------------
// Mount and Endpoint typed wrappers
// ---------------------------------------------------------------------------

declare const mountVarsBrand: unique symbol
declare const endpointVarsBrand: unique symbol

/**
 * Mount declaration carried by `AgentDefinition.http`. Compiled to
 * `agent-type.http-mount` (`HttpMountDetails`) at registration time.
 *
 * The phantom `MountVars` parameter is **not** present at runtime —
 * `agent.ts` enforces `MountVars extends keyof ConstructorParams` to
 * give a compile-time signal when a `{var}` in the mount path doesn't
 * match a constructor param.
 *
 * Instances are {@link Pipeable.Pipeable}: the pipeable-builder
 * combinators ({@link withAuth}, {@link withCors},
 * {@link withPhantomAgent}, {@link withWebhookSuffix}) compose
 * additively with the literal-options form accepted by {@link mount}.
 */
export interface MountDef<MountVars extends string> extends Pipeable.Pipeable {
  readonly [mountVarsBrand]?: MountVars
  readonly pathPrefix: ReadonlyArray<PathSegment>
  readonly authRequired: boolean
  readonly cors: ReadonlyArray<string>
  readonly phantomAgent: boolean
  readonly webhookSuffix: ReadonlyArray<PathSegment>
}

/**
 * Endpoint declaration carried by `MethodSpec.http`. Compiled to one
 * entry of `agent-method.http-endpoint` (`HttpEndpointDetails`) at
 * registration time.
 *
 * `EndpointVars` collects all variable names referenced by the path,
 * query string, and header bindings of this single endpoint.
 *
 * Instances are {@link Pipeable.Pipeable}: the pipeable-builder
 * combinators ({@link withAuth}, {@link withCors}, {@link withHeader},
 * {@link withHeaders}) compose additively with the literal-options
 * form accepted by {@link endpoint} and the verb shorthands.
 */
export interface EndpointDef<EndpointVars extends string> extends Pipeable.Pipeable {
  readonly [endpointVarsBrand]?: EndpointVars
  readonly verb: HttpVerb
  readonly pathSuffix: ReadonlyArray<PathSegment>
  readonly queryVars: ReadonlyArray<QueryVariable>
  readonly headerVars: ReadonlyArray<HeaderVariable>
  /** `undefined` = inherit from mount; `true`/`false` = override. */
  readonly authRequired?: boolean
  readonly cors: ReadonlyArray<string>
}

// ---------------------------------------------------------------------------
// String-template parser (single source of truth at runtime)
// ---------------------------------------------------------------------------

interface ParsedPath {
  readonly path: ReadonlyArray<PathSegment>
  readonly query: ReadonlyArray<QueryVariable>
}

const parseSegmentToken = (
  token: string,
  isLast: boolean,
  context: string,
): Effect.Effect<PathSegment, HttpRouteError> => {
  if (token === "") {
    return Effect.fail(
      new HttpRouteError(`${context}: empty path segment (consecutive '/' or trailing '/')`),
    )
  }
  // A `{var}` or `{*rest}` token must occupy the entire segment — no
  // mixing of literals and variables within one segment.
  const startsWithBrace = token.startsWith("{")
  const endsWithBrace = token.endsWith("}")
  if (startsWithBrace !== endsWithBrace) {
    return Effect.fail(
      new HttpRouteError(`${context}: malformed segment '${token}' (unbalanced '{' / '}')`),
    )
  }
  if (!startsWithBrace) {
    if (token.includes("{") || token.includes("}")) {
      return Effect.fail(
        new HttpRouteError(
          `${context}: segment '${token}' mixes literal text with '{...}' variables — each segment must be either a literal or a single '{var}' / '{*rest}'`,
        ),
      )
    }
    return Effect.succeed(literal(token))
  }
  // Trim the surrounding braces.
  const inner = token.slice(1, -1)
  if (inner === "") {
    return Effect.fail(new HttpRouteError(`${context}: empty variable name in '{}'`))
  }
  if (inner.includes("{") || inner.includes("}")) {
    return Effect.fail(new HttpRouteError(`${context}: nested or malformed braces in '${token}'`))
  }
  if (inner.startsWith("*")) {
    if (!isLast) {
      return Effect.fail(
        new HttpRouteError(
          `${context}: catch-all segment '${token}' is only allowed as the LAST path segment`,
        ),
      )
    }
    const name = inner.slice(1)
    if (name === "") {
      return Effect.fail(new HttpRouteError(`${context}: catch-all variable has no name`))
    }
    if (!isValidVarName(name)) {
      return Effect.fail(
        new HttpRouteError(`${context}: invalid catch-all variable name '${name}'`),
      )
    }
    return Effect.succeed(restVar(name))
  }
  if (inner === "agent-type") return Effect.succeed(agentType())
  if (inner === "agent-version") return Effect.succeed(agentVersion())
  if (!isValidVarName(inner)) {
    return Effect.fail(new HttpRouteError(`${context}: invalid path-variable name '${inner}'`))
  }
  return Effect.succeed(pathVar(inner))
}

const isValidVarName = (s: string): boolean => /^[A-Za-z_][A-Za-z0-9_-]*$/.test(s)

const parseQueryString = (
  query: string,
  context: string,
): Effect.Effect<ReadonlyArray<QueryVariable>, HttpRouteError> =>
  Effect.gen(function* () {
    if (query === "") return []
    const out: Array<QueryVariable> = []
    const seenKeys = new Set<string>()
    for (const pair of query.split("&")) {
      if (pair === "") {
        return yield* Effect.fail(
          new HttpRouteError(`${context}: empty query parameter (extra '&')`),
        )
      }
      const eq = pair.indexOf("=")
      if (eq < 0) {
        return yield* Effect.fail(
          new HttpRouteError(`${context}: query parameter '${pair}' missing '={var}'`),
        )
      }
      const key = pair.slice(0, eq)
      const value = pair.slice(eq + 1)
      if (key === "") {
        return yield* Effect.fail(new HttpRouteError(`${context}: empty query parameter name`))
      }
      if (!value.startsWith("{") || !value.endsWith("}")) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${context}: query parameter '${key}' value must be '{varName}', got '${value}'`,
          ),
        )
      }
      const varName = value.slice(1, -1)
      if (!isValidVarName(varName)) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${context}: query parameter '${key}' has invalid variable name '${varName}'`,
          ),
        )
      }
      if (seenKeys.has(key)) {
        return yield* Effect.fail(
          new HttpRouteError(`${context}: duplicate query parameter key '${key}'`),
        )
      }
      seenKeys.add(key)
      out.push({ queryParam: key, varName })
    }
    return out
  })

const parsePathString = (
  rawPath: string,
  context: string,
): Effect.Effect<ReadonlyArray<PathSegment>, HttpRouteError> =>
  Effect.gen(function* () {
    if (!rawPath.startsWith("/")) {
      return yield* Effect.fail(new HttpRouteError(`${context}: path must start with '/'`))
    }
    if (rawPath === "/") return []
    const stripped = rawPath.slice(1)
    if (stripped.endsWith("/")) {
      return yield* Effect.fail(
        new HttpRouteError(`${context}: path must not end with '/' (except '/')`),
      )
    }
    const tokens = stripped.split("/")
    const segments: Array<PathSegment> = []
    for (let i = 0; i < tokens.length; i++) {
      const seg = yield* parseSegmentToken(tokens[i]!, i === tokens.length - 1, context)
      segments.push(seg)
    }
    return segments
  })

/**
 * Parse a mount path. Mount paths must NOT contain a query string (the
 * Golem WIT does not support mount-level query bindings) and must NOT
 * include a catch-all (`{*rest}`) segment.
 */
export const parseMountPath = (
  raw: string,
): Effect.Effect<ReadonlyArray<PathSegment>, HttpRouteError> =>
  Effect.gen(function* () {
    if (raw.includes("?")) {
      return yield* Effect.fail(
        new HttpRouteError(
          `mount path '${raw}' may not include a query string ('?…') — query parameters are only valid on endpoints`,
        ),
      )
    }
    const segments = yield* parsePathString(raw, `mount path '${raw}'`)
    for (const s of segments) {
      if (s._tag === "RestVar") {
        return yield* Effect.fail(
          new HttpRouteError(
            `mount path '${raw}' may not include a catch-all segment '{*${s.name}}'`,
          ),
        )
      }
    }
    return segments
  })

/**
 * Parse an endpoint path. Returns the parsed path segments and the
 * inline query bindings discovered after `?…`.
 */
export const parseEndpointPath = (raw: string): Effect.Effect<ParsedPath, HttpRouteError> =>
  Effect.gen(function* () {
    const qIdx = raw.indexOf("?")
    const pathPart = qIdx < 0 ? raw : raw.slice(0, qIdx)
    const queryPart = qIdx < 0 ? "" : raw.slice(qIdx + 1)
    if (raw.indexOf("?", qIdx + 1) >= 0) {
      return yield* Effect.fail(
        new HttpRouteError(`endpoint path '${raw}' contains more than one '?'`),
      )
    }
    const segments = yield* parsePathString(pathPart, `endpoint path '${raw}'`)
    const query = yield* parseQueryString(queryPart, `endpoint path '${raw}'`)
    return { path: segments, query }
  })

const runParse = <A>(eff: Effect.Effect<A, HttpRouteError>): A => {
  const exit = Effect.runSyncExit(eff)
  if (exit._tag === "Failure") {
    // The parser is pure synchronous — surface its typed failure as a
    // throw so the simple builder API stays single-line. Compilation
    // (registration) re-validates and produces an Effect-channel error.
    const cause = exit.cause
    let msg = "HttpRouteError"
    // Best-effort extraction of the failure value.
    const fail = (cause as unknown as { failures?: ReadonlyArray<unknown> }).failures
    if (fail && fail.length > 0) {
      const f0 = fail[0] as { reason?: string }
      if (f0?.reason) msg = f0.reason
    }
    throw new HttpRouteError(msg)
  }
  return exit.value
}

// ---------------------------------------------------------------------------
// Public mount / endpoint factories
// ---------------------------------------------------------------------------

/** Options accepted by {@link mount}. */
export interface MountOptions {
  /** When `true`, the host treats every endpoint as authentication-required. */
  readonly auth?: boolean
  /** CORS allowed-origin patterns advertised at the mount level. */
  readonly cors?: ReadonlyArray<string>
  /** Mark this agent as a phantom agent (one fresh instance per HTTP request). */
  readonly phantomAgent?: boolean
  /**
   * Optional custom webhook suffix path. Parsed with the same rules as
   * the mount path (no query, no catch-all).
   */
  readonly webhookSuffix?: string
}

/**
 * Declare an HTTP mount for an agent. The path may include `{var}` and
 * `{agent-type}` / `{agent-version}` segments; every `{var}` must
 * correspond to a constructor parameter on the agent.
 */
export const mount: <const Path extends string>(
  path: Path,
  opts?: MountOptions,
) => MountDef<Exclude<PathVarsOf<Path>, SystemVariableName>> = ((
  path: string,
  opts?: MountOptions,
) => {
  const segments = runParse(parseMountPath(path))
  const webhookSuffix = opts?.webhookSuffix ? runParse(parseMountPath(opts.webhookSuffix)) : []
  return withPipe({
    pathPrefix: segments,
    authRequired: opts?.auth ?? false,
    cors: opts?.cors ?? [],
    phantomAgent: opts?.phantomAgent ?? false,
    webhookSuffix,
  }) as unknown as MountDef<never>
}) as never

/**
 * Default value for the `H` (headers) generic on the endpoint helpers.
 * An "empty record" sentinel that signals "no header bindings"; using
 * `{}` directly would trip `@typescript-eslint/no-empty-object-type`
 * and would also widen `ValuesOf<H>` to `string`.
 */
export type NoHeaderBindings = Readonly<Record<string, never>>

/** Options accepted by {@link endpoint} and the verb shorthands. */
export interface EndpointOptions<H extends Readonly<Record<string, string>> = NoHeaderBindings> {
  /**
   * Map of HTTP header name → method-parameter name. Header names are
   * case-insensitive at HTTP level; collisions after lower-casing are
   * rejected at registration time.
   */
  readonly headers?: H
  /** Override the mount-level auth requirement for this endpoint only. */
  readonly auth?: boolean
  /** Additional CORS allowed-origin patterns for this endpoint. */
  readonly cors?: ReadonlyArray<string>
}

const buildEndpoint = <H extends Readonly<Record<string, string>>>(
  verb: HttpVerb,
  rawPath: string,
  opts?: EndpointOptions<H>,
): EndpointDef<string> => {
  const parsed = runParse(parseEndpointPath(rawPath))
  const headerVars: Array<HeaderVariable> = []
  if (opts?.headers !== undefined) {
    for (const [header, varName] of Object.entries(opts.headers)) {
      headerVars.push({ header, varName: String(varName) })
    }
  }
  return withPipe({
    verb,
    pathSuffix: parsed.path,
    queryVars: parsed.query,
    headerVars,
    authRequired: opts?.auth,
    cors: opts?.cors ?? [],
  }) as unknown as EndpointDef<string>
}

/**
 * Declare an HTTP endpoint for a method. The path is relative to the
 * agent's mount prefix and may include `{var}`, `{*rest}`,
 * `{agent-type}`, `{agent-version}`, and inline `?key={var}&…` query
 * bindings.
 */
export const endpoint: <
  const Path extends string,
  H extends Readonly<Record<string, string>> = NoHeaderBindings,
>(
  verb: HttpVerb,
  path: Path,
  opts?: EndpointOptions<H>,
) => EndpointDef<Exclude<PathVarsOf<Path> | QueryVarsOf<Path>, SystemVariableName> | ValuesOf<H>> =
  buildEndpoint as never

const verbHelper = (verb: HttpVerb) =>
  (<const Path extends string, H extends Readonly<Record<string, string>> = NoHeaderBindings>(
    path: Path,
    opts?: EndpointOptions<H>,
  ): EndpointDef<Exclude<PathVarsOf<Path> | QueryVarsOf<Path>, SystemVariableName> | ValuesOf<H>> =>
    buildEndpoint(verb, path, opts) as never) as never

/** Shorthand for `Http.endpoint("GET", path, opts?)`. */
export const get: <
  const Path extends string,
  H extends Readonly<Record<string, string>> = NoHeaderBindings,
>(
  path: Path,
  opts?: EndpointOptions<H>,
) => EndpointDef<Exclude<PathVarsOf<Path> | QueryVarsOf<Path>, SystemVariableName> | ValuesOf<H>> =
  verbHelper("GET")
/** Shorthand for `Http.endpoint("HEAD", path, opts?)`. */
export const head: typeof get = verbHelper("HEAD")
/** Shorthand for `Http.endpoint("POST", path, opts?)`. */
export const post: typeof get = verbHelper("POST")
/** Shorthand for `Http.endpoint("PUT", path, opts?)`. */
export const put: typeof get = verbHelper("PUT")
/** Shorthand for `Http.endpoint("DELETE", path, opts?)`. */
export const del: typeof get = verbHelper("DELETE")
/** Shorthand for `Http.endpoint("PATCH", path, opts?)`. */
export const patch: typeof get = verbHelper("PATCH")
/** Shorthand for `Http.endpoint("OPTIONS", path, opts?)`. */
export const options: typeof get = verbHelper("OPTIONS")
/** Shorthand for `Http.endpoint("TRACE", path, opts?)`. */
export const trace: typeof get = verbHelper("TRACE")
/** Shorthand for `Http.endpoint("CONNECT", path, opts?)`. */
export const connect: typeof get = verbHelper("CONNECT")

/** Shorthand for a custom (non-standard) HTTP verb. */
export const custom: <
  const Path extends string,
  H extends Readonly<Record<string, string>> = NoHeaderBindings,
>(
  verb: string,
  path: Path,
  opts?: EndpointOptions<H>,
) => EndpointDef<Exclude<PathVarsOf<Path> | QueryVarsOf<Path>, SystemVariableName> | ValuesOf<H>> =
  ((verb: string, path: string, opts?: EndpointOptions<NoHeaderBindings>) =>
    buildEndpoint({ custom: verb }, path, opts)) as never

// ---------------------------------------------------------------------------
// Pipeable combinators
//
// These layer cross-cutting facets onto a previously-built `MountDef` /
// `EndpointDef` so users can compose them with the canonical Effect
// `.pipe(...)` style, e.g.:
//
//   Http.mount("/c/{name}").pipe(
//     Http.withAuth(true),
//     Http.withCors("https://x.com"),
//     Http.withWebhookSuffix("/inbox"),
//   )
//
//   Http.get("/v").pipe(
//     Http.withAuth(false),
//     Http.withHeader("X-Idem", "key"),
//   )
//
// Every combinator returns a fresh, pipeable record — input is never
// mutated. The literal-options form passed to `mount(..., {...})` and
// `endpoint(..., {...})` keeps working unchanged.
// ---------------------------------------------------------------------------

/**
 * Override the `auth` flag on a mount or endpoint. Replaces any previous
 * value (mount default `false`; endpoint default `undefined` =
 * inherit-from-mount). Pipeable; `EndpointVars` / `MountVars` are
 * preserved unchanged.
 */
export const withAuth =
  (auth: boolean) =>
  <T extends MountDef<string> | EndpointDef<string>>(t: T): T =>
    withPipe({ ...t, authRequired: auth }) as unknown as T

/**
 * Replace the CORS `allowed-origin` pattern list on a mount or
 * endpoint. Pipeable; `EndpointVars` / `MountVars` are preserved
 * unchanged.
 *
 * Replaces (does not append) the previous list, matching the semantics
 * of `HttpApiEndpoint.setCors` in `@effect/platform`. Use multiple
 * `withCors(...)` calls if you want the last one to win, or pass all
 * patterns to a single call.
 */
export const withCors =
  (...patterns: ReadonlyArray<string>) =>
  <T extends MountDef<string> | EndpointDef<string>>(t: T): T =>
    withPipe({ ...t, cors: patterns }) as unknown as T

/**
 * Append a single header → method-parameter binding to an endpoint.
 * Header names are case-insensitive at HTTP level; collisions after
 * lower-casing are rejected at registration time. Widens the
 * `EndpointVars` phantom to include the bound parameter name.
 */
export const withHeader =
  <const Var extends string>(header: string, varName: Var) =>
  <V extends string>(ep: EndpointDef<V>): EndpointDef<V | Var> =>
    withPipe({
      ...ep,
      headerVars: [...ep.headerVars, { header, varName }],
    }) as unknown as EndpointDef<V | Var>

/**
 * Append multiple header → method-parameter bindings to an endpoint.
 * Equivalent to chaining a series of {@link withHeader} calls. Widens
 * the `EndpointVars` phantom to include every bound parameter name.
 */
export const withHeaders =
  <const H extends Readonly<Record<string, string>>>(headers: H) =>
  <V extends string>(ep: EndpointDef<V>): EndpointDef<V | ValuesOf<H>> => {
    const headerVars = [...ep.headerVars]
    for (const [header, varName] of Object.entries(headers)) {
      headerVars.push({ header, varName: String(varName) })
    }
    return withPipe({ ...ep, headerVars }) as unknown as EndpointDef<V | ValuesOf<H>>
  }

/**
 * Set the `phantom-agent` flag on a mount (one fresh agent instance per
 * HTTP request). Pipeable; `MountVars` is preserved unchanged.
 */
export const withPhantomAgent =
  (phantom: boolean = true) =>
  <V extends string>(m: MountDef<V>): MountDef<V> =>
    withPipe({ ...m, phantomAgent: phantom }) as unknown as MountDef<V>

/**
 * Override the webhook-suffix path on a mount. Parsed with the same
 * rules as the mount path itself — no query string and no catch-all
 * (`{*rest}`) are allowed. Webhook-suffix path variables are validated
 * against constructor-parameter names at registration time, so they
 * are NOT folded into the `MountVars` phantom.
 */
export const withWebhookSuffix =
  (suffix: string) =>
  <V extends string>(m: MountDef<V>): MountDef<V> =>
    withPipe({
      ...m,
      webhookSuffix: runParse(parseMountPath(suffix)),
    }) as unknown as MountDef<V>

// ---------------------------------------------------------------------------
// Compilation: MountDef / EndpointDef → WIT records
// ---------------------------------------------------------------------------

const segmentToWit = (s: PathSegment): AgentCommon.PathSegment => {
  switch (s._tag) {
    case "Literal":
      return { tag: "literal", val: s.value }
    case "PathVar":
      return { tag: "path-variable", val: { variableName: s.name } }
    case "RestVar":
      return { tag: "remaining-path-variable", val: { variableName: s.name } }
    case "SystemVar":
      return { tag: "system-variable", val: s.name }
  }
}

/** Compile a {@link MountDef} to the WIT `http-mount-details` record. */
export const compileMount = (mountDef: MountDef<string>): AgentCommon.HttpMountDetails => ({
  pathPrefix: mountDef.pathPrefix.map(segmentToWit),
  authDetails: mountDef.authRequired ? { required: true } : undefined,
  phantomAgent: mountDef.phantomAgent,
  corsOptions: { allowedPatterns: [...mountDef.cors] },
  webhookSuffix: mountDef.webhookSuffix.map(segmentToWit),
})

/** Compile a single {@link EndpointDef} to a WIT `http-endpoint-details` record. */
export const compileEndpoint = (ep: EndpointDef<string>): AgentCommon.HttpEndpointDetails => ({
  httpMethod: verbToWit(ep.verb),
  pathSuffix: ep.pathSuffix.map(segmentToWit),
  headerVars: ep.headerVars.map((h) => ({
    headerName: h.header,
    variableName: h.varName,
  })),
  queryVars: ep.queryVars.map((q) => ({
    queryParamName: q.queryParam,
    variableName: q.varName,
  })),
  authDetails: ep.authRequired === undefined ? undefined : { required: ep.authRequired },
  corsOptions: { allowedPatterns: [...ep.cors] },
})

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/**
 * Result of `validateAgentHttp`: the compiled WIT mount (when present)
 * and a per-method-name list of compiled endpoints (always present;
 * empty for methods that did not declare any HTTP endpoints).
 */
export interface CompiledHttp {
  readonly mount: AgentCommon.HttpMountDetails | undefined
  readonly endpoints: ReadonlyMap<string, ReadonlyArray<AgentCommon.HttpEndpointDetails>>
}

/** Per-method input describing what the validator can see at registration time. */
export interface MethodHttpInput {
  readonly name: string
  readonly params: Readonly<Record<string, unknown>>
  readonly endpoints: ReadonlyArray<EndpointDef<string>>
  /**
   * Names of method parameters that are NOT eligible to be bound from
   * a string source (path / query / header) — typically multimodal /
   * unstructured-binary / unstructured-text.
   */
  readonly nonStringBindableParams: ReadonlySet<string>
  /**
   * Names of method parameters whose schema is a plain string-bindable
   * Schema (string / number / bigint / boolean / literal / branded
   * variants thereof).
   */
  readonly stringBindableParams: ReadonlySet<string>
}

/** Per-agent input for `validateAgentHttp`. */
export interface AgentHttpInput {
  readonly agentName: string
  readonly mount: MountDef<string> | undefined
  readonly constructorParamNames: ReadonlyArray<string>
  /**
   * Names of constructor parameters that are NOT eligible to be bound
   * from a path variable (e.g. multimodal / unstructured-binary).
   */
  readonly nonStringBindableConstructorParams: ReadonlySet<string>
  readonly methods: ReadonlyArray<MethodHttpInput>
}

const lowerCase = (s: string) => s.toLowerCase()

const validateEndpoint = (
  agentName: string,
  m: MethodHttpInput,
  ep: EndpointDef<string>,
  endpointIndex: number,
): Effect.Effect<void, HttpRouteError> =>
  Effect.gen(function* () {
    const ctx = `agent '${agentName}' method '${m.name}' endpoint #${endpointIndex}`
    const seenSources = new Map<string, "path" | "query" | "header">()

    // Path-suffix variables.
    let restCount = 0
    for (let i = 0; i < ep.pathSuffix.length; i++) {
      const seg = ep.pathSuffix[i]!
      if (seg._tag === "Literal" || seg._tag === "SystemVar") continue
      if (seg._tag === "RestVar") {
        restCount += 1
        if (i !== ep.pathSuffix.length - 1) {
          return yield* Effect.fail(
            new HttpRouteError(
              `${ctx}: catch-all '{*${seg.name}}' is only allowed as the LAST path segment`,
            ),
          )
        }
      }
      const name = seg.name
      if (!(name in m.params)) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${ctx}: path variable '${name}' does not match any method parameter (params: ${Object.keys(m.params).join(", ") || "<none>"})`,
          ),
        )
      }
      if (m.nonStringBindableParams.has(name)) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${ctx}: parameter '${name}' is multimodal/unstructured and cannot be bound from a path variable`,
          ),
        )
      }
      if (!m.stringBindableParams.has(name)) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${ctx}: parameter '${name}' has a schema that is not bindable from a path variable (only String, Number, BigInt, Boolean, Literal, or branded variants thereof are supported)`,
          ),
        )
      }
      const prev = seenSources.get(name)
      if (prev !== undefined) {
        return yield* Effect.fail(
          new HttpRouteError(`${ctx}: parameter '${name}' is bound from both '${prev}' and 'path'`),
        )
      }
      seenSources.set(name, "path")
    }
    if (restCount > 1) {
      return yield* Effect.fail(
        new HttpRouteError(`${ctx}: more than one catch-all variable in path`),
      )
    }

    // Query variables.
    const seenQueryKeys = new Set<string>()
    for (const q of ep.queryVars) {
      if (seenQueryKeys.has(q.queryParam)) {
        return yield* Effect.fail(
          new HttpRouteError(`${ctx}: duplicate query parameter '${q.queryParam}'`),
        )
      }
      seenQueryKeys.add(q.queryParam)
      if (!(q.varName in m.params)) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${ctx}: query variable '${q.varName}' (from query parameter '${q.queryParam}') does not match any method parameter`,
          ),
        )
      }
      if (m.nonStringBindableParams.has(q.varName)) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${ctx}: parameter '${q.varName}' is multimodal/unstructured and cannot be bound from a query parameter`,
          ),
        )
      }
      if (!m.stringBindableParams.has(q.varName)) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${ctx}: parameter '${q.varName}' has a schema that is not bindable from a query parameter`,
          ),
        )
      }
      const prev = seenSources.get(q.varName)
      if (prev !== undefined) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${ctx}: parameter '${q.varName}' is bound from both '${prev}' and 'query'`,
          ),
        )
      }
      seenSources.set(q.varName, "query")
    }

    // Header variables (case-insensitive uniqueness).
    const seenHeaderKeys = new Set<string>()
    for (const h of ep.headerVars) {
      const key = lowerCase(h.header)
      if (seenHeaderKeys.has(key)) {
        return yield* Effect.fail(
          new HttpRouteError(`${ctx}: duplicate header '${h.header}' (case-insensitive)`),
        )
      }
      seenHeaderKeys.add(key)
      if (!(h.varName in m.params)) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${ctx}: header variable '${h.varName}' (from header '${h.header}') does not match any method parameter`,
          ),
        )
      }
      if (m.nonStringBindableParams.has(h.varName)) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${ctx}: parameter '${h.varName}' is multimodal/unstructured and cannot be bound from a header`,
          ),
        )
      }
      if (!m.stringBindableParams.has(h.varName)) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${ctx}: parameter '${h.varName}' has a schema that is not bindable from a header`,
          ),
        )
      }
      const prev = seenSources.get(h.varName)
      if (prev !== undefined) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${ctx}: parameter '${h.varName}' is bound from both '${prev}' and 'header'`,
          ),
        )
      }
      seenSources.set(h.varName, "header")
    }

    // Body restrictions on bodyless verbs.
    if (isBodylessVerb(ep.verb)) {
      for (const paramName of Object.keys(m.params)) {
        if (!seenSources.has(paramName)) {
          return yield* Effect.fail(
            new HttpRouteError(
              `${ctx}: ${verbLabel(ep.verb)} endpoints may not have unbound parameters (parameter '${paramName}' is not bound to any path/query/header variable, so it would map to the request body)`,
            ),
          )
        }
      }
    }
  })

const verbLabel = (v: HttpVerb): string => (typeof v === "string" ? v : `custom('${v.custom}')`)

/**
 * Validate a complete agent's HTTP route metadata, producing the
 * compiled WIT records for everything that passes. Errors surface as
 * `HttpRouteError` typed failures.
 */
export const validateAgentHttp = (
  input: AgentHttpInput,
): Effect.Effect<CompiledHttp, HttpRouteError> =>
  Effect.gen(function* () {
    const anyEndpoints = input.methods.some((m) => m.endpoints.length > 0)

    // Mount must exist when any endpoint is declared.
    if (anyEndpoints && input.mount === undefined) {
      return yield* Effect.fail(
        new HttpRouteError(
          `agent '${input.agentName}' declares HTTP endpoints but no mount — add an 'http: Http.mount(...)' to the agent definition`,
        ),
      )
    }

    // Mount validation.
    let compiledMount: AgentCommon.HttpMountDetails | undefined = undefined
    if (input.mount !== undefined) {
      compiledMount = yield* validateMount(input)
    }

    // Per-method endpoints.
    const endpoints = new Map<string, ReadonlyArray<AgentCommon.HttpEndpointDetails>>()
    for (const m of input.methods) {
      const compiled: Array<AgentCommon.HttpEndpointDetails> = []
      for (let i = 0; i < m.endpoints.length; i++) {
        yield* validateEndpoint(input.agentName, m, m.endpoints[i]!, i)
        compiled.push(compileEndpoint(m.endpoints[i]!))
      }
      endpoints.set(m.name, compiled)
    }

    return { mount: compiledMount, endpoints }
  })

const validateMount = (
  input: AgentHttpInput,
): Effect.Effect<AgentCommon.HttpMountDetails, HttpRouteError> =>
  Effect.gen(function* () {
    const mountDef = input.mount!
    const ctx = `agent '${input.agentName}' mount`
    const mountVars = new Set<string>()
    for (let i = 0; i < mountDef.pathPrefix.length; i++) {
      const s = mountDef.pathPrefix[i]!
      if (s._tag === "Literal" || s._tag === "SystemVar") continue
      if (s._tag === "RestVar") {
        // Already rejected by the parser, but defend in depth in case
        // callers built a MountDef structurally.
        return yield* Effect.fail(
          new HttpRouteError(`${ctx}: mount path may not include catch-all '{*${s.name}}'`),
        )
      }
      if (mountVars.has(s.name)) {
        return yield* Effect.fail(new HttpRouteError(`${ctx}: duplicate path variable '${s.name}'`))
      }
      mountVars.add(s.name)
      if (!input.constructorParamNames.includes(s.name)) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${ctx}: path variable '${s.name}' does not match any constructor parameter (constructorParams: ${input.constructorParamNames.join(", ") || "<none>"})`,
          ),
        )
      }
      if (input.nonStringBindableConstructorParams.has(s.name)) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${ctx}: constructor parameter '${s.name}' is multimodal/unstructured and cannot be bound from a path variable`,
          ),
        )
      }
    }
    // Every constructor param must be covered by a mount path variable.
    for (const cp of input.constructorParamNames) {
      if (!mountVars.has(cp)) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${ctx}: constructor parameter '${cp}' is not covered by a mount path variable — add '{${cp}}' to the mount path`,
          ),
        )
      }
    }
    // Webhook suffix variables, if any, must also resolve to constructor params.
    for (const s of mountDef.webhookSuffix) {
      if (s._tag === "PathVar" && !input.constructorParamNames.includes(s.name)) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${ctx}: webhook-suffix path variable '${s.name}' does not match any constructor parameter`,
          ),
        )
      }
    }
    return compileMount(mountDef)
  })

// ---------------------------------------------------------------------------
// Schema-side helper used by `agent.ts` / `method.ts`
// ---------------------------------------------------------------------------

/**
 * Walk a {@link Schema.Top}'s AST to determine whether values of this
 * schema can be safely decoded from a string source (URL path, query
 * parameter, or header). Returns `true` for `Schema.String`,
 * `Schema.Number`, `Schema.BigInt`, `Schema.Boolean`, literal/enum
 * schemas, and any refinements/transformations layered on top of them.
 */
export const isStringBindableSchema = (schema: Schema.Top): boolean =>
  isStringBindableAst(schema.ast)

const isStringBindableAst = (ast: SchemaAST.AST): boolean => {
  switch (ast._tag) {
    case "String":
    case "Number":
    case "BigInt":
    case "Boolean":
      return true
    case "Literal":
      // string/number/boolean/bigint literals are all valid in URL
      // contexts; null literal also serialisable.
      return true
    case "TemplateLiteral":
      return true
    case "UniqueSymbol":
      return false
    case "Union": {
      const u = ast as unknown as { types?: ReadonlyArray<SchemaAST.AST> }
      if (u.types) return u.types.every(isStringBindableAst)
      return false
    }
    default:
      break
  }
  // Refinements / suspends / transformations: peel one layer.
  const a = ast as unknown as { from?: SchemaAST.AST; to?: SchemaAST.AST }
  if (a.from && typeof (a.from as SchemaAST.AST)._tag === "string") {
    return isStringBindableAst(a.from as SchemaAST.AST)
  }
  if (a.to && typeof (a.to as SchemaAST.AST)._tag === "string") {
    return isStringBindableAst(a.to as SchemaAST.AST)
  }
  return false
}
