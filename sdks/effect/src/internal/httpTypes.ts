/**
 * Type-level helpers for compile-time HTTP route validation.
 *
 * Internal — consumers reach the public surface through `src/Http.ts`
 * (which re-exports `Invalid`, `ValidMountPath`, `ValidEndpointPath`)
 * and `src/internal/agent.ts` / `src/internal/method.ts` (which
 * consume `BindableKeys` and `MountDefCovering`).
 *
 * Each helper here is paired with — and never replaces — the runtime
 * validators in `src/Http.ts`. The compile-time pre-filter only
 * shrinks the universe of programs `tsc` accepts; the runtime
 * validators keep firing as defence-in-depth so non-literal /
 * dynamically-built paths still get checked.
 *
 * @since 1.5.0
 */
import type { EndpointDef, EndpointKind, MountDef } from "../Http.js"
// Multimodal / ElementSpec are matched by brand below
// (the discriminating `_effectGolem` literal-string field) so we no
// longer need the nominal types — keeping the comments / JSDoc links
// pointing at them is enough.

declare const InvalidBrand: unique symbol

/**
 * Branded "compile-time error" carrier. When a type-level check
 * fails, the helper resolves to a value that intersects with
 * `Invalid<"…">`. Because `[InvalidBrand]: Reason` is a non-optional
 * unique-symbol property that no real value carries, the failing
 * assignment is rejected — and the IDE / `tsc` message includes the
 * literal `Reason` string verbatim, rendering as a readable error
 * rather than a bare `never`.
 *
 * @since 1.5.0
 * @category errors
 */
export interface Invalid<Reason extends string> {
  readonly [InvalidBrand]: Reason
}

// ---------------------------------------------------------------------------
// ValidMountPath / ValidEndpointPath — trivial template-literal checks
// ---------------------------------------------------------------------------

type StartsWithSlash<S extends string> = S extends `/${string}` ? true : false
type HasTrailingSlash<S extends string> = S extends "/"
  ? false
  : S extends `${string}/`
    ? true
    : false
type HasDoubleSlash<S extends string> = S extends `${string}//${string}` ? true : false
type HasQuery<S extends string> = S extends `${string}?${string}` ? true : false

type CountQuery<
  S extends string,
  Acc extends ReadonlyArray<unknown> = [],
> = S extends `${string}?${infer Rest}` ? CountQuery<Rest, [...Acc, unknown]> : Acc["length"]

// ---------------------------------------------------------------------------
// Segment-level rules (segment shape, catch-all placement, empty `{}` / nested braces)
// ---------------------------------------------------------------------------
//
// - each segment must be either a pure literal OR a single `{var}` /
//   `{*rest}` (no mixing of literal text with `{…}` variables);
// - catch-all `{*rest}` is only allowed as the LAST path segment of an
//   endpoint, and is forbidden in mount paths entirely;
// - catch-all variable name must be non-empty (`{*}` is rejected);
// - empty `{}` variable name is rejected;
// - nested / malformed braces inside `{…}` are rejected (these last
//   two parser-level shape checks mirror `parseSegmentToken`).
//
// Brace balance for unrelated malformed cases and the var-name regex
// stay in the runtime parser — low ROI for compile-time and noisy
// error messages.

type HasOpenBrace<S extends string> = S extends `${string}{${string}` ? true : false
type HasCloseBrace<S extends string> = S extends `${string}}${string}` ? true : false

// The path portion of an endpoint template is everything before the
// first '?'. Mount paths reach segment validation only after
// `HasQuery` returns false, so the whole `S` is the path part.
type EndpointPathPart<S extends string> = S extends `${infer P}?${string}` ? P : S

// Validate the inside of a `{…}` segment.
type ValidateBracedSegment<
  Inner extends string,
  Seg extends string,
  IsMount extends boolean,
  IsLast extends boolean,
  S extends string,
> = Inner extends ""
  ? Invalid<`path '${S}' has an empty variable name in '${Seg}'`>
  : HasOpenBrace<Inner> extends true
    ? Invalid<`path '${S}' has nested or malformed braces in '${Seg}'`>
    : HasCloseBrace<Inner> extends true
      ? Invalid<`path '${S}' has nested or malformed braces in '${Seg}'`>
      : Inner extends `*${infer Name}`
        ? IsMount extends true
          ? Invalid<`mount path '${S}' may not include a catch-all segment '${Seg}'`>
          : Name extends ""
            ? Invalid<`endpoint path '${S}' has a catch-all variable with no name in '${Seg}'`>
            : IsLast extends true
              ? "ok"
              : Invalid<`endpoint path '${S}' has catch-all segment '${Seg}' that is not the last path segment`>
        : "ok"

type ValidateSegment<
  Seg extends string,
  IsMount extends boolean,
  IsLast extends boolean,
  S extends string,
> = Seg extends ""
  ? // Empty segments are caught earlier by `HasDoubleSlash` /
    // `HasTrailingSlash`; treat as `"ok"` here so the recursion
    // doesn't double-fault on the same input.
    "ok"
  : Seg extends `{${infer Inner}}`
    ? ValidateBracedSegment<Inner, Seg, IsMount, IsLast, S>
    : HasOpenBrace<Seg> extends true
      ? Invalid<`path '${S}' segment '${Seg}' mixes literal text with '{...}' variables`>
      : HasCloseBrace<Seg> extends true
        ? Invalid<`path '${S}' segment '${Seg}' mixes literal text with '{...}' variables`>
        : "ok"

type ValidateSegmentsRec<
  Rest extends string,
  IsMount extends boolean,
  S extends string,
> = Rest extends `${infer Head}/${infer Tail}`
  ? ValidateSegment<Head, IsMount, false, S> extends infer R
    ? R extends Invalid<string>
      ? R
      : ValidateSegmentsRec<Tail, IsMount, S>
    : never
  : ValidateSegment<Rest, IsMount, true, S>

// Strip the leading '/' (already verified by `StartsWithSlash`) and
// validate each segment. Returns `"ok"` on success or `Invalid<…>` on
// the first failing segment.
type ValidateSegments<
  PathPart extends string,
  IsMount extends boolean,
  S extends string,
> = PathPart extends "/"
  ? "ok"
  : PathPart extends `/${infer Rest}`
    ? ValidateSegmentsRec<Rest, IsMount, S>
    : "ok"

type ValidateMountShape<S extends string> =
  StartsWithSlash<S> extends false
    ? Invalid<`mount path '${S}' must start with '/'`>
    : HasTrailingSlash<S> extends true
      ? Invalid<`mount path '${S}' must not end with '/' (except '/')`>
      : HasDoubleSlash<S> extends true
        ? Invalid<`mount path '${S}' must not contain '//'`>
        : HasQuery<S> extends true
          ? Invalid<`mount path '${S}' may not include a query string`>
          : ValidateSegments<S, true, S> extends infer R
            ? R extends Invalid<string>
              ? R
              : S
            : never

type ValidateEndpointShape<S extends string> =
  StartsWithSlash<S> extends false
    ? Invalid<`endpoint path '${S}' must start with '/'`>
    : HasTrailingSlash<S> extends true
      ? Invalid<`endpoint path '${S}' must not end with '/' (except '/')`>
      : HasDoubleSlash<S> extends true
        ? Invalid<`endpoint path '${S}' must not contain '//'`>
        : CountQuery<S> extends 0 | 1
          ? ValidateSegments<EndpointPathPart<S>, false, S> extends infer R
            ? R extends Invalid<string>
              ? R
              : ValidateEndpointQuery<S>
            : never
          : Invalid<`endpoint path '${S}' contains more than one '?'`>

// Validate the query portion if present (rejecting empty parameter
// names and empty `&` segments).
// `${infer _P}?${infer Q}` is non-greedy: Q is everything after the
// first '?'. We only run this branch after CountQuery has confirmed
// at most one '?' is present, so the captured Q is the entire query.
//
// After the shape rules pass, we also walk the pairs once more to
// enforce duplicate-query-key detection — see
// {@link UniqueQueryKeys}. Keeping the uniqueness sweep separate from
// the shape sweep keeps the per-step error messages crisp ("empty
// '&' segment" never collides with "duplicate query key 'a'") and
// ensures duplicate-key reporting only fires once the basic
// `pair&pair&pair` shape is known to be sound.
type ValidateEndpointQuery<S extends string> = S extends `${string}?${infer Q}`
  ? ValidateQueryString<Q, S> extends infer R
    ? R extends Invalid<string>
      ? R
      : UniqueQueryKeys<Q, S>
    : never
  : S

type ValidateQueryString<Q extends string, S extends string> = Q extends ""
  ? S
  : Q extends `&${string}`
    ? Invalid<`endpoint path '${S}' has an empty '&' segment in its query string`>
    : Q extends `${string}&`
      ? Invalid<`endpoint path '${S}' has an empty '&' segment in its query string`>
      : Q extends `${string}&&${string}`
        ? Invalid<`endpoint path '${S}' has an empty '&' segment in its query string`>
        : ValidateQueryPairs<Q, S>

type ValidateQueryPairs<
  Q extends string,
  S extends string,
> = Q extends `${infer Pair}&${infer Rest}`
  ? CheckPair<Pair, S> extends infer R
    ? R extends Invalid<string>
      ? R
      : ValidateQueryPairs<Rest, S>
    : never
  : CheckPair<Q, S>

// A query pair beginning with '=' has an empty key. We deliberately
// don't validate the value side here — runtime owns the '{var}'
// shape and var-name regex.
type CheckPair<P extends string, S extends string> = P extends `=${string}`
  ? Invalid<`endpoint path '${S}' has an empty query parameter name`>
  : S

// ---------------------------------------------------------------------------
// Duplicate query parameter keys
// ---------------------------------------------------------------------------
//
// Walks the query portion (the part after the first '?') one pair at
// a time, accumulating each pair's key into a tuple. On collision,
// resolves to `Invalid<"endpoint path '…' has duplicate query key
// '…'">`. Otherwise resolves to the original `S`.
//
// Tuple-shaped accumulator (NOT a string union): TS's intrinsic
// string-literal manipulation collapses unions of literal strings, so
// `Seen` must be a `ReadonlyArray<string>` to preserve duplicates and
// allow `Includes<Seen, K>` to fire. The same shape is reused for
// case-fold header-name uniqueness below.
//
// Mirrors (defence-in-depth) the runtime `seenKeys` Set in
// `parseQueryString` (Http.ts L424 / L457-460).

type ExtractQueryKey<Pair extends string> = Pair extends `${infer K}=${string}` ? K : Pair

type IncludesString<T extends ReadonlyArray<string>, K extends string> = T extends readonly [
  infer Head,
  ...infer Tail,
]
  ? [Head, K] extends [K, Head]
    ? true
    : Tail extends ReadonlyArray<string>
      ? IncludesString<Tail, K>
      : false
  : false

type UniqueQueryPairsRec<
  Q extends string,
  Seen extends ReadonlyArray<string>,
  S extends string,
> = Q extends `${infer Pair}&${infer Rest}`
  ? ExtractQueryKey<Pair> extends infer K extends string
    ? IncludesString<Seen, K> extends true
      ? Invalid<`endpoint path '${S}' has duplicate query key '${K}'`>
      : UniqueQueryPairsRec<Rest, [...Seen, K], S>
    : never
  : ExtractQueryKey<Q> extends infer K extends string
    ? IncludesString<Seen, K> extends true
      ? Invalid<`endpoint path '${S}' has duplicate query key '${K}'`>
      : S
    : never

/**
 * Compile-time gate for query-string key uniqueness. Walks `Q` (the
 * portion of an endpoint path *after* the first `?`) one pair at a
 * time, accumulating each pair's key in a readonly tuple. Collisions
 * resolve to `Invalid<"endpoint path '…' has duplicate query key
 * '…'">`; otherwise the helper resolves to `S` unchanged.
 *
 * Composed into {@link ValidEndpointPath} after the shape rules pass,
 * so callers of `Http.endpoint` / `Http.get` / etc. get the check
 * transparently. Mount paths are not validated here — they reject `?`
 * upstream.
 *
 * @since 1.5.0
 * @category models
 */
export type UniqueQueryKeys<Q extends string, S extends string> = UniqueQueryPairsRec<Q, [], S>

/**
 * Compile-time gate for mount paths: returns the literal `S` itself
 * when the trivial template-literal rules hold (must start with `/`,
 * no trailing `/`, no `//`, no `?`), else an `Invalid<"…">` carrier
 * that does not match any string value. When `S` widens to plain
 * `string` (i.e. the user passes a non-literal expression), the
 * helper short-circuits to `S` and the runtime parser becomes the
 * only line of defence.
 *
 * @since 1.5.0
 * @category models
 */
export type ValidMountPath<S extends string> = string extends S ? S : ValidateMountShape<S>

/**
 * Compile-time gate for endpoint paths. Same trivial rules as
 * {@link ValidMountPath} except `?` is permitted at most once.
 * Segment-level checks (`{*rest}` only as last, etc.) and full query
 * key uniqueness are enforced at runtime by `parseEndpointPath`.
 *
 * @since 1.5.0
 * @category models
 */
export type ValidEndpointPath<S extends string> = string extends S ? S : ValidateEndpointShape<S>

// ---------------------------------------------------------------------------
// BindableKeys — keys whose value is statically eligible for path/query/header binding
// ---------------------------------------------------------------------------

// `any`-safe guard. Without this, `[any] extends [Multimodal<…>]` and
// `[any] extends [ElementSpec<…>]` both evaluate to `true`, which
// would collapse `BindableKeys<any>` to `never` and silently break
// the structural compatibility check between `Method<...>` (which
// extends `MethodSpec<Params, ...>`) and `MethodSpec<any, any, any>`
// — the constraint `T extends MethodSpec<any, any, any>` used by
// `withDescription` / `withPromptHint` would then reject every
// `Method`.
type IsAny<V> = 0 extends 1 & V ? true : false

// Match on the discriminating `_effectGolem` brand rather than a
// nominal `V extends Multimodal<…>` / `V extends ElementSpec<…>`
// check. The latter goes through structural assignability of every
// property (including the `compile()` method's full return type),
// which can fail for highly-parameterised instantiations — leaving
// real Multimodal values not recognised as Multimodal. The brand
// check is O(1), discriminator-keyed, and matches the runtime
// `isMultimodal` / `isElementSpec` guards exactly.
type IsBindable<V> =
  IsAny<V> extends true
    ? true
    : V extends { readonly _effectGolem: "Multimodal" }
      ? false
      : V extends { readonly _effectGolem: "ElementSpec" }
        ? false
        : true

/**
 * Subset of `keyof C & string` whose value is statically eligible
 * for binding from a string source (path / query / header) — i.e.
 * NOT a {@link Multimodal} carrier and NOT an {@link ElementSpec}
 * carrier. Full string-bindability (rejecting `Schema.Struct` etc.)
 * still depends on a runtime AST inspection by `isStringBindableSchema`.
 *
 * @since 1.5.0
 * @category models
 */
export type BindableKeys<C> = {
  [K in keyof C & string]: IsBindable<C[K]> extends true ? K : never
}[keyof C & string]

// ---------------------------------------------------------------------------
// MountDefCovering — every constructor param must appear as a {var} in the mount path
// ---------------------------------------------------------------------------

/**
 * Resolves to `MountDef<V>` when every `keyof C & string` is present
 * in `V` (i.e. the mount path covers every constructor parameter),
 * else to `MountDef<V> & Invalid<"…">`.
 *
 * The intersection trick is what makes this useful at the
 * `defineAgent` call site: TypeScript infers `V` from the mount
 * value's `MountDef<V>` brand directly (no conditional inference
 * needed), then evaluates the `Exclude<…>` separately. When something
 * is missing, the resulting type carries an `[InvalidBrand]: "mount
 * path missing var '…'"` requirement that no real `MountDef` value
 * can satisfy — so the assignment fails with a readable message
 * instead of a silent over-acceptance.
 *
 * Mirrors (defence-in-depth) the runtime "every constructor param
 * covered" loop in `validateMount` (Http.ts L1238-1246).
 *
 * @since 1.5.0
 * @category models
 */
export type MountDefCovering<C, V extends string, W extends string = never> =
  Exclude<keyof C & string, V> extends never
    ? MountDef<V, W>
    : MountDef<V, W> & Invalid<`mount path missing var '${Exclude<keyof C & string, V> & string}'`>

// ---------------------------------------------------------------------------
// WebhookVarsValid — every webhook-suffix {var} must match a bindable constructor param
// ---------------------------------------------------------------------------

declare const WebhookVarsValidBrand: unique symbol

/**
 * Compile-time gate for webhook-suffix path variables. Returns `unknown`
 * (a no-op intersection at the assignment site) when every `{var}` in
 * the webhook suffix:
 *
 * - is a constructor-parameter name (`keyof C & string`); AND
 * - is statically eligible for binding from a string source (i.e. NOT
 *   a {@link Multimodal} or {@link ElementSpec} carrier — see
 *   {@link BindableKeys}).
 *
 * On violation, resolves to a `{ [WebhookVarsValidBrand]: "…" }` carrier
 * that no real value can satisfy, surfacing a readable error in
 * `tsc` / IDE hover output.
 *
 * Mirrors (defence-in-depth) the runtime webhook-suffix validation
 * loop inside `validateMount` in `src/Http.ts`.
 *
 * @since 1.5.0
 * @category models
 */
export type WebhookVarsValid<C, WebhookVars extends string> = [WebhookVars] extends [never]
  ? unknown
  : Exclude<WebhookVars, BindableKeys<C>> extends never
    ? unknown
    : Exclude<WebhookVars, keyof C & string> extends never
      ? {
          readonly [WebhookVarsValidBrand]: `webhook-suffix var '${Exclude<WebhookVars, BindableKeys<C>> & string}' refers to a multimodal/unstructured constructor param and cannot be bound from a path variable`
        }
      : {
          readonly [WebhookVarsValidBrand]: `webhook-suffix var '${Exclude<WebhookVars, keyof C & string> & string}' does not match any constructor parameter`
        }

// ---------------------------------------------------------------------------
// A method param can be bound at most once across path/query/header
// ---------------------------------------------------------------------------
//
// Carried as a *third* phantom `Bound` on `EndpointDef`: a structured
// record `{ path; query; header }` of three readonly tuples of literal
// strings, each tuple holding the parameter names bound from that
// source. Tuple shape (NOT a string union) is required because TS's
// intrinsic union machinery collapses `"a" | "a"` into `"a"` — we
// would lose the duplicate before being able to detect it.
//
// `EndpointBoundAny` is the default for callers that do not — or
// cannot — track per-source bindings (e.g. `EndpointDef<string>` used
// by `compileEndpoint` / `validateEndpoint`, or after `withHeaders`
// pushes an unbounded array into the header slot). Its three fields
// are bare `ReadonlyArray<string>` — not a tuple — so the recursive
// `FindFirstDuplicate` walk bottoms out immediately and resolves to
// `unknown`, deferring to the runtime `seenSources` check in
// `validateEndpoint`.
//
// Mirrors (defence-in-depth) the runtime "bound from both 'X' and 'Y'"
// loop in `validateEndpoint` (Http.ts L1065 + L1104-1110, L1148-1155,
// L1190-1198). The runtime check stays the canonical error source —
// the type-level pre-filter only refuses to compile literal endpoint
// templates whose duplicates can be spotted statically.

/**
 * Structured shape of an endpoint's bound parameter names, broken
 * down by binding source (path / query / header). Carried as a phantom
 * on {@link "../Http.js".EndpointDef} so that `NoDuplicateBindings`
 * can detect a parameter being bound from more than one source within
 * the same endpoint.
 *
 * @since 1.5.0
 * @category models
 */
export interface EndpointBound {
  readonly path: ReadonlyArray<string>
  readonly query: ReadonlyArray<string>
  readonly header: ReadonlyArray<string>
}

/**
 * Default ("unknown") `Bound` value used when no per-source tracking
 * is available — for instance the wide `EndpointDef<string>` used by
 * the runtime validators, the result of {@link "../Http.js".withHeaders}
 * (which appends an unbounded array of header-bound names), or any
 * code that accepts `EndpointDef` without inferring its tuple shape.
 *
 * Each slot is a plain `ReadonlyArray<string>` rather than a tuple
 * literal, so {@link NoDuplicateBindings} resolves to `unknown` (no
 * compile-time error) and the runtime `seenSources` check remains the
 * canonical defence.
 *
 * @since 1.5.0
 * @category models
 */
export interface EndpointBoundAny extends EndpointBound {
  readonly path: ReadonlyArray<string>
  readonly query: ReadonlyArray<string>
  readonly header: ReadonlyArray<string>
}

// Concatenate the three binding-source tuples into a single tuple.
// When any slot is a non-tuple `ReadonlyArray<string>` the spread
// degrades the result into a non-tuple as well, which short-circuits
// `FindFirstDuplicate` (the recursive head/tail destructuring only
// fires on actual tuple types).
type AllBoundNames<B extends EndpointBound> = readonly [...B["path"], ...B["query"], ...B["header"]]

type FindFirstDuplicate<
  T extends ReadonlyArray<string>,
  Seen extends ReadonlyArray<string> = readonly [],
> = T extends readonly [infer Head extends string, ...infer Tail extends ReadonlyArray<string>]
  ? IncludesString<Seen, Head> extends true
    ? Head
    : FindFirstDuplicate<Tail, readonly [...Seen, Head]>
  : never

/**
 * Compile-time gate ensuring a single endpoint binds a given method
 * parameter from at most one of path / query / header.
 *
 * Resolves to `unknown` (no-op intersection) when every name across
 * `B["path"]`, `B["query"]`, and `B["header"]` is unique, OR when any
 * slot is a non-tuple array (deferring to the runtime check in
 * `validateEndpoint`). Resolves to {@link Invalid} when a name appears
 * in more than one slot — the message names the first colliding
 * parameter so the IDE / `tsc` output is readable.
 *
 * Wired in at the `method({...})` factory call site (see
 * `src/internal/method.ts`) by mapping over the user's literal `Eps`
 * tuple and intersecting each element with this carrier — a fail
 * surfaces an `Invalid<…>` value at that array position which the
 * user's `EndpointDef` literal cannot satisfy.
 *
 * @since 1.5.0
 * @category models
 */
export type NoDuplicateBindings<B extends EndpointBound> =
  FindFirstDuplicate<AllBoundNames<B>> extends infer D
    ? [D] extends [never]
      ? unknown
      : D extends string
        ? Invalid<`endpoint binds parameter '${D}' more than once across path/query/header`>
        : unknown
    : unknown

// ---------------------------------------------------------------------------
// Case-insensitive uniqueness of header names within one endpoint
// ---------------------------------------------------------------------------
//
// The HTTP spec treats header names case-insensitively, so declaring
// both `"X-A"` and `"x-a"` on the same endpoint is a configuration
// error. Carried as a *fourth* phantom `HeaderNames` on `EndpointDef`:
// a readonly tuple of literal strings holding every header name that
// has been declared on the endpoint (via `EndpointOptions.headers`,
// `withHeader`, or `withHeaders`).
//
// Tuple shape (NOT a string union) is required because TS's intrinsic
// `Lowercase<...>` distributes over unions and unions de-duplicate —
// `Lowercase<"X-A" | "x-a"> = "x-a" | "x-a" = "x-a"` collapses the
// duplicate before we can detect it. The walker below threads the
// already-lowercased prefix through an accumulator so each new entry
// is tested against the same case-folded view of every prior one.
//
// When `HeaderNames` widens to a non-tuple `ReadonlyArray<string>`
// (e.g. because a header was added via a non-literal expression, or a
// caller used the wide `EndpointDef<string>` form), the recursive
// destructuring bottoms out at the very first step and the helper
// resolves to `unknown` — deferring to the runtime `seenHeaderKeys`
// check in `validateEndpoint` (Http.ts L1336-L1345). The runtime check
// stays the canonical error source.

type _NoCaseFoldDup<
  Hs extends ReadonlyArray<string>,
  Seen extends ReadonlyArray<string> = readonly [],
> = Hs extends readonly [infer Head extends string, ...infer Tail extends ReadonlyArray<string>]
  ? string extends Head
    ? unknown
    : IncludesString<Seen, Lowercase<Head>> extends true
      ? Invalid<`endpoint declares header '${Head}' more than once (case-insensitive)`>
      : _NoCaseFoldDup<Tail, readonly [...Seen, Lowercase<Head>]>
  : unknown

/**
 * Compile-time gate ensuring a single endpoint does not declare the
 * same header twice when names are compared case-insensitively
 * (HTTP header names are case-insensitive on the wire). Walks the
 * tuple, lowercasing each entry and comparing it against an
 * accumulator of previously-seen lowercase names.
 *
 * Resolves to `unknown` (no-op intersection) when every name's
 * lowercase form is unique, OR when the tuple widens to a non-tuple
 * `ReadonlyArray<string>` (deferring to the runtime check). Resolves
 * to {@link Invalid} on the first collision — the message names the
 * offending header so the IDE / `tsc` output is readable.
 *
 * Mirrors (defence-in-depth) the runtime `seenHeaderKeys` Set in
 * `validateEndpoint` (Http.ts L1336-L1345). The union form
 * (`Lowercase<keyof H>`) is intentionally NOT used: TypeScript's
 * intrinsic string-literal manipulation collapses unions, which is
 * exactly the duplication signal we need to preserve.
 *
 * @since 1.5.0
 * @category models
 */
export type NoCaseFoldDuplicates<Hs extends ReadonlyArray<string>> = _NoCaseFoldDup<Hs>

// ---------------------------------------------------------------------------
// Union-to-tuple helper used to lift `keyof H & string` (the keys of an
// `EndpointOptions.headers` record) into a tuple shape that the
// `NoCaseFoldDuplicates` walker can consume.
//
// The classic "function-overload accumulation" trick: each member of
// the union is encoded as a function returning that member, then
// `UnionToIntersection` merges them, and the intersection's last
// signature pulls one member out (the order is implementation-defined
// but consistent within a single `tsc` run). We then `Exclude` that
// member from the union and recurse.
//
// Order is non-deterministic across TS versions, but order does NOT
// matter for case-fold collision detection — only set membership does.
// Tuple width is the count of distinct header keys, typically 1-5, so
// recursion depth is fine.

type UnionToIntersection<U> = (U extends unknown ? (k: U) => void : never) extends (
  k: infer I,
) => void
  ? I
  : never

type LastOf<U> =
  UnionToIntersection<U extends unknown ? () => U : never> extends () => infer L ? L : never

/**
 * Convert a union of string literals to a readonly tuple of string
 * literals. When `U` widens to `string`, resolves to a non-tuple
 * `ReadonlyArray<string>` so downstream tuple-walking helpers
 * short-circuit and defer to runtime checks.
 *
 * @since 1.5.0
 * @category models
 */
export type UnionToTuple<U, Last = LastOf<U>> = string extends U
  ? ReadonlyArray<string>
  : [U] extends [never]
    ? readonly []
    : Last extends string
      ? readonly [...UnionToTuple<Exclude<U, Last>>, Last]
      : ReadonlyArray<string>

/**
 * Tuple of header-name keys extracted from an `EndpointOptions.headers`
 * record. Empty literal records — including the default
 * `NoHeaderBindings` record (`Readonly<Record<string, never>>`, whose
 * `keyof` widens to `string`) — collapse to `readonly []` so chained
 * `withHeader` calls produce real tuples that drive the
 * `NoCaseFoldDuplicates` walker correctly. Non-literal / widened
 * records (where the user supplies `headers` whose keys are not
 * statically known) degrade to `ReadonlyArray<string>`,
 * short-circuiting the walker and deferring to the runtime check.
 *
 * @since 1.5.0
 * @category models
 */
export type HeaderKeysTuple<H> = [H[keyof H]] extends [never]
  ? readonly []
  : string extends keyof H
    ? ReadonlyArray<string>
    : [keyof H & string] extends [never]
      ? readonly []
      : UnionToTuple<keyof H & string>

// ---------------------------------------------------------------------------
// Bodyless verbs (GET / HEAD) cannot have unbound method parameters
// ---------------------------------------------------------------------------
//
// `EndpointVars` (the first phantom on `EndpointDef`) is the union of
// every name bound by the endpoint — path, query, AND header. So
// `Exclude<keyof Params & string, EndpointVars> extends never`
// expresses "every method parameter is bound somewhere on this
// endpoint". When the endpoint's `Kind` is narrowed to `"bodyless"`
// (only the `Http.get` / `Http.head` shorthands do this), an unbound
// param is a hard error: there is no request body in which the host
// could deliver its value.
//
// `Http.endpoint("GET", ...)` and `Http.custom(verb, ...)` are
// deliberately tagged `"bodyful"` regardless of the verb string —
// they do NOT participate in this check, matching the runtime
// `isBodylessVerb` convention which only treats the literal `"GET"`
// / `"HEAD"` shorthands as bodyless.
//
// Mirrors (defence-in-depth) the runtime "GET/HEAD endpoints may not
// have unbound parameters" loop in `validateEndpoint` (Http.ts
// L1136-1146). The runtime check stays the canonical error source —
// the type-level pre-filter only refuses to compile call sites whose
// missing bindings are statically visible.

// "bodyless" is currently the only bodyless tag; a `K extends
// "bodyless"` distribution preserves the message wording for any
// future tag without enumerating cases here.
type BodylessLabel<K extends string> = K extends "bodyless" ? "GET/HEAD" : K

/**
 * Per-element validator for the `http` array of a `MethodSpec`.
 *
 * Maps each endpoint positionally — endpoints with different `Kind`
 * values and different bound-var sets in the same array are validated
 * independently. Endpoints whose `Kind` extends `"bodyless"` AND
 * whose bound-var union does NOT cover every key of `Params` are
 * replaced with an {@link Invalid} carrier whose message names the
 * missing parameter(s); every other endpoint passes through
 * unchanged. Bodyful endpoints are always passed through (their
 * unbound params map to JSON body fields keyed by name).
 *
 * Used in `src/internal/method.ts` as the constraint on the
 * `method({ ..., http })` factory's `http` field: the array literal
 * is intersected with this mapped tuple, surfacing a compile error
 * at the offending endpoint position with the missing parameter
 * baked into the error message.
 *
 * @since 1.5.0
 * @category models
 */
export type ValidateBodylessEndpoints<
  Endpoints extends ReadonlyArray<EndpointDef<string, EndpointKind, EndpointBound, unknown>>,
  Params,
> = {
  readonly [I in keyof Endpoints]: Endpoints[I] extends EndpointDef<
    infer Bound,
    infer Kind,
    EndpointBound,
    unknown
  >
    ? Kind extends "bodyless"
      ? [Exclude<keyof Params & string, Bound>] extends [never]
        ? Endpoints[I]
        : Invalid<`${BodylessLabel<Kind>} endpoint cannot have unbound param '${Exclude<
            keyof Params & string,
            Bound
          > &
            string}' (only path / query / header bindings are allowed because there is no request body)`>
      : Endpoints[I]
    : Endpoints[I]
}

/**
 * Spec-side intersection variant of {@link ValidateBodylessEndpoints}
 * for use in curried combinators (e.g. `withHttp`) where the endpoints
 * are received first and the spec — carrying `params` — arrives in a
 * later call. Folds the per-element results into a single union of
 * intersection partners: every valid endpoint contributes `unknown`
 * (a no-op intersection), every bodyless-but-uncovered endpoint
 * contributes an {@link Invalid} carrier. Intersecting the spec arg
 * with the result forces tsc to surface the failure at the spec
 * call-site rather than at the (already-passed) endpoints arg.
 *
 * @since 1.5.0
 * @category models
 */
export type RequireValidBodylessEndpoints<
  Endpoints extends ReadonlyArray<EndpointDef<string, EndpointKind, EndpointBound, unknown>>,
  Params,
> = {
  readonly [I in keyof Endpoints]: Endpoints[I] extends EndpointDef<
    infer Bound,
    infer Kind,
    EndpointBound,
    unknown
  >
    ? Kind extends "bodyless"
      ? [Exclude<keyof Params & string, Bound>] extends [never]
        ? unknown
        : Invalid<`${BodylessLabel<Kind>} endpoint cannot have unbound param '${Exclude<
            keyof Params & string,
            Bound
          > &
            string}' (only path / query / header bindings are allowed because there is no request body)`>
      : unknown
    : unknown
}[number]
