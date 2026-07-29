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

// Plain-data HTTP routing surface for the fluent SDK (issue #3449, Phase 6).
//
// An agent declares its HTTP routing entirely with config objects: a single
// mount on `defineAgent({ http: ... })` and one-or-more per-method endpoints on
// `method({ http: ... })`. The surface is plain data: a `mount` constructor,
// verb shorthands (`get`/`post`/...), path builders (`literal`/`pathVar`/
// `restVar`), the `withAuth`/`withCors`/`withWebhookSuffix` combinators, and the
// `compileMount`/`compileEndpoint` compilers to the WIT records.
//
// Path templates use the `{var}` / `{*rest}` / `{agent-type}` brace syntax and
// are parsed by the same decorator-era parsers (`src/internal/http/path.ts`,
// `query.ts`) that back the `@agent`/`@endpoint` decorators, so the fluent and
// decorator surfaces share one parser and produce identical WIT records. The
// decorator-era `src/internal/http/validation.ts` validators are intentionally
// NOT reused: they reach into the per-class param registries
// (`RuntimeTypeInfo`) populated by the decorators, which the fluent runtime
// never fills. The lightweight, registry-free consistency checks the fluent
// surface needs (mount vars exist in the id record, endpoint vars exist in the
// method input) live in `runtime.ts` next to the codecs.

import {
  HttpEndpointDetails,
  HttpMethod,
  HttpMountDetails,
  PathSegment,
} from 'golem:agent/common@2.0.0';
import { parsePath } from '../internal/http/path';
import { parseQuery } from '../internal/http/query';
import { rejectEmptyString, rejectQueryParamsInPath } from '../internal/http/validation';
import type {
  EndpointBound,
  EndpointBoundAny,
  HeaderKeysTuple,
  HeaderValuesArray,
  PathTupleOf,
  PathVarsOf,
  QueryTupleOf,
  QueryVarsOf,
  SystemVariableName,
  UnionToTuple,
  ValidEndpointPath,
  ValidMountPath,
  ValuesOf,
} from './httpTypes';

// Re-export the type-level validators / helpers that make up the public
// compile-time HTTP surface (used by `defineAgent` / `method` and available to
// advanced callers). `Invalid<…>` is intentionally NOT re-exported: it is an
// internal carrier users only ever meet as a hover/error message.
export type {
  EndpointBound,
  EndpointBoundAny,
  HeaderKeysTuple,
  NoCaseFoldDuplicates,
  NoDuplicateBindings,
  UnionToTuple,
  ValidEndpointPath,
  ValidMountPath,
} from './httpTypes';

/**
 * A path-segment builder argument: either a raw {@link PathSegment} (escape
 * hatch) or one of the {@link literal} / {@link pathVar} / {@link restVar} /
 * {@link agentType} / {@link agentVersion} helpers below.
 */
export type { PathSegment } from 'golem:agent/common@2.0.0';

/** Build a literal path segment, e.g. `http.literal('counters')`. */
export const literal = (value: string): PathSegment => ({ tag: 'literal', val: value });

/** Build a path-variable segment, e.g. `http.pathVar('name')` → `{name}`. */
export const pathVar = (variableName: string): PathSegment => ({
  tag: 'path-variable',
  val: { variableName },
});

/** Build a catch-all (remaining-path) variable segment; only valid last. */
export const restVar = (variableName: string): PathSegment => ({
  tag: 'remaining-path-variable',
  val: { variableName },
});

/** `{agent-type}` — runtime-injected by the host. */
export const agentType = (): PathSegment => ({ tag: 'system-variable', val: 'agent-type' });

/** `{agent-version}` — runtime-injected by the host. */
export const agentVersion = (): PathSegment => ({ tag: 'system-variable', val: 'agent-version' });

/** A path: either a `{var}`-template string or an array of segment builders. */
export type PathInput = string | readonly PathSegment[];

function resolvePath(path: PathInput, entityName: string): PathSegment[] {
  if (typeof path !== 'string') return [...path];
  rejectQueryParamsInPath(path, entityName);
  rejectEmptyString(path, entityName);
  return parsePath(path);
}

// ---------------------------------------------------------------------------
// HTTP verbs
// ---------------------------------------------------------------------------

/**
 * The HTTP verbs supported by the Golem host. Standard verbs are the lowercase
 * literals; `{ custom: 'VERB' }` carries a non-standard verb verbatim through
 * to the WIT `http-method.custom(string)`.
 */
export type HttpVerb =
  | 'get'
  | 'head'
  | 'post'
  | 'put'
  | 'delete'
  | 'connect'
  | 'options'
  | 'trace'
  | 'patch'
  | { readonly custom: string };

function verbToWit(v: HttpVerb): HttpMethod {
  if (typeof v !== 'string') return { tag: 'custom', val: v.custom };
  return { tag: v };
}

// ---------------------------------------------------------------------------
// Mount / endpoint spec records (plain data carried on the config objects)
//
// The generic parameters are PHANTOM: they are carried only by the optional
// brand fields below (unique-symbol keys that never exist at runtime — the
// factories cast plain objects into the branded types). They let `defineAgent`
// and `method` bind mount/endpoint `{var}` names to the agent id record and the
// method input at compile time. Callers that construct the wide, unparameterised
// `HttpMountSpec` / `HttpEndpointSpec` (e.g. the runtime compilers) are
// unaffected — the defaults widen the phantoms away.
// ---------------------------------------------------------------------------

declare const mountVarsBrand: unique symbol;
declare const mountWebhookVarsBrand: unique symbol;
declare const endpointVarsBrand: unique symbol;
declare const endpointKindBrand: unique symbol;
declare const endpointBoundBrand: unique symbol;
declare const endpointHeaderNamesBrand: unique symbol;

/**
 * Whether an endpoint's HTTP verb permits a request body. `get` / `head` are
 * `"bodyless"`; every other verb (and `custom`) is `"bodyful"`. Used as a
 * phantom on {@link HttpEndpointSpec} so `method({...})` can statically reject a
 * bodyless endpoint whose bindings do not cover every method parameter.
 */
export type EndpointKind = 'bodyless' | 'bodyful';

/**
 * Mount declaration carried by `defineAgent({ http })`. Compiled to the WIT
 * `agent-type.http-mount` (`http-mount-details`) at registration time via
 * {@link compileMount}.
 *
 * `MountVars` is the union of `{var}` names in the mount path (system vars
 * stripped); `WebhookVars` the union of `{var}` names in the optional
 * `webhookSuffix`. Both are phantom — see the block comment above.
 */
export interface HttpMountSpec<
  MountVars extends string = string,
  WebhookVars extends string = never,
> {
  readonly [mountVarsBrand]?: MountVars;
  readonly [mountWebhookVarsBrand]?: WebhookVars;
  /** Mount path prefix; a `{var}` template string or segment builders. */
  readonly path: PathInput;
  /** When `true`, every endpoint requires authentication. Default `false`. */
  readonly auth?: boolean;
  /** CORS allowed-origin patterns advertised at the mount level. */
  readonly cors?: readonly string[];
  /** Mark this agent as a phantom agent (fresh instance per request). */
  readonly phantomAgent?: boolean;
  /** Optional custom webhook-suffix path (same rules as the mount path). */
  readonly webhookSuffix?: PathInput;
}

/**
 * Endpoint declaration carried by `method({ http })`. Compiled to one entry of
 * `agent-method.http-endpoint` (`http-endpoint-details`) via
 * {@link compileEndpoint}.
 *
 * `EndpointVars` is the union of every name the endpoint binds (path + query +
 * header value); `Kind` tracks bodyless/bodyful; `Bound` is the structured
 * `{ path; query; header }` tuple view (for duplicate-binding detection);
 * `HeaderNames` is the tuple of declared header names (for case-fold
 * uniqueness). All four are phantom — see the block comment above.
 */
export interface HttpEndpointSpec<
  EndpointVars extends string = string,
  Kind extends EndpointKind = EndpointKind,
  // `Bound` is intentionally left unconstrained (rather than `extends
  // EndpointBound`): eagerly checking that constraint against the structured
  // `{ path; query; header }` computed by the factories overflows tsc's
  // comparison depth. The type-level validators (`NoDuplicateBindings`,
  // `ValidateEndpointStructure`) re-assert `B extends EndpointBound` lazily at
  // their concrete call sites, so nothing downstream loses safety.
  Bound = EndpointBoundAny,
  HeaderNames = ReadonlyArray<string>,
> {
  readonly [endpointVarsBrand]?: EndpointVars;
  readonly [endpointKindBrand]?: Kind;
  readonly [endpointBoundBrand]?: Bound;
  readonly [endpointHeaderNamesBrand]?: HeaderNames;
  /** HTTP verb; standard lowercase literal or `{ custom }`. */
  readonly method: HttpVerb;
  /**
   * Endpoint path relative to the mount prefix. As a template string it may
   * include `{var}` / `{*rest}` segments and an inline `?key={var}&…` query.
   */
  readonly path: PathInput;
  /** Map of HTTP header name → method-parameter (variable) name. */
  readonly headers?: Readonly<Record<string, string>>;
  /**
   * Query-param bindings. Either via the inline `?…` of a template `path`, or
   * explicitly as a map of query-param name → method-parameter name.
   */
  readonly query?: Readonly<Record<string, string>>;
  /** Override the mount-level auth requirement for this endpoint only. */
  readonly auth?: boolean;
  /** Additional CORS allowed-origin patterns for this endpoint. */
  readonly cors?: readonly string[];
}

// ---------------------------------------------------------------------------
// Verb shorthands — sugar producing an HttpEndpointSpec.
//
// Each verb factory is overloaded: a template-string form that infers and
// checks the path `{var}` / `?query` bindings at compile time, and an
// escape-hatch `PathSegment[]` form that widens to the unparameterised spec
// (no compile-time checking; the runtime parser/validators still apply). The
// runtime body is a single plain-object builder cast into the overloaded type;
// the phantom brands never exist at runtime.
// ---------------------------------------------------------------------------

/**
 * Default value for the `H` (headers) generic. An "empty record" sentinel that
 * signals "no header bindings"; using `{}` directly would widen `ValuesOf<H>` to
 * `string`.
 */
export type NoHeaderBindings = Readonly<Record<string, never>>;

/**
 * Options accepted by the verb factories and `custom`. `H` is the header
 * name → method-parameter map; `Q` is the explicit query-param → method-parameter
 * map. Both maps' VALUES are the bound method-parameter names.
 */
export interface EndpointOptsFor<
  H extends Readonly<Record<string, string>> = NoHeaderBindings,
  Q extends Readonly<Record<string, string>> = NoHeaderBindings,
> {
  /** Map of HTTP header name → method-parameter name. */
  readonly headers?: H;
  /**
   * Explicit query-param name → method-parameter name map. Its VALUES are
   * threaded into the endpoint's `EndpointVars` (so binding to a non-parameter
   * errors) and into the structured `Bound.query` tuple (so it participates in
   * cross-source duplicate-binding detection), on par with the inline
   * `?k={var}` form. Use `as const` to preserve the literal values.
   */
  readonly query?: Q;
  /** Override the mount-level auth requirement for this endpoint only. */
  readonly auth?: boolean;
  /** Additional CORS allowed-origin patterns for this endpoint. */
  readonly cors?: readonly string[];
}

/** Escape-hatch (segment-array) form options: maps un-parameterised. */
type EndpointSugarOpts = EndpointOptsFor<
  Readonly<Record<string, string>>,
  Readonly<Record<string, string>>
>;

/** Tuple of method-parameter names bound by an explicit `query` map's VALUES. */
type QueryValuesTuple<Q extends Readonly<Record<string, string>>> = UnionToTuple<ValuesOf<Q>>;

/** The branded endpoint spec produced for a literal path `P`, headers `H`, query `Q`. */
type EndpointSpecFor<
  P extends string,
  H extends Readonly<Record<string, string>>,
  Q extends Readonly<Record<string, string>>,
  Kind extends EndpointKind,
> = HttpEndpointSpec<
  Exclude<PathVarsOf<P> | QueryVarsOf<P>, SystemVariableName> | ValuesOf<H> | ValuesOf<Q>,
  Kind,
  {
    readonly path: PathTupleOf<P>;
    readonly query: readonly [...QueryTupleOf<P>, ...QueryValuesTuple<Q>];
    readonly header: HeaderValuesArray<H>;
  },
  HeaderKeysTuple<H>
>;

/** Overloaded verb-factory shape, parameterised by {@link EndpointKind}. */
export interface VerbFactory<Kind extends EndpointKind> {
  <
    const P extends string,
    H extends Readonly<Record<string, string>> = NoHeaderBindings,
    Q extends Readonly<Record<string, string>> = NoHeaderBindings,
  >(
    path: ValidEndpointPath<P>,
    opts?: EndpointOptsFor<H, Q>,
  ): EndpointSpecFor<P, H, Q, Kind>;
  (path: readonly PathSegment[], opts?: EndpointSugarOpts): HttpEndpointSpec;
}

const verbShorthand =
  (method: HttpVerb) =>
  (path: PathInput, opts: EndpointSugarOpts = {}): HttpEndpointSpec =>
    ({ method, path, ...opts }) as HttpEndpointSpec;

/** Shorthand for `{ method: 'get', path, ...opts }`. Bodyless. */
export const get = verbShorthand('get') as unknown as VerbFactory<'bodyless'>;
/** Shorthand for `{ method: 'head', path, ...opts }`. Bodyless. */
export const head = verbShorthand('head') as unknown as VerbFactory<'bodyless'>;
/** Shorthand for `{ method: 'post', path, ...opts }`. Bodyful. */
export const post = verbShorthand('post') as unknown as VerbFactory<'bodyful'>;
/** Shorthand for `{ method: 'put', path, ...opts }`. Bodyful. */
export const put = verbShorthand('put') as unknown as VerbFactory<'bodyful'>;
/** Shorthand for `{ method: 'delete', path, ...opts }`. Bodyful. */
export const del = verbShorthand('delete') as unknown as VerbFactory<'bodyful'>;
/** Shorthand for `{ method: 'patch', path, ...opts }`. Bodyful. */
export const patch = verbShorthand('patch') as unknown as VerbFactory<'bodyful'>;
/** Shorthand for `{ method: 'options', path, ...opts }`. Bodyful. */
export const options = verbShorthand('options') as unknown as VerbFactory<'bodyful'>;
/** Shorthand for `{ method: 'connect', path, ...opts }`. Bodyful. */
export const connect = verbShorthand('connect') as unknown as VerbFactory<'bodyful'>;
/** Shorthand for `{ method: 'trace', path, ...opts }`. Bodyful. */
export const trace = verbShorthand('trace') as unknown as VerbFactory<'bodyful'>;

/** Overloaded factory shape for {@link custom} (always bodyful). */
export interface CustomFactory {
  <
    const P extends string,
    H extends Readonly<Record<string, string>> = NoHeaderBindings,
    Q extends Readonly<Record<string, string>> = NoHeaderBindings,
  >(
    verb: string,
    path: ValidEndpointPath<P>,
    opts?: EndpointOptsFor<H, Q>,
  ): EndpointSpecFor<P, H, Q, 'bodyful'>;
  (verb: string, path: readonly PathSegment[], opts?: EndpointSugarOpts): HttpEndpointSpec;
}

/** Shorthand for a custom (non-standard) verb. Always bodyful. */
export const custom = ((
  verb: string,
  path: PathInput,
  opts: EndpointSugarOpts = {},
): HttpEndpointSpec =>
  ({ method: { custom: verb }, path, ...opts }) as HttpEndpointSpec) as unknown as CustomFactory;

/** Options accepted by {@link mount}. */
export interface MountOptionsFor<W extends string = string> {
  /** When `true`, the host treats every endpoint as authentication-required. */
  readonly auth?: boolean;
  /** CORS allowed-origin patterns advertised at the mount level. */
  readonly cors?: readonly string[];
  /** Mark this agent as a phantom agent (one fresh instance per HTTP request). */
  readonly phantomAgent?: boolean;
  /**
   * Optional custom webhook suffix path. Validated with the same
   * {@link ValidMountPath} rules as the mount path; a non-literal `string`
   * (default `W = string`) reduces the constraint to plain `string` and is
   * enforced by the runtime parser.
   */
  readonly webhookSuffix?: string extends W ? string : ValidMountPath<W>;
}

/** Escape-hatch (segment-array) form options for {@link mount}. */
type MountSugarOpts = MountOptionsFor<string> & { readonly webhookSuffix?: PathInput };

/** Overloaded factory shape for {@link mount}. */
export interface MountFactory {
  <const P extends string, const W extends string = string>(
    path: ValidMountPath<P>,
    opts?: MountOptionsFor<W>,
  ): HttpMountSpec<
    Exclude<PathVarsOf<P>, SystemVariableName>,
    Exclude<PathVarsOf<W>, SystemVariableName>
  >;
  (path: readonly PathSegment[], opts?: MountSugarOpts): HttpMountSpec;
}

/**
 * Declare an HTTP mount for an agent. Convenience constructor; equivalent to
 * writing the `HttpMountSpec` object literal directly. The template-string form
 * binds the path `{var}` names into the `MountVars` phantom (and any
 * `webhookSuffix` `{var}` names into `WebhookVars`) so `defineAgent` can check
 * coverage against the agent id.
 */
export const mount = ((path: PathInput, opts: MountSugarOpts = {}): HttpMountSpec =>
  ({ path, ...opts }) as HttpMountSpec) as unknown as MountFactory;

// ---------------------------------------------------------------------------
// Pipeable-free combinators — additive copies (input never mutated).
// ---------------------------------------------------------------------------

/** Return a copy of a mount/endpoint spec with `auth` set. */
export function withAuth<T extends HttpMountSpec | HttpEndpointSpec>(spec: T, auth: boolean): T {
  return { ...spec, auth };
}

/** Return a copy of a mount/endpoint spec with the CORS pattern list replaced. */
export function withCors<T extends HttpMountSpec | HttpEndpointSpec>(
  spec: T,
  ...patterns: string[]
): T {
  return { ...spec, cors: patterns };
}

/** Return a copy of a mount spec with the webhook suffix set. */
export function withWebhookSuffix(spec: HttpMountSpec, webhookSuffix: PathInput): HttpMountSpec {
  return { ...spec, webhookSuffix };
}

// ---------------------------------------------------------------------------
// Compilation: spec → WIT records
// ---------------------------------------------------------------------------

/** Compile an {@link HttpMountSpec} to the WIT `http-mount-details` record. */
export function compileMount(spec: HttpMountSpec): HttpMountDetails {
  const pathPrefix = resolvePath(spec.path, 'mount');
  return {
    pathPrefix,
    authDetails: spec.auth ? { required: true } : { required: false },
    phantomAgent: spec.phantomAgent ?? false,
    corsOptions: { allowedPatterns: spec.cors ? [...spec.cors] : [] },
    webhookSuffix: spec.webhookSuffix ? resolvePath(spec.webhookSuffix, 'webhook suffix') : [],
  };
}

/**
 * Split a template `path` into its path portion and inline query (`?…`) before
 * parsing, so a template like `/add?by={by}` yields both path segments and
 * query bindings — matching the decorator endpoint behaviour.
 */
function resolveEndpointPath(
  path: PathInput,
  entityName: string,
): {
  pathSuffix: PathSegment[];
  inlineQuery: ReturnType<typeof parseQuery>;
} {
  if (typeof path !== 'string') {
    return { pathSuffix: [...path], inlineQuery: [] };
  }
  const qIdx = path.indexOf('?');
  const pathPart = qIdx < 0 ? path : path.slice(0, qIdx);
  const queryPart = qIdx < 0 ? '' : path.slice(qIdx + 1);
  rejectEmptyString(pathPart, entityName);
  return {
    pathSuffix: parsePath(pathPart),
    inlineQuery: queryPart ? parseQuery(queryPart) : [],
  };
}

/** Compile a single {@link HttpEndpointSpec} to a WIT `http-endpoint-details`. */
export function compileEndpoint(spec: HttpEndpointSpec): HttpEndpointDetails {
  const { pathSuffix, inlineQuery } = resolveEndpointPath(spec.path, 'endpoint');

  const explicitQuery = spec.query
    ? Object.entries(spec.query).map(([queryParamName, variableName]) => ({
        queryParamName,
        variableName,
      }))
    : [];

  const headerVars = spec.headers
    ? Object.entries(spec.headers).map(([headerName, variableName]) => ({
        headerName,
        variableName,
      }))
    : [];

  return {
    httpMethod: verbToWit(spec.method),
    pathSuffix,
    headerVars,
    queryVars: [...inlineQuery, ...explicitQuery],
    authDetails: spec.auth === undefined ? undefined : { required: spec.auth },
    corsOptions: { allowedPatterns: spec.cors ? [...spec.cors] : [] },
  };
}

/** Collect the `path-variable` names referenced by a list of path segments. */
export function pathVariableNames(segments: readonly PathSegment[]): Set<string> {
  const names = new Set<string>();
  for (const s of segments) {
    if (s.tag === 'path-variable' || s.tag === 'remaining-path-variable') {
      names.add(s.val.variableName);
    }
  }
  return names;
}
