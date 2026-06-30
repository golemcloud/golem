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

// ArkType schema walker (vendor `"arktype"`). ArkType's structural model lives in
// the type's internal node tree (`.internal`): each node has a `.kind`
// (`domain`, `unit`, `union`, `intersection`, `structure`, `sequence`, …) plus
// `.inner` child accessors. This walker recurses over that node tree *internally*
// (children are raw internal nodes that expose `.kind` directly rather than a
// `.internal` wrapper). It does NOT `import 'arktype'`, so ArkType is never baked
// into the SDK / WASM. The per-kind `t.*` / `v.*` shapes match the Zod walker.
//
// COVERAGE: primitives (string / number→f64 / bigint→u64 / boolean), array→list,
// object→record, optional/nullable→option, tuple, string-literal union→enum,
// single literal, and index-signature record→map. ArkType has no first-class
// discriminated-union *carrier* distinct from a plain union, and it does not
// preserve object field declaration order (required keys are sorted, then
// optional keys); fields round-trip self-consistently in the walker's own
// emitted order. Plain non-literal unions are deferred with a clear error.

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

type Node = any;

/** Resolve a value to an ArkType internal node (unwrap the `.internal` wrapper). */
function nodeOf(schemaOrNode: any): Node {
  const n = schemaOrNode?.internal ?? schemaOrNode;
  if (!n || typeof n.kind !== 'string') {
    throw new Error('Unrecognised ArkType schema shape (expected an internal node with a `.kind`)');
  }
  return n;
}

function leaf(root: SchemaType, toValue: FluentCodec['toValue'], fromValue: FluentCodec['fromValue']): FluentCodec {
  return { graph: { defs: new Map(), root }, toValue, fromValue };
}

/** The literal value of a `unit` node (`.unit` / `.inner.unit`). */
function unitValueOf(node: Node): unknown {
  return 'unit' in node ? node.unit : node.inner?.unit;
}

function primitiveLiteral(lit: unknown): FluentCodec {
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
      throw new Error(`ArkType literal of type '${typeof lit}' is not supported by the fluent SDK walker.`);
  }
}

function primitiveDomain(domain: string): FluentCodec {
  switch (domain) {
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
    case 'bigint':
      return leaf(
        t.u64(),
        (value) => v.u64(value as bigint),
        (sv) => (sv as Extract<SchemaValue, { tag: 'u64' }>).value,
      );
    case 'boolean':
      return leaf(
        t.bool(),
        (value) => v.bool(value as boolean),
        (sv) => (sv as Extract<SchemaValue, { tag: 'bool' }>).value,
      );
    default:
      throw new Error(`ArkType domain '${domain}' is not supported by the fluent SDK walker.`);
  }
}

/** Detect a `boolean` node: a union of the two unit branches `false` / `true`. */
function isBooleanUnion(node: Node): boolean {
  const branches: Node[] = node.branches ?? [];
  if (branches.length !== 2) return false;
  const vals = branches.map(unitValueOf).filter((x) => typeof x === 'boolean');
  return vals.length === 2 && vals.includes(true) && vals.includes(false);
}

/** Walk an ArkType internal node into a `FluentCodec`. */
function walkNode(node: Node): FluentCodec {
  const kind: string = node.kind;
  const inner = node.inner ?? {};

  switch (kind) {
    case 'domain':
      return primitiveDomain(node.domain ?? inner.domain);

    case 'unit':
      return primitiveLiteral(unitValueOf(node));

    case 'union':
      return walkUnion(node);

    case 'intersection':
      return walkIntersection(node);

    case 'structure':
      return walkStructure(node);

    case 'sequence':
      return walkSequence(node);

    default:
      throw new Error(
        `ArkType node kind '${kind}' is not yet supported by the fluent SDK walker ` +
          `(supported: string, number, bigint, boolean, array, object, tuple, optional/nullable, ` +
          `string-literal enum, single literal, and index-signature record).`,
      );
  }
}

/**
 * `union` node: `boolean`, a string-literal enum, or an optional/nullable
 * (`T | undefined` / `T | null`). Plain non-literal unions are deferred.
 */
function walkUnion(node: Node): FluentCodec {
  if (isBooleanUnion(node)) {
    return leaf(
      t.bool(),
      (value) => v.bool(value as boolean),
      (sv) => (sv as Extract<SchemaValue, { tag: 'bool' }>).value,
    );
  }

  const branches: Node[] = node.branches ?? [];

  // Optional / nullable: exactly one `undefined`/`null` unit branch + one real
  // branch → WIT option.
  const isEmptyBranch = (b: Node): boolean => {
    const u = b.kind === 'unit' ? unitValueOf(b) : undefined;
    return u === undefined || u === null;
  };
  const emptyBranches = branches.filter((b) => b.kind === 'unit' && isEmptyBranch(b));
  const realBranches = branches.filter((b) => !(b.kind === 'unit' && isEmptyBranch(b)));
  if (emptyBranches.length >= 1 && realBranches.length === 1) {
    const innerCodec = walkNode(realBranches[0]);
    return {
      graph: { defs: innerCodec.graph.defs, root: t.option(innerCodec.graph.root) },
      toValue: (value) =>
        value === undefined || value === null ? v.option(undefined) : v.option(innerCodec.toValue(value)),
      fromValue: (sv) => {
        const opt = (sv as Extract<SchemaValue, { tag: 'option' }>).value;
        return opt === undefined ? undefined : innerCodec.fromValue(opt);
      },
    };
  }

  // String-literal enum: every branch is a string `unit`.
  if (
    branches.length > 0 &&
    branches.every((b) => b.kind === 'unit' && typeof unitValueOf(b) === 'string')
  ) {
    const cases = branches.map((b) => unitValueOf(b) as string);
    return leaf(
      t.enum(cases),
      (value) => v.enum(cases.indexOf(value as string)),
      (sv) => cases[(sv as Extract<SchemaValue, { tag: 'enum' }>).caseIndex],
    );
  }

  throw new Error(
    `Plain ArkType unions are not yet supported by the fluent SDK walker; only string-literal enums, ` +
      `'T | undefined' / 'T | null' optionals, and 'boolean' are recognised.`,
  );
}

/**
 * `intersection` node: ArkType wraps objects, arrays, and tuples as an
 * intersection of a `domain`/`proto` constraint with a `structure` node.
 */
function walkIntersection(node: Node): FluentCodec {
  const structure: Node | undefined = node.inner?.structure;
  if (!structure) {
    throw new Error('ArkType intersection without a `structure` child is not supported by the fluent SDK walker.');
  }
  return walkStructure(structure);
}

/** `structure` node: object/record (`required`/`optional`/`index`) or array/tuple (`sequence`). */
function walkStructure(node: Node): FluentCodec {
  const inner = node.inner ?? node;

  if (inner.sequence) {
    return walkSequence(inner.sequence);
  }

  // Index signature only → WIT map.
  const required: Node[] = inner.required ?? [];
  const optional: Node[] = inner.optional ?? [];
  const index: Node[] = inner.index ?? [];

  if (required.length === 0 && optional.length === 0 && index.length === 1) {
    const idx = index[0];
    const keyCodec = walkNode(nodeOf(idx.signature));
    const valCodec = walkNode(nodeOf(idx.value));
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

  if (index.length > 0) {
    throw new Error('ArkType index signatures combined with properties are not supported by the fluent SDK walker.');
  }

  // Object → WIT record. ArkType does not preserve declaration order: required
  // keys come first (sorted), then optional keys. We emit fields in that same
  // order and encode/decode in lockstep, so the round-trip is self-consistent.
  type FieldCodec = { name: string; codec: FluentCodec; optional: boolean };
  const fieldCodecs: FieldCodec[] = [
    ...required.map((r) => ({ name: r.key as string, codec: walkNode(nodeOf(r.value)), optional: false })),
    ...optional.map((o) => ({
      name: o.key as string,
      codec: optionCodec(walkNode(nodeOf(o.value))),
      optional: true,
    })),
  ];
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
  };
}

/** Wrap a codec in `option<inner>` for ArkType optional object properties. */
function optionCodec(innerCodec: FluentCodec): FluentCodec {
  return {
    graph: { defs: innerCodec.graph.defs, root: t.option(innerCodec.graph.root) },
    toValue: (value) =>
      value === undefined || value === null ? v.option(undefined) : v.option(innerCodec.toValue(value)),
    fromValue: (sv) => {
      const opt = (sv as Extract<SchemaValue, { tag: 'option' }>).value;
      return opt === undefined ? undefined : innerCodec.fromValue(opt);
    },
  };
}

/** `sequence` node: variadic (`.variadic`) → list, fixed prefix (`.prefix`) → tuple. */
function walkSequence(node: Node): FluentCodec {
  const inner = node.inner ?? node;

  if (inner.variadic) {
    const elemCodec = walkNode(nodeOf(inner.variadic));
    return {
      graph: { defs: elemCodec.graph.defs, root: t.list(elemCodec.graph.root) },
      toValue: (value) => v.list((value as unknown[]).map((e) => elemCodec.toValue(e))),
      fromValue: (sv) =>
        (sv as Extract<SchemaValue, { tag: 'list' }>).elements.map((e) => elemCodec.fromValue(e)),
    };
  }

  if (Array.isArray(inner.prefix) && inner.prefix.length > 0) {
    const itemCodecs = (inner.prefix as Node[]).map((p) => walkNode(nodeOf(p)));
    const defs = mergeGraphDefs(itemCodecs.map((c) => c.graph));
    return {
      graph: { defs, root: t.tuple(itemCodecs.map((c) => c.graph.root)) },
      toValue: (value) => v.tuple((value as unknown[]).map((e, i) => itemCodecs[i].toValue(e))),
      fromValue: (sv) =>
        (sv as Extract<SchemaValue, { tag: 'tuple' }>).elements.map((e, i) => itemCodecs[i].fromValue(e)),
    };
  }

  throw new Error('ArkType sequence with neither a variadic element nor a fixed prefix is not supported.');
}

const arktypeWalker: SchemaWalker = (schema): FluentCodec => {
  return walkNode(nodeOf(schema));
};

registerSchemaWalker('arktype', arktypeWalker);
