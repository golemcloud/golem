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

import type {
  SchemaGraph,
  SchemaType,
  SchemaTypeBody,
  SchemaValue,
} from '../internal/schema-model';
import { datetimeFromISOString, datetimeToISOString } from '../bridge/schema';
import { SchemaRenderError, type JsonValue } from './ref';

type Path = readonly (string | number)[];

function fail(path: Path, message: string): never {
  throw new SchemaRenderError(message, path);
}

function resolve(graph: SchemaGraph, type: SchemaType, seen = new Set<string>()): SchemaType {
  if (type.body.tag !== 'ref') return type;
  if (seen.has(type.body.id))
    throw new SchemaRenderError(`reference cycle through '${type.body.id}'`);
  const definition = graph.defs.get(type.body.id);
  if (!definition) throw new SchemaRenderError(`dangling reference '${type.body.id}'`);
  seen.add(type.body.id);
  return resolve(graph, definition.body, seen);
}

export function fromCanonicalJson(
  graph: SchemaGraph,
  type: SchemaType,
  json: JsonValue,
  path: Path = [],
): SchemaValue {
  const body = resolve(graph, type).body;
  switch (body.tag) {
    case 'bool':
      return { tag: 'bool', value: expectBoolean(json, path) };
    case 's8':
      return { tag: 's8', value: expectInteger(json, path, -128, 127) };
    case 's16':
      return { tag: 's16', value: expectInteger(json, path, -32768, 32767) };
    case 's32':
      return { tag: 's32', value: expectInteger(json, path, -(2 ** 31), 2 ** 31 - 1) };
    case 'u8':
      return { tag: 'u8', value: expectInteger(json, path, 0, 255) };
    case 'u16':
      return { tag: 'u16', value: expectInteger(json, path, 0, 65535) };
    case 'u32':
      return { tag: 'u32', value: expectInteger(json, path, 0, 2 ** 32 - 1) };
    case 's64':
      return { tag: 's64', value: BigInt(expectSafeInteger(json, path)) };
    case 'u64': {
      const value = expectSafeInteger(json, path);
      if (value < 0) fail(path, 'expected an unsigned integer');
      return { tag: 'u64', value: BigInt(value) };
    }
    case 'f32':
      return { tag: 'f32', value: Math.fround(expectNumber(json, path)) };
    case 'f64':
      return { tag: 'f64', value: expectNumber(json, path) };
    case 'char': {
      const value = expectString(json, path);
      if ([...value].length !== 1) fail(path, 'expected one Unicode scalar');
      return { tag: 'char', value };
    }
    case 'string':
      return { tag: 'string', value: expectString(json, path) };
    case 'text':
      return decodeText(json, path);
    case 'binary':
      return decodeBinary(json, path);
    case 'path':
      return { tag: 'path', value: expectNonEmptyString(json, path) };
    case 'url':
      return { tag: 'url', value: expectNonEmptyString(json, path) };
    case 'datetime':
      return { tag: 'datetime', value: datetimeFromISOString(expectString(json, path)) };
    case 'duration':
      return { tag: 'duration', nanoseconds: decodeDuration(json, path) };
    case 'quantity':
      return decodeQuantity(json, path);
    case 'record': {
      const object = expectObject(json, path);
      const expected = new Set(body.fields.map((field) => field.name));
      for (const key of Object.keys(object))
        if (!expected.has(key)) fail([...path, key], 'unknown field');
      return {
        tag: 'record',
        fields: body.fields.map((field) => {
          if (!(field.name in object)) fail([...path, field.name], 'missing field');
          return fromCanonicalJson(graph, field.body, object[field.name], [...path, field.name]);
        }),
      };
    }
    case 'variant': {
      if (typeof json === 'string') {
        const caseIndex = body.cases.findIndex((entry) => entry.name === json && !entry.payload);
        if (caseIndex < 0) fail(path, `unknown payload-free variant case '${json}'`);
        return { tag: 'variant', caseIndex };
      }
      const object = expectObject(json, path);
      const entries = Object.entries(object);
      if (entries.length !== 1) fail(path, 'expected a single-key variant object');
      const [name, payload] = entries[0];
      const caseIndex = body.cases.findIndex((entry) => entry.name === name);
      const variantCase = body.cases[caseIndex];
      if (!variantCase?.payload) fail(path, `unknown payload variant case '${name}'`);
      return {
        tag: 'variant',
        caseIndex,
        payload: fromCanonicalJson(graph, variantCase.payload, payload, [...path, name]),
      };
    }
    case 'enum': {
      const name = expectString(json, path);
      const caseIndex = body.cases.indexOf(name);
      if (caseIndex < 0) fail(path, `unknown enum case '${name}'`);
      return { tag: 'enum', caseIndex };
    }
    case 'flags': {
      const selected = expectArray(json, path).map((entry, index) =>
        expectString(entry, [...path, index]),
      );
      const seen = new Set<string>();
      selected.forEach((name, index) => {
        if (!body.names.includes(name)) fail([...path, index], `unknown flag '${name}'`);
        if (seen.has(name)) fail([...path, index], `duplicate flag '${name}'`);
        seen.add(name);
      });
      return { tag: 'flags', flags: body.names.map((name) => selected.includes(name)) };
    }
    case 'tuple': {
      const values = expectArray(json, path);
      if (values.length !== body.elements.length)
        fail(path, `expected ${body.elements.length} tuple elements`);
      return {
        tag: 'tuple',
        elements: body.elements.map((entry, index) =>
          fromCanonicalJson(graph, entry, values[index], [...path, index]),
        ),
      };
    }
    case 'list':
    case 'fixed-list': {
      const values = expectArray(json, path);
      if (body.tag === 'fixed-list' && values.length !== body.length)
        fail(path, `expected ${body.length} elements`);
      const elements = values.map((entry, index) =>
        fromCanonicalJson(graph, body.element, entry, [...path, index]),
      );
      return body.tag === 'list' ? { tag: 'list', elements } : { tag: 'fixed-list', elements };
    }
    case 'map': {
      const entries = expectArray(json, path).map((entry, index) => {
        const pair = expectArray(entry, [...path, index]);
        if (pair.length !== 2) fail([...path, index], 'expected a two-element map entry');
        return {
          key: fromCanonicalJson(graph, body.key, pair[0], [...path, index, 0]),
          value: fromCanonicalJson(graph, body.value, pair[1], [...path, index, 1]),
        };
      });
      return { tag: 'map', entries };
    }
    case 'option':
      return json === null
        ? { tag: 'option' }
        : { tag: 'option', value: fromCanonicalJson(graph, body.element, json, path) };
    case 'result': {
      const object = expectObject(json, path);
      const keys = Object.keys(object);
      if (keys.length !== 1 || (keys[0] !== 'ok' && keys[0] !== 'err'))
        fail(path, "expected {'ok': ...} or {'err': ...}");
      const tag = keys[0] as 'ok' | 'err';
      const expected = tag === 'ok' ? body.ok : body.err;
      const payload = object[tag];
      if (!expected) {
        if (payload !== null) fail([...path, tag], 'expected null unit payload');
        return { tag: 'result', result: { tag } };
      }
      return {
        tag: 'result',
        result: { tag, value: fromCanonicalJson(graph, expected, payload, [...path, tag]) },
      };
    }
    case 'union': {
      const matches = body.branches.filter((branch) =>
        discriminatorMatches(branch.discriminator, json),
      );
      if (matches.length !== 1)
        fail(path, `expected exactly one matching union branch, found ${matches.length}`);
      return {
        tag: 'union',
        unionTag: matches[0].tag,
        body: fromCanonicalJson(graph, matches[0].body, json, path),
      };
    }
    case 'secret':
    case 'quota-token':
    case 'permission-card':
      fail(path, `${body.tag} values cannot be constructed from JSON`);
    case 'future':
    case 'stream':
      fail(path, `${body.tag} values have no JSON representation`);
    case 'ref':
      throw new Error('unreachable');
  }
}

export function toCanonicalJson(
  graph: SchemaGraph,
  type: SchemaType,
  value: SchemaValue,
  path: Path = [],
): JsonValue {
  const body = resolve(graph, type).body;
  if (body.tag === 'secret' || body.tag === 'quota-token' || body.tag === 'permission-card')
    fail(path, `${body.tag} values cannot be exposed as JSON`);
  if (body.tag === 'future' || body.tag === 'stream')
    fail(path, `${body.tag} values have no JSON representation`);
  if (body.tag !== value.tag && !(body.tag === 'ref'))
    fail(path, `expected ${body.tag} schema value, found ${value.tag}`);
  switch (value.tag) {
    case 'bool':
    case 's8':
    case 's16':
    case 's32':
    case 'u8':
    case 'u16':
    case 'u32':
    case 'f32':
    case 'f64':
      return value.value;
    case 's64':
    case 'u64': {
      const number = Number(value.value);
      if (!Number.isSafeInteger(number))
        fail(path, '64-bit integer cannot be represented losslessly as a JavaScript JSON number');
      return number;
    }
    case 'char':
    case 'string':
    case 'path':
    case 'url':
      return value.value;
    case 'text':
      return {
        text: value.text,
        ...(value.language === undefined ? {} : { language: value.language }),
      };
    case 'binary':
      return {
        bytes: bytesToBase64(value.bytes),
        ...(value.mimeType === undefined ? {} : { mimeType: value.mimeType }),
      };
    case 'datetime':
      return datetimeToISOString(value.value);
    case 'duration':
      return encodeDuration(value.nanoseconds);
    case 'quantity':
      return {
        mantissa: bigintToSafeJsonNumber(value.value.mantissa, path, 'quantity mantissa'),
        scale: value.value.scale,
        unit: value.value.unit,
      };
    case 'record': {
      const fields = (body as Extract<SchemaTypeBody, { tag: 'record' }>).fields;
      if (value.fields.length !== fields.length)
        fail(path, `expected ${fields.length} record fields, found ${value.fields.length}`);
      return Object.fromEntries(
        fields.map((field, index) => [
          field.name,
          toCanonicalJson(graph, field.body, value.fields[index], [...path, field.name]),
        ]),
      );
    }
    case 'variant': {
      const entry = (body as Extract<SchemaTypeBody, { tag: 'variant' }>).cases[value.caseIndex];
      if (!entry) fail(path, `variant case index ${value.caseIndex} is out of range`);
      return value.payload === undefined
        ? entry.name
        : {
            [entry.name]: toCanonicalJson(graph, entry.payload!, value.payload, [
              ...path,
              entry.name,
            ]),
          };
    }
    case 'enum':
      return (
        (body as Extract<SchemaTypeBody, { tag: 'enum' }>).cases[value.caseIndex] ??
        fail(path, 'enum case index is out of range')
      );
    case 'flags': {
      const names = (body as Extract<SchemaTypeBody, { tag: 'flags' }>).names;
      if (value.flags.length !== names.length)
        fail(path, `expected ${names.length} flag values, found ${value.flags.length}`);
      return names.filter((_, index) => value.flags[index]);
    }
    case 'tuple': {
      const elements = (body as Extract<SchemaTypeBody, { tag: 'tuple' }>).elements;
      if (value.elements.length !== elements.length)
        fail(path, `expected ${elements.length} tuple elements, found ${value.elements.length}`);
      return value.elements.map((entry, index) =>
        toCanonicalJson(graph, elements[index], entry, [...path, index]),
      );
    }
    case 'list':
    case 'fixed-list':
      if (body.tag === 'fixed-list' && value.elements.length !== body.length)
        fail(path, `expected ${body.length} elements, found ${value.elements.length}`);
      return value.elements.map((entry, index) =>
        toCanonicalJson(
          graph,
          (body as Extract<SchemaTypeBody, { tag: 'list' | 'fixed-list' }>).element,
          entry,
          [...path, index],
        ),
      );
    case 'map':
      return value.entries.map((entry, index) => [
        toCanonicalJson(graph, (body as Extract<SchemaTypeBody, { tag: 'map' }>).key, entry.key, [
          ...path,
          index,
          0,
        ]),
        toCanonicalJson(
          graph,
          (body as Extract<SchemaTypeBody, { tag: 'map' }>).value,
          entry.value,
          [...path, index, 1],
        ),
      ]);
    case 'option':
      return value.value === undefined
        ? null
        : toCanonicalJson(
            graph,
            (body as Extract<SchemaTypeBody, { tag: 'option' }>).element,
            value.value,
            path,
          );
    case 'result': {
      const expected =
        value.result.tag === 'ok'
          ? (body as Extract<SchemaTypeBody, { tag: 'result' }>).ok
          : (body as Extract<SchemaTypeBody, { tag: 'result' }>).err;
      return {
        [value.result.tag]:
          value.result.value === undefined
            ? null
            : toCanonicalJson(graph, expected!, value.result.value, [...path, value.result.tag]),
      };
    }
    case 'union':
      return toCanonicalJson(
        graph,
        (body as Extract<SchemaTypeBody, { tag: 'union' }>).branches.find(
          (entry) => entry.tag === value.unionTag,
        )?.body ?? fail(path, `unknown union tag '${value.unionTag}'`),
        value.body,
        path,
      );
    case 'secret':
    case 'quota-token':
    case 'permission-card':
      fail(path, `${value.tag} values cannot be exposed as JSON`);
    case 'stream':
      fail(path, 'stream values have no JSON representation');
  }
}

export function toCanonicalJsonSchema(
  graph: SchemaGraph,
  type: SchemaType,
  includeDraftMarker: boolean,
): JsonValue {
  const root = renderSchema(graph, type);
  const defs = Object.fromEntries(
    [...graph.defs].map(([id, definition]) => {
      const rendered = renderSchema(graph, definition.body);
      return [
        id,
        definition.name === undefined || rendered.title !== undefined
          ? rendered
          : { ...rendered, title: definition.name },
      ];
    }),
  );
  return {
    ...(includeDraftMarker ? { $schema: 'https://json-schema.org/draft/2020-12/schema' } : {}),
    ...root,
    ...(Object.keys(defs).length ? { $defs: defs } : {}),
  };
}

function renderSchema(graph: SchemaGraph, type: SchemaType): Record<string, JsonValue> {
  const body = type.body;
  if (body.tag === 'ref')
    return { $ref: `#/$defs/${body.id.replaceAll('~', '~0').replaceAll('/', '~1')}` };
  let rendered: Record<string, JsonValue>;
  switch (body.tag) {
    case 'bool':
      rendered = { type: 'boolean' };
      break;
    case 's8':
      rendered = integerSchema(-128, 127);
      break;
    case 's16':
      rendered = integerSchema(-32768, 32767);
      break;
    case 's32':
      rendered = integerSchema(-(2 ** 31), 2 ** 31 - 1);
      break;
    case 's64':
      rendered = integerSchema(Number(-(2n ** 63n)), Number(2n ** 63n - 1n));
      break;
    case 'u8':
      rendered = integerSchema(0, 255);
      break;
    case 'u16':
      rendered = integerSchema(0, 65535);
      break;
    case 'u32':
      rendered = integerSchema(0, 2 ** 32 - 1);
      break;
    case 'u64':
      rendered = integerSchema(0, Number(2n ** 64n - 1n));
      break;
    case 'f32':
    case 'f64':
      rendered = { type: 'number' };
      break;
    case 'char':
      rendered = { type: 'string', minLength: 1, maxLength: 1 };
      break;
    case 'string':
      rendered = { type: 'string' };
      break;
    case 'text': {
      const text: Record<string, JsonValue> = { type: 'string' };
      if (body.restrictions.minLength !== undefined) text.minLength = body.restrictions.minLength;
      if (body.restrictions.maxLength !== undefined) text.maxLength = body.restrictions.maxLength;
      if (body.restrictions.regex !== undefined) text.pattern = body.restrictions.regex;
      rendered = {
        type: 'object',
        properties: { text, language: { type: 'string' } },
        required: ['text'],
        additionalProperties: false,
        ...(body.restrictions.languages === undefined
          ? {}
          : { description: `Allowed languages: ${body.restrictions.languages.join(', ')}` }),
      };
      break;
    }
    case 'binary':
      rendered = {
        type: 'object',
        required: ['bytes'],
        properties: {
          bytes: {
            type: 'string',
            contentEncoding: 'base64url',
            ...(body.restrictions.minBytes === undefined
              ? {}
              : { minLength: base64UrlLength(body.restrictions.minBytes) }),
            ...(body.restrictions.maxBytes === undefined
              ? {}
              : { maxLength: base64UrlLength(body.restrictions.maxBytes) }),
          },
          mimeType: { type: 'string', pattern: MIME_TYPE_PATTERN.source },
        },
        additionalProperties: false,
        ...(body.restrictions.mimeTypes === undefined
          ? {}
          : { description: `Allowed MIME types: ${body.restrictions.mimeTypes.join(', ')}` }),
      };
      break;
    case 'path': {
      const direction = body.spec.direction === 'in-out' ? 'inout' : body.spec.direction;
      const descriptions = [
        body.spec.allowedExtensions === undefined
          ? undefined
          : `Allowed extensions: ${body.spec.allowedExtensions.join(', ')}`,
        body.spec.allowedMimeTypes === undefined
          ? undefined
          : `Allowed MIME types: ${body.spec.allowedMimeTypes.join(', ')}`,
      ].filter((entry): entry is string => entry !== undefined);
      rendered = {
        type: 'string',
        format: 'file-path',
        title: `${direction} ${body.spec.kind} path`,
        ...(descriptions.length ? { description: descriptions.join('; ') } : {}),
      };
      break;
    }
    case 'url': {
      const descriptions = [
        body.restrictions.allowedSchemes === undefined
          ? undefined
          : `Allowed schemes: ${body.restrictions.allowedSchemes.join(', ')}`,
        body.restrictions.allowedHosts === undefined
          ? undefined
          : `Allowed hosts: ${body.restrictions.allowedHosts.join(', ')}`,
      ].filter((entry): entry is string => entry !== undefined);
      rendered = {
        type: 'string',
        format: 'uri',
        title: 'URL',
        ...(descriptions.length ? { description: descriptions.join('; ') } : {}),
      };
      break;
    }
    case 'datetime':
      rendered = { type: 'string', format: 'date-time' };
      break;
    case 'duration':
      rendered = { type: 'string', format: 'duration' };
      break;
    case 'quantity':
      rendered = {
        type: 'object',
        required: ['mantissa', 'scale', 'unit'],
        properties: {
          mantissa: { type: 'integer' },
          scale: { type: 'integer' },
          unit: { type: 'string' },
        },
        additionalProperties: false,
        title: `Quantity (${body.spec.baseUnit})`,
      };
      break;
    case 'record':
      rendered = {
        type: 'object',
        properties: Object.fromEntries(
          body.fields.map((field) => [
            field.name,
            attachMetadata(renderSchema(graph, field.body), field.metadata),
          ]),
        ),
        required: body.fields
          .filter((field) => resolve(graph, field.body).body.tag !== 'option')
          .map((field) => field.name),
        additionalProperties: false,
      };
      break;
    case 'variant':
      rendered = {
        oneOf: body.cases.map<JsonValue>((entry) => {
          if (!entry.payload) return { const: entry.name } as JsonValue;
          return {
            type: 'object',
            required: [entry.name],
            properties: { [entry.name]: renderSchema(graph, entry.payload) },
            additionalProperties: false,
          } as JsonValue;
        }),
      };
      break;
    case 'enum':
      rendered = { type: 'string', enum: body.cases };
      break;
    case 'flags':
      rendered = { type: 'array', items: { type: 'string', enum: body.names }, uniqueItems: true };
      break;
    case 'tuple':
      rendered =
        body.elements.length === 0
          ? { type: 'array', minItems: 0, maxItems: 0 }
          : {
              type: 'array',
              prefixItems: body.elements.map((entry) => renderSchema(graph, entry)),
              items: false,
              minItems: body.elements.length,
            };
      break;
    case 'list':
      rendered = { type: 'array', items: renderSchema(graph, body.element) };
      break;
    case 'fixed-list':
      rendered = {
        type: 'array',
        items: renderSchema(graph, body.element),
        minItems: body.length,
        maxItems: body.length,
      };
      break;
    case 'map':
      rendered = {
        type: 'array',
        items: {
          type: 'array',
          prefixItems: [renderSchema(graph, body.key), renderSchema(graph, body.value)],
          items: false,
          minItems: 2,
          maxItems: 2,
        },
      };
      break;
    case 'option':
      rendered = { oneOf: [{ type: 'null' }, renderSchema(graph, body.element)] };
      break;
    case 'result':
      rendered = {
        oneOf: [
          {
            type: 'object',
            required: ['ok'],
            properties: { ok: body.ok ? renderSchema(graph, body.ok) : { type: 'null' } },
            additionalProperties: false,
          },
          {
            type: 'object',
            required: ['err'],
            properties: { err: body.err ? renderSchema(graph, body.err) : { type: 'null' } },
            additionalProperties: false,
          },
        ],
      };
      break;
    case 'union':
      rendered = {
        oneOf: body.branches.map((entry) =>
          applyDiscriminator(renderSchema(graph, entry.body), entry.discriminator),
        ),
      };
      break;
    case 'secret':
    case 'quota-token':
    case 'permission-card':
      rendered = { writeOnly: true, 'x-golem-capability': body.tag };
      break;
    case 'future':
    case 'stream':
      rendered = { type: 'null', description: 'WASI P3 placeholder' };
      break;
  }
  return attachMetadata(rendered, type.metadata);
}

function integerSchema(minimum: number, maximum: number): Record<string, JsonValue> {
  return { type: 'integer', minimum, maximum };
}

function base64UrlLength(bytes: number): number {
  return 4 * Math.floor(bytes / 3) + (bytes % 3 === 0 ? 0 : bytes % 3 === 1 ? 2 : 3);
}

function attachMetadata(
  schema: Record<string, JsonValue>,
  metadata: SchemaType['metadata'],
): Record<string, JsonValue> {
  return {
    ...schema,
    ...(metadata.doc === undefined || schema.description !== undefined
      ? {}
      : { description: metadata.doc }),
    ...(metadata.examples.length === 0 || schema.examples !== undefined
      ? {}
      : { examples: metadata.examples }),
    ...(metadata.deprecated === undefined
      ? {}
      : {
          deprecated: true,
          'x-golem-deprecation-note': metadata.deprecated,
        }),
  };
}

function applyDiscriminator(
  schema: Record<string, JsonValue>,
  rule: { tag: string; val?: unknown },
): Record<string, JsonValue> {
  switch (rule.tag) {
    case 'prefix':
      return { ...schema, pattern: `^${escapeRegex(rule.val as string)}` };
    case 'suffix':
      return { ...schema, pattern: `${escapeRegex(rule.val as string)}$` };
    case 'contains':
      return { ...schema, pattern: escapeRegex(rule.val as string) };
    case 'regex':
      return { ...schema, pattern: rule.val as string };
    case 'field-equals': {
      const field = rule.val as { fieldName: string; literal?: string };
      const properties = (schema.properties ?? {}) as Record<string, JsonValue>;
      const fieldSchema = (properties[field.fieldName] ?? { type: 'string' }) as Record<
        string,
        JsonValue
      >;
      return {
        ...schema,
        required: [
          ...new Set([...((schema.required as string[] | undefined) ?? []), field.fieldName]),
        ],
        ...(field.literal === undefined
          ? {}
          : {
              properties: {
                ...properties,
                [field.fieldName]: { ...fieldSchema, const: field.literal },
              },
            }),
      };
    }
    case 'field-absent':
      return { ...schema, not: { required: [rule.val as string] } };
    default:
      return schema;
  }
}

function escapeRegex(value: string): string {
  return value.replace(/[\\^$.*+?()[\]{}|]/gu, '\\$&');
}

function expectBoolean(value: JsonValue, path: Path): boolean {
  if (typeof value !== 'boolean') fail(path, 'expected a JSON boolean');
  return value;
}
function expectNumber(value: JsonValue, path: Path): number {
  if (typeof value !== 'number' || !Number.isFinite(value))
    fail(path, 'expected a finite JSON number');
  return value;
}
function expectSafeInteger(value: JsonValue, path: Path): number {
  const n = expectNumber(value, path);
  if (!Number.isSafeInteger(n)) fail(path, 'expected a safe JSON integer');
  return n;
}
function expectInteger(value: JsonValue, path: Path, min: number, max: number): number {
  const n = expectSafeInteger(value, path);
  if (n < min || n > max) fail(path, `integer outside ${min}..${max}`);
  return n;
}
function expectString(value: JsonValue, path: Path): string {
  if (typeof value !== 'string') fail(path, 'expected a JSON string');
  return value;
}
function expectNonEmptyString(value: JsonValue, path: Path): string {
  const string = expectString(value, path);
  if (string.length === 0) fail(path, 'expected a non-empty JSON string');
  return string;
}
function expectArray(value: JsonValue, path: Path): JsonValue[] {
  if (!Array.isArray(value)) fail(path, 'expected a JSON array');
  return value as JsonValue[];
}
function expectObject(value: JsonValue, path: Path): Record<string, JsonValue> {
  if (value === null || Array.isArray(value) || typeof value !== 'object')
    fail(path, 'expected a JSON object');
  return value as Record<string, JsonValue>;
}
function decodeText(value: JsonValue, path: Path): SchemaValue {
  const object = expectObject(value, path);
  rejectUnknownFields(object, ['text', 'language'], path);
  return {
    tag: 'text',
    text: expectString(object.text, [...path, 'text']),
    ...(object.language === undefined
      ? {}
      : { language: expectString(object.language, [...path, 'language']) }),
  };
}
function decodeBinary(value: JsonValue, path: Path): SchemaValue {
  const object = expectObject(value, path);
  rejectUnknownFields(object, ['bytes', 'mimeType'], path);
  const mimeType =
    object.mimeType === undefined
      ? undefined
      : expectString(object.mimeType, [...path, 'mimeType']);
  if (mimeType !== undefined && !MIME_TYPE_PATTERN.test(mimeType)) {
    fail([...path, 'mimeType'], 'invalid MIME type');
  }
  return {
    tag: 'binary',
    bytes: base64UrlToBytes(expectString(object.bytes, [...path, 'bytes']), [...path, 'bytes']),
    ...(mimeType === undefined ? {} : { mimeType }),
  };
}
function decodeQuantity(value: JsonValue, path: Path): SchemaValue {
  const object = expectObject(value, path);
  rejectUnknownFields(object, ['mantissa', 'scale', 'unit'], path);
  return {
    tag: 'quantity',
    value: {
      mantissa: BigInt(expectSafeInteger(object.mantissa, [...path, 'mantissa'])),
      scale: expectInteger(object.scale, [...path, 'scale'], -2147483648, 2147483647),
      unit: expectString(object.unit, [...path, 'unit']),
    },
  };
}

const I64_MIN = -(2n ** 63n);
const I64_MAX = 2n ** 63n - 1n;
const NS_PER_SECOND = 1_000_000_000n;
const NS_PER_MINUTE = 60n * NS_PER_SECOND;
const NS_PER_HOUR = 60n * NS_PER_MINUTE;
const NS_PER_DAY = 24n * NS_PER_HOUR;

function encodeDuration(nanoseconds: bigint): string {
  if (nanoseconds === 0n) return 'PT0S';
  const negative = nanoseconds < 0n;
  let remaining = negative ? -nanoseconds : nanoseconds;
  const days = remaining / NS_PER_DAY;
  remaining %= NS_PER_DAY;
  const hours = remaining / NS_PER_HOUR;
  remaining %= NS_PER_HOUR;
  const minutes = remaining / NS_PER_MINUTE;
  remaining %= NS_PER_MINUTE;
  const seconds = remaining / NS_PER_SECOND;
  const nanos = remaining % NS_PER_SECOND;

  let result = negative ? '-P' : 'P';
  if (days !== 0n) result += `${days}D`;
  if (hours !== 0n || minutes !== 0n || seconds !== 0n || nanos !== 0n) {
    result += 'T';
    if (hours !== 0n) result += `${hours}H`;
    if (minutes !== 0n) result += `${minutes}M`;
    if (seconds !== 0n || nanos !== 0n) {
      result += `${seconds}`;
      if (nanos !== 0n) result += `.${nanos.toString().padStart(9, '0').replace(/0+$/u, '')}`;
      result += 'S';
    }
  }
  return result;
}

function decodeDuration(value: JsonValue, path: Path): bigint {
  if (typeof value === 'object' && value !== null && !Array.isArray(value)) {
    rejectUnknownFields(value, ['nanoseconds'], path);
    return checkedI64(BigInt(expectSafeInteger(value.nanoseconds, [...path, 'nanoseconds'])), path);
  }
  const text = expectString(value, path);
  const shorthand = text.match(/^(-?\d+)(ns|us|ms|s)$/u);
  if (shorthand) {
    const factor =
      shorthand[2] === 'ns'
        ? 1n
        : shorthand[2] === 'us'
          ? 1_000n
          : shorthand[2] === 'ms'
            ? 1_000_000n
            : NS_PER_SECOND;
    return checkedI64(BigInt(shorthand[1]) * factor, path);
  }

  const iso = text.match(
    /^(-)?P(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)(?:\.(\d{1,9}))?S)?)?$/u,
  );
  if (
    !iso ||
    (iso[2] === undefined && iso[3] === undefined && iso[4] === undefined && iso[5] === undefined)
  ) {
    fail(path, 'expected an ISO 8601 duration');
  }
  let result =
    BigInt(iso[2] ?? 0) * NS_PER_DAY +
    BigInt(iso[3] ?? 0) * NS_PER_HOUR +
    BigInt(iso[4] ?? 0) * NS_PER_MINUTE +
    BigInt(iso[5] ?? 0) * NS_PER_SECOND +
    BigInt((iso[6] ?? '').padEnd(9, '0') || 0);
  if (iso[1]) result = -result;
  return checkedI64(result, path);
}

function checkedI64(value: bigint, path: Path): bigint {
  if (value < I64_MIN || value > I64_MAX) fail(path, 'duration nanoseconds out of i64 range');
  return value;
}

function discriminatorMatches(rule: { tag: string; val?: unknown }, value: JsonValue): boolean {
  if (rule.tag === 'prefix')
    return typeof value === 'string' && value.startsWith(rule.val as string);
  if (rule.tag === 'suffix') return typeof value === 'string' && value.endsWith(rule.val as string);
  if (rule.tag === 'contains')
    return typeof value === 'string' && value.includes(rule.val as string);
  if (rule.tag === 'regex')
    return typeof value === 'string' && new RegExp(rule.val as string, 'u').test(value);
  if (rule.tag === 'field-equals') {
    const field = rule.val as { fieldName: string; literal?: string };
    return (
      value !== null &&
      !Array.isArray(value) &&
      typeof value === 'object' &&
      field.fieldName in value &&
      (field.literal === undefined ||
        (value as Record<string, JsonValue>)[field.fieldName] === field.literal)
    );
  }
  if (rule.tag === 'field-absent') {
    return (
      value !== null &&
      !Array.isArray(value) &&
      typeof value === 'object' &&
      !((rule.val as string) in value)
    );
  }
  return false;
}
const MIME_TYPE_PATTERN = /^[A-Za-z0-9!#$&^_.+\-]+\/[A-Za-z0-9!#$&^_.+\-]+$/u;

function bytesToBase64(bytes: Uint8Array): string {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';
  let result = '';
  for (let index = 0; index < bytes.length; index += 3) {
    const first = bytes[index];
    const second = bytes[index + 1];
    const third = bytes[index + 2];
    result += alphabet[first >> 2];
    result += alphabet[((first & 3) << 4) | ((second ?? 0) >> 4)];
    if (second !== undefined) result += alphabet[((second & 15) << 2) | ((third ?? 0) >> 6)];
    if (third !== undefined) result += alphabet[third & 63];
  }
  return result;
}
function base64UrlToBytes(value: string, path: Path): Uint8Array {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_';
  if (!/^[A-Za-z0-9_-]*$/u.test(value) || value.length % 4 === 1) {
    fail(path, 'invalid base64url without padding');
  }
  const bytes: number[] = [];
  for (let index = 0; index < value.length; index += 4) {
    const a = alphabet.indexOf(value[index]);
    const b = alphabet.indexOf(value[index + 1]);
    const c = value[index + 2] === undefined ? 0 : alphabet.indexOf(value[index + 2]);
    const d = value[index + 3] === undefined ? 0 : alphabet.indexOf(value[index + 3]);
    bytes.push((a << 2) | (b >> 4));
    if (value[index + 2] !== undefined) bytes.push(((b & 15) << 4) | (c >> 2));
    if (value[index + 3] !== undefined) bytes.push(((c & 3) << 6) | d);
  }
  return Uint8Array.from(bytes);
}

function rejectUnknownFields(
  object: Record<string, JsonValue>,
  allowed: readonly string[],
  path: Path,
): void {
  Object.keys(object).forEach((key) => {
    if (!allowed.includes(key)) fail([...path, key], 'unknown field');
  });
}

function bigintToSafeJsonNumber(value: bigint, path: Path, label: string): number {
  const number = Number(value);
  if (!Number.isSafeInteger(number)) fail(path, `${label} cannot be represented losslessly`);
  return number;
}
