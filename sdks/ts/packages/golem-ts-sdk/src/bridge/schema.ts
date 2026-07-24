// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

import type {
  BinaryRestrictions,
  Datetime,
  DiscriminatorRule,
  MetadataEnvelope,
  NumericBound,
  NumericRestrictions,
  PathDirection,
  PathKind,
  PathSpec,
  QuantityValue,
  TextRestrictions,
  UrlRestrictions,
} from 'golem:core/types@2.0.0';
import {
  emptyMetadata,
  type SchemaGraph,
  type SchemaType,
  type SchemaTypeBody,
  type TypedSchemaValue,
} from '../internal/schema-model';

type JsonObject = Record<string, unknown>;

function object(value: unknown, position: string): JsonObject {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error(`Expected object at ${position}`);
  }
  return value as JsonObject;
}

function required(value: JsonObject, name: string, position: string): unknown {
  const result = value[name];
  if (result === undefined || result === null) throw new Error(`Missing ${position}.${name}`);
  return result;
}

function array(value: unknown, position: string): unknown[] {
  if (!Array.isArray(value)) throw new Error(`Expected array at ${position}`);
  for (let index = 0; index < value.length; index++) {
    if (!Object.hasOwn(value, index)) throw new Error(`Sparse array at ${position}`);
  }
  return value;
}

function string(value: unknown, position: string): string {
  if (typeof value !== 'string') throw new Error(`Expected string at ${position}`);
  return value;
}

function integer(value: unknown, position: string): number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value)) {
    throw new Error(`Expected safe integer at ${position}`);
  }
  return value;
}

function rangedInteger(value: unknown, min: number, max: number, position: string): number {
  const result = integer(value, position);
  if (result < min || result > max) throw new Error(`Integer out of range at ${position}`);
  return result;
}

function bigint(value: unknown, position: string): bigint {
  if (typeof value !== 'string' && typeof value !== 'number') {
    throw new Error(`Expected integer string at ${position}`);
  }
  if (typeof value === 'number' && !Number.isSafeInteger(value)) {
    throw new Error(`Expected safe integer at ${position}`);
  }
  if (typeof value === 'string' && !/^-?\d+$/.test(value)) {
    throw new Error(`Invalid integer at ${position}`);
  }
  try {
    return BigInt(value);
  } catch {
    throw new Error(`Invalid integer at ${position}`);
  }
}

function rangedBigint(value: unknown, min: bigint, max: bigint, position: string): bigint {
  const result = bigint(value, position);
  if (result < min || result > max) throw new Error(`Integer out of range at ${position}`);
  return result;
}

function stringArray(value: unknown, position: string): string[] {
  return array(value, position).map((entry, index) => string(entry, `${position}[${index}]`));
}

function metadata(value: unknown): MetadataEnvelope {
  if (value === undefined || value === null) return emptyMetadata();
  const json = object(value, 'metadata');
  const roleJson = json.role === undefined ? undefined : object(json.role, 'metadata.role');
  const roleTag = roleJson
    ? string(required(roleJson, 'tag', 'metadata.role'), 'metadata.role.tag')
    : undefined;
  const role =
    roleTag === undefined
      ? undefined
      : roleTag === 'other'
        ? {
            tag: 'other' as const,
            val: string(required(roleJson!, 'value', 'metadata.role'), 'metadata.role.value'),
          }
        : roleTag === 'multimodal' ||
            roleTag === 'unstructured-text' ||
            roleTag === 'unstructured-binary'
          ? { tag: roleTag }
          : { tag: 'other' as const, val: roleTag };
  return {
    doc: json.doc === undefined ? undefined : string(json.doc, 'metadata.doc'),
    aliases: json.aliases === undefined ? [] : stringArray(json.aliases, 'metadata.aliases'),
    examples: json.examples === undefined ? [] : stringArray(json.examples, 'metadata.examples'),
    deprecated:
      json.deprecated === undefined ? undefined : string(json.deprecated, 'metadata.deprecated'),
    role: role as MetadataEnvelope['role'],
  };
}

function numericBound(value: unknown, position: string): NumericBound {
  const json = object(value, position);
  const kind = string(required(json, 'kind', position), `${position}.kind`);
  if (kind !== 'signed' && kind !== 'unsigned' && kind !== 'float-bits') {
    throw new Error(`Unknown numeric bound kind '${kind}'`);
  }
  const boundValue = required(json, 'value', position);
  const boundPosition = `${position}.value`;
  switch (kind) {
    case 'signed':
      return {
        tag: kind,
        val: rangedBigint(boundValue, -(2n ** 63n), 2n ** 63n - 1n, boundPosition),
      };
    case 'unsigned':
      return { tag: kind, val: rangedBigint(boundValue, 0n, 2n ** 64n - 1n, boundPosition) };
    case 'float-bits': {
      const bits = rangedBigint(boundValue, 0n, 2n ** 64n - 1n, boundPosition);
      return { tag: kind, val: bits === 0x8000000000000000n ? 0n : bits };
    }
  }
}

function numericRestrictions(value: unknown, position: string): NumericRestrictions | undefined {
  if (value === undefined || value === null) return undefined;
  const json = object(value, position);
  const result: NumericRestrictions = {
    min: json.min === undefined ? undefined : numericBound(json.min, `${position}.min`),
    max: json.max === undefined ? undefined : numericBound(json.max, `${position}.max`),
    unit:
      json.unit === undefined || json.unit === ''
        ? undefined
        : string(json.unit, `${position}.unit`),
  };
  return result.min === undefined && result.max === undefined && result.unit === undefined
    ? undefined
    : result;
}

function optionalU32(json: JsonObject, name: string, position: string): number | undefined {
  return json[name] === undefined
    ? undefined
    : rangedInteger(json[name], 0, 2 ** 32 - 1, `${position}.${name}`);
}

function optionalStringArray(
  json: JsonObject,
  name: string,
  position: string,
): string[] | undefined {
  return json[name] === undefined ? undefined : stringArray(json[name], `${position}.${name}`);
}

function textRestrictions(value: unknown, position: string): TextRestrictions {
  const json = object(value, position);
  return {
    languages: optionalStringArray(json, 'languages', position),
    minLength: optionalU32(json, 'minLength', position),
    maxLength: optionalU32(json, 'maxLength', position),
    regex: json.regex === undefined ? undefined : string(json.regex, `${position}.regex`),
  };
}

function binaryRestrictions(value: unknown, position: string): BinaryRestrictions {
  const json = object(value, position);
  return {
    mimeTypes: optionalStringArray(json, 'mimeTypes', position),
    minBytes: optionalU32(json, 'minBytes', position),
    maxBytes: optionalU32(json, 'maxBytes', position),
  };
}

function pathSpec(value: unknown, position: string): PathSpec {
  const json = object(value, position);
  const direction = string(required(json, 'direction', position), `${position}.direction`);
  const kind = string(required(json, 'kind', position), `${position}.kind`);
  if (!(['input', 'output', 'in-out'] as string[]).includes(direction))
    throw new Error(`Unknown path direction '${direction}'`);
  if (!(['file', 'directory', 'any'] as string[]).includes(kind))
    throw new Error(`Unknown path kind '${kind}'`);
  return {
    direction: direction as PathDirection,
    kind: kind as PathKind,
    allowedMimeTypes: optionalStringArray(json, 'allowedMimeTypes', position),
    allowedExtensions: optionalStringArray(json, 'allowedExtensions', position),
  };
}

function urlRestrictions(value: unknown, position: string): UrlRestrictions {
  const json = object(value, position);
  return {
    allowedSchemes: optionalStringArray(json, 'allowedSchemes', position),
    allowedHosts: optionalStringArray(json, 'allowedHosts', position),
  };
}

function discriminatorRule(value: unknown, position: string): DiscriminatorRule {
  const json = object(value, position);
  const rule = string(required(json, 'rule', position), `${position}.rule`);
  switch (rule) {
    case 'prefix':
    case 'suffix':
    case 'contains':
    case 'regex':
    case 'field-absent': {
      const wrapped = object(required(json, 'value', position), `${position}.value`);
      const field =
        rule === 'prefix'
          ? 'prefix'
          : rule === 'suffix'
            ? 'suffix'
            : rule === 'contains'
              ? 'substring'
              : rule === 'regex'
                ? 'regex'
                : 'fieldName';
      return {
        tag: rule,
        val: string(required(wrapped, field, `${position}.value`), `${position}.value.${field}`),
      };
    }
    case 'field-equals': {
      const field = object(required(json, 'value', position), `${position}.value`);
      return {
        tag: 'field-equals',
        val: {
          fieldName: string(required(field, 'fieldName', position), `${position}.value.fieldName`),
          literal:
            field.literal == null ? undefined : string(field.literal, `${position}.value.literal`),
        },
      };
    }
    default:
      throw new Error(`Unknown discriminator rule '${rule}'`);
  }
}

function quantityValue(value: unknown, position: string): QuantityValue {
  const json = object(value, position);
  return {
    mantissa: rangedBigint(
      required(json, 'mantissa', position),
      -(2n ** 63n),
      2n ** 63n - 1n,
      `${position}.mantissa`,
    ),
    scale: rangedInteger(
      required(json, 'scale', position),
      -(2 ** 31),
      2 ** 31 - 1,
      `${position}.scale`,
    ),
    unit: string(required(json, 'unit', position), `${position}.unit`),
  };
}

function schemaType(value: unknown, position: string): SchemaType {
  const json = object(value, position);
  const kind = string(required(json, 'kind', position), `${position}.kind`);
  const payload = json.value === undefined ? {} : object(json.value, `${position}.value`);
  const child = (name: string) =>
    schemaType(required(payload, name, `${position}.value`), `${position}.${name}`);
  const meta = metadata(payload.metadata ?? json.metadata);
  let body: SchemaTypeBody;
  switch (kind) {
    case 'ref':
      body = { tag: 'ref', id: string(required(payload, 'id', position), `${position}.id`) };
      break;
    case 'bool':
    case 'char':
    case 'string':
    case 'datetime':
    case 'duration':
      body = { tag: kind };
      break;
    case 's8':
    case 's16':
    case 's32':
    case 's64':
    case 'u8':
    case 'u16':
    case 'u32':
    case 'u64':
    case 'f32':
    case 'f64':
      body = {
        tag: kind,
        restrictions: numericRestrictions(payload.restrictions, `${position}.restrictions`),
      };
      break;
    case 'record':
      body = {
        tag: 'record',
        fields: array(required(payload, 'fields', position), `${position}.fields`).map(
          (entry, i) => {
            const f = object(entry, `${position}.fields[${i}]`);
            return {
              name: string(required(f, 'name', position), `${position}.fields[${i}].name`),
              body: schemaType(required(f, 'body', position), `${position}.fields[${i}].body`),
              metadata: metadata(f.metadata),
            };
          },
        ),
      };
      break;
    case 'variant':
      body = {
        tag: 'variant',
        cases: array(required(payload, 'cases', position), `${position}.cases`).map((entry, i) => {
          const c = object(entry, `${position}.cases[${i}]`);
          return {
            name: string(required(c, 'name', position), `${position}.cases[${i}].name`),
            payload:
              c.payload === undefined
                ? undefined
                : schemaType(c.payload, `${position}.cases[${i}].payload`),
            metadata: metadata(c.metadata),
          };
        }),
      };
      break;
    case 'enum':
      body = {
        tag: 'enum',
        cases: stringArray(required(payload, 'cases', position), `${position}.cases`),
      };
      break;
    case 'flags':
      body = {
        tag: 'flags',
        names: stringArray(required(payload, 'flags', position), `${position}.flags`),
      };
      break;
    case 'tuple':
      body = {
        tag: 'tuple',
        elements: array(required(payload, 'elements', position), `${position}.elements`).map(
          (x, i) => schemaType(x, `${position}.elements[${i}]`),
        ),
      };
      break;
    case 'list':
      body = { tag: 'list', element: child('element') };
      break;
    case 'fixed-list':
      body = {
        tag: 'fixed-list',
        element: child('element'),
        length: rangedInteger(
          required(payload, 'length', position),
          0,
          2 ** 32 - 1,
          `${position}.length`,
        ),
      };
      break;
    case 'map':
      body = { tag: 'map', key: child('key'), value: child('value') };
      break;
    case 'option':
      body = { tag: 'option', element: child('inner') };
      break;
    case 'result': {
      const spec = object(required(payload, 'spec', position), `${position}.spec`);
      body = {
        tag: 'result',
        ok: spec.ok == null ? undefined : schemaType(spec.ok, `${position}.ok`),
        err: spec.err == null ? undefined : schemaType(spec.err, `${position}.err`),
      };
      break;
    }
    case 'text':
      body = {
        tag: 'text',
        restrictions: textRestrictions(
          required(payload, 'restrictions', position),
          `${position}.restrictions`,
        ),
      };
      break;
    case 'binary':
      body = {
        tag: 'binary',
        restrictions: binaryRestrictions(
          required(payload, 'restrictions', position),
          `${position}.restrictions`,
        ),
      };
      break;
    case 'path':
      body = {
        tag: 'path',
        spec: pathSpec(required(payload, 'spec', position), `${position}.spec`),
      };
      break;
    case 'url':
      body = {
        tag: 'url',
        restrictions: urlRestrictions(
          required(payload, 'restrictions', position),
          `${position}.restrictions`,
        ),
      };
      break;
    case 'quantity': {
      const spec = object(required(payload, 'spec', position), `${position}.spec`);
      body = {
        tag: 'quantity',
        spec: {
          baseUnit: string(required(spec, 'baseUnit', position), `${position}.baseUnit`),
          allowedSuffixes:
            spec.allowedSuffixes === undefined
              ? []
              : stringArray(spec.allowedSuffixes, `${position}.allowedSuffixes`),
          min: spec.min == null ? undefined : quantityValue(spec.min, `${position}.min`),
          max: spec.max == null ? undefined : quantityValue(spec.max, `${position}.max`),
        },
      };
      break;
    }
    case 'union': {
      const spec = object(required(payload, 'spec', position), `${position}.spec`);
      body = {
        tag: 'union',
        branches: array(required(spec, 'branches', position), `${position}.branches`).map(
          (entry, i) => {
            const b = object(entry, `${position}.branches[${i}]`);
            return {
              tag: string(required(b, 'tag', position), `${position}.tag`),
              body: schemaType(required(b, 'body', position), `${position}.body`),
              discriminator: discriminatorRule(
                required(b, 'discriminator', position),
                `${position}.branches[${i}].discriminator`,
              ),
              metadata: metadata(b.metadata),
            };
          },
        ),
      };
      break;
    }
    case 'secret': {
      const spec = object(required(payload, 'spec', position), `${position}.spec`);
      body = {
        tag: 'secret',
        inner:
          spec.inner == null
            ? { body: { tag: 'string' }, metadata: emptyMetadata() }
            : schemaType(spec.inner, `${position}.inner`),
        spec: {
          category:
            spec.category == null ? undefined : string(spec.category, `${position}.category`),
        },
      };
      break;
    }
    case 'quota-token': {
      const spec = object(required(payload, 'spec', position), `${position}.spec`);
      body = {
        tag: 'quota-token',
        spec: {
          resourceName:
            spec.resourceName == null
              ? undefined
              : string(spec.resourceName, `${position}.resourceName`),
        },
      };
      break;
    }
    case 'future':
    case 'stream':
      body = {
        tag: kind,
        element: payload.inner == null ? undefined : schemaType(payload.inner, `${position}.inner`),
      };
      break;
    default:
      throw new Error(`Unknown schema type kind '${kind}'`);
  }
  return { body, metadata: meta };
}

/** Decode trusted bridge-generator JSON with precision-sensitive integers stringified. */
export function schemaGraphFromJson(json: string): SchemaGraph {
  const root = object(JSON.parse(json), 'schema graph');
  const entries = (root.defs === undefined ? [] : array(root.defs, 'schema graph.defs')).map(
    (entry, index) => {
      const def = object(entry, `schema graph.defs[${index}]`);
      const id = string(required(def, 'id', 'schema definition'), `schema graph.defs[${index}].id`);
      return [
        id,
        {
          name: def.name == null ? undefined : string(def.name, `${id}.name`),
          body: schemaType(required(def, 'body', id), `${id}.body`),
        },
      ] as const;
    },
  );
  const defs = new Map<string, (typeof entries)[number][1]>();
  for (const [id, def] of entries) {
    if (defs.has(id)) throw new Error(`Duplicate schema definition id '${id}'`);
    defs.set(id, def);
  }
  return { defs, root: schemaType(required(root, 'root', 'schema graph'), 'schema graph.root') };
}

export function typedSchemaValueFromJson(
  json: string,
  value: TypedSchemaValue['value'],
): TypedSchemaValue {
  return { graph: schemaGraphFromJson(json), value };
}

/** Convert an ISO-8601 instant to the nanosecond-precise schema datetime shape. */
export function datetimeFromISOString(value: string): Datetime {
  const match = value.match(/^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.(\d{1,9}))?Z$/);
  if (!match) throw new Error(`Invalid UTC datetime '${value}'`);
  const wholeSeconds = `${match[1]}-${match[2]}-${match[3]}T${match[4]}:${match[5]}:${match[6]}`;
  const milliseconds = Date.parse(`${wholeSeconds}Z`);
  if (!Number.isFinite(milliseconds)) throw new Error(`Invalid datetime '${value}'`);
  if (new Date(milliseconds).toISOString().slice(0, 19) !== wholeSeconds) {
    throw new Error(`Invalid datetime '${value}'`);
  }
  return {
    seconds: BigInt(milliseconds / 1000),
    nanoseconds: Number((match[7] ?? '').padEnd(9, '0') || 0),
  };
}

/** Convert a schema datetime to its canonical ISO-8601 UTC representation. */
export function datetimeToISOString(value: Datetime): string {
  if (
    !Number.isFinite(value.nanoseconds) ||
    !Number.isInteger(value.nanoseconds) ||
    value.nanoseconds < 0 ||
    value.nanoseconds >= 1_000_000_000
  ) {
    throw new Error(`Datetime nanoseconds out of range: ${value.nanoseconds}`);
  }
  const minimumSeconds = -62_167_219_200n;
  const maximumSeconds = 253_402_300_799n;
  if (value.seconds < minimumSeconds || value.seconds > maximumSeconds) {
    throw new Error(`Datetime seconds outside canonical year range 0000..9999: ${value.seconds}`);
  }
  const date = new Date(Number(value.seconds * 1000n));
  if (!Number.isFinite(date.getTime())) {
    throw new Error(`Invalid datetime seconds: ${value.seconds}`);
  }
  const base = date.toISOString().replace('.000Z', '');
  const fraction =
    value.nanoseconds === 0
      ? ''
      : `.${String(value.nanoseconds).padStart(9, '0').replace(/0+$/, '')}`;
  return `${base}${fraction}Z`;
}
