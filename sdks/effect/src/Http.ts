import { Effect, Pipeable, Schema, SchemaAST } from "effect"
import type * as AgentCommon from "golem:agent/common@1.5.0"
import { withPipe } from "./internal/pipeable.js"
import type {
  EndpointBound,
  EndpointBoundAny,
  HeaderKeysTuple,
  ValidEndpointPath,
  ValidMountPath,
} from "./internal/httpTypes.js"

export type {
  EndpointBound,
  EndpointBoundAny,
  HeaderKeysTuple,
  NoCaseFoldDuplicates,
  NoDuplicateBindings,
  UnionToTuple,
  ValidEndpointPath,
  ValidMountPath,
} from "./internal/httpTypes.js"

// `Invalid<…>` (the branded "compile-time error" carrier used by every
// type-level helper above) is intentionally NOT re-exported. It is an
// internal mechanism — user code never names the type directly; it
// only ever encounters `Invalid<"…">` as a hover/error message
// produced by the compile-time validators (mount-coverage, duplicate
// bindings, case-fold header uniqueness, bodyless-unbound, etc.). The
// literal `Reason` string carries the diagnostic, so exposing the
// carrier publicly would only widen the SDK's surface area without
// improving any user-visible workflow.

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
 * **Example**
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
 *
 * @since 1.5.0
 */

// ---------------------------------------------------------------------------
// Path-segment IR (also exposed as escape hatches)
// ---------------------------------------------------------------------------

/**
 * Internal representation of a single URL path segment, mirroring the
 * Golem WIT `path-segment` variant.
 *
 * @since 1.5.0
 * @category models
 */
export type PathSegment =
  | { readonly _tag: "Literal"; readonly value: string }
  | { readonly _tag: "PathVar"; readonly name: string }
  | { readonly _tag: "RestVar"; readonly name: string }
  | { readonly _tag: "SystemVar"; readonly name: SystemVariableName }

/**
 * System-variable names the Golem host injects into routes.
 *
 * @since 1.5.0
 * @category models
 */
export type SystemVariableName = "agent-type" | "agent-version"

/**
 * Build a literal path segment, e.g. `Http.literal("api")`.
 *
 * @since 1.5.0
 * @category constructors
 */
export const literal = (value: string): PathSegment => ({ _tag: "Literal", value })

/**
 * Build a path-variable segment, e.g. `Http.pathVar("id")`.
 *
 * @since 1.5.0
 * @category constructors
 */
export const pathVar = (name: string): PathSegment => ({ _tag: "PathVar", name })

/**
 * Build a catch-all (remaining-path) variable segment. Only valid as the last segment.
 *
 * @since 1.5.0
 * @category constructors
 */
export const restVar = (name: string): PathSegment => ({ _tag: "RestVar", name })

/**
 * `{agent-type}` — runtime-injected by the host.
 *
 * @since 1.5.0
 * @category constructors
 */
export const agentType = (): PathSegment => ({ _tag: "SystemVar", name: "agent-type" })

/**
 * `{agent-version}` — runtime-injected by the host.
 *
 * @since 1.5.0
 * @category constructors
 */
export const agentVersion = (): PathSegment => ({ _tag: "SystemVar", name: "agent-version" })

/**
 * Bind one HTTP query parameter to a method parameter.
 *
 * @since 1.5.0
 * @category models
 */
export interface QueryVariable {
  readonly queryParam: string
  readonly varName: string
}

/**
 * Bind one HTTP header to a method parameter.
 *
 * @since 1.5.0
 * @category models
 */
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
 *
 * @since 1.5.0
 * @category models
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
 *
 * @since 1.5.0
 * @category errors
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

/**
 * Extract all path-variable names from a literal path template (system vars included).
 *
 * @since 1.5.0
 * @category models
 */
export type PathVarsOf<S extends string> = ExtractVarsFromPath<SplitPathQuery<S>["path"]>

/**
 * Extract all query-variable values from a literal path template's `?…` portion.
 *
 * @since 1.5.0
 * @category models
 */
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
 *
 * @since 1.5.0
 * @category models
 */
export type ValuesOf<R> = [keyof R] extends [never]
  ? never
  : R extends Readonly<Record<string, infer V>>
    ? V & string
    : never

// ---------------------------------------------------------------------------
// Tuple-emitting variants of PathVarsOf / QueryVarsOf
// ---------------------------------------------------------------------------
//
// `PathVarsOf` and `QueryVarsOf` collapse to a *union* of variable
// names, which is what the `EndpointVars` phantom on `EndpointDef`
// consumes. The duplicate-binding check needs *tuples* (the union
// form drops `"a" | "a"` to `"a"` before we can spot the duplicate),
// so we extract them via the tuple-emitting helpers below and feed
// them into the structured `Bound` phantom on `EndpointDef`.

type ExtractPathTupleRec<
  S extends string,
  Acc extends ReadonlyArray<string> = readonly [],
> = S extends `${string}{${infer V}}${infer Rest}`
  ? ExtractPathTupleRec<Rest, readonly [...Acc, CleanVarName<V>]>
  : Acc

type ExtractQueryTupleRec<
  S extends string,
  Acc extends ReadonlyArray<string> = readonly [],
> = S extends `${string}={${infer V}}${infer Rest}`
  ? ExtractQueryTupleRec<Rest, readonly [...Acc, V]>
  : Acc

type FilterSystemVarsRec<
  T extends ReadonlyArray<string>,
  Acc extends ReadonlyArray<string> = readonly [],
> = T extends readonly [infer Head extends string, ...infer Tail extends ReadonlyArray<string>]
  ? Head extends SystemVariableName
    ? FilterSystemVarsRec<Tail, Acc>
    : FilterSystemVarsRec<Tail, readonly [...Acc, Head]>
  : Acc

/**
 * Tuple of path-variable names extracted from a literal endpoint path
 * template, with `{agent-type}` / `{agent-version}` system variables
 * stripped. Catch-all (`{*rest}`) names are included with the leading
 * `*` already removed.
 *
 * Used as the `path` slot of the structured `Bound` phantom on
 * {@link EndpointDef} (consumed by `NoDuplicateBindings`). For the
 * union-shaped variant — which is what `EndpointVars` consumes — see
 * {@link PathVarsOf}.
 *
 * @since 1.5.0
 * @category models
 */
export type PathTupleOf<S extends string> = FilterSystemVarsRec<
  ExtractPathTupleRec<SplitPathQuery<S>["path"]>
>

/**
 * Tuple of query-variable names extracted from the inline query
 * portion of a literal endpoint path template.
 *
 * Used as the `query` slot of the structured `Bound` phantom on
 * {@link EndpointDef} (consumed by `NoDuplicateBindings`). For the
 * union-shaped variant — which is what `EndpointVars` consumes — see
 * {@link QueryVarsOf}.
 *
 * @since 1.5.0
 * @category models
 */
export type QueryTupleOf<S extends string> = ExtractQueryTupleRec<SplitPathQuery<S>["query"]>

// ---------------------------------------------------------------------------
// Mount and Endpoint typed wrappers
// ---------------------------------------------------------------------------

declare const mountVarsBrand: unique symbol
declare const mountWebhookVarsBrand: unique symbol
declare const endpointVarsBrand: unique symbol
declare const endpointKindBrand: unique symbol
declare const endpointBoundBrand: unique symbol
declare const endpointHeaderNamesBrand: unique symbol

/**
 * Whether an endpoint's HTTP verb permits a request body. Used as a
 * phantom on {@link EndpointDef} so the `method({...})` factory can
 * statically reject bodyless endpoints (`GET` / `HEAD`) whose path /
 * query / header bindings do not cover every method parameter — the
 * Golem host has no body in which to deliver an unbound value.
 *
 * Custom verbs (`Http.endpoint(verb, ...)` / `Http.custom(verb, ...)`)
 * always tag as `"bodyful"` regardless of the verb string, matching
 * the runtime `isBodylessVerb` convention which only treats the
 * literal `"GET"` / `"HEAD"` shorthands as bodyless.
 *
 * @since 1.5.0
 * @category models
 */
export type EndpointKind = "bodyful" | "bodyless"

/**
 * Mount declaration carried by `AgentMetadata.http`. Compiled to
 * `agent-type.http-mount` (`HttpMountDetails`) at registration time.
 *
 * The phantom `MountVars` parameter is **not** present at runtime —
 * `agent.ts` enforces `MountVars extends keyof ConstructorParams` to
 * give a compile-time signal when a `{var}` in the mount path doesn't
 * match a constructor param.
 *
 * The second phantom `WebhookVars` carries the union of `{var}` names
 * appearing in the optional `webhookSuffix`. It is intentionally kept
 * SEPARATE from `MountVars`: webhook-suffix vars are validated against
 * constructor parameters rather than being part of the routable mount
 * URL, and folding them into `MountVars` would conflate "this var is
 * resolved during HTTP routing" with "this var is rendered into the
 * webhook URL at deploy time". `agent.ts` consumes `WebhookVars` via
 * `WebhookVarsValid<C, WebhookVars>` to enforce the constructor-param
 * + bindability constraints at compile time.
 *
 * Instances are {@link Pipeable.Pipeable}: the pipeable-builder
 * combinators ({@link withAuth}, {@link withCors},
 * {@link withPhantomAgent}, {@link withWebhookSuffix}) compose
 * additively with the literal-options form accepted by {@link mount}.
 *
 * @since 1.5.0
 * @category models
 */
export interface MountDef<MountVars extends string, WebhookVars extends string = never>
  extends Pipeable.Pipeable {
  readonly [mountVarsBrand]?: MountVars
  readonly [mountWebhookVarsBrand]?: WebhookVars
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
 * query string, and header bindings of this single endpoint as a
 * *union* — used by {@link "./internal/method.js".MethodSpec}'s
 * `http?` array constraint to ensure every binding maps to a method
 * parameter. `Kind` tracks whether the endpoint's HTTP verb permits a
 * request body (see {@link EndpointKind}); narrowed to `"bodyless"`
 * by the `Http.get` / `Http.head` shorthands and to `"bodyful"` by
 * every other verb constructor. Defaults to the full `EndpointKind`
 * union so existing structural code (`EndpointDef<string>`,
 * `compileEndpoint`, …) keeps working unchanged.
 *
 * `Bound` carries the *structured* shape of the same bindings —
 * `{ path; query; header }` of three readonly tuples of literal
 * strings — so that {@link NoDuplicateBindings} can detect a
 * parameter being bound from more than one source within the same
 * endpoint. Defaults to {@link EndpointBoundAny} (each
 * slot a non-tuple `ReadonlyArray<string>`), which short-circuits the
 * dup walk and defers to the runtime `seenSources` check — preserving
 * back-compat for code that only declares the union form.
 *
 * `HeaderNames` carries the readonly tuple of *header name* literals
 * (e.g. `"X-Idempotency-Key"`) declared via
 * {@link EndpointOptions.headers}, {@link withHeader}, and
 * {@link withHeaders}. Note this is the **header-name** tuple, which
 * is distinct from `Bound["header"]` (which holds the **method-param
 * name** the header maps to). Consumed by `NoCaseFoldDuplicates` to
 * reject endpoints declaring the same header twice when names are
 * compared case-insensitively. Defaults to a non-tuple
 * `ReadonlyArray<string>` so back-compat callers short-circuit and
 * defer to the runtime `seenHeaderKeys` check.
 *
 * Instances are {@link Pipeable.Pipeable}: the pipeable-builder
 * combinators ({@link withAuth}, {@link withCors}, {@link withHeader},
 * {@link withHeaders}) compose additively with the literal-options
 * form accepted by {@link endpoint} and the verb shorthands.
 *
 * @since 1.5.0
 * @category models
 */
export interface EndpointDef<
  EndpointVars extends string,
  Kind extends EndpointKind = EndpointKind,
  Bound extends EndpointBound = EndpointBoundAny,
  HeaderNames = ReadonlyArray<string>,
>
  extends Pipeable.Pipeable {
  readonly [endpointVarsBrand]?: EndpointVars
  readonly [endpointKindBrand]?: Kind
  readonly [endpointBoundBrand]?: Bound
  readonly [endpointHeaderNamesBrand]?: HeaderNames
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
 *
 * @since 1.5.0
 * @category utils
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
 *
 * @since 1.5.0
 * @category utils
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

/**
 * Options accepted by {@link mount}.
 *
 * @since 1.5.0
 * @category models
 */
export interface MountOptions<W extends string = string> {
  /** When `true`, the host treats every endpoint as authentication-required. */
  readonly auth?: boolean
  /** CORS allowed-origin patterns advertised at the mount level. */
  readonly cors?: ReadonlyArray<string>
  /** Mark this agent as a phantom agent (one fresh instance per HTTP request). */
  readonly phantomAgent?: boolean
  /**
   * Optional custom webhook suffix path. Parsed with the same rules as
   * the mount path (no query, no catch-all). Validated at the type
   * level via {@link ValidMountPath}: when a non-literal `string` is
   * supplied (default `W = string`) the constraint reduces to plain
   * `string` and is enforced by the runtime parser; literal templates
   * are validated at compile time.
   */
  readonly webhookSuffix?: string extends W ? string : ValidMountPath<W>
}

/**
 * Declare an HTTP mount for an agent. The path may include `{var}` and
 * `{agent-type}` / `{agent-version}` segments; every `{var}` must
 * correspond to a constructor parameter on the agent.
 *
 * The optional `opts.webhookSuffix` is parsed with the same rules as
 * the mount path (no query, no catch-all). Its `{var}` names are
 * extracted into the `WebhookVars` phantom on the returned
 * {@link MountDef} so `agent.ts` can validate them against the agent's
 * constructor parameters at compile time via `WebhookVarsValid`.
 *
 * **Compile-time guarantees**
 *
 * When the path argument is a string literal (the typical call shape),
 * the following rules are enforced by `tsc` before the call ever runs:
 *
 * - The path must start with `/`, must not end with `/` (except `"/"`
 *   itself), must not contain `//`, and must not include a `?` (mounts
 *   have no query string).
 * - Each segment must be either a pure literal OR a single `{var}` /
 *   `{*rest}` (no mixing of literal text with `{…}` variables) and
 *   variable braces must be balanced and non-empty (also rejecting
 *   empty `{}` and nested-brace shapes).
 * - Mount paths may NOT contain a catch-all `{*rest}` segment.
 * - The optional `webhookSuffix` is parsed with the same rules.
 *
 * Coverage of constructor parameters by `{var}` segments and
 * webhook-suffix `{var}` validity are enforced separately, at the
 * `defineAgent` call site, via the `MountDefCovering<C, V>` and
 * `WebhookVarsValid<C, W>` constraints applied to the agent's `http`
 * field.
 *
 * **Runtime fallbacks (defence-in-depth)**
 *
 * The brace-balance check, the var-name regex, AND full
 * string-bindability of the bound constructor parameter (i.e.
 * rejecting `Schema.Struct` / `Schema.Class` schemas on a path var)
 * remain runtime-only because they need either parser-level loops or
 * `Schema.AST` introspection that cannot be expressed at the type
 * level without unreasonable hover output. They surface as
 * `HttpRouteError` from `registerAgent` (re-thrown by `defineAgent`
 * at module-import time).
 *
 * Non-literal path arguments (e.g. `Http.mount(somePathVariable)`)
 * widen `Path` to plain `string`; the compile-time gates short-circuit
 * and the runtime parser becomes the only line of defence.
 *
 * @since 1.5.0
 * @category constructors
 */
export const mount: <const Path extends string, const W extends string = string>(
  path: ValidMountPath<Path>,
  opts?: MountOptions<W>,
) => MountDef<
  Exclude<PathVarsOf<Path>, SystemVariableName>,
  Exclude<PathVarsOf<W>, SystemVariableName>
> = ((path: string, opts?: MountOptions) => {
  const segments = runParse(parseMountPath(path))
  const webhookSuffix = opts?.webhookSuffix ? runParse(parseMountPath(opts.webhookSuffix)) : []
  return withPipe({
    pathPrefix: segments,
    authRequired: opts?.auth ?? false,
    cors: opts?.cors ?? [],
    phantomAgent: opts?.phantomAgent ?? false,
    webhookSuffix,
  }) as unknown as MountDef<never, never>
}) as never

/**
 * Default value for the `H` (headers) generic on the endpoint helpers.
 * An "empty record" sentinel that signals "no header bindings"; using
 * `{}` directly would trip `@typescript-eslint/no-empty-object-type`
 * and would also widen `ValuesOf<H>` to `string`.
 *
 * @since 1.5.0
 * @category models
 */
export type NoHeaderBindings = Readonly<Record<string, never>>

/**
 * Options accepted by {@link endpoint} and the verb shorthands.
 *
 * @since 1.5.0
 * @category models
 */
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
 * Header values extracted from `H`, used as the seed for the
 * structured `Bound`'s `header` slot on the verb shorthands and on
 * {@link endpoint} / {@link custom}.
 *
 * The empty case (`ValuesOf<H>` collapses to `never` — either because
 * the user did not pass `headers` at all, OR because the `headers`
 * record is the empty `{}` literal) resolves to a *tuple* `readonly []`
 * so that {@link withHeader}'s subsequent `readonly [...B["header"],
 * Var]` produces a real tuple `readonly [Var]` (and therefore drives
 * `NoDuplicateBindings` correctly). Anything else falls through to a
 * `ReadonlyArray<…>` (NOT a tuple), short-circuiting the dup walk for
 * the headers-via-literal-options form — the runtime `seenHeaderKeys`
 * / `seenSources` checks remain the canonical defence in that case.
 *
 * @since 1.5.0
 * @category models
 */
export type HeaderValuesArray<H extends Readonly<Record<string, string>>> = [ValuesOf<H>] extends [
  never,
]
  ? readonly []
  : ReadonlyArray<ValuesOf<H>>

/**
 * Shape of a verb-shorthand factory, parameterised by {@link EndpointKind}.
 *
 * `Http.get` / `Http.head` instantiate it with `"bodyless"` so the
 * compile-time check rejecting `GET` / `HEAD` endpoints with unbound
 * method parameters fires; every other verb shorthand instantiates it
 * with `"bodyful"` so unbound parameters are allowed to map to the
 * request body.
 *
 * @since 1.5.0
 * @category models
 */
export type EndpointFactory<Kind extends EndpointKind> = <
  const Path extends string,
  H extends Readonly<Record<string, string>> = NoHeaderBindings,
>(
  path: ValidEndpointPath<Path>,
  opts?: EndpointOptions<H>,
) => EndpointDef<
  Exclude<PathVarsOf<Path> | QueryVarsOf<Path>, SystemVariableName> | ValuesOf<H>,
  Kind,
  {
    readonly path: PathTupleOf<Path>
    readonly query: QueryTupleOf<Path>
    readonly header: HeaderValuesArray<H>
  },
  HeaderKeysTuple<H>
>

/**
 * Declare an HTTP endpoint for a method. The path is relative to the
 * agent's mount prefix and may include `{var}`, `{*rest}`,
 * `{agent-type}`, `{agent-version}`, and inline `?key={var}&…` query
 * bindings.
 *
 * The returned endpoint is tagged `"bodyful"` regardless of the verb
 * string — `Http.endpoint("GET", ...)` does NOT participate in the
 * compile-time bodyless-binding check; use the `Http.get(...)`
 * shorthand if you want that. This matches the runtime `isBodylessVerb`
 * convention in `validateEndpoint`, which only treats the literal
 * `"GET"` / `"HEAD"` shorthands as bodyless.
 *
 * **Compile-time guarantees**
 *
 * When the path argument is a string literal (the typical call shape),
 * the following rules are enforced by `tsc` before the call ever runs:
 *
 * - The path must start with `/`, must not end with `/` (except `"/"`
 *   itself), must not contain `//`, and must contain at most one `?`.
 * - Each segment must be either a pure literal OR a single `{var}` /
 *   `{*rest}` (no mixing of literal text with `{…}` variables) and
 *   variable braces must be balanced and non-empty (also rejecting
 *   empty `{}` and nested-brace shapes).
 * - A `{*rest}` catch-all segment is only allowed as the LAST path
 *   segment.
 * - The query portion (the part after `?`, if any) may not contain
 *   empty `&` runs (`&&`, leading `&`, trailing `&`), may not contain
 *   empty parameter names (`?=…`), and may not declare the same query
 *   key twice.
 *
 * The endpoint's bound parameter names are also tracked via three
 * structural phantoms on the returned {@link EndpointDef} —
 * {@link NoDuplicateBindings}, {@link NoCaseFoldDuplicates}, and the
 * `"bodyless"` / `"bodyful"` `Kind` tag — so that the `method({...})`
 * factory can additionally enforce, also at compile time:
 *
 * - A method parameter may be bound from at most one source
 *   (path / query / header) within the same endpoint.
 * - Header names declared on the same endpoint must be unique when
 *   compared case-insensitively.
 * - Bodyless verbs (`GET` / `HEAD`) — only via the {@link get} /
 *   {@link head} shorthands — may not have any unbound method
 *   parameter, since there is no request body in which to deliver it.
 * - Path / query / header bindings to multimodal or unstructured
 *   parameters (i.e. `Multimodal` or `ElementSpec` carriers) are
 *   rejected via the `BindableKeys<Params>` constraint on
 *   `EndpointDef`.
 *
 * **Runtime fallbacks (defence-in-depth)**
 *
 * The brace-balance check, the var-name regex, AND full
 * string-bindability of bound parameters (i.e. rejecting a
 * `Schema.Struct` schema as a path var) remain runtime-only because
 * they need parser-level loops or `Schema.AST` introspection. The
 * matching-parameter check for every binding is enforced by the type
 * system via `EndpointDef<BindableKeys<Params>>` for literal call
 * shapes and by `validateEndpoint` at registration time for the
 * widened cases.
 *
 * @since 1.5.0
 * @category constructors
 */
export const endpoint: <
  const Path extends string,
  H extends Readonly<Record<string, string>> = NoHeaderBindings,
>(
  verb: HttpVerb,
  path: ValidEndpointPath<Path>,
  opts?: EndpointOptions<H>,
) => EndpointDef<
  Exclude<PathVarsOf<Path> | QueryVarsOf<Path>, SystemVariableName> | ValuesOf<H>,
  "bodyful",
  {
    readonly path: PathTupleOf<Path>
    readonly query: QueryTupleOf<Path>
    readonly header: HeaderValuesArray<H>
  },
  HeaderKeysTuple<H>
> = buildEndpoint as never

const verbHelper = <Kind extends EndpointKind>(verb: HttpVerb): EndpointFactory<Kind> =>
  ((path: string, opts?: EndpointOptions<NoHeaderBindings>) =>
    buildEndpoint(verb, path, opts) as never) as never

/**
 * Shorthand for `Http.endpoint("GET", path, opts?)`. Bodyless: every
 * method parameter MUST be bound from a path / query / header variable
 * within the same endpoint; unbound params would otherwise have to
 * travel in the request body, which `GET` does not have. The
 * "no-unbound-param" check is enforced at compile time by tagging the
 * returned endpoint as `"bodyless"` and surfacing an `Invalid<…>`
 * carrier at the offending `method({ http: [...] })` call site;
 * see {@link endpoint} for the full set of compile-time guarantees
 * shared with every verb shorthand.
 *
 * @since 1.5.0
 * @category constructors
 */
export const get: EndpointFactory<"bodyless"> = verbHelper<"bodyless">("GET")
/**
 * Shorthand for `Http.endpoint("HEAD", path, opts?)`. Bodyless: see
 * {@link get} for the compile-time parameter-binding restriction.
 *
 * @since 1.5.0
 * @category constructors
 */
export const head: EndpointFactory<"bodyless"> = verbHelper<"bodyless">("HEAD")
/**
 * Shorthand for `Http.endpoint("POST", path, opts?)`. Bodyful — unbound
 * method parameters are allowed to map to JSON body fields keyed by
 * name. Compile-time path-shape, duplicate-binding, and case-fold
 * header-uniqueness rules apply identically to every verb shorthand;
 * see {@link endpoint} for the full list.
 *
 * @since 1.5.0
 * @category constructors
 */
export const post: EndpointFactory<"bodyful"> = verbHelper<"bodyful">("POST")
/**
 * Shorthand for `Http.endpoint("PUT", path, opts?)`. Bodyful — see
 * {@link post}.
 *
 * @since 1.5.0
 * @category constructors
 */
export const put: EndpointFactory<"bodyful"> = verbHelper<"bodyful">("PUT")
/**
 * Shorthand for `Http.endpoint("DELETE", path, opts?)`. Bodyful — see
 * {@link post}.
 *
 * @since 1.5.0
 * @category constructors
 */
export const del: EndpointFactory<"bodyful"> = verbHelper<"bodyful">("DELETE")
/**
 * Shorthand for `Http.endpoint("PATCH", path, opts?)`. Bodyful — see
 * {@link post}.
 *
 * @since 1.5.0
 * @category constructors
 */
export const patch: EndpointFactory<"bodyful"> = verbHelper<"bodyful">("PATCH")
/**
 * Shorthand for `Http.endpoint("OPTIONS", path, opts?)`. Bodyful — see
 * {@link post}.
 *
 * @since 1.5.0
 * @category constructors
 */
export const options: EndpointFactory<"bodyful"> = verbHelper<"bodyful">("OPTIONS")
/**
 * Shorthand for `Http.endpoint("TRACE", path, opts?)`. Bodyful — see
 * {@link post}.
 *
 * @since 1.5.0
 * @category constructors
 */
export const trace: EndpointFactory<"bodyful"> = verbHelper<"bodyful">("TRACE")
/**
 * Shorthand for `Http.endpoint("CONNECT", path, opts?)`. Bodyful — see
 * {@link post}.
 *
 * @since 1.5.0
 * @category constructors
 */
export const connect: EndpointFactory<"bodyful"> = verbHelper<"bodyful">("CONNECT")

/**
 * Shorthand for a custom (non-standard) HTTP verb. Always tagged
 * `"bodyful"` — custom verbs do NOT participate in the compile-time
 * bodyless-binding check, matching the runtime `isBodylessVerb`
 * convention which only treats the literal `"GET"` / `"HEAD"`
 * shorthands as bodyless. All other compile-time guarantees on the
 * path string (shape, query-key uniqueness, …) and on the resulting
 * endpoint (duplicate bindings, case-fold header uniqueness,
 * `BindableKeys` filtering of multimodal/unstructured params) apply
 * identically; see {@link endpoint} for the full list.
 *
 * @since 1.5.0
 * @category constructors
 */
export const custom: <
  const Path extends string,
  H extends Readonly<Record<string, string>> = NoHeaderBindings,
>(
  verb: string,
  path: ValidEndpointPath<Path>,
  opts?: EndpointOptions<H>,
) => EndpointDef<
  Exclude<PathVarsOf<Path> | QueryVarsOf<Path>, SystemVariableName> | ValuesOf<H>,
  "bodyful",
  {
    readonly path: PathTupleOf<Path>
    readonly query: QueryTupleOf<Path>
    readonly header: HeaderValuesArray<H>
  },
  HeaderKeysTuple<H>
> = ((verb: string, path: string, opts?: EndpointOptions<NoHeaderBindings>) =>
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
 *
 * @since 1.5.0
 * @category combinators
 */
export const withAuth =
  (auth: boolean) =>
  <T extends MountDef<string, string> | EndpointDef<string>>(t: T): T =>
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
 *
 * @since 1.5.0
 * @category combinators
 */
export const withCors =
  (...patterns: ReadonlyArray<string>) =>
  <T extends MountDef<string, string> | EndpointDef<string>>(t: T): T =>
    withPipe({ ...t, cors: patterns }) as unknown as T

/**
 * Append a single header → method-parameter binding to an endpoint.
 *
 * **Compile-time guarantees**
 *
 * Adds the bound `varName` to the structured `Bound["header"]` tuple
 * carried by the endpoint, and adds `header` to the `HeaderNames`
 * tuple. As a result the `method({ http: [...] })` factory:
 *
 * - rejects this endpoint if `varName` is already bound from a path
 *   or query variable (or from an earlier `withHeader` call) — that
 *   is the same parameter bound from more than one source.
 * - rejects this endpoint if `header` matches an earlier-declared
 *   header on the same endpoint after both are lowercased.
 *
 * Both checks fire only when the header name and `varName` are string
 * literals (the typical call shape); a non-literal `header` widens
 * `HeaderNames` to `ReadonlyArray<string>` and the case-fold walker
 * short-circuits, deferring to the runtime `seenHeaderKeys` /
 * `seenSources` checks in `validateEndpoint`.
 *
 * Header names are case-insensitive at HTTP level — collisions after
 * lower-casing are rejected at registration time even when the
 * compile-time check has been short-circuited.
 *
 * @since 1.5.0
 * @category combinators
 */
export const withHeader =
  <const HName extends string, const Var extends string>(header: HName, varName: Var) =>
  <V extends string, K extends EndpointKind, B extends EndpointBound, HN>(
    ep: EndpointDef<V, K, B, HN>,
  ): EndpointDef<
    V | Var,
    K,
    {
      readonly path: B["path"]
      readonly query: B["query"]
      readonly header: readonly [...B["header"], Var]
    },
    HN extends ReadonlyArray<string> ? readonly [...HN, HName] : ReadonlyArray<string>
  > =>
    withPipe({
      ...ep,
      headerVars: [...ep.headerVars, { header, varName }],
    }) as never

/**
 * Append multiple header → method-parameter bindings to an endpoint.
 * Equivalent to chaining a series of {@link withHeader} calls.
 *
 * **Compile-time guarantees**
 *
 * The header names from `H` are appended to the endpoint's
 * `HeaderNames` tuple via {@link UnionToTuple}, so the
 * `method({ http: [...] })` factory still rejects the endpoint when a
 * later header collides with an earlier one (case-insensitive).
 *
 * The bound `varName`s are appended to the structured `Bound["header"]`
 * slot as a `ReadonlyArray<…>` (NOT a tuple — TypeScript's mapped
 * types do not preserve record-key insertion order well enough to
 * drive duplicate detection inside the appended block). The
 * compile-time {@link NoDuplicateBindings} walk therefore short-
 * circuits inside the `withHeaders`-introduced range; the runtime
 * `seenSources` check in `validateEndpoint` remains the canonical
 * defence for cross-source collisions involving header names
 * introduced via `withHeaders`. To preserve the compile-time
 * cross-source check, prefer chained {@link withHeader} calls over
 * a single `withHeaders({...})` block.
 *
 * @since 1.5.0
 * @category combinators
 */
export const withHeaders =
  <const H extends Readonly<Record<string, string>>>(headers: H) =>
  <V extends string, K extends EndpointKind, B extends EndpointBound, HN>(
    ep: EndpointDef<V, K, B, HN>,
  ): EndpointDef<
    V | ValuesOf<H>,
    K,
    {
      readonly path: B["path"]
      readonly query: B["query"]
      readonly header: readonly [...B["header"], ...HeaderValuesArray<H>]
    },
    HN extends ReadonlyArray<string>
      ? HeaderKeysTuple<H> extends infer Hk extends ReadonlyArray<string>
        ? readonly [...HN, ...Hk]
        : ReadonlyArray<string>
      : ReadonlyArray<string>
  > => {
    const headerVars = [...ep.headerVars]
    for (const [header, varName] of Object.entries(headers)) {
      headerVars.push({ header, varName: String(varName) })
    }
    return withPipe({ ...ep, headerVars }) as never
  }

/**
 * Set the `phantom-agent` flag on a mount (one fresh agent instance per
 * HTTP request). Pipeable; both `MountVars` and `WebhookVars` are
 * preserved unchanged.
 *
 * @since 1.5.0
 * @category combinators
 */
export const withPhantomAgent =
  (phantom: boolean = true) =>
  <V extends string, W extends string>(m: MountDef<V, W>): MountDef<V, W> =>
    withPipe({ ...m, phantomAgent: phantom }) as unknown as MountDef<V, W>

/**
 * Override the webhook-suffix path on a mount.
 *
 * **Compile-time guarantees**
 *
 * The suffix string is run through the same {@link ValidMountPath}
 * checks as the mount path itself — must start with `/`, no trailing
 * `/`, no `//`, no `?`, no catch-all `{*rest}`, balanced and
 * non-empty `{var}` braces, no mixed literal+`{…}` segments.
 *
 * The webhook-suffix `{var}` names are extracted into the `WebhookVars`
 * phantom on the returned {@link MountDef}, replacing (not merging)
 * any previously-declared suffix vars — most-recent call wins,
 * matching the runtime behaviour where a later call overwrites the
 * earlier `webhookSuffix` array. At the `defineAgent` call site,
 * `WebhookVarsValid<C, WebhookVars>` then enforces that every
 * suffix `{var}` matches a constructor-parameter name AND is
 * statically eligible for binding (i.e. NOT a {@link Multimodal} or
 * {@link ElementSpec} carrier — see {@link BindableKeys}).
 *
 * Webhook-suffix vars are intentionally NOT folded into `MountVars` —
 * that union represents URL routing, while webhook vars are rendered
 * into the deploy-time webhook URL. Conflating the two would
 * over-constrain `MountDef` at every call site that does not declare
 * a webhook suffix.
 *
 * Full string-bindability of webhook-suffix vars (rejecting
 * `Schema.Struct` etc.) and webhook-suffix `{var}` uniqueness within
 * the suffix remain runtime-only — see `validateMount` in `Http.ts`.
 *
 * @since 1.5.0
 * @category combinators
 */
export const withWebhookSuffix =
  <const Suffix extends string>(suffix: ValidMountPath<Suffix>) =>
  <V extends string>(
    m: MountDef<V, string>,
  ): MountDef<V, Exclude<PathVarsOf<Suffix>, SystemVariableName>> =>
    withPipe({
      ...m,
      webhookSuffix: runParse(parseMountPath(suffix as unknown as string)),
    }) as unknown as MountDef<V, Exclude<PathVarsOf<Suffix>, SystemVariableName>>

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

/**
 * Compile a {@link MountDef} to the WIT `http-mount-details` record.
 *
 * @since 1.5.0
 * @category metadata
 */
export const compileMount = (mountDef: MountDef<string, string>): AgentCommon.HttpMountDetails => ({
  pathPrefix: mountDef.pathPrefix.map(segmentToWit),
  authDetails: mountDef.authRequired ? { required: true } : undefined,
  phantomAgent: mountDef.phantomAgent,
  corsOptions: { allowedPatterns: [...mountDef.cors] },
  webhookSuffix: mountDef.webhookSuffix.map(segmentToWit),
})

/**
 * Compile a single {@link EndpointDef} to a WIT `http-endpoint-details` record.
 *
 * @since 1.5.0
 * @category metadata
 */
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
 *
 * @since 1.5.0
 * @category models
 */
export interface CompiledHttp {
  readonly mount: AgentCommon.HttpMountDetails | undefined
  readonly endpoints: ReadonlyMap<string, ReadonlyArray<AgentCommon.HttpEndpointDetails>>
}

/**
 * Per-method input describing what the validator can see at registration time.
 *
 * @since 1.5.0
 * @category models
 */
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

/**
 * Per-agent input for `validateAgentHttp`.
 *
 * @since 1.5.0
 * @category models
 */
export interface AgentHttpInput {
  readonly agentName: string
  readonly mount: MountDef<string, string> | undefined
  readonly constructorParamNames: ReadonlyArray<string>
  /**
   * Names of constructor parameters that are NOT eligible to be bound
   * from a path variable (e.g. multimodal / unstructured-binary).
   */
  readonly nonStringBindableConstructorParams: ReadonlySet<string>
  /**
   * Names of constructor parameters whose schema is a plain
   * string-bindable Schema (string / number / bigint / boolean /
   * literal / branded variants thereof).
   */
  readonly stringBindableConstructorParams: ReadonlySet<string>
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
 *
 * @since 1.5.0
 * @category metadata
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
      if (!input.stringBindableConstructorParams.has(s.name)) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${ctx}: constructor parameter '${s.name}' has a schema that is not bindable from a path variable (only String, Number, BigInt, Boolean, Literal, or branded variants thereof are supported)`,
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
    // Webhook suffix variables, if any, must:
    //   (a) be unique within the suffix,
    //   (b) match a constructor parameter,
    //   (c) NOT refer to a multimodal / unstructured constructor param,
    //   (d) be on a string-bindable schema (string / number / bigint /
    //       boolean / literal / branded variants thereof).
    // Mirrors the mount-path checks above so the rules are consistent
    // between the routable mount path and the deploy-time webhook URL.
    const webhookVars = new Set<string>()
    for (const s of mountDef.webhookSuffix) {
      if (s._tag !== "PathVar") continue
      if (webhookVars.has(s.name)) {
        return yield* Effect.fail(
          new HttpRouteError(`${ctx}: duplicate webhook-suffix path variable '${s.name}'`),
        )
      }
      webhookVars.add(s.name)
      if (!input.constructorParamNames.includes(s.name)) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${ctx}: webhook-suffix path variable '${s.name}' does not match any constructor parameter`,
          ),
        )
      }
      if (input.nonStringBindableConstructorParams.has(s.name)) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${ctx}: webhook-suffix constructor parameter '${s.name}' is multimodal/unstructured and cannot be bound from a path variable`,
          ),
        )
      }
      if (!input.stringBindableConstructorParams.has(s.name)) {
        return yield* Effect.fail(
          new HttpRouteError(
            `${ctx}: webhook-suffix constructor parameter '${s.name}' has a schema that is not bindable from a path variable (only String, Number, BigInt, Boolean, Literal, or branded variants thereof are supported)`,
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
 *
 * @since 1.5.0
 * @category guards
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
