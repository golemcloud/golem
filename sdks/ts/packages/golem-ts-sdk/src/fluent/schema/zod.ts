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

// Zod schema walker (vendor `"zod"`). Walks the schema's runtime structure once
// (effect-golem `WitCodec.walk` analog) to produce a `FluentCodec` — a
// `SchemaType` (new model) plus a `SchemaValue` encode/decode pair. It
// duck-types Zod's internals (v4 `._zod.def`, v3 `._def`) and does NOT
// `import 'zod'`, so Zod is never baked into the SDK/WASM. No `Type.Type`.

import {
  field,
  mergeGraphDefs,
  NamedFieldType,
  SchemaType,
  SchemaValue,
  t,
  v,
} from '../../internal/schema-model';
import { FluentCodec, SchemaWalker } from './codec';
import { registerSchemaWalker } from './adapter';

interface NormalizedDef {
  kind: string;
  inner?: unknown;
  element?: unknown;
  shape?: Record<string, unknown>;
}

const V3_TYPE_NAMES: Record<string, string> = {
  ZodString: 'string',
  ZodNumber: 'number',
  ZodBoolean: 'boolean',
  ZodBigInt: 'bigint',
  ZodArray: 'array',
  ZodObject: 'object',
  ZodOptional: 'optional',
  ZodNullable: 'nullable',
  ZodDefault: 'default',
  ZodReadonly: 'readonly',
  ZodCatch: 'catch',
  ZodEffects: 'effects',
  ZodVoid: 'void',
  ZodUndefined: 'undefined',
  ZodNull: 'null',
};

/** Normalize a Zod schema (v3 or v4) to a kind + child accessors. */
function normalize(schema: any): NormalizedDef {
  if (schema?._zod?.def) {
    // Zod v4
    const def = schema._zod.def;
    return { kind: def.type, inner: def.innerType ?? def.in, element: def.element, shape: def.shape };
  }
  if (schema?._def?.typeName) {
    // Zod v3
    const def = schema._def;
    const shape = typeof def.shape === 'function' ? def.shape() : def.shape;
    return {
      kind: V3_TYPE_NAMES[def.typeName] ?? def.typeName,
      inner: def.innerType ?? def.schema,
      element: def.type,
      shape,
    };
  }
  throw new Error('Unrecognised Zod schema shape (expected Zod v3 or v4 internals)');
}

/** Wrappers that pass through their inner schema unchanged. */
const TRANSPARENT = new Set(['default', 'readonly', 'catch', 'effects', 'nonoptional', 'pipe']);
/** Wrappers that make the value optional. */
const OPTIONAL = new Set(['optional', 'nullable']);

function leaf(root: SchemaType, toValue: FluentCodec['toValue'], fromValue: FluentCodec['fromValue']): FluentCodec {
  return { graph: { defs: new Map(), root }, toValue, fromValue };
}

const zodWalker: SchemaWalker = (schema, recurse): FluentCodec => {
  const { kind, inner, element, shape } = normalize(schema);

  if (OPTIONAL.has(kind)) {
    const innerCodec = recurse(inner);
    const root: SchemaType = t.option(innerCodec.graph.root);
    return {
      graph: { defs: innerCodec.graph.defs, root },
      toValue: (value) =>
        value === undefined || value === null ? v.option(undefined) : v.option(innerCodec.toValue(value)),
      fromValue: (sv) => {
        const opt = (sv as Extract<SchemaValue, { tag: 'option' }>).value;
        return opt === undefined ? undefined : innerCodec.fromValue(opt);
      },
    };
  }

  if (TRANSPARENT.has(kind)) {
    return recurse(inner);
  }

  switch (kind) {
    case 'void':
    case 'undefined':
      // Unit/void: maps to WIT `output-schema.unit`; `graph` is a placeholder.
      return {
        graph: { defs: new Map(), root: t.record([]) },
        toValue: () => v.record([]),
        fromValue: () => undefined,
        isUnit: true,
      };
    case 'string':
      return leaf(
        t.string(),
        (value) => v.string(value as string),
        (sv) => (sv as Extract<SchemaValue, { tag: 'string' }>).value,
      );
    case 'number':
    case 'int':
    case 'float':
      return leaf(
        t.f64(),
        (value) => v.f64(value as number),
        (sv) => (sv as Extract<SchemaValue, { tag: 'f64' }>).value,
      );
    case 'boolean':
      return leaf(
        t.bool(),
        (value) => v.bool(value as boolean),
        (sv) => (sv as Extract<SchemaValue, { tag: 'bool' }>).value,
      );
    case 'bigint':
      return leaf(
        t.u64(),
        (value) => v.u64(value as bigint),
        (sv) => (sv as Extract<SchemaValue, { tag: 'u64' }>).value,
      );
    case 'array': {
      const elemCodec = recurse(element);
      return {
        graph: { defs: elemCodec.graph.defs, root: t.list(elemCodec.graph.root) },
        toValue: (value) => v.list((value as unknown[]).map((e) => elemCodec.toValue(e))),
        fromValue: (sv) =>
          (sv as Extract<SchemaValue, { tag: 'list' }>).elements.map((e) => elemCodec.fromValue(e)),
      };
    }
    case 'object': {
      if (!shape) throw new Error('Zod object schema has no shape');
      const keys = Object.keys(shape); // declaration order = authoritative field order
      const fieldCodecs = keys.map((k) => ({ name: k, codec: recurse(shape[k]) }));
      const fields: NamedFieldType[] = fieldCodecs.map((f) => field(f.name, f.codec.graph.root));
      const defs = mergeGraphDefs(fieldCodecs.map((f) => f.codec.graph));
      return {
        graph: { defs, root: t.record(fields) },
        toValue: (value) =>
          v.record(fieldCodecs.map((f) => f.codec.toValue((value as Record<string, unknown>)[f.name]))),
        fromValue: (sv) => {
          const recFields = (sv as Extract<SchemaValue, { tag: 'record' }>).fields;
          const out: Record<string, unknown> = {};
          fieldCodecs.forEach((f, i) => {
            out[f.name] = f.codec.fromValue(recFields[i]);
          });
          return out;
        },
      };
    }
    default:
      throw new Error(
        `Zod schema of kind '${kind}' is not yet supported by the fluent SDK walker ` +
          `(supported: string, number, boolean, bigint, array, object, and optional/nullable/default wrappers).`,
      );
  }
};

registerSchemaWalker('zod', zodWalker);
