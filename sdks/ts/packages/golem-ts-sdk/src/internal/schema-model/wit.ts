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

// Codecs between the recursive in-memory schema model (./model) and the flat
// `golem:core/types@2.0.0` WIT carriers (`schema-graph`, `schema-value-tree`,
// `typed-schema-value`).
//
// Flattening assigns a `type-node` / `value-node` index to every node and
// records definitions in a deterministic order (sorted by id). `ref` bodies
// flatten to `ref-type(def-index)`. Unflattening walks indices back into the
// recursive form, guarding against out-of-range and cyclic indices.

import type {
  SchemaGraph as WitSchemaGraph,
  SchemaTypeNode as WitSchemaTypeNode,
  SchemaTypeBody as WitSchemaTypeBody,
  SchemaTypeDef as WitSchemaTypeDef,
  SchemaValueTree as WitSchemaValueTree,
  SchemaValueNode as WitSchemaValueNode,
  TypeNodeIndex,
  ValueNodeIndex,
  DefIndex,
  TypedSchemaValue as WitTypedSchemaValue,
} from 'golem:core/types@2.0.0';

import {
  type SchemaGraph,
  type SchemaType,
  type SchemaTypeBody,
  type SchemaTypeDef,
  type SchemaValue,
  type TypedSchemaValue,
  type TypeId,
  emptyMetadata,
} from './model';
import { SchemaDecodeError, SchemaEncodeError } from './errors';

// ============================================================
// Schema type / graph
// ============================================================

/**
 * Incremental encoder for a single flat {@link WitSchemaGraph} that holds
 * several independent root types in one shared `type-nodes` pool. Mirrors the
 * Rust `golem-schema` `GraphEncoder`.
 *
 * Seed the encoder with the agent's merged named definitions, then call
 * {@link GraphEncoder.encodeType} once per inline root (constructor / method /
 * config), collecting the returned `type-node-index` values, and finally
 * {@link GraphEncoder.finish} to obtain the graph with a placeholder root. The
 * graph's own `root` is a structurally-required placeholder (an empty record);
 * agent-layer carriers never consult it — the real roots are the returned
 * indices.
 *
 * Do NOT encode each agent root via {@link schemaGraphToWit} and reuse those
 * indices: per-graph node indices are only valid within a single encoding.
 */
export class GraphEncoder {
  private readonly typeNodes: WitSchemaTypeNode[] = [];
  private readonly defIndexById = new Map<TypeId, number>();
  private readonly witDefs: WitSchemaTypeDef[];

  constructor(defs: ReadonlyMap<TypeId, SchemaTypeDef>) {
    // Deterministic def ordering (sorted by id); `ref-type` carries the def index.
    const ids = [...defs.keys()].sort();
    ids.forEach((id, i) => this.defIndexById.set(id, i));

    // Reserve def slots first so `ref-type` resolves forward references during
    // body encoding, then fill each body.
    this.witDefs = ids.map((id) => ({ id, name: defs.get(id)!.name, body: -1 }));
    ids.forEach((id, i) => {
      this.witDefs[i].body = this.encodeType(defs.get(id)!.body);
    });
  }

  /** Flatten one (possibly recursive) schema type into the shared pool and return its index. */
  encodeType(st: SchemaType): TypeNodeIndex {
    const body = this.encodeBody(st.body);
    this.typeNodes.push({ body, metadata: st.metadata });
    return this.typeNodes.length - 1;
  }

  private encodeBody(body: SchemaTypeBody): WitSchemaTypeBody {
    switch (body.tag) {
      case 'ref': {
        const di = this.defIndexById.get(body.id);
        if (di === undefined) {
          throw new SchemaEncodeError(`schema graph references unknown type id '${body.id}'`);
        }
        return { tag: 'ref-type', val: di };
      }
      case 'bool':
        return { tag: 'bool-type' };
      case 's8':
        return { tag: 's8-type' };
      case 's16':
        return { tag: 's16-type' };
      case 's32':
        return { tag: 's32-type' };
      case 's64':
        return { tag: 's64-type' };
      case 'u8':
        return { tag: 'u8-type' };
      case 'u16':
        return { tag: 'u16-type' };
      case 'u32':
        return { tag: 'u32-type' };
      case 'u64':
        return { tag: 'u64-type' };
      case 'f32':
        return { tag: 'f32-type' };
      case 'f64':
        return { tag: 'f64-type' };
      case 'char':
        return { tag: 'char-type' };
      case 'string':
        return { tag: 'string-type' };
      case 'record':
        return {
          tag: 'record-type',
          val: body.fields.map((f) => ({
            name: f.name,
            body: this.encodeType(f.body),
            metadata: f.metadata,
          })),
        };
      case 'variant':
        return {
          tag: 'variant-type',
          val: body.cases.map((c) => ({
            name: c.name,
            payload: c.payload !== undefined ? this.encodeType(c.payload) : undefined,
            metadata: c.metadata,
          })),
        };
      case 'enum':
        return { tag: 'enum-type', val: body.cases };
      case 'flags':
        return { tag: 'flags-type', val: body.names };
      case 'tuple':
        return { tag: 'tuple-type', val: body.elements.map((e) => this.encodeType(e)) };
      case 'list':
        return { tag: 'list-type', val: this.encodeType(body.element) };
      case 'fixed-list':
        return {
          tag: 'fixed-list-type',
          val: { element: this.encodeType(body.element), length: body.length },
        };
      case 'map':
        return {
          tag: 'map-type',
          val: { key: this.encodeType(body.key), value: this.encodeType(body.value) },
        };
      case 'option':
        return { tag: 'option-type', val: this.encodeType(body.element) };
      case 'result':
        return {
          tag: 'result-type',
          val: {
            ok: body.ok !== undefined ? this.encodeType(body.ok) : undefined,
            err: body.err !== undefined ? this.encodeType(body.err) : undefined,
          },
        };
      case 'text':
        return { tag: 'text-type', val: body.restrictions };
      case 'binary':
        return { tag: 'binary-type', val: body.restrictions };
      case 'path':
        return { tag: 'path-type', val: body.spec };
      case 'url':
        return { tag: 'url-type', val: body.restrictions };
      case 'datetime':
        return { tag: 'datetime-type' };
      case 'duration':
        return { tag: 'duration-type' };
      case 'quantity':
        return { tag: 'quantity-type', val: body.spec };
      case 'union':
        return {
          tag: 'union-type',
          val: {
            branches: body.branches.map((br) => ({
              tag: br.tag,
              body: this.encodeType(br.body),
              discriminator: br.discriminator,
              metadata: br.metadata,
            })),
          },
        };
      case 'secret':
        return { tag: 'secret-type', val: body.spec };
      case 'quota-token':
        return { tag: 'quota-token-type', val: body.spec };
      case 'future':
        return {
          tag: 'future-type',
          val: body.element !== undefined ? this.encodeType(body.element) : undefined,
        };
      case 'stream':
        return {
          tag: 'stream-type',
          val: body.element !== undefined ? this.encodeType(body.element) : undefined,
        };
      default:
        throw new SchemaEncodeError(
          `unknown schema type body tag '${(body as { tag: string }).tag}'`,
        );
    }
  }

  /** Encode `root` as the graph root and return the finished flat graph. */
  encodeGraphRoot(root: SchemaType): WitSchemaGraph {
    const rootIdx = this.encodeType(root);
    return { typeNodes: this.typeNodes, defs: this.witDefs, root: rootIdx };
  }

  /**
   * Finish the graph with a placeholder empty-record root. Use after collecting
   * the real root indices via {@link GraphEncoder.encodeType}.
   */
  finish(): WitSchemaGraph {
    const root = this.encodeType({ body: { tag: 'record', fields: [] }, metadata: emptyMetadata() });
    return { typeNodes: this.typeNodes, defs: this.witDefs, root };
  }
}

export function schemaGraphToWit(graph: SchemaGraph): WitSchemaGraph {
  return new GraphEncoder(graph.defs).encodeGraphRoot(graph.root);
}

export function schemaGraphFromWit(wit: WitSchemaGraph): SchemaGraph {
  const nodes = wit.typeNodes;
  const witDefs = wit.defs;
  // See `schemaValueFromWit`: a flat on-path `Uint8Array` (`1` = on the current
  // DFS path) replaces a `Set<number>` + `try/finally`, with identical
  // cycle-detection semantics. Legitimate recursion goes through `ref-type`
  // (which resolves to a def id without recursing here), so only a structural
  // back-edge in raw type-node indices is reported as a cycle.
  const onPath = new Uint8Array(nodes.length);

  function idByDefIndex(di: DefIndex): TypeId {
    if (di < 0 || di >= witDefs.length) {
      throw new SchemaDecodeError(`def index out of range: ${di} (defs: ${witDefs.length})`);
    }
    return witDefs[di].id;
  }

  function fromType(idx: TypeNodeIndex): SchemaType {
    if (idx < 0 || idx >= nodes.length) {
      throw new SchemaDecodeError(`type node index out of range: ${idx} (nodes: ${nodes.length})`);
    }
    if (onPath[idx] === 1) {
      throw new SchemaDecodeError(`cyclic type node reference at index ${idx}`);
    }
    onPath[idx] = 1;
    const node = nodes[idx];
    const result = { body: fromBody(node.body), metadata: node.metadata };
    onPath[idx] = 0;
    return result;
  }

  function fromBody(body: WitSchemaTypeBody): SchemaTypeBody {
    switch (body.tag) {
      case 'ref-type':
        return { tag: 'ref', id: idByDefIndex(body.val) };
      case 'bool-type':
        return { tag: 'bool' };
      case 's8-type':
        return { tag: 's8' };
      case 's16-type':
        return { tag: 's16' };
      case 's32-type':
        return { tag: 's32' };
      case 's64-type':
        return { tag: 's64' };
      case 'u8-type':
        return { tag: 'u8' };
      case 'u16-type':
        return { tag: 'u16' };
      case 'u32-type':
        return { tag: 'u32' };
      case 'u64-type':
        return { tag: 'u64' };
      case 'f32-type':
        return { tag: 'f32' };
      case 'f64-type':
        return { tag: 'f64' };
      case 'char-type':
        return { tag: 'char' };
      case 'string-type':
        return { tag: 'string' };
      case 'record-type':
        return {
          tag: 'record',
          fields: body.val.map((f) => ({
            name: f.name,
            body: fromType(f.body),
            metadata: f.metadata,
          })),
        };
      case 'variant-type':
        return {
          tag: 'variant',
          cases: body.val.map((c) => ({
            name: c.name,
            payload: c.payload !== undefined ? fromType(c.payload) : undefined,
            metadata: c.metadata,
          })),
        };
      case 'enum-type':
        return { tag: 'enum', cases: body.val };
      case 'flags-type':
        return { tag: 'flags', names: body.val };
      case 'tuple-type':
        return { tag: 'tuple', elements: body.val.map((i) => fromType(i)) };
      case 'list-type':
        return { tag: 'list', element: fromType(body.val) };
      case 'fixed-list-type':
        return { tag: 'fixed-list', element: fromType(body.val.element), length: body.val.length };
      case 'map-type':
        return { tag: 'map', key: fromType(body.val.key), value: fromType(body.val.value) };
      case 'option-type':
        return { tag: 'option', element: fromType(body.val) };
      case 'result-type':
        return {
          tag: 'result',
          ok: body.val.ok !== undefined ? fromType(body.val.ok) : undefined,
          err: body.val.err !== undefined ? fromType(body.val.err) : undefined,
        };
      case 'text-type':
        return { tag: 'text', restrictions: body.val };
      case 'binary-type':
        return { tag: 'binary', restrictions: body.val };
      case 'path-type':
        return { tag: 'path', spec: body.val };
      case 'url-type':
        return { tag: 'url', restrictions: body.val };
      case 'datetime-type':
        return { tag: 'datetime' };
      case 'duration-type':
        return { tag: 'duration' };
      case 'quantity-type':
        return { tag: 'quantity', spec: body.val };
      case 'union-type':
        return {
          tag: 'union',
          branches: body.val.branches.map((br) => ({
            tag: br.tag,
            body: fromType(br.body),
            discriminator: br.discriminator,
            metadata: br.metadata,
          })),
        };
      case 'secret-type':
        return { tag: 'secret', spec: body.val };
      case 'quota-token-type':
        return { tag: 'quota-token', spec: body.val };
      case 'future-type':
        return { tag: 'future', element: body.val !== undefined ? fromType(body.val) : undefined };
      case 'stream-type':
        return { tag: 'stream', element: body.val !== undefined ? fromType(body.val) : undefined };
      default:
        throw new SchemaDecodeError(
          `unknown schema type body tag '${(body as { tag: string }).tag}'`,
        );
    }
  }

  const defs = new Map<TypeId, SchemaTypeDef>();
  for (const d of witDefs) {
    if (defs.has(d.id)) {
      throw new SchemaDecodeError(`duplicate def id '${d.id}' in schema graph`);
    }
    defs.set(d.id, { name: d.name, body: fromType(d.body) });
  }
  const root = fromType(wit.root);
  return { defs, root };
}

// ============================================================
// Schema value
// ============================================================

export function schemaValueToWit(value: SchemaValue): WitSchemaValueTree {
  const valueNodes: WitSchemaValueNode[] = [];

  function emit(v: SchemaValue): ValueNodeIndex {
    valueNodes.push(emitNode(v));
    return valueNodes.length - 1;
  }

  function emitNode(v: SchemaValue): WitSchemaValueNode {
    switch (v.tag) {
      case 'bool':
        return { tag: 'bool-value', val: v.value };
      case 's8':
        return { tag: 's8-value', val: v.value };
      case 's16':
        return { tag: 's16-value', val: v.value };
      case 's32':
        return { tag: 's32-value', val: v.value };
      case 's64':
        return { tag: 's64-value', val: v.value };
      case 'u8':
        return { tag: 'u8-value', val: v.value };
      case 'u16':
        return { tag: 'u16-value', val: v.value };
      case 'u32':
        return { tag: 'u32-value', val: v.value };
      case 'u64':
        return { tag: 'u64-value', val: v.value };
      case 'f32':
        return { tag: 'f32-value', val: v.value };
      case 'f64':
        return { tag: 'f64-value', val: v.value };
      case 'char':
        return { tag: 'char-value', val: v.value };
      case 'string':
        return { tag: 'string-value', val: v.value };
      case 'record':
        return { tag: 'record-value', val: v.fields.map((f) => emit(f)) };
      case 'variant':
        return {
          tag: 'variant-value',
          val: {
            case_: v.caseIndex,
            payload: v.payload !== undefined ? emit(v.payload) : undefined,
          },
        };
      case 'enum':
        return { tag: 'enum-value', val: v.caseIndex };
      case 'flags':
        return { tag: 'flags-value', val: v.flags };
      case 'tuple':
        return { tag: 'tuple-value', val: v.elements.map((e) => emit(e)) };
      case 'list':
        return { tag: 'list-value', val: v.elements.map((e) => emit(e)) };
      case 'fixed-list':
        return { tag: 'fixed-list-value', val: v.elements.map((e) => emit(e)) };
      case 'map':
        return {
          tag: 'map-value',
          val: v.entries.map((e) => ({ key: emit(e.key), value: emit(e.value) })),
        };
      case 'option':
        return { tag: 'option-value', val: v.value !== undefined ? emit(v.value) : undefined };
      case 'result':
        return {
          tag: 'result-value',
          val:
            v.result.tag === 'ok'
              ? {
                  tag: 'ok-value',
                  val: v.result.value !== undefined ? emit(v.result.value) : undefined,
                }
              : {
                  tag: 'err-value',
                  val: v.result.value !== undefined ? emit(v.result.value) : undefined,
                },
        };
      case 'text':
        return { tag: 'text-value', val: { text: v.text, language: v.language } };
      case 'binary':
        return { tag: 'binary-value', val: { bytes: v.bytes, mimeType: v.mimeType } };
      case 'path':
        return { tag: 'path-value', val: v.value };
      case 'url':
        return { tag: 'url-value', val: v.value };
      case 'datetime':
        return { tag: 'datetime-value', val: v.value };
      case 'duration':
        return { tag: 'duration-value', val: { nanoseconds: v.nanoseconds } };
      case 'quantity':
        return { tag: 'quantity-value-node', val: v.value };
      case 'union':
        return { tag: 'union-value', val: { tag: v.unionTag, body: emit(v.body) } };
      case 'secret':
        return { tag: 'secret-value', val: { secretRef: v.secretRef } };
      case 'quota-token':
        return { tag: 'quota-token-value', val: v.value };
      default:
        throw new SchemaEncodeError(`unknown schema value tag '${(v as { tag: string }).tag}'`);
    }
  }

  const root = emit(value);
  return { valueNodes, root };
}

export function schemaValueFromWit(wit: WitSchemaValueTree): SchemaValue {
  const nodes = wit.valueNodes;
  // Cycle guard tracking the nodes currently on the DFS path: `1` = on path,
  // `0` = off path. A flat `Uint8Array` indexed by node is much cheaper than a
  // `Set<number>` (no per-node hashing for has/add/delete) and lets us drop the
  // per-recursion `try/finally`. Semantics are identical to the previous Set:
  // a back-edge to a node still on the current path is a cycle, while a node
  // shared across sibling branches (a DAG, not a cycle) is re-decoded. On any
  // thrown error the whole decode is aborted and this local array is discarded,
  // so leaving stale `1`s during unwinding is harmless.
  const onPath = new Uint8Array(nodes.length);

  function fromIdx(idx: ValueNodeIndex): SchemaValue {
    if (idx < 0 || idx >= nodes.length) {
      throw new SchemaDecodeError(`value node index out of range: ${idx} (nodes: ${nodes.length})`);
    }
    if (onPath[idx] === 1) {
      throw new SchemaDecodeError(`cyclic value node reference at index ${idx}`);
    }
    onPath[idx] = 1;
    const result = fromNode(nodes[idx]);
    onPath[idx] = 0;
    return result;
  }

  function fromNode(n: WitSchemaValueNode): SchemaValue {
    switch (n.tag) {
      case 'bool-value':
        return { tag: 'bool', value: n.val };
      case 's8-value':
        return { tag: 's8', value: n.val };
      case 's16-value':
        return { tag: 's16', value: n.val };
      case 's32-value':
        return { tag: 's32', value: n.val };
      case 's64-value':
        return { tag: 's64', value: n.val };
      case 'u8-value':
        return { tag: 'u8', value: n.val };
      case 'u16-value':
        return { tag: 'u16', value: n.val };
      case 'u32-value':
        return { tag: 'u32', value: n.val };
      case 'u64-value':
        return { tag: 'u64', value: n.val };
      case 'f32-value':
        return { tag: 'f32', value: n.val };
      case 'f64-value':
        return { tag: 'f64', value: n.val };
      case 'char-value':
        return { tag: 'char', value: n.val };
      case 'string-value':
        return { tag: 'string', value: n.val };
      case 'record-value':
        return { tag: 'record', fields: n.val.map((i) => fromIdx(i)) };
      case 'variant-value':
        return {
          tag: 'variant',
          caseIndex: n.val.case_,
          payload: n.val.payload !== undefined ? fromIdx(n.val.payload) : undefined,
        };
      case 'enum-value':
        return { tag: 'enum', caseIndex: n.val };
      case 'flags-value':
        return { tag: 'flags', flags: n.val };
      case 'tuple-value':
        return { tag: 'tuple', elements: n.val.map((i) => fromIdx(i)) };
      case 'list-value':
        return { tag: 'list', elements: n.val.map((i) => fromIdx(i)) };
      case 'fixed-list-value':
        return { tag: 'fixed-list', elements: n.val.map((i) => fromIdx(i)) };
      case 'map-value':
        return {
          tag: 'map',
          entries: n.val.map((e) => ({ key: fromIdx(e.key), value: fromIdx(e.value) })),
        };
      case 'option-value':
        return { tag: 'option', value: n.val !== undefined ? fromIdx(n.val) : undefined };
      case 'result-value': {
        const r = n.val;
        switch (r.tag) {
          case 'ok-value':
            return {
              tag: 'result',
              result: { tag: 'ok', value: r.val !== undefined ? fromIdx(r.val) : undefined },
            };
          case 'err-value':
            return {
              tag: 'result',
              result: { tag: 'err', value: r.val !== undefined ? fromIdx(r.val) : undefined },
            };
          default:
            throw new SchemaDecodeError(
              `unknown result value payload tag '${(r as { tag: string }).tag}'`,
            );
        }
      }
      case 'text-value':
        return { tag: 'text', text: n.val.text, language: n.val.language };
      case 'binary-value':
        return { tag: 'binary', bytes: n.val.bytes, mimeType: n.val.mimeType };
      case 'path-value':
        return { tag: 'path', value: n.val };
      case 'url-value':
        return { tag: 'url', value: n.val };
      case 'datetime-value':
        return { tag: 'datetime', value: n.val };
      case 'duration-value':
        return { tag: 'duration', nanoseconds: n.val.nanoseconds };
      case 'quantity-value-node':
        return { tag: 'quantity', value: n.val };
      case 'union-value':
        return { tag: 'union', unionTag: n.val.tag, body: fromIdx(n.val.body) };
      case 'secret-value':
        return { tag: 'secret', secretRef: n.val.secretRef };
      case 'quota-token-value':
        return { tag: 'quota-token', value: n.val };
      default:
        throw new SchemaDecodeError(
          `unknown schema value node tag '${(n as { tag: string }).tag}'`,
        );
    }
  }

  return fromIdx(wit.root);
}

// ============================================================
// Typed schema value
// ============================================================

export function typedSchemaValueToWit(tv: TypedSchemaValue): WitTypedSchemaValue {
  return { graph: schemaGraphToWit(tv.graph), value: schemaValueToWit(tv.value) };
}

export function typedSchemaValueFromWit(wit: WitTypedSchemaValue): TypedSchemaValue {
  return { graph: schemaGraphFromWit(wit.graph), value: schemaValueFromWit(wit.value) };
}
