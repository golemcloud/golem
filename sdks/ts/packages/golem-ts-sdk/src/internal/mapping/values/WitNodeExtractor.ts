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

import { WitNode } from 'golem:core/types@1.5.0';

export class WitNodeExtractor {
  constructor(
    private readonly nodes: WitNode[],
    private readonly index: number = 0,
  ) {}

  u8(): number | undefined {
    const n = this.nodes[this.index];
    return n.tag === 'prim-u8' ? n.val : undefined;
  }

  u16(): number | undefined {
    const n = this.nodes[this.index];
    return n.tag === 'prim-u16' ? n.val : undefined;
  }

  u32(): number | undefined {
    const n = this.nodes[this.index];
    return n.tag === 'prim-u32' ? n.val : undefined;
  }

  u64(): bigint | undefined {
    const n = this.nodes[this.index];
    return n.tag === 'prim-u64' ? n.val : undefined;
  }

  s8(): number | undefined {
    const n = this.nodes[this.index];
    return n.tag === 'prim-s8' ? n.val : undefined;
  }

  s16(): number | undefined {
    const n = this.nodes[this.index];
    return n.tag === 'prim-s16' ? n.val : undefined;
  }

  s32(): number | undefined {
    const n = this.nodes[this.index];
    return n.tag === 'prim-s32' ? n.val : undefined;
  }

  s64(): bigint | undefined {
    const n = this.nodes[this.index];
    return n.tag === 'prim-s64' ? n.val : undefined;
  }

  f32(): number | undefined {
    const n = this.nodes[this.index];
    return n.tag === 'prim-float32' ? n.val : undefined;
  }

  f64(): number | undefined {
    const n = this.nodes[this.index];
    return n.tag === 'prim-float64' ? n.val : undefined;
  }

  char(): string | undefined {
    const n = this.nodes[this.index];
    return n.tag === 'prim-char' ? n.val : undefined;
  }

  bool(): boolean | undefined {
    const n = this.nodes[this.index];
    return n.tag === 'prim-bool' ? n.val : undefined;
  }

  string(): string | undefined {
    const n = this.nodes[this.index];
    return n.tag === 'prim-string' ? n.val : undefined;
  }

  field(fieldIdx: number): WitNodeExtractor | undefined {
    const n = this.nodes[this.index];
    if (n.tag !== 'record-value') return undefined;
    const childIdx = n.val[fieldIdx];
    if (childIdx === undefined) return undefined;
    return new WitNodeExtractor(this.nodes, childIdx);
  }

  tupleElement(elemIdx: number): WitNodeExtractor | undefined {
    const n = this.nodes[this.index];
    if (n.tag !== 'tuple-value') return undefined;
    const childIdx = n.val[elemIdx];
    if (childIdx === undefined) return undefined;
    return new WitNodeExtractor(this.nodes, childIdx);
  }

  tupleLength(): number | undefined {
    const n = this.nodes[this.index];
    if (n.tag !== 'tuple-value') return undefined;
    return n.val.length;
  }

  listElements(): WitNodeExtractor[] | undefined {
    const n = this.nodes[this.index];
    if (n.tag !== 'list-value') return undefined;
    return n.val.map((idx) => new WitNodeExtractor(this.nodes, idx));
  }

  listLength(): number | undefined {
    const n = this.nodes[this.index];
    if (n.tag !== 'list-value') return undefined;
    return n.val.length;
  }

  listElement(elemIdx: number): WitNodeExtractor | undefined {
    const n = this.nodes[this.index];
    if (n.tag !== 'list-value') return undefined;
    const childIdx = n.val[elemIdx];
    if (childIdx === undefined) return undefined;
    return new WitNodeExtractor(this.nodes, childIdx);
  }

  // Returns: undefined = wrong tag, null = None, WitNodeExtractor = Some
  option(): WitNodeExtractor | null | undefined {
    const n = this.nodes[this.index];
    if (n.tag !== 'option-value') return undefined;
    if (n.val === undefined) return null;
    return new WitNodeExtractor(this.nodes, n.val);
  }

  variant(): { caseIdx: number; inner?: WitNodeExtractor } | undefined {
    const n = this.nodes[this.index];
    if (n.tag !== 'variant-value') return undefined;
    const [caseIdx, maybeChildIdx] = n.val;
    return {
      caseIdx,
      inner:
        maybeChildIdx !== undefined ? new WitNodeExtractor(this.nodes, maybeChildIdx) : undefined,
    };
  }

  enumValue(): number | undefined {
    const n = this.nodes[this.index];
    return n.tag === 'enum-value' ? n.val : undefined;
  }

  flags(): boolean[] | undefined {
    const n = this.nodes[this.index];
    return n.tag === 'flags-value' ? n.val : undefined;
  }

  result(): { tag: 'ok' | 'err'; inner?: WitNodeExtractor } | undefined {
    const n = this.nodes[this.index];
    if (n.tag !== 'result-value') return undefined;
    return {
      tag: n.val.tag,
      inner: n.val.val !== undefined ? new WitNodeExtractor(this.nodes, n.val.val) : undefined,
    };
  }

  handle(): { uri: string; resourceId: bigint } | undefined {
    const n = this.nodes[this.index];
    if (n.tag !== 'handle') return undefined;
    return { uri: n.val[0].value, resourceId: n.val[1] };
  }

  // Returns true if the current node is a record-value
  isRecord(): boolean {
    return this.nodes[this.index].tag === 'record-value';
  }

  recordLength(): number | undefined {
    const n = this.nodes[this.index];
    return n.tag === 'record-value' ? n.val.length : undefined;
  }

  // Returns true if the current node is an option-value
  isOption(): boolean {
    return this.nodes[this.index].tag === 'option-value';
  }

  // Returns the number for any numeric prim type
  number(): number | undefined {
    const n = this.nodes[this.index];
    switch (n.tag) {
      case 'prim-u8':
      case 'prim-u16':
      case 'prim-u32':
      case 'prim-s8':
      case 'prim-s16':
      case 'prim-s32':
      case 'prim-float32':
      case 'prim-float64':
        return n.val;
      case 'prim-u64':
      case 'prim-s64':
        return Number(n.val);
      default:
        return undefined;
    }
  }

  // Returns bigint for u64 or s64
  bigint(): bigint | undefined {
    const n = this.nodes[this.index];
    if (n.tag === 'prim-u64' || n.tag === 'prim-s64') return n.val;
    return undefined;
  }

  tag(): string {
    return this.nodes[this.index].tag;
  }
}
