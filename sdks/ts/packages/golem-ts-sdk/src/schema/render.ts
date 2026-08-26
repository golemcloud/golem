// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

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
      return { tag: 'path', value: expectString(json, path) };
    case 'url':
      return { tag: 'url', value: expectString(json, path) };
    case 'datetime':
      return { tag: 'datetime', value: datetimeFromISOString(expectString(json, path)) };
    case 'duration':
      return { tag: 'duration', nanoseconds: BigInt(expectString(json, path)) };
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
      return value.language === undefined
        ? value.text
        : { text: value.text, language: value.language };
    case 'binary':
      return {
        bytes: bytesToBase64(value.bytes),
        ...(value.mimeType === undefined ? {} : { mimeType: value.mimeType }),
      };
    case 'datetime':
      return datetimeToISOString(value.value);
    case 'duration':
      return value.nanoseconds.toString();
    case 'quantity':
      return {
        mantissa: value.value.mantissa.toString(),
        scale: value.value.scale,
        unit: value.value.unit,
      };
    case 'record':
      return Object.fromEntries(
        (body as Extract<SchemaTypeBody, { tag: 'record' }>).fields.map((field, index) => [
          field.name,
          toCanonicalJson(graph, field.body, value.fields[index], [...path, field.name]),
        ]),
      );
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
    case 'flags':
      return (body as Extract<SchemaTypeBody, { tag: 'flags' }>).names.filter(
        (_, index) => value.flags[index],
      );
    case 'tuple':
      return value.elements.map((entry, index) =>
        toCanonicalJson(
          graph,
          (body as Extract<SchemaTypeBody, { tag: 'tuple' }>).elements[index],
          entry,
          [...path, index],
        ),
      );
    case 'list':
    case 'fixed-list':
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
  }
}

export function toCanonicalJsonSchema(
  graph: SchemaGraph,
  type: SchemaType,
  includeDraftMarker: boolean,
): JsonValue {
  const root = renderSchema(graph, type);
  const defs = Object.fromEntries(
    [...graph.defs].map(([id, definition]) => [id, renderSchema(graph, definition.body)]),
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
  switch (body.tag) {
    case 'bool':
      return { type: 'boolean' };
    case 's8':
    case 's16':
    case 's32':
    case 's64':
    case 'u8':
    case 'u16':
    case 'u32':
    case 'u64':
      return { type: 'integer' };
    case 'f32':
    case 'f64':
      return { type: 'number' };
    case 'char':
      return { type: 'string', minLength: 1, maxLength: 2 };
    case 'string':
    case 'text':
    case 'path':
    case 'url':
    case 'datetime':
    case 'duration':
      return { type: 'string' };
    case 'binary':
      return {
        type: 'object',
        required: ['bytes'],
        properties: {
          bytes: { type: 'string', contentEncoding: 'base64' },
          mimeType: { type: 'string' },
        },
        additionalProperties: false,
      };
    case 'quantity':
      return {
        type: 'object',
        required: ['mantissa', 'scale', 'unit'],
        properties: {
          mantissa: { type: 'string', pattern: '^-?[0-9]+$' },
          scale: { type: 'integer' },
          unit: { type: 'string' },
        },
        additionalProperties: false,
      };
    case 'record':
      return {
        type: 'object',
        properties: Object.fromEntries(
          body.fields.map((field) => [field.name, renderSchema(graph, field.body)]),
        ),
        required: body.fields
          .filter((field) => resolve(graph, field.body).body.tag !== 'option')
          .map((field) => field.name),
        additionalProperties: false,
      };
    case 'variant':
      return {
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
    case 'enum':
      return { type: 'string', enum: body.cases };
    case 'flags':
      return { type: 'array', items: { type: 'string', enum: body.names }, uniqueItems: true };
    case 'tuple':
      return {
        type: 'array',
        prefixItems: body.elements.map((entry) => renderSchema(graph, entry)),
        minItems: body.elements.length,
        maxItems: body.elements.length,
      };
    case 'list':
      return { type: 'array', items: renderSchema(graph, body.element) };
    case 'fixed-list':
      return {
        type: 'array',
        items: renderSchema(graph, body.element),
        minItems: body.length,
        maxItems: body.length,
      };
    case 'map':
      return {
        type: 'array',
        items: {
          type: 'array',
          prefixItems: [renderSchema(graph, body.key), renderSchema(graph, body.value)],
          minItems: 2,
          maxItems: 2,
        },
      };
    case 'option':
      return { anyOf: [renderSchema(graph, body.element), { type: 'null' }] };
    case 'result':
      return {
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
    case 'union':
      return { oneOf: body.branches.map((entry) => renderSchema(graph, entry.body)) };
    case 'secret':
    case 'quota-token':
    case 'permission-card':
      return { writeOnly: true, 'x-golem-capability': body.tag };
    case 'future':
    case 'stream':
      return { 'x-golem-unsupported': body.tag };
  }
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
  if (typeof value === 'string') return { tag: 'text', text: value };
  const object = expectObject(value, path);
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
  return {
    tag: 'binary',
    bytes: base64ToBytes(expectString(object.bytes, [...path, 'bytes'])),
    ...(object.mimeType === undefined
      ? {}
      : { mimeType: expectString(object.mimeType, [...path, 'mimeType']) }),
  };
}
function decodeQuantity(value: JsonValue, path: Path): SchemaValue {
  const object = expectObject(value, path);
  return {
    tag: 'quantity',
    value: {
      mantissa: BigInt(expectString(object.mantissa, [...path, 'mantissa'])),
      scale: expectInteger(object.scale, [...path, 'scale'], -2147483648, 2147483647),
      unit: expectString(object.unit, [...path, 'unit']),
    },
  };
}
function discriminatorMatches(rule: { tag: string; val?: unknown }, value: JsonValue): boolean {
  if (rule.tag === 'prefix')
    return typeof value === 'string' && value.startsWith(rule.val as string);
  if (rule.tag === 'suffix') return typeof value === 'string' && value.endsWith(rule.val as string);
  if (rule.tag === 'regex')
    return typeof value === 'string' && new RegExp(rule.val as string, 'u').test(value);
  if (rule.tag === 'field') {
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
  return false;
}
function bytesToBase64(bytes: Uint8Array): string {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
  let result = '';
  for (let index = 0; index < bytes.length; index += 3) {
    const first = bytes[index];
    const second = bytes[index + 1];
    const third = bytes[index + 2];
    result += alphabet[first >> 2];
    result += alphabet[((first & 3) << 4) | ((second ?? 0) >> 4)];
    result += second === undefined ? '=' : alphabet[((second & 15) << 2) | ((third ?? 0) >> 6)];
    result += third === undefined ? '=' : alphabet[third & 63];
  }
  return result;
}
function base64ToBytes(value: string): Uint8Array {
  const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
  if (
    value.length % 4 !== 0 ||
    !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value)
  ) {
    throw new SchemaRenderError('invalid base64');
  }
  const bytes: number[] = [];
  for (let index = 0; index < value.length; index += 4) {
    const a = alphabet.indexOf(value[index]);
    const b = alphabet.indexOf(value[index + 1]);
    const c = value[index + 2] === '=' ? 0 : alphabet.indexOf(value[index + 2]);
    const d = value[index + 3] === '=' ? 0 : alphabet.indexOf(value[index + 3]);
    bytes.push((a << 2) | (b >> 4));
    if (value[index + 2] !== '=') bytes.push(((b & 15) << 4) | (c >> 2));
    if (value[index + 3] !== '=') bytes.push(((c & 3) << 6) | d);
  }
  return Uint8Array.from(bytes);
}
