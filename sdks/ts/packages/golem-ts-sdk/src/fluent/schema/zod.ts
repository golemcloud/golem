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
  type NumericBound,
  type NumericRestrictions,
  SchemaType,
  SchemaValue,
  t,
  v,
  variantCase,
  VariantCaseType,
} from '../../internal/schema-model';
import { FluentCodec, SchemaWalker } from './codec';

const F64_BITS_VIEW = new DataView(new ArrayBuffer(8));
function f64Bits(x: number): bigint {
  // Canonicalize -0.0 to +0.0 so equal bounds compare equal (mirrors the codec).
  F64_BITS_VIEW.setFloat64(0, x === 0 ? 0 : x);
  return F64_BITS_VIEW.getBigUint64(0);
}

/**
 * Read min/max bounds off a Zod number schema (v3 `_def.checks` / v4
 * `_zod.def.checks`) into f64 `NumericRestrictions` (the bound is the inclusive
 * value; the model carries no inclusivity flag). `undefined` when unconstrained.
 */
function numberRestrictions(schema: any): NumericRestrictions | undefined {
  const def = schema?._zod?.def ?? schema?._def;
  const checks: any[] = def?.checks ?? [];
  let min: number | undefined;
  let max: number | undefined;
  for (const c of checks) {
    const cd = c?._zod?.def ?? c?._def ?? c;
    const kind = cd?.check ?? cd?.kind;
    const value = cd?.value;
    if (typeof value !== 'number') continue;
    if (kind === 'greater_than' || kind === 'min') min = value;
    else if (kind === 'less_than' || kind === 'max') max = value;
  }
  if (min === undefined && max === undefined) return undefined;
  const bound = (x: number): NumericBound => ({ tag: 'float-bits', val: f64Bits(x) });
  return {
    min: min !== undefined ? bound(min) : undefined,
    max: max !== undefined ? bound(max) : undefined,
    unit: undefined,
  };
}
import { registerSchemaWalker } from './adapter';

interface NormalizedDef {
  kind: string;
  inner?: unknown;
  element?: unknown;
  shape?: Record<string, unknown>;
  /** Tuple element schemas (fixed-length). */
  items?: unknown[];
  /** Union / discriminated-union member schemas. */
  options?: unknown[];
  /** Discriminated-union discriminator key. */
  discriminator?: string;
  /** Record key / value schemas (`z.record(k, v)`). */
  keyType?: unknown;
  valueType?: unknown;
  /** Enum string cases (in declaration order). */
  enumCases?: string[];
  /** Literal value(s). */
  literalValues?: unknown[];
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
  ZodTuple: 'tuple',
  ZodEnum: 'enum',
  ZodNativeEnum: 'enum',
  ZodUnion: 'union',
  ZodDiscriminatedUnion: 'discriminatedUnion',
  ZodRecord: 'record',
  ZodMap: 'map',
  ZodLiteral: 'literal',
};

/** Coerce a Zod enum's `entries`/`values` (object or array) to ordered string cases. */
function enumCasesOf(raw: unknown): string[] {
  if (Array.isArray(raw)) return raw.map(String);
  if (raw && typeof raw === 'object') {
    // v4 `entries` / v3 nativeEnum `values`: { KEY: value }. Numeric enums also
    // carry reverse mappings (value→key); keep only the string-valued forward keys.
    return Object.values(raw as Record<string, unknown>).filter((x) => typeof x === 'string') as string[];
  }
  return [];
}

/** Normalize a Zod schema (v3 or v4) to a kind + child accessors. */
function normalize(schema: any): NormalizedDef {
  if (schema?._zod?.def) {
    // Zod v4
    const def = schema._zod.def;
    return {
      // In Zod v4 a discriminated union reports `type: 'union'` but carries a
      // `discriminator`; distinguish it from a plain union here.
      kind: def.type === 'union' && def.discriminator ? 'discriminatedUnion' : def.type,
      inner: def.innerType ?? def.in,
      element: def.element,
      shape: def.shape,
      items: def.items,
      options: def.options,
      discriminator: def.discriminator,
      keyType: def.keyType,
      valueType: def.valueType,
      enumCases: def.type === 'enum' ? enumCasesOf(def.entries) : undefined,
      literalValues: def.type === 'literal' ? def.values : undefined,
    };
  }
  if (schema?._def?.typeName) {
    // Zod v3
    const def = schema._def;
    const shape = typeof def.shape === 'function' ? def.shape() : def.shape;
    const kind = V3_TYPE_NAMES[def.typeName] ?? def.typeName;
    return {
      kind,
      inner: def.innerType ?? def.schema,
      element: def.type,
      shape,
      items: def.items,
      options: def.options,
      discriminator: def.discriminator,
      keyType: def.keyType,
      valueType: def.valueType,
      enumCases: kind === 'enum' ? enumCasesOf(def.values) : undefined,
      literalValues: def.typeName === 'ZodLiteral' ? [def.value] : undefined,
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
  const { kind, inner, element, shape, items, options, discriminator, keyType, valueType, enumCases, literalValues } =
    normalize(schema);

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
        t.f64(numberRestrictions(schema)),
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
    case 'tuple': {
      const itemCodecs = (items ?? []).map((it) => recurse(it));
      const defs = mergeGraphDefs(itemCodecs.map((c) => c.graph));
      return {
        graph: { defs, root: t.tuple(itemCodecs.map((c) => c.graph.root)) },
        toValue: (value) => v.tuple((value as unknown[]).map((e, i) => itemCodecs[i].toValue(e))),
        fromValue: (sv) =>
          (sv as Extract<SchemaValue, { tag: 'tuple' }>).elements.map((e, i) => itemCodecs[i].fromValue(e)),
      };
    }
    case 'enum': {
      const cases = enumCases ?? [];
      return leaf(
        t.enum(cases),
        (value) => v.enum(cases.indexOf(value as string)),
        (sv) => cases[(sv as Extract<SchemaValue, { tag: 'enum' }>).caseIndex],
      );
    }
    case 'literal': {
      // A single-value literal travels as its base primitive on the wire; the
      // Standard Schema validate already pinned the value, so encode/decode are
      // constant. Multi-value string literals behave like an enum.
      const lits = literalValues ?? [];
      if (lits.length > 1 && lits.every((l) => typeof l === 'string')) {
        const cases = lits as string[];
        return leaf(
          t.enum(cases),
          (value) => v.enum(cases.indexOf(value as string)),
          (sv) => cases[(sv as Extract<SchemaValue, { tag: 'enum' }>).caseIndex],
        );
      }
      const lit = lits[0];
      switch (typeof lit) {
        case 'string':
          return leaf(t.string(), () => v.string(lit as string), () => lit);
        case 'number':
          return leaf(t.f64(), () => v.f64(lit as number), () => lit);
        case 'boolean':
          return leaf(t.bool(), () => v.bool(lit as boolean), () => lit);
        case 'bigint':
          return leaf(t.u64(), () => v.u64(lit as bigint), () => lit);
        default:
          throw new Error(`Zod literal of type '${typeof lit}' is not supported by the fluent SDK walker.`);
      }
    }
    case 'record': {
      // `z.record(k, v)` over a plain object → WIT `map`.
      const keyCodec = recurse(keyType);
      const valCodec = recurse(valueType);
      const defs = mergeGraphDefs([keyCodec.graph, valCodec.graph]);
      return {
        graph: { defs, root: t.map(keyCodec.graph.root, valCodec.graph.root) },
        toValue: (value) =>
          v.map(
            Object.entries(value as Record<string, unknown>).map(([k, val]) => ({
              key: keyCodec.toValue(k),
              value: valCodec.toValue(val),
            })),
          ),
        fromValue: (sv) => {
          const out: Record<string, unknown> = {};
          (sv as Extract<SchemaValue, { tag: 'map' }>).entries.forEach((e) => {
            out[keyCodec.fromValue(e.key) as string] = valCodec.fromValue(e.value);
          });
          return out;
        },
      };
    }
    case 'map': {
      // `z.map(k, v)` over a JS `Map` → WIT `map` (arbitrary key types).
      const keyCodec = recurse(keyType);
      const valCodec = recurse(valueType);
      const defs = mergeGraphDefs([keyCodec.graph, valCodec.graph]);
      return {
        graph: { defs, root: t.map(keyCodec.graph.root, valCodec.graph.root) },
        toValue: (value) =>
          v.map(
            Array.from((value as Map<unknown, unknown>).entries()).map(([k, val]) => ({
              key: keyCodec.toValue(k),
              value: valCodec.toValue(val),
            })),
          ),
        fromValue: (sv) =>
          new Map(
            (sv as Extract<SchemaValue, { tag: 'map' }>).entries.map((e) => [
              keyCodec.fromValue(e.key),
              valCodec.fromValue(e.value),
            ]),
          ),
      };
    }
    case 'discriminatedUnion': {
      const opts = (options ?? []) as unknown[];
      const disc = discriminator!;
      const optCodecs = opts.map((o) => recurse(o));
      const defs = mergeGraphDefs(optCodecs.map((c) => c.graph));
      const cases: VariantCaseType[] = optCodecs.map((c, i) => variantCase(`case${i}`, c.graph.root));
      // Map each branch's discriminator literal value → case index, for encode.
      const discToIndex = new Map<unknown, number>();
      opts.forEach((o, i) => {
        const litSchema = normalize(o).shape?.[disc];
        const lit = litSchema ? normalize(litSchema).literalValues?.[0] : undefined;
        if (lit !== undefined) discToIndex.set(lit, i);
      });
      return {
        graph: { defs, root: t.variant(cases) },
        toValue: (value) => {
          const i = discToIndex.get((value as Record<string, unknown>)[disc]);
          if (i === undefined) {
            throw new Error(`No discriminated-union branch for ${disc}=${String((value as never)[disc])}`);
          }
          return v.variant(i, optCodecs[i].toValue(value));
        },
        fromValue: (sv) => {
          const vv = sv as Extract<SchemaValue, { tag: 'variant' }>;
          return optCodecs[vv.caseIndex].fromValue(vv.payload!);
        },
      };
    }
    case 'union': {
      // Plain (non-discriminated) unions compile to a WIT `variant`. Robust
      // structural disambiguation on encode (the effect-golem `genericVariantNode`
      // analog) is deferred; for now require a discriminated union or an
      // `s.variant(...)` marker for ambiguous cases. We still support the common
      // case by trying each branch's validator-equivalent on decode by caseIndex.
      throw new Error(
        `Plain z.union(...) is not yet supported by the fluent SDK walker; use z.discriminatedUnion(...) ` +
          `or an SDK variant marker. (kind '${kind}')`,
      );
    }
    default:
      throw new Error(
        `Zod schema of kind '${kind}' is not yet supported by the fluent SDK walker ` +
          `(supported: string, number, boolean, bigint, array, object, tuple, enum, literal, record, map, ` +
          `discriminatedUnion, and optional/nullable/default wrappers).`,
      );
  }
};

registerSchemaWalker('zod', zodWalker);
