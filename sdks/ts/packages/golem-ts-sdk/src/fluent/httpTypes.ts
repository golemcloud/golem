// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Type-level helpers for compile-time HTTP route validation in the fluent SDK.
//
// Every helper here is a compile-time PRE-FILTER: it only shrinks the set of
// programs `tsc` accepts. The runtime validators in `http.ts` / `runtime.ts`
// keep firing as defence-in-depth, so non-literal / dynamically-built paths
// (which widen to plain `string` and short-circuit these gates) still get
// checked at registration time.
//
// `import type` is used throughout to avoid a runtime import cycle with
// `http.ts` (which imports the value-free validators back from here).

import type { EndpointKind, HttpEndpointSpec, HttpMountSpec } from './http';
import type { MarkerKindOf } from './schema/markers';

declare const InvalidBrand: unique symbol;

/**
 * Branded "compile-time error" carrier. When a type-level check fails, the
 * helper resolves to a value that intersects with `Invalid<"…">`. Because
 * `[InvalidBrand]: Reason` is a non-optional unique-symbol property that no real
 * value carries, the failing assignment is rejected — and the `tsc` / IDE
 * message includes the literal `Reason` string verbatim, rendering as a readable
 * error rather than a bare `never`.
 */
export interface Invalid<Reason extends string> {
  readonly [InvalidBrand]: Reason;
}

/** System-variable names the Golem host injects into routes. */
export type SystemVariableName = 'agent-type' | 'agent-version';

// ---------------------------------------------------------------------------
// ValidMountPath / ValidEndpointPath — template-literal path-shape checks
// ---------------------------------------------------------------------------

type StartsWithSlash<S extends string> = S extends `/${string}` ? true : false;
type HasTrailingSlash<S extends string> = S extends '/'
  ? false
  : S extends `${string}/`
    ? true
    : false;
type HasDoubleSlash<S extends string> = S extends `${string}//${string}` ? true : false;
type HasQuery<S extends string> = S extends `${string}?${string}` ? true : false;

type CountQuery<
  S extends string,
  Acc extends ReadonlyArray<unknown> = [],
> = S extends `${string}?${infer Rest}` ? CountQuery<Rest, [...Acc, unknown]> : Acc['length'];

type HasOpenBrace<S extends string> = S extends `${string}{${string}` ? true : false;
type HasCloseBrace<S extends string> = S extends `${string}}${string}` ? true : false;

// The path portion of an endpoint template is everything before the first '?'.
type EndpointPathPart<S extends string> = S extends `${infer P}?${string}` ? P : S;

// Validate the inside of a `{…}` segment.
type ValidateBracedSegment<
  Inner extends string,
  Seg extends string,
  IsMount extends boolean,
  IsLast extends boolean,
  S extends string,
> = Inner extends ''
  ? Invalid<`path '${S}' has an empty variable name in '${Seg}'`>
  : HasOpenBrace<Inner> extends true
    ? Invalid<`path '${S}' has nested or malformed braces in '${Seg}'`>
    : HasCloseBrace<Inner> extends true
      ? Invalid<`path '${S}' has nested or malformed braces in '${Seg}'`>
      : Inner extends `*${infer Name}`
        ? IsMount extends true
          ? Invalid<`mount path '${S}' may not include a catch-all segment '${Seg}'`>
          : Name extends ''
            ? Invalid<`endpoint path '${S}' has a catch-all variable with no name in '${Seg}'`>
            : IsLast extends true
              ? 'ok'
              : Invalid<`endpoint path '${S}' has catch-all segment '${Seg}' that is not the last path segment`>
        : 'ok';

type ValidateSegment<
  Seg extends string,
  IsMount extends boolean,
  IsLast extends boolean,
  S extends string,
> = Seg extends ''
  ? // Empty segments are caught earlier by `HasDoubleSlash` / `HasTrailingSlash`.
    'ok'
  : Seg extends `{${infer Inner}}`
    ? ValidateBracedSegment<Inner, Seg, IsMount, IsLast, S>
    : HasOpenBrace<Seg> extends true
      ? Invalid<`path '${S}' segment '${Seg}' mixes literal text with '{...}' variables`>
      : HasCloseBrace<Seg> extends true
        ? Invalid<`path '${S}' segment '${Seg}' mixes literal text with '{...}' variables`>
        : 'ok';

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
  : ValidateSegment<Rest, IsMount, true, S>;

type ValidateSegments<
  PathPart extends string,
  IsMount extends boolean,
  S extends string,
> = PathPart extends '/'
  ? 'ok'
  : PathPart extends `/${infer Rest}`
    ? ValidateSegmentsRec<Rest, IsMount, S>
    : 'ok';

type ValidateMountShape<S extends string> = StartsWithSlash<S> extends false
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
          : never;

type ValidateEndpointShape<S extends string> = StartsWithSlash<S> extends false
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
        : Invalid<`endpoint path '${S}' contains more than one '?'`>;

type ValidateEndpointQuery<S extends string> = S extends `${string}?${infer Q}`
  ? ValidateQueryString<Q, S> extends infer R
    ? R extends Invalid<string>
      ? R
      : UniqueQueryKeys<Q, S>
    : never
  : S;

type ValidateQueryString<Q extends string, S extends string> = Q extends ''
  ? S
  : Q extends `&${string}`
    ? Invalid<`endpoint path '${S}' has an empty '&' segment in its query string`>
    : Q extends `${string}&`
      ? Invalid<`endpoint path '${S}' has an empty '&' segment in its query string`>
      : Q extends `${string}&&${string}`
        ? Invalid<`endpoint path '${S}' has an empty '&' segment in its query string`>
        : ValidateQueryPairs<Q, S>;

type ValidateQueryPairs<
  Q extends string,
  S extends string,
> = Q extends `${infer Pair}&${infer Rest}`
  ? CheckPair<Pair, S> extends infer R
    ? R extends Invalid<string>
      ? R
      : ValidateQueryPairs<Rest, S>
    : never
  : CheckPair<Q, S>;

// A query pair beginning with '=' has an empty key. The '{var}' shape and
// var-name regex on the value side are owned by the runtime parser.
type CheckPair<P extends string, S extends string> = P extends `=${string}`
  ? Invalid<`endpoint path '${S}' has an empty query parameter name`>
  : S;

// ---------------------------------------------------------------------------
// Duplicate query parameter keys
// ---------------------------------------------------------------------------

type ExtractQueryKey<Pair extends string> = Pair extends `${infer K}=${string}` ? K : Pair;

type IncludesString<T extends ReadonlyArray<string>, K extends string> = T extends readonly [
  infer Head,
  ...infer Tail,
]
  ? [Head, K] extends [K, Head]
    ? true
    : Tail extends ReadonlyArray<string>
      ? IncludesString<Tail, K>
      : false
  : false;

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
    : never;

/**
 * Compile-time gate for query-string key uniqueness. Walks `Q` (the portion of
 * an endpoint path *after* the first `?`) one pair at a time; collisions resolve
 * to `Invalid<"…duplicate query key '…'">`, otherwise resolves to `S` unchanged.
 */
export type UniqueQueryKeys<Q extends string, S extends string> = UniqueQueryPairsRec<Q, [], S>;

/**
 * Compile-time gate for mount paths: returns the literal `S` itself when the
 * template-literal rules hold (must start with `/`, no trailing `/`, no `//`, no
 * `?`, valid segment shapes, no catch-all), else an `Invalid<"…">` carrier. When
 * `S` widens to plain `string` (non-literal expression), the helper
 * short-circuits to `S` and the runtime parser is the only line of defence.
 */
export type ValidMountPath<S extends string> = string extends S ? S : ValidateMountShape<S>;

/**
 * Compile-time gate for endpoint paths. Same rules as {@link ValidMountPath}
 * except `?` is permitted at most once and `{*rest}` is allowed as the last
 * segment; also enforces query-key uniqueness.
 */
export type ValidEndpointPath<S extends string> = string extends S
  ? S
  : ValidateEndpointShape<S>;

// ---------------------------------------------------------------------------
// Type-level path/query-variable extraction
// ---------------------------------------------------------------------------

type SplitPathQuery<S extends string> = S extends `${infer P}?${infer Q}`
  ? { path: P; query: Q }
  : { path: S; query: '' };

type CleanVarName<V extends string> = V extends `*${infer N}` ? N : V;

type ExtractVarsFromPath<S extends string> = S extends `${string}{${infer V}}${infer Rest}`
  ? CleanVarName<V> | ExtractVarsFromPath<Rest>
  : never;

type ExtractVarsFromQuery<S extends string> = S extends `${string}={${infer V}}${infer Rest}`
  ? V | ExtractVarsFromQuery<Rest>
  : never;

/** Extract all path-variable names from a literal path template (system vars included). */
export type PathVarsOf<S extends string> = ExtractVarsFromPath<SplitPathQuery<S>['path']>;

/** Extract all query-variable values from a literal path template's `?…` portion. */
export type QueryVarsOf<S extends string> = ExtractVarsFromQuery<SplitPathQuery<S>['query']>;

/**
 * Extract the union of values from a `Record<string, string>` literal. Returns
 * `never` for empty records so `{}` does not widen the binding union to `string`.
 */
export type ValuesOf<R> = [keyof R] extends [never]
  ? never
  : R extends Readonly<Record<string, infer V>>
    ? V & string
    : never;

// Tuple-emitting variants (the union forms collapse `"a" | "a"` to `"a"`, so the
// duplicate-binding walk needs the tuple shape to preserve duplicates).

type ExtractPathTupleRec<
  S extends string,
  Acc extends ReadonlyArray<string> = readonly [],
> = S extends `${string}{${infer V}}${infer Rest}`
  ? ExtractPathTupleRec<Rest, readonly [...Acc, CleanVarName<V>]>
  : Acc;

type ExtractQueryTupleRec<
  S extends string,
  Acc extends ReadonlyArray<string> = readonly [],
> = S extends `${string}={${infer V}}${infer Rest}`
  ? ExtractQueryTupleRec<Rest, readonly [...Acc, V]>
  : Acc;

type FilterSystemVarsRec<
  T extends ReadonlyArray<string>,
  Acc extends ReadonlyArray<string> = readonly [],
> = T extends readonly [infer Head extends string, ...infer Tail extends ReadonlyArray<string>]
  ? Head extends SystemVariableName
    ? FilterSystemVarsRec<Tail, Acc>
    : FilterSystemVarsRec<Tail, readonly [...Acc, Head]>
  : Acc;

/** Tuple of path-variable names (system vars stripped, catch-all `*` removed). */
export type PathTupleOf<S extends string> = FilterSystemVarsRec<
  ExtractPathTupleRec<SplitPathQuery<S>['path']>
>;

/** Tuple of query-variable names extracted from the inline `?…` portion. */
export type QueryTupleOf<S extends string> = ExtractQueryTupleRec<SplitPathQuery<S>['query']>;

// ---------------------------------------------------------------------------
// BindableKeys — keys whose value is statically eligible for path/query/header binding
// ---------------------------------------------------------------------------

// `any`-safe guard. Without it, `BindableKeys<any>` would collapse to `never`
// (via a spurious non-bindable classification) and break the structural
// compatibility of `Method`-shaped values against `MethodSpec<any, any>`.
type IsAny<V> = 0 extends 1 & V ? true : false;

// Marker kinds that carry rich payloads and therefore CANNOT be bound from a
// string path / query / header variable.
type NonBindableKind = 'multimodal' | 'unstructured-text' | 'unstructured-binary';

// A value is bindable unless it is a marker tagged with a non-bindable kind.
// Plain schemas (Zod / Valibot / ArkType / Effect Schema) and scalar markers
// resolve to `undefined` / `'scalar'` respectively, both of which are bindable.
type IsBindable<V> = IsAny<V> extends true
  ? true
  : [MarkerKindOf<V>] extends [NonBindableKind]
    ? false
    : true;

/**
 * Subset of `keyof C & string` whose value is statically eligible for binding
 * from a string source (path / query / header) — i.e. NOT a multimodal or
 * unstructured marker (`s.multimodal(...)` / `s.unstructuredText(...)` /
 * `s.unstructuredBinary(...)`), which carry rich payloads. Full
 * string-bindability (rejecting struct-shaped schemas etc.) still depends on the
 * runtime AST inspection in `runtime.ts`.
 */
export type BindableKeys<C> = {
  [K in keyof C & string]: IsBindable<C[K]> extends true ? K : never;
}[keyof C & string];

// ---------------------------------------------------------------------------
// MountSpecCovering — every id field must appear as a {var} in the mount path
// ---------------------------------------------------------------------------

/**
 * Resolves to `HttpMountSpec<V, W>` when every id field of `Id` is present in
 * `V` (the mount path's `{var}` names), else to `HttpMountSpec<V, W> &
 * Invalid<"…">`. The intersection surfaces a readable `[InvalidBrand]: "mount
 * path missing var '…'"` requirement that no real spec value can satisfy, so the
 * `defineAgent` assignment fails with a clear message.
 *
 * Mirrors (defence-in-depth) the runtime "every mount var is an id field" loop
 * in `runtime.ts`.
 */
export type MountSpecCovering<Id, V extends string, W extends string = never> = [
  Exclude<keyof Id & string, V>,
] extends [never]
  ? HttpMountSpec<V, W>
  : HttpMountSpec<V, W> & Invalid<`mount path missing var '${Exclude<keyof Id & string, V> & string}'`>;

// ---------------------------------------------------------------------------
// WebhookVarsValid — every webhook-suffix {var} must be a *bindable* id field
// ---------------------------------------------------------------------------

declare const WebhookVarsValidBrand: unique symbol;

/**
 * Compile-time gate for webhook-suffix path variables. Returns `unknown` (a
 * no-op intersection at the assignment site) when every `{var}` in the webhook
 * suffix is an agent id field that is itself bindable (i.e. not a multimodal /
 * unstructured marker — see {@link BindableKeys}). On violation, resolves to a
 * carrier that no real value can satisfy, surfacing a readable error:
 * - when a suffix `{var}` IS an id field but that field is a non-bindable
 *   marker, the message says it refers to a multimodal/unstructured id field;
 * - when a suffix `{var}` is not an id field at all, the message says it does
 *   not match any agent id field.
 */
export type WebhookVarsValid<Id, WebhookVars extends string> = [WebhookVars] extends [never]
  ? unknown
  : [Exclude<WebhookVars, BindableKeys<Id>>] extends [never]
    ? unknown
    : [Exclude<WebhookVars, keyof Id & string>] extends [never]
      ? {
          readonly [WebhookVarsValidBrand]: `webhook-suffix var '${Exclude<WebhookVars, BindableKeys<Id>> & string}' refers to a multimodal/unstructured id field and cannot be bound from a path variable`;
        }
      : {
          readonly [WebhookVarsValidBrand]: `webhook-suffix var '${Exclude<WebhookVars, keyof Id & string> & string}' does not match any agent id field`;
        };

// ---------------------------------------------------------------------------
// A method param can be bound at most once across path/query/header
// ---------------------------------------------------------------------------

/**
 * Structured shape of an endpoint's bound parameter names, broken down by
 * binding source. Carried as a phantom on {@link HttpEndpointSpec} so
 * {@link NoDuplicateBindings} can detect a parameter bound from more than one
 * source within the same endpoint.
 */
export interface EndpointBound {
  readonly path: ReadonlyArray<string>;
  readonly query: ReadonlyArray<string>;
  readonly header: ReadonlyArray<string>;
}

/**
 * Default ("unknown") `Bound` value used when no per-source tracking is
 * available (e.g. the wide `HttpEndpointSpec` used by the runtime validators).
 * Each slot is a non-tuple `ReadonlyArray<string>`, so {@link NoDuplicateBindings}
 * short-circuits to `unknown` and the runtime check remains canonical.
 */
export interface EndpointBoundAny extends EndpointBound {
  readonly path: ReadonlyArray<string>;
  readonly query: ReadonlyArray<string>;
  readonly header: ReadonlyArray<string>;
}

type AllBoundNames<B extends EndpointBound> = readonly [
  ...B['path'],
  ...B['query'],
  ...B['header'],
];

type FindFirstDuplicate<
  T extends ReadonlyArray<string>,
  Seen extends ReadonlyArray<string> = readonly [],
> = T extends readonly [infer Head extends string, ...infer Tail extends ReadonlyArray<string>]
  ? IncludesString<Seen, Head> extends true
    ? Head
    : FindFirstDuplicate<Tail, readonly [...Seen, Head]>
  : never;

/**
 * Compile-time gate ensuring a single endpoint binds a given method parameter
 * from at most one of path / query / header. Resolves to `unknown` when every
 * name is unique or a slot is a non-tuple array (deferring to the runtime
 * check); to {@link Invalid} when a name appears in more than one slot.
 */
export type NoDuplicateBindings<B extends EndpointBound> =
  FindFirstDuplicate<AllBoundNames<B>> extends infer D
    ? [D] extends [never]
      ? unknown
      : D extends string
        ? Invalid<`endpoint binds parameter '${D}' more than once across path/query/header`>
        : unknown
    : unknown;

// ---------------------------------------------------------------------------
// Case-insensitive uniqueness of header names within one endpoint
// ---------------------------------------------------------------------------

type _NoCaseFoldDup<
  Hs extends ReadonlyArray<string>,
  Seen extends ReadonlyArray<string> = readonly [],
> = Hs extends readonly [infer Head extends string, ...infer Tail extends ReadonlyArray<string>]
  ? string extends Head
    ? unknown
    : IncludesString<Seen, Lowercase<Head>> extends true
      ? Invalid<`endpoint declares header '${Head}' more than once (case-insensitive)`>
      : _NoCaseFoldDup<Tail, readonly [...Seen, Lowercase<Head>]>
  : unknown;

/**
 * Compile-time gate ensuring a single endpoint does not declare the same header
 * twice when names are compared case-insensitively (HTTP header names are
 * case-insensitive on the wire). Resolves to `unknown` when unique or the tuple
 * widens to a non-tuple array; to {@link Invalid} on the first collision.
 */
export type NoCaseFoldDuplicates<Hs extends ReadonlyArray<string>> = _NoCaseFoldDup<Hs>;

// ---------------------------------------------------------------------------
// Union-to-tuple (lift `keyof H & string` into a tuple for the case-fold walker)
// ---------------------------------------------------------------------------

type UnionToIntersection<U> = (U extends unknown ? (k: U) => void : never) extends (
  k: infer I,
) => void
  ? I
  : never;

type LastOf<U> =
  UnionToIntersection<U extends unknown ? () => U : never> extends () => infer L ? L : never;

/**
 * Convert a union of string literals to a readonly tuple. When `U` widens to
 * `string`, resolves to a non-tuple `ReadonlyArray<string>` so downstream tuple
 * walkers short-circuit and defer to runtime checks.
 */
export type UnionToTuple<U, Last = LastOf<U>> = string extends U
  ? ReadonlyArray<string>
  : [U] extends [never]
    ? readonly []
    : Last extends string
      ? readonly [...UnionToTuple<Exclude<U, Last>>, Last]
      : ReadonlyArray<string>;

/**
 * Tuple of header-name keys extracted from an endpoint's `headers` record. Empty
 * / widened records degrade to `ReadonlyArray<string>` so the case-fold walker
 * short-circuits and defers to the runtime check.
 */
export type HeaderKeysTuple<H> = [H[keyof H]] extends [never]
  ? readonly []
  : string extends keyof H
    ? ReadonlyArray<string>
    : [keyof H & string] extends [never]
      ? readonly []
      : UnionToTuple<keyof H & string>;

/**
 * Header values extracted from `H`, used as the seed for the structured
 * `Bound["header"]` slot. Empty (`ValuesOf<H>` = `never`) resolves to `readonly
 * []`; otherwise a non-tuple `ReadonlyArray<…>`.
 */
export type HeaderValuesArray<H extends Readonly<Record<string, string>>> = [
  ValuesOf<H>,
] extends [never]
  ? readonly []
  : ReadonlyArray<ValuesOf<H>>;

// ---------------------------------------------------------------------------
// Bodyless verbs (GET / HEAD) cannot have unbound method parameters
// ---------------------------------------------------------------------------

// "bodyless" is currently the only bodyless tag; the `K extends "bodyless"`
// distribution preserves the message wording for any future tag.
type BodylessLabel<K extends string> = K extends 'bodyless' ? 'GET/HEAD' : K;

/**
 * Per-element validator for the `http` array of a `MethodSpec`. Endpoints whose
 * `Kind` extends `"bodyless"` and whose bound-var union does NOT cover every key
 * of `Params` are replaced with an {@link Invalid} carrier naming the missing
 * parameter; every other endpoint passes through unchanged.
 */
export type ValidateBodylessEndpoints<
  Endpoints extends ReadonlyArray<HttpEndpointSpec<string, EndpointKind, EndpointBound, unknown>>,
  Params,
> = {
  readonly [I in keyof Endpoints]: Endpoints[I] extends HttpEndpointSpec<
    infer Bound,
    infer Kind,
    EndpointBound,
    unknown
  >
    ? Kind extends 'bodyless'
      ? [Exclude<keyof Params & string, Bound>] extends [never]
        ? Endpoints[I]
        : Invalid<`${BodylessLabel<Kind>} endpoint cannot have unbound param '${Exclude<
            keyof Params & string,
            Bound
          > &
            string}' (only path / query / header bindings are allowed because there is no request body)`>
      : Endpoints[I]
    : Endpoints[I];
};

/**
 * Spec-side intersection variant of {@link ValidateBodylessEndpoints}: folds the
 * per-element results into a single union of intersection partners (each valid
 * endpoint contributes `unknown`, each bodyless-but-uncovered endpoint an
 * {@link Invalid} carrier), so tsc surfaces the failure at the spec call site.
 */
export type RequireValidBodylessEndpoints<
  Endpoints extends ReadonlyArray<HttpEndpointSpec<string, EndpointKind, EndpointBound, unknown>>,
  Params,
> = {
  readonly [I in keyof Endpoints]: Endpoints[I] extends HttpEndpointSpec<
    infer Bound,
    infer Kind,
    EndpointBound,
    unknown
  >
    ? Kind extends 'bodyless'
      ? [Exclude<keyof Params & string, Bound>] extends [never]
        ? unknown
        : Invalid<`${BodylessLabel<Kind>} endpoint cannot have unbound param '${Exclude<
            keyof Params & string,
            Bound
          > &
            string}' (only path / query / header bindings are allowed because there is no request body)`>
      : unknown
    : unknown;
}[number];

// ---------------------------------------------------------------------------
// Combined per-endpoint validation used by the `method({...})` factory
// ---------------------------------------------------------------------------

// Cross-source binding uniqueness + case-insensitive header-name uniqueness,
// factored out so the bodyless-verb wrapper can dispatch on `Kind`.
type ValidateEndpointStructure<E, B, HN> = B extends EndpointBound
  ? HN extends ReadonlyArray<string>
    ? NoDuplicateBindings<B> extends infer R1
      ? [R1] extends [Invalid<string>]
        ? R1
        : NoCaseFoldDuplicates<HN> extends infer R2
          ? [R2] extends [Invalid<string>]
            ? R2
            : E
          : E
      : E
    : E
  : E;

/**
 * Positional validator for the `http` array of a `method({...})` call: for each
 * endpoint, enforce (in order) the bodyless-unbound-param check, then the
 * cross-source duplicate-binding check, then the case-fold header check. A
 * failing element is replaced with an {@link Invalid} carrier at that position.
 */
export type ValidateEndpointsTuple<
  Eps extends ReadonlyArray<HttpEndpointSpec<string>>,
  Params,
> = {
  readonly [K in keyof Eps]: Eps[K] extends HttpEndpointSpec<infer V, infer Kind, infer B, infer HN>
    ? Kind extends 'bodyless'
      ? [Exclude<keyof Params & string, V>] extends [never]
        ? ValidateEndpointStructure<Eps[K], B, HN>
        : Invalid<`GET/HEAD endpoint cannot have unbound param '${Exclude<
            keyof Params & string,
            V
          > &
            string}' (only path / query / header bindings are allowed because there is no request body)`>
      : ValidateEndpointStructure<Eps[K], B, HN>
    : Eps[K];
};

/**
 * Single-endpoint variant of {@link ValidateEndpointsTuple} for the `method`
 * factory's non-array `http` form. Wraps the spec in a one-tuple, validates, and
 * unwraps.
 */
export type ValidateSingleEndpoint<
  Ep extends HttpEndpointSpec<string>,
  Params,
> = ValidateEndpointsTuple<readonly [Ep], Params> extends readonly [infer R] ? R : Ep;
