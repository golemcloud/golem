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

// Valibot schema walker (vendor `"valibot"`). Mirrors `./zod.ts`: walks the
// schema's runtime structure once to produce a `FluentCodec`. It duck-types
// Valibot's internals (`.type` discriminator plus child accessors `.wrapped`,
// `.item`, `.items`, `.entries`, `.options`, `.key`/`.value`, `.literal`,
// `.pipe`) and does NOT `import 'valibot'`, so Valibot is never baked into the
// SDK / WASM. Only the `normalize` step differs from the Zod walker; the per-kind
// `t.*` / `v.*` value-codec shapes are identical.

import {
  field,
  mergeGraphDefs,
  NamedFieldType,
  SchemaType,
  SchemaValue,
  t,
  v,
  variantCase,
  VariantCaseType,
} from '../../internal/schema-model';
import { FluentCodec, SchemaWalker } from './codec';
import { StandardSchemaV1 } from './standardSchema';
import { buildUnionVariantCodec, matchesStandard } from './union';
import { registerSchemaWalker } from './adapter';

interface NormalizedDef {
  kind: string;
  inner?: unknown;
  element?: unknown;
  shape?: Record<string, unknown>;
  /** Tuple element schemas (fixed-length). */
  items?: unknown[];
  /** Variant member schemas. */
  options?: unknown[];
  /** Variant discriminator key. */
  discriminator?: string;
  /** Record / map key + value schemas. */
  keyType?: unknown;
  valueType?: unknown;
  /** Enum / picklist string cases (in declaration order). */
  enumCases?: string[];
  /** Literal value(s). */
  literalValues?: unknown[];
  /** `v.lazy(() => S)` thunk returning the (stable) inner schema. */
  getter?: (input?: unknown) => unknown;
}

/**
 * Valibot pipes wrap a base schema in a `.pipe` array whose first element is the
 * base schema (e.g. `v.pipe(v.string(), v.minLength(2))`). Unwrap to the base so
 * structural classification uses the underlying schema, not the validation
 * actions appended after it.
 */
function unwrapPipe(schema: any): any {
  let cur = schema;
  // A piped schema reports its base `.type` already, but its child structure
  // (entries/item/…) lives on the first pipe entry. For schemas whose own
  // top-level fields already carry the structure this is a no-op.
  while (cur && Array.isArray(cur.pipe) && cur.pipe.length > 0 && cur.pipe[0] !== cur) {
    const base = cur.pipe[0];
    if (base && typeof base === 'object' && base.type === cur.type) {
      cur = base;
    } else {
      break;
    }
  }
  return cur;
}

/** Normalize a Valibot schema to a kind + child accessors. */
function normalize(raw: any): NormalizedDef {
  const schema = unwrapPipe(raw);
  if (!schema || typeof schema.type !== 'string') {
    throw new Error('Unrecognised Valibot schema shape (expected a `.type` discriminator)');
  }
  const kind = schema.type;
  return {
    kind,
    inner: schema.wrapped,
    element: schema.item,
    shape: schema.entries,
    items: schema.items,
    options: schema.options,
    discriminator: schema.key,
    keyType: schema.key,
    valueType: schema.value,
    // `picklist` exposes `.options` as a string array; `enum` exposes `.options`
    // as its forward values array (`.enum` is the source object).
    enumCases:
      kind === 'picklist' || kind === 'enum'
        ? (schema.options as unknown[]).map(String)
        : undefined,
    literalValues: kind === 'literal' ? [schema.literal] : undefined,
    // `v.lazy(() => S)` stores the thunk on `.getter` (called with the input).
    getter: kind === 'lazy' ? schema.getter : undefined,
  };
}

/** Wrappers that make the value optional. */
const OPTIONAL = new Set(['optional', 'nullable', 'nullish', 'undefinedable']);

function leaf(
  root: SchemaType,
  toValue: FluentCodec['toValue'],
  fromValue: FluentCodec['fromValue'],
): FluentCodec {
  return { graph: { defs: new Map(), root }, toValue, fromValue };
}

const valibotWalker: SchemaWalker = (schema, recurse): FluentCodec => {
  const {
    kind,
    inner,
    element,
    shape,
    items,
    options,
    discriminator,
    keyType,
    valueType,
    enumCases,
    literalValues,
    getter,
  } = normalize(schema);

  if (kind === 'lazy') {
    // `v.lazy(() => S)` defers to its (stable) inner schema. Recursion is handled
    // by the cycle-aware `recurse`: a self-reference resolves to the same schema
    // object and closes to a `ref`. The getter ignores its input for recursive
    // definitions, so we pass `undefined`.
    if (typeof getter !== 'function') throw new Error('Valibot lazy schema has no getter thunk');
    return recurse(getter(undefined));
  }

  if (OPTIONAL.has(kind)) {
    const innerCodec = recurse(inner);
    const root: SchemaType = t.option(innerCodec.graph.root);
    const optionCodec: FluentCodec = {
      graph: { defs: innerCodec.graph.defs, root },
      toValue: (value) =>
        value === undefined || value === null
          ? v.option(undefined)
          : v.option(innerCodec.toValue(value)),
      fromValue: (sv) => {
        const opt = (sv as Extract<SchemaValue, { tag: 'option' }>).value;
        return opt === undefined ? undefined : innerCodec.fromValue(opt);
      },
    };
    // An OPTIONAL object group: expose the inner object's per-field codecs (so the
    // config surface can descend it) and flag it optional (so descended leaves are
    // declared as `option<leaf>` with required-child presence). See zod.ts.
    if (innerCodec.fields !== undefined) {
      return { ...optionCodec, fields: innerCodec.fields, optionalGroup: true };
    }
    return optionCodec;
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
    case 'object':
    case 'looseObject':
    case 'strictObject': {
      if (!shape) throw new Error('Valibot object schema has no `entries`');
      const keys = Object.keys(shape); // declaration order = authoritative field order
      const fieldCodecs = keys.map((k) => ({ name: k, codec: recurse(shape[k]) }));
      const fields: NamedFieldType[] = fieldCodecs.map((f) => field(f.name, f.codec.graph.root));
      const defs = mergeGraphDefs(fieldCodecs.map((f) => f.codec.graph));
      return {
        graph: { defs, root: t.record(fields) },
        toValue: (value) =>
          v.record(
            fieldCodecs.map((f) => f.codec.toValue((value as Record<string, unknown>)[f.name])),
          ),
        fromValue: (sv) => {
          const recFields = (sv as Extract<SchemaValue, { tag: 'record' }>).fields;
          const out: Record<string, unknown> = {};
          fieldCodecs.forEach((f, i) => {
            out[f.name] = f.codec.fromValue(recFields[i]);
          });
          return out;
        },
        // Expose per-field codecs so the config surface can flatten nested config.
        fields: fieldCodecs,
      };
    }
    case 'tuple':
    case 'strictTuple':
    case 'looseTuple': {
      const itemCodecs = (items ?? []).map((it) => recurse(it));
      const defs = mergeGraphDefs(itemCodecs.map((c) => c.graph));
      return {
        graph: { defs, root: t.tuple(itemCodecs.map((c) => c.graph.root)) },
        toValue: (value) => v.tuple((value as unknown[]).map((e, i) => itemCodecs[i].toValue(e))),
        fromValue: (sv) =>
          (sv as Extract<SchemaValue, { tag: 'tuple' }>).elements.map((e, i) =>
            itemCodecs[i].fromValue(e),
          ),
      };
    }
    case 'enum':
    case 'picklist': {
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
          return leaf(
            t.string(),
            () => v.string(lit as string),
            () => lit,
          );
        case 'number':
          return leaf(
            t.f64(),
            () => v.f64(lit as number),
            () => lit,
          );
        case 'boolean':
          return leaf(
            t.bool(),
            () => v.bool(lit as boolean),
            () => lit,
          );
        case 'bigint':
          return leaf(
            t.u64(),
            () => v.u64(lit as bigint),
            () => lit,
          );
        default:
          throw new Error(
            `Valibot literal of type '${typeof lit}' is not supported by the fluent SDK walker.`,
          );
      }
    }
    case 'record': {
      // `v.record(k, v)` over a plain object → WIT `map`.
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
      // `v.map(k, v)` over a JS `Map` → WIT `map` (arbitrary key types).
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
    case 'variant': {
      // `v.variant(key, [...])`: a discriminated union keyed on a literal field.
      const opts = (options ?? []) as unknown[];
      const disc = discriminator!;
      const optCodecs = opts.map((o) => recurse(o));
      const defs = mergeGraphDefs(optCodecs.map((c) => c.graph));
      const cases: VariantCaseType[] = optCodecs.map((c, i) =>
        variantCase(`case${i}`, c.graph.root),
      );
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
            throw new Error(`No variant branch for ${disc}=${String((value as never)[disc])}`);
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
      // Plain (non-discriminated) union → WIT `variant` with auto-named cases
      // `case0..caseN-1`. Decode by `caseIndex`; encode disambiguates by running
      // each member's own `~standard.validate` in declaration order.
      const opts = (options ?? []) as StandardSchemaV1[];
      if (opts.length === 0) throw new Error('v.union(...) has no members');
      const memberCodecs = opts.map((o) => recurse(o));
      return buildUnionVariantCodec(
        memberCodecs,
        (value) => opts.findIndex((o) => matchesStandard(o, value)),
        'v.union',
      );
    }
    default:
      throw new Error(
        `Valibot schema of kind '${kind}' is not yet supported by the fluent SDK walker ` +
          `(supported: string, number, boolean, bigint, array, object, tuple, enum, picklist, literal, record, ` +
          `map, variant, and optional/nullable wrappers).`,
      );
  }
};

registerSchemaWalker('valibot', valibotWalker);
