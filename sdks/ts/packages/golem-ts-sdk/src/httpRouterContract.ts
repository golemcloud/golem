// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import type { FileMapping, HttpEndpointDetails, HttpMountDetails } from 'golem:agent/common@2.0.0';

/** A canonical field occurrence. Values are bytes, not UTF-8 text. */
export interface HttpHeader {
  readonly name: string;
  readonly value: Uint8Array;
}

/** The original public request; undefined query differs from an empty query. */
export interface HttpRequest<Body> {
  readonly method: string;
  readonly scheme: string;
  readonly authority: string;
  readonly path: string;
  readonly query: string | undefined;
  readonly headers: readonly HttpHeader[];
  readonly body: Body;
}

/** Canonical response. Body ownership is supplied by the language runtime. */
export interface HttpResponse<Body> {
  readonly status: number;
  readonly headers: readonly HttpHeader[];
  readonly body: Body;
}

export interface FileExposure {
  readonly route: string;
  readonly path: string;
}

/** Safe diagnostics never contain document, header, or body contents. */
export class HttpRouterError extends TypeError {
  constructor(readonly category: string) {
    super(category);
    this.name = 'HttpRouterError';
  }
}

function fail(category: string): never {
  throw new HttpRouterError(category);
}

function validUnicode(value: string): boolean {
  for (const char of value) {
    const code = char.codePointAt(0)!;
    if (code >= 0xd800 && code <= 0xdfff) return false;
  }
  return true;
}

function validSegment(value: string): boolean {
  return (
    value !== '' &&
    value !== '.' &&
    value !== '..' &&
    !/[\x00-\x1f\x7f/\\]/.test(value) &&
    validUnicode(value)
  );
}

function publicSegments(path: string): string[] {
  if (path === '/') return [];
  if (!path.startsWith('/') || /[$*?#]/.test(path)) fail('source-path');
  return path
    .slice(1)
    .split('/')
    .map((raw) => {
      if (!/^(?:[A-Za-z0-9\-._~!&'()+,;=:@]|%[A-Fa-f0-9]{2})+$/.test(raw)) fail('source-path');
      let segment: string;
      try {
        segment = decodeURIComponent(raw);
      } catch {
        return fail('source-path');
      }
      if (!validSegment(segment)) fail('source-path');
      return segment;
    });
}

/** Compile once to WIT mappings. Only identical compiled pairs are duplicates. */
export function compileFileMappings(mappings: readonly FileExposure[]): FileMapping[] {
  const seen = new Set<string>();
  return mappings.map(({ route, path }) => {
    const subtree = route.endsWith('/*');
    const source = subtree ? route.slice(0, -2) || '/' : route;
    if (source.includes('*')) fail('source-wildcard');
    if (subtree && source !== '/' && source.endsWith('/')) fail('source-path');
    // `//*` is not the root wildcard spelling.
    if (subtree && route !== '/*' && source === '/') fail('source-path');
    const segments = publicSegments(source);
    if (subtree && !path.endsWith('/$1')) fail('target-placeholder');
    const target = subtree ? path.slice(0, -3) || '/' : path;
    if (target.includes('$')) fail('target-placeholder');
    if (
      !target.startsWith('/') ||
      (!subtree && target === '/') ||
      /[\x00-\x1f\x7f-\x9f]/.test(target) ||
      !validUnicode(target) ||
      (target !== '/' && !target.slice(1).split('/').every(validSegment)) ||
      (subtree && path !== '/$1' && target === '/')
    )
      fail('target-path');
    const mapping: FileMapping = subtree
      ? { tag: 'subtree', val: { publicPrefix: segments, filesystemRoot: target } }
      : { tag: 'exact', val: { publicPath: segments, filePath: target } };
    const key = JSON.stringify(mapping);
    if (seen.has(key)) fail('duplicate-mapping');
    seen.add(key);
    return mapping;
  });
}

export interface RouterMountOptions {
  readonly mount: string;
  readonly auth?: boolean;
  readonly cors?: readonly string[];
  readonly staticBindings?: readonly FileMapping[];
  readonly handlerMethod?: string;
  readonly openapiProviderMethod?: string;
}

/** Framework-neutral mount/role metadata; schemas and registration stay with the SDK. */
export function compileRouterMount(options: RouterMountOptions): {
  mount: HttpMountDetails;
  handlerBinding: HttpEndpointDetails | undefined;
} {
  const { handlerMethod, openapiProviderMethod } = options;
  if (
    handlerMethod === '' ||
    openapiProviderMethod === '' ||
    (handlerMethod !== undefined && handlerMethod === openapiProviderMethod)
  )
    fail('router-methods');
  const pathPrefix = publicSegments(options.mount).map((val) => ({ tag: 'literal' as const, val }));
  return {
    mount: {
      pathPrefix,
      authDetails: { required: options.auth ?? false },
      corsOptions: { allowedPatterns: [...(options.cors ?? [])] },
      phantomAgent: false,
      webhookSuffix: [],
      staticBindings: [...(options.staticBindings ?? [])],
      filesystemBindings: [],
      openapiProviderMethod,
    },
    handlerBinding:
      handlerMethod === undefined
        ? undefined
        : {
            httpMethod: { tag: 'any' },
            pathSuffix: [],
            headerVars: [],
            queryVars: [],
            authDetails: undefined,
            corsOptions: { allowedPatterns: [] },
          },
  };
}

/** Validate and copy canonical headers without normalizing away occurrences. */
export function copyHttpHeaders(headers: readonly HttpHeader[]): HttpHeader[] {
  return headers.map(({ name, value }) => {
    if (
      !/^[!#$%&'*+.^_`|~0-9a-z-]+$/.test(name) ||
      !(value instanceof Uint8Array) ||
      value.some((byte) => byte === 127 || (byte < 32 && byte !== 9))
    )
      fail('invalid-header');
    return { name, value: value.slice() };
  });
}

/** OpenAPI 3.1 provider input. Full semantic validation/merging belongs to the host. */
export function serializeOpenApi(document: unknown): string {
  if (document === null || typeof document !== 'object' || Array.isArray(document))
    fail('openapi-document');
  const doc = document as Record<string, unknown>;
  if (
    doc.openapi !== '3.1.0' ||
    !doc.info ||
    typeof doc.info !== 'object' ||
    !doc.paths ||
    typeof doc.paths !== 'object' ||
    Array.isArray(doc.paths)
  )
    fail('openapi-document');
  const allowed = new Set([
    'openapi',
    'info',
    'paths',
    'components',
    'tags',
    'security',
    'servers',
    'externalDocs',
  ]);
  if (Object.keys(doc).some((key) => !allowed.has(key) && !key.startsWith('x-')))
    fail('openapi-section');
  const limit = 1024 * 1024;
  let bytes = 0;
  const pieces: string[] = [];
  const ancestors = new Set<object>();
  const encoder = new TextEncoder();
  function write(text: string) {
    bytes += encoder.encode(text).byteLength;
    if (bytes > limit) fail('openapi-size');
    pieces.push(text);
  }
  function string(value: string) {
    if (value.length > limit) fail('openapi-size');
    if (!validUnicode(value)) fail('openapi-unicode');
    write(JSON.stringify(value));
  }
  function visit(value: unknown, depth: number): void {
    if (value === null) return write('null');
    switch (typeof value) {
      case 'string':
        return string(value);
      case 'boolean':
        return write(String(value));
      case 'number':
        if (!Number.isFinite(value)) fail('openapi-json');
        return write(JSON.stringify(value));
      case 'object':
        break;
      default:
        return fail('openapi-json');
    }
    if (depth > 64) fail('openapi-depth');
    if (ancestors.has(value)) fail('openapi-cycle');
    if (
      !Array.isArray(value) &&
      Object.getPrototypeOf(value) !== Object.prototype &&
      Object.getPrototypeOf(value) !== null
    )
      fail('openapi-json');
    ancestors.add(value);
    if (Array.isArray(value)) {
      write('[');
      for (let i = 0; i < value.length; i++) {
        if (i) write(',');
        visit(value[i], depth + 1);
      }
      write(']');
    } else {
      write('{');
      const keys = Object.keys(value).sort(compareUnicode);
      keys.forEach((key, i) => {
        if (i) write(',');
        string(key);
        write(':');
        visit((value as Record<string, unknown>)[key], depth + 1);
      });
      write('}');
    }
    ancestors.delete(value);
  }
  visit(document, 1);
  return pieces.join('');
}

function compareUnicode(a: string, b: string): number {
  const left = Array.from(a),
    right = Array.from(b);
  for (let i = 0; i < Math.min(left.length, right.length); i++) {
    const delta = left[i].codePointAt(0)! - right[i].codePointAt(0)!;
    if (delta) return delta;
  }
  return left.length - right.length;
}
