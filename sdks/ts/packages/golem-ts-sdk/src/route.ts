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

import { PathSegment } from 'golem:agent/common@2.0.0';
import { parsePath } from './internal/http/path';
import { parseQuery } from './internal/http/query';

/**
 * Options controlling how a routed URL is rendered.
 */
export interface RouteOptions {
  /**
   * Base URL of the Golem deployment, e.g. `"mygolemhost.com"`.
   * May include a scheme (`https://mygolemhost.com`), in which case {@link scheme}
   * is ignored. When omitted, the returned URL is an absolute path
   * (e.g. `/assets/myasset`).
   */
  baseUrl?: string;
  /**
   * Scheme used when {@link baseUrl} does not already include one.
   * Defaults to `'https'`.
   */
  scheme?: 'http' | 'https';
}

/**
 * Values substituted into an endpoint path template's `{var}` placeholders,
 * for both path segments and inline query parameters.
 */
export type RouteArgs = Record<string, string | number | boolean>;

function splitTemplate(template: string): { pathPart: string; queryPart: string } {
  const qIdx = template.indexOf('?');
  return {
    pathPart: qIdx < 0 ? template : template.slice(0, qIdx),
    queryPart: qIdx < 0 ? '' : template.slice(qIdx + 1),
  };
}

function encodePathValue(value: string, allowSlashes: boolean): string {
  const encoded = encodeURIComponent(value);
  // A `{*rest}` catch-all segment legitimately spans multiple path segments,
  // so its separators must survive encoding.
  return allowSlashes ? encoded.replace(/%2F/gi, '/') : encoded;
}

/**
 * Builds a URL for an HTTP endpoint declared via `method({ http: ... })`,
 * substituting named arguments into the endpoint's `{var}` path segments and
 * inline query parameters.
 *
 * Mirrors the template syntax accepted by the HTTP routing surface:
 *
 * ```ts
 * import { route } from '@golemcloud/golem-ts-sdk';
 *
 * // Fully qualified URL against a deployment host:
 * route('/assets/{assetName}', { assetName: 'logo.png' }, { baseUrl: 'mygolemhost.com' })
 * // → 'https://mygolemhost.com/assets/logo.png'
 *
 * // Absolute path only:
 * route('/assets/{assetName}', { assetName: 'logo.png' })
 * // → '/assets/logo.png'
 *
 * // Inline query parameters:
 * route('/search?q={term}&page={page}', { term: 'hello world', page: 2 })
 * // → '/search?q=hello%20world&page=2'
 * ```
 *
 * Every `{var}` placeholder in the template (path or query) must have a
 * corresponding entry in `args`; conversely, unknown entries are rejected.
 * Values are percent-encoded; a `{*rest}` catch-all variable preserves `/`
 * separators so multi-segment paths remain valid.
 *
 * Note that the template is an endpoint path relative to the agent's HTTP
 * mount. If the mount prefix contains variables (including the system
 * variables `{agent-type}` / `{agent-version}`), pass their values as regular
 * entries in `args` and include the prefix in the template.
 *
 * @param template - Endpoint path template, e.g. `/assets/{assetName}` or
 *   `/add?by={by}`.
 * @param args - Named values for every variable referenced by the template.
 * @param options - Optional base URL / scheme controls.
 * @returns The rendered URL string.
 * @throws If the template is malformed or the arguments do not match it.
 */
export function route(template: string, args: RouteArgs = {}, options: RouteOptions = {}): string {
  const { pathPart, queryPart } = splitTemplate(template);

  const segments: PathSegment[] = parsePath(pathPart);
  const queryVars = queryPart ? parseQuery(queryPart) : [];

  const missing: string[] = [];
  const consume = (variableName: string): string => {
    const value = args[variableName];
    if (value === undefined) {
      missing.push(variableName);
      return '';
    }
    return String(value);
  };

  let path = '';
  for (const segment of segments) {
    switch (segment.tag) {
      case 'literal':
        path += `/${segment.val}`;
        break;
      case 'path-variable':
        path += `/${encodePathValue(consume(segment.val.variableName), false)}`;
        break;
      case 'remaining-path-variable':
        path += `/${encodePathValue(consume(segment.val.variableName), true)}`;
        break;
      case 'system-variable':
        path += `/${encodePathValue(consume(segment.val), false)}`;
        break;
    }
  }

  let query = '';
  for (const [index, { queryParamName, variableName }] of queryVars.entries()) {
    const separator = index === 0 ? '?' : '&';
    query += `${separator}${encodeURIComponent(queryParamName)}=${encodeURIComponent(consume(variableName))}`;
  }

  if (missing.length > 0) {
    throw new Error(
      `Missing value(s) for route variable(s): ${missing.map((n) => `"${n}"`).join(', ')}`,
    );
  }

  const known = new Set<string>();
  for (const segment of segments) {
    if (segment.tag === 'path-variable' || segment.tag === 'remaining-path-variable') {
      known.add(segment.val.variableName);
    } else if (segment.tag === 'system-variable') {
      known.add(segment.val);
    }
  }
  for (const { variableName } of queryVars) {
    known.add(variableName);
  }
  for (const name of Object.keys(args)) {
    if (!known.has(name)) {
      throw new Error(`Unknown route argument "${name}"`);
    }
  }

  const baseUrl = options.baseUrl?.trim();
  if (!baseUrl) {
    return path + query;
  }

  const stripped = baseUrl.replace(/\/+$/, '');
  const prefixed = /^[a-zA-Z][a-zA-Z\d+\-.]*:\/\//.test(stripped)
    ? stripped
    : `${options.scheme ?? 'https'}://${stripped}`;

  return `${prefixed}${path}${query}`;
}
