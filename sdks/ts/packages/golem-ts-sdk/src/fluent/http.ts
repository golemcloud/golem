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
// `method({ http: ... })`. This module mirrors the de-Effect-ified shape of
// effect-golem's `Http.ts` (`mount`, verb shorthands `get`/`post`/..., path
// builders `literal`/`pathVar`/`restVar`, `withAuth`/`withCors`/
// `withWebhookSuffix`, `compileMount`/`compileEndpoint`) but stays plain data —
// no Effect runtime.
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
// ---------------------------------------------------------------------------

/**
 * Mount declaration carried by `defineAgent({ http })`. Compiled to the WIT
 * `agent-type.http-mount` (`http-mount-details`) at registration time via
 * {@link compileMount}.
 */
export interface HttpMountSpec {
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
 */
export interface HttpEndpointSpec {
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
// ---------------------------------------------------------------------------

type EndpointSugarOpts = Omit<HttpEndpointSpec, 'method' | 'path'>;

const verbShorthand =
  (method: HttpVerb) =>
  (path: PathInput, opts: EndpointSugarOpts = {}): HttpEndpointSpec => ({ method, path, ...opts });

/** Shorthand for `{ method: 'get', path, ...opts }`. */
export const get = verbShorthand('get');
/** Shorthand for `{ method: 'head', path, ...opts }`. */
export const head = verbShorthand('head');
/** Shorthand for `{ method: 'post', path, ...opts }`. */
export const post = verbShorthand('post');
/** Shorthand for `{ method: 'put', path, ...opts }`. */
export const put = verbShorthand('put');
/** Shorthand for `{ method: 'delete', path, ...opts }`. */
export const del = verbShorthand('delete');
/** Shorthand for `{ method: 'patch', path, ...opts }`. */
export const patch = verbShorthand('patch');
/** Shorthand for `{ method: 'options', path, ...opts }`. */
export const options = verbShorthand('options');
/** Shorthand for `{ method: 'connect', path, ...opts }`. */
export const connect = verbShorthand('connect');
/** Shorthand for `{ method: 'trace', path, ...opts }`. */
export const trace = verbShorthand('trace');
/** Shorthand for a custom (non-standard) verb. */
export const custom = (verb: string, path: PathInput, opts: EndpointSugarOpts = {}): HttpEndpointSpec => ({
  method: { custom: verb },
  path,
  ...opts,
});

/**
 * Declare an HTTP mount for an agent. Convenience constructor mirroring
 * effect-golem's `Http.mount`; equivalent to writing the `HttpMountSpec`
 * object literal directly.
 */
export const mount = (path: PathInput, opts: Omit<HttpMountSpec, 'path'> = {}): HttpMountSpec => ({
  path,
  ...opts,
});

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
function resolveEndpointPath(path: PathInput, entityName: string): {
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
    ? Object.entries(spec.headers).map(([headerName, variableName]) => ({ headerName, variableName }))
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
