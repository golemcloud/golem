import { SchemaRef, type JsonValue } from './ref';
import { graphMayContainCapability, sourceValueIsCanonical, type SchemaCodec } from './codec';
import { canonicalConfigGraph, canonicalConfigName } from './config';

export function collectConfigDefault(
  codec: SchemaCodec,
  children: readonly SchemaCodec[],
): SchemaCodec {
  if (codec.configDefault !== undefined) return codec;
  if (codec.fields && graphMayContainCapability(codec.graph)) return codec;
  if (!children.some((child) => child.configDefault !== undefined)) return codec;
  if (codec.fields && codec.graph.root.body.tag === 'record') {
    const values: Record<string, JsonValue> = {};
    for (const field of codec.fields) {
      if (field.codec.configDefault !== undefined) {
        values[canonicalConfigName(field.name)] = field.codec.configDefault;
      } else if (field.codec.graph.root.body.tag !== 'option') {
        throw new TypeError(
          'A partial nested config default cannot be exported; use s.default on the whole record',
        );
      }
    }
    return { ...codec, configDefault: portableDefault(values) };
  }
  throw new TypeError(
    'Nested config defaults cannot be exported here; use s.default on the whole static field',
  );
}

/** Snapshot JSON data without calling getters, factories or toJSON hooks. */
export function portableDefault(value: unknown): JsonValue {
  const active = new Set<object>();
  const visit = (input: unknown): JsonValue => {
    if (input === null || typeof input === 'string' || typeof input === 'boolean') return input;
    if (typeof input === 'number' && Number.isFinite(input)) return input;
    if (typeof input !== 'object' || input === null || active.has(input)) {
      throw new TypeError(
        'Config defaults must be acyclic JSON data, not factories or capabilities',
      );
    }
    if (
      !Array.isArray(input) &&
      Object.getPrototypeOf(input) !== Object.prototype &&
      Object.getPrototypeOf(input) !== null
    ) {
      throw new TypeError('Config defaults must contain only plain JSON objects and arrays');
    }
    active.add(input);
    const output: Record<string, JsonValue> | JsonValue[] = Array.isArray(input) ? [] : {};
    for (const key of Reflect.ownKeys(input)) {
      if (Array.isArray(input) && key === 'length') continue;
      const descriptor = Object.getOwnPropertyDescriptor(input, key)!;
      if (typeof key !== 'string' || !descriptor.enumerable || !('value' in descriptor)) {
        throw new TypeError('Config defaults cannot contain accessors or non-JSON properties');
      }
      if (Array.isArray(input) && (!/^(0|[1-9][0-9]*)$/.test(key) || Number(key) >= input.length)) {
        throw new TypeError('Config default arrays cannot have named properties');
      }
      Object.defineProperty(output, key, { value: visit(descriptor.value), enumerable: true });
    }
    if (Array.isArray(input) && Object.keys(output).length !== input.length) {
      throw new TypeError('Config default arrays must be dense');
    }
    active.delete(input);
    Object.freeze(output);
    return output;
  };
  return visit(value);
}

/** Read a vendor's literal default without invoking an opaque accessor or callback. */
export function nativeConfigDefault(
  owner: object,
  key: string,
  codec: SchemaCodec,
): JsonValue | undefined {
  const descriptor = Object.getOwnPropertyDescriptor(owner, key);
  if (descriptor === undefined || ('value' in descriptor && descriptor.value === undefined))
    return undefined;
  if (!('value' in descriptor) || typeof descriptor.value === 'function') {
    throw new TypeError(
      'Cannot extract an opaque native config default; use s.default(schema, JSON data)',
    );
  }
  const native = portableDefault(descriptor.value);
  const encoded = codec.toValue(native);
  if (!sourceValueIsCanonical(codec, native, encoded)) {
    throw new TypeError('Native config default changes during encoding');
  }
  return new SchemaRef(canonicalConfigGraph(codec.graph)).unpackJson(encoded);
}
