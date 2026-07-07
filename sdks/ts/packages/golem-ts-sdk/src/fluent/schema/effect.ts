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

// Effect Schema walker (vendor `"effect"`). The Standard Schema value here is the
// `Schema.standardSchemaV1(schema)` wrapper, which carries the schema's `ast`.
// Unlike Zod/Valibot, an Effect Schema AST node's children do NOT themselves
// carry a `~standard` brand, so this walker recurses over the AST *internally*
// rather than via the adapter's `recurse` callback. It duck-types the AST `_tag`
// discriminators
// (`StringKeyword`, `NumberKeyword`, `TypeLiteral`, `TupleType`, `Union`,
// `Literal`, `Enums`, `Refinement`/`Transformation`, …) and does NOT
// `import 'effect'`, so Effect is never baked into the SDK / WASM. The per-kind
// `t.*` / `v.*` shapes match the Zod walker.

import {
  field,
  mergeGraphDefs,
  NamedFieldType,
  SchemaType,
  SchemaValue,
  SchemaGraph,
  t,
  v,
  variantCase,
  VariantCaseType,
} from '../../internal/schema-model';
import { FluentCodec, SchemaWalker } from './codec';
import { buildUnionVariantCodec, matchesSchemaType } from './union';
import { registerSchemaWalker } from './adapter';
import { RecursionRegistry } from './recursion';

type Ast = any;

const VOID_LIKE = new Set(['UndefinedKeyword', 'VoidKeyword']);

/**
 * Whether an (unwrapped) AST node represents a `null` / `undefined` union member.
 * Effect encodes `null` as a `Literal` whose `.literal` is `null` (not a distinct
 * `Null` tag), while `undefined`/`void` are dedicated keyword nodes.
 */
function isNullOrUndefinedAst(node: Ast): boolean {
  if (!node) return false;
  if (node._tag === 'UndefinedKeyword' || node._tag === 'VoidKeyword' || node._tag === 'Null')
    return true;
  return node._tag === 'Literal' && node.literal === null;
}

/**
 * Unwrap `Refinement` (`.from`) and `Transformation` (`.from`, the encoded side)
 * nodes to the underlying structural AST. We walk the *encoded* form so the
 * wire shape (not the decoded domain type) is what we encode/decode.
 *
 * `Suspend` (recursion) is intentionally NOT unwrapped here — resolving it
 * eagerly would infinite-loop on a recursive schema. It is handled in
 * {@link walkAst} via the {@link RecursionRegistry}, which detects the cycle.
 */
function unwrap(ast: Ast): Ast {
  let cur = ast;
  for (;;) {
    if (cur && cur._tag === 'Refinement' && cur.from) {
      cur = cur.from;
    } else if (cur && cur._tag === 'Transformation' && cur.from) {
      cur = cur.from;
    } else {
      return cur;
    }
  }
}

function leaf(
  root: SchemaType,
  toValue: FluentCodec['toValue'],
  fromValue: FluentCodec['fromValue'],
): FluentCodec {
  return { graph: { defs: new Map(), root }, toValue, fromValue };
}

/**
 * Walk an Effect Schema AST node into a `FluentCodec`, routing every node through
 * the {@link RecursionRegistry} keyed on the node's identity. `Schema.suspend`
 * (recursion) is resolved to its STABLE target AST first — every recursive
 * reference's `.f()` returns the same target object — so a back-reference is
 * detected by identity and closed to a `ref`; non-recursive nodes pass through
 * inline. Effect recurses over its AST internally (its child nodes carry no
 * `~standard` brand), so it drives its own registry rather than the adapter's
 * `recurse`.
 */
function walkAst(rawAst: Ast, reg: RecursionRegistry): FluentCodec {
  let ast = unwrap(rawAst);
  while (ast && ast._tag === 'Suspend' && typeof ast.f === 'function') {
    ast = unwrap(ast.f());
  }
  return reg.compile(ast, () => walkAstBody(ast, reg));
}

/** Structural dispatch for an (already suspend-resolved) Effect AST node. */
function walkAstBody(ast: Ast, reg: RecursionRegistry): FluentCodec {
  const tag: string = ast?._tag;

  switch (tag) {
    case 'StringKeyword':
      return leaf(
        t.string(),
        (value) => v.string(value as string),
        (sv) => (sv as Extract<SchemaValue, { tag: 'string' }>).value,
      );
    case 'NumberKeyword':
      return leaf(
        t.f64(),
        (value) => v.f64(value as number),
        (sv) => (sv as Extract<SchemaValue, { tag: 'f64' }>).value,
      );
    case 'BooleanKeyword':
      return leaf(
        t.bool(),
        (value) => v.bool(value as boolean),
        (sv) => (sv as Extract<SchemaValue, { tag: 'bool' }>).value,
      );
    case 'BigIntKeyword':
      return leaf(
        t.u64(),
        (value) => v.u64(value as bigint),
        (sv) => (sv as Extract<SchemaValue, { tag: 'u64' }>).value,
      );
    case 'Literal': {
      const lit = ast.literal;
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
            `Effect literal of type '${typeof lit}' is not supported by the fluent SDK walker.`,
          );
      }
    }
    case 'Enums': {
      // `Schema.Enums(E)` carries `.enums: Array<[name, value]>`; cases are the
      // declaration-order values, coerced to strings (matches the Zod walker).
      const cases = (ast.enums as Array<[string, unknown]>).map(([, val]) => String(val));
      return leaf(
        t.enum(cases),
        (value) => v.enum(cases.indexOf(String(value))),
        (sv) => cases[(sv as Extract<SchemaValue, { tag: 'enum' }>).caseIndex],
      );
    }
    case 'TupleType':
      return walkTupleType(ast, reg);
    case 'TypeLiteral':
      return walkTypeLiteral(ast, reg);
    case 'Union':
      return walkUnion(ast, reg);
    default:
      throw new Error(
        `Effect Schema AST node '${tag}' is not yet supported by the fluent SDK walker ` +
          `(supported: string, number, boolean, bigint, literal, enum, tuple, array, struct/record, ` +
          `index-signature map, option (NullOr/UndefinedOr), and tagged unions).`,
      );
  }
}

/** `TupleType`: rest-only → list, fixed elements → tuple. */
function walkTupleType(ast: Ast, reg: RecursionRegistry): FluentCodec {
  const elements: Ast[] = ast.elements ?? [];
  const rest: Ast[] = ast.rest ?? [];

  if (elements.length === 0 && rest.length === 1) {
    // `Schema.Array(x)` → WIT list. Rest entries wrap the AST in `.type`.
    const elemCodec = walkAst(rest[0].type ?? rest[0], reg);
    return {
      graph: { defs: elemCodec.graph.defs, root: t.list(elemCodec.graph.root) },
      toValue: (value) => v.list((value as unknown[]).map((e) => elemCodec.toValue(e))),
      fromValue: (sv) =>
        (sv as Extract<SchemaValue, { tag: 'list' }>).elements.map((e) => elemCodec.fromValue(e)),
    };
  }

  if (rest.length === 0 && elements.length > 0) {
    // `Schema.Tuple(...)` → WIT tuple. Each element wraps the AST in `.type`.
    const itemCodecs = elements.map((el) => walkAst(el.type ?? el, reg));
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

  throw new Error('Effect mixed tuple/rest arrays are not supported by the fluent SDK walker.');
}

/** `TypeLiteral`: index signature → map, property signatures → record. */
function walkTypeLiteral(ast: Ast, reg: RecursionRegistry): FluentCodec {
  const props: Ast[] = ast.propertySignatures ?? [];
  const indexSigs: Ast[] = ast.indexSignatures ?? [];

  // `Schema.Record({ key, value })` → WIT map (no own properties).
  if (props.length === 0 && indexSigs.length === 1) {
    const is = indexSigs[0];
    const keyCodec = walkAst(is.parameter, reg);
    const valCodec = walkAst(is.type, reg);
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

  if (indexSigs.length > 0) {
    throw new Error(
      'Effect index signatures combined with properties are not supported by the fluent SDK walker.',
    );
  }

  // Plain struct → WIT record (declaration order = property-signature order).
  type FieldCodec = { name: string; codec: FluentCodec; optional: boolean };
  const fieldCodecs: FieldCodec[] = props.map((ps) => {
    const optional = ps.isOptional === true;
    // Optional Effect properties encode as `Union(T, Undefined)`; unwrap to the
    // real member so the field becomes `option<T>`.
    const codec = optional ? optionCodec(realMemberOf(ps.type), reg) : walkAst(ps.type, reg);
    return { name: String(ps.name), codec, optional };
  });
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
        const decoded = f.codec.fromValue(recFields[i]);
        if (!f.optional || decoded !== undefined) out[f.name] = decoded;
      });
      return out;
    },
    // Expose per-field codecs so the config surface can flatten nested config.
    fields: fieldCodecs.map((f) => ({ name: f.name, codec: f.codec })),
  };
}

/** From an optional property's `Union(T, Undefined)`, extract the real member T. */
function realMemberOf(ast: Ast): Ast {
  const node = unwrap(ast);
  if (node?._tag === 'Union') {
    const real = (node.types as Ast[]).filter((m) => !isNullOrUndefinedAst(unwrap(m)));
    if (real.length === 1) return real[0];
  }
  return node;
}

/** Build an `option<inner>` codec wrapping the codec for `innerAst`. */
function optionCodec(innerAst: Ast, reg: RecursionRegistry): FluentCodec {
  const innerCodec = walkAst(innerAst, reg);
  const wrapped: FluentCodec = {
    graph: { defs: innerCodec.graph.defs, root: t.option(innerCodec.graph.root) },
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
  // config surface can descend it) and flag it optional. See zod.ts.
  if (innerCodec.fields !== undefined) {
    return { ...wrapped, fields: innerCodec.fields, optionalGroup: true };
  }
  return wrapped;
}

/** `Union`: string-literal enum, NullOr/UndefinedOr option, or tagged variant. */
function walkUnion(ast: Ast, reg: RecursionRegistry): FluentCodec {
  const types: Ast[] = (ast.types ?? []).map((m: Ast) => m);

  // String-literal enum: every member is a string `Literal`.
  const unwrapped = types.map(unwrap);
  if (
    unwrapped.length > 0 &&
    unwrapped.every((m) => m?._tag === 'Literal' && typeof m.literal === 'string')
  ) {
    const cases = unwrapped.map((m) => m.literal as string);
    return leaf(
      t.enum(cases),
      (value) => v.enum(cases.indexOf(value as string)),
      (sv) => cases[(sv as Extract<SchemaValue, { tag: 'enum' }>).caseIndex],
    );
  }

  // NullOr / UndefinedOr: exactly one null/undefined member + one real member.
  const emptyMembers = unwrapped.filter((m) => isNullOrUndefinedAst(m));
  const realMembers = types.filter((m) => !isNullOrUndefinedAst(unwrap(m)));
  if (emptyMembers.length >= 1 && realMembers.length === 1) {
    return optionCodec(realMembers[0], reg);
  }

  // Tagged variant: every member is a `TypeLiteral` with a required string-literal
  // `_tag` discriminator.
  if (
    unwrapped.length > 0 &&
    unwrapped.every((m) => m?._tag === 'TypeLiteral' && tagLiteralOf(m) !== undefined)
  ) {
    return walkTaggedVariant(unwrapped, reg);
  }

  // Plain (non-tagged) union → WIT `variant` with auto-named cases
  // `case0..caseN-1`. Decode by `caseIndex`; encode disambiguates structurally
  // over the compiled member types (Effect members are AST nodes, not Standard
  // Schema values).
  if (types.length === 0) throw new Error('Effect Schema.Union(...) has no members');
  const memberCodecs = types.map((m) => walkAst(m, reg));
  return buildUnionVariantCodec(
    memberCodecs,
    (value) => memberCodecs.findIndex((c) => matchesSchemaType(c.graph.defs, c.graph.root, value)),
    'Effect Schema.Union',
  );
}

/** Required string-literal `_tag` value of a `TypeLiteral`, if present. */
function tagLiteralOf(typeLiteral: Ast): string | undefined {
  const ps = (typeLiteral.propertySignatures as Ast[]).find((p) => p.name === '_tag');
  if (!ps || ps.isOptional) return undefined;
  const ty = unwrap(ps.type);
  if (ty?._tag === 'Literal' && typeof ty.literal === 'string') return ty.literal;
  return undefined;
}

/** Build a WIT variant from tagged-union members (each `case<tag>` payload = the rest of the record). */
function walkTaggedVariant(members: Ast[], reg: RecursionRegistry): FluentCodec {
  type Case = { tag: string; payload?: FluentCodec };
  const cases: Case[] = members.map((m) => {
    const tag = tagLiteralOf(m)!;
    const rest = (m.propertySignatures as Ast[]).filter((p) => p.name !== '_tag');
    if (rest.length === 0) return { tag };
    // Build a record codec from the non-`_tag` properties.
    const payload = walkTypeLiteral({ ...m, propertySignatures: rest, indexSignatures: [] }, reg);
    return { tag, payload };
  });

  const tagToIdx = new Map(cases.map((c, i) => [c.tag, i] as const));
  const graphs: SchemaGraph[] = cases.filter((c) => c.payload).map((c) => c.payload!.graph);
  const defs = mergeGraphDefs(graphs);
  const variantCases: VariantCaseType[] = cases.map((c) =>
    variantCase(c.tag, c.payload?.graph.root),
  );

  return {
    graph: { defs, root: t.variant(variantCases) },
    toValue: (value) => {
      const obj = value as Record<string, unknown> & { _tag: string };
      const i = tagToIdx.get(obj._tag);
      if (i === undefined)
        throw new Error(`Effect tagged union: unknown _tag '${String(obj._tag)}'`);
      const c = cases[i];
      if (!c.payload) return v.variant(i, undefined);
      const { _tag, ...rest } = obj;
      void _tag;
      return v.variant(i, c.payload.toValue(rest));
    },
    fromValue: (sv) => {
      const vv = sv as Extract<SchemaValue, { tag: 'variant' }>;
      const c = cases[vv.caseIndex];
      if (!c.payload || vv.payload === undefined) return { _tag: c.tag };
      return { _tag: c.tag, ...(c.payload.fromValue(vv.payload) as Record<string, unknown>) };
    },
  };
}

const effectWalker: SchemaWalker = (schema): FluentCodec => {
  const ast = (schema as { ast?: Ast }).ast;
  if (!ast || typeof ast._tag !== 'string') {
    throw new Error(
      'Unrecognised Effect Schema shape (expected a `Schema.standardSchemaV1(schema)` wrapper carrying `.ast`)',
    );
  }
  const encoded = unwrap(ast);
  if (VOID_LIKE.has(encoded?._tag)) {
    // Unit/void: maps to WIT `output-schema.unit`; `graph` is a placeholder.
    return {
      graph: { defs: new Map(), root: t.record([]) },
      toValue: () => v.record([]),
      fromValue: () => undefined,
      isUnit: true,
    };
  }
  // One registry per top-level compile: Effect recurses over its AST internally,
  // so it drives its own cycle detection rather than the adapter's `recurse`.
  return walkAst(ast, new RecursionRegistry());
};

registerSchemaWalker('effect', effectWalker);
