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

import { WitNode, WitValue } from 'golem:core/types@1.5.0';

/**
 * Builds a flat WitNode[] array using the placeholder-then-backpatch pattern,
 * matching the Rust SDK's WitValueBuilder. Composite nodes (record, tuple, list,
 * option, variant, result) are pushed first with placeholder children, then
 * backpatched via finishSeq() / finishChild() once child indices are known.
 * This ensures the root is always at index 0 without any reordering step.
 */
export class WitNodeBuilder {
  readonly nodes: WitNode[] = [];

  // ── Primitives (leaf nodes) ──────────────────────────────────────────

  u8(value: number): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'prim-u8', val: value });
    return idx;
  }

  u16(value: number): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'prim-u16', val: value });
    return idx;
  }

  u32(value: number): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'prim-u32', val: value });
    return idx;
  }

  u64(value: bigint): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'prim-u64', val: value });
    return idx;
  }

  s8(value: number): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'prim-s8', val: value });
    return idx;
  }

  s16(value: number): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'prim-s16', val: value });
    return idx;
  }

  s32(value: number): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'prim-s32', val: value });
    return idx;
  }

  s64(value: bigint): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'prim-s64', val: value });
    return idx;
  }

  f32(value: number): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'prim-float32', val: value });
    return idx;
  }

  f64(value: number): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'prim-float64', val: value });
    return idx;
  }

  char(value: string): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'prim-char', val: value });
    return idx;
  }

  bool(value: boolean): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'prim-bool', val: value });
    return idx;
  }

  string(value: string): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'prim-string', val: value });
    return idx;
  }

  enumValue(caseIdx: number): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'enum-value', val: caseIdx });
    return idx;
  }

  flags(values: boolean[]): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'flags-value', val: values });
    return idx;
  }

  handle(uri: string, resourceId: bigint): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'handle', val: [{ value: uri }, resourceId] });
    return idx;
  }

  // ── Composite nodes (placeholder-then-backpatch) ─────────────────────

  addRecord(): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'record-value', val: [] });
    return idx;
  }

  addTuple(): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'tuple-value', val: [] });
    return idx;
  }

  addList(): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'list-value', val: [] });
    return idx;
  }

  addOptionSome(): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'option-value', val: -1 });
    return idx;
  }

  optionNone(): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'option-value', val: undefined });
    return idx;
  }

  addVariant(caseIdx: number): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'variant-value', val: [caseIdx, -1] });
    return idx;
  }

  variantUnit(caseIdx: number): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'variant-value', val: [caseIdx, undefined] });
    return idx;
  }

  addResultOk(): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'result-value', val: { tag: 'ok', val: -1 } });
    return idx;
  }

  resultOkUnit(): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'result-value', val: { tag: 'ok', val: undefined } });
    return idx;
  }

  addResultErr(): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'result-value', val: { tag: 'err', val: -1 } });
    return idx;
  }

  resultErrUnit(): number {
    const idx = this.nodes.length;
    this.nodes.push({ tag: 'result-value', val: { tag: 'err', val: undefined } });
    return idx;
  }

  // ── Backpatch methods ────────────────────────────────────────────────

  finishSeq(targetIdx: number, childIndices: number[]): void {
    const node = this.nodes[targetIdx];
    switch (node.tag) {
      case 'record-value':
      case 'tuple-value':
      case 'list-value':
        node.val = childIndices;
        break;
      default:
        throw new Error(`finishSeq called on a node that is not record/tuple/list: ${node.tag}`);
    }
  }

  finishChild(targetIdx: number, childIdx: number): void {
    const node = this.nodes[targetIdx];
    switch (node.tag) {
      case 'option-value':
        (node as any).val = childIdx;
        break;
      case 'variant-value':
        node.val[1] = childIdx;
        break;
      case 'result-value':
        node.val.val = childIdx;
        break;
      default:
        throw new Error(`finishChild called on a node that is not option/variant/result: ${node.tag}`);
    }
  }

  // ── Build ────────────────────────────────────────────────────────────

  build(): WitValue {
    return { nodes: this.nodes };
  }
}
