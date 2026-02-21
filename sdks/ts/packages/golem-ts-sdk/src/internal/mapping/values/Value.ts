// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

import { WitNode, WitValue } from 'golem:rpc/types@0.2.2';

export type Value =
  | {
      kind: 'bool';
      value: boolean;
    }
  | {
      kind: 'u8';
      value: number;
    }
  | {
      kind: 'u16';
      value: number;
    }
  | {
      kind: 'u32';
      value: number;
    }
  | {
      kind: 'u64';
      value: bigint;
    }
  | {
      kind: 's8';
      value: number;
    }
  | {
      kind: 's16';
      value: number;
    }
  | {
      kind: 's32';
      value: number;
    }
  | {
      kind: 's64';
      value: bigint;
    }
  | {
      kind: 'f32';
      value: number;
    }
  | {
      kind: 'f64';
      value: number;
    }
  | {
      kind: 'char';
      value: string;
    }
  | {
      kind: 'string';
      value: string;
    }
  | {
      kind: 'list';
      value: Value[];
    }
  | {
      kind: 'tuple';
      value: Value[];
    }
  | {
      kind: 'record';
      value: Value[];
    }
  | {
      kind: 'variant';
      caseIdx: number;
      caseValue?: Value;
    }
  | {
      kind: 'enum';
      value: number;
    }
  | {
      kind: 'flags';
      value: boolean[];
    }
  | {
      kind: 'option';
      value?: Value;
    }
  | {
      kind: 'result';
      value: {
        ok?: Value;
        err?: Value;
      };
    }
  | {
      kind: 'handle';
      uri: string;
      resourceId: bigint;
    };

export function fromWitValue(wit: WitValue): Value {
  if (!wit.nodes.length) throw new Error('Empty nodes in WitValue');
  return buildTree(wit.nodes[0], wit.nodes);
}

function buildTree(node: WitNode, nodes: WitNode[]): Value {
  switch (node.tag) {
    case 'record-value':
      return {
        kind: 'record',
        value: node.val.map((idx) => buildTree(nodes[idx], nodes)),
      };

    case 'variant-value': {
      const [caseIdx, maybeIndex] = node.val;
      if (maybeIndex !== undefined) {
        return {
          kind: 'variant',
          caseIdx,
          caseValue: buildTree(nodes[maybeIndex], nodes),
        };
      } else {
        return {
          kind: 'variant',
          caseIdx,
          caseValue: undefined,
        };
      }
    }

    case 'enum-value':
      return {
        kind: 'enum',
        value: node.val,
      };

    case 'flags-value':
      return {
        kind: 'flags',
        value: node.val,
      };

    case 'tuple-value':
      return {
        kind: 'tuple',
        value: node.val.map((idx) => buildTree(nodes[idx], nodes)),
      };

    case 'list-value':
      return {
        kind: 'list',
        value: node.val.map((idx) => buildTree(nodes[idx], nodes)),
      };

    case 'option-value':
      if (node.val === undefined) {
        return {
          kind: 'option',
          value: undefined,
        };
      }
      return {
        kind: 'option',
        value: buildTree(nodes[node.val], nodes),
      };

    case 'result-value': {
      const res = node.val;
      if (res.tag === 'ok') {
        return {
          kind: 'result',
          value: {
            ok: res.val !== undefined ? buildTree(nodes[res.val], nodes) : undefined,
          },
        };
      } else {
        return {
          kind: 'result',
          value: {
            err: res.val !== undefined ? buildTree(nodes[res.val], nodes) : undefined,
          },
        };
      }
    }

    case 'prim-u8':
      return {
        kind: 'u8',
        value: node.val,
      };
    case 'prim-u16':
      return {
        kind: 'u16',
        value: node.val,
      };
    case 'prim-u32':
      return {
        kind: 'u32',
        value: node.val,
      };
    case 'prim-u64':
      return {
        kind: 'u64',
        value: node.val,
      };
    case 'prim-s8':
      return {
        kind: 's8',
        value: node.val,
      };
    case 'prim-s16':
      return {
        kind: 's16',
        value: node.val,
      };
    case 'prim-s32':
      return {
        kind: 's32',
        value: node.val,
      };
    case 'prim-s64':
      return {
        kind: 's64',
        value: node.val,
      };
    case 'prim-float32':
      return {
        kind: 'f32',
        value: node.val,
      };
    case 'prim-float64':
      return {
        kind: 'f64',
        value: node.val,
      };
    case 'prim-char':
      return {
        kind: 'char',
        value: node.val,
      };
    case 'prim-bool':
      return {
        kind: 'bool',
        value: node.val,
      };
    case 'prim-string':
      return {
        kind: 'string',
        value: node.val,
      };

    case 'handle': {
      const [uri, resourceId] = node.val;
      return {
        kind: 'handle',
        uri: uri.value,
        resourceId,
      };
    }

    default:
      throw new Error(`Unhandled tag: ${(node as any).tag}`);
  }
}

export function toWitValue(value: Value): WitValue {
  const nodes: WitNode[] = [];
  buildNodes(value, nodes);
  return { nodes };
}

function buildNodes(value: Value, nodes: WitNode[]): number {
  const idx = nodes.length;
  nodes.push({
    tag: 'placeholder',
    val: undefined,
  } as any);

  switch (value.kind) {
    case 'record': {
      const recordIndices = value.value.map((v) => buildNodes(v, nodes));
      nodes[idx] = {
        tag: 'record-value',
        val: recordIndices,
      };
      return idx;
    }

    case 'variant': {
      if (value.caseValue !== undefined) {
        const innerIdx = buildNodes(value.caseValue, nodes);
        nodes[idx] = {
          tag: 'variant-value',
          val: [value.caseIdx, innerIdx],
        };
      } else {
        nodes[idx] = {
          tag: 'variant-value',
          val: [value.caseIdx, undefined],
        };
      }
      return idx;
    }

    case 'enum':
      nodes[idx] = {
        tag: 'enum-value',
        val: value.value,
      };
      return idx;

    case 'flags':
      nodes[idx] = {
        tag: 'flags-value',
        val: value.value,
      };
      return idx;

    case 'tuple': {
      const tupleIndices = value.value.map((v) => buildNodes(v, nodes));
      nodes[idx] = {
        tag: 'tuple-value',
        val: tupleIndices,
      };
      return idx;
    }

    case 'list': {
      const listIndices = value.value.map((v) => buildNodes(v, nodes));
      nodes[idx] = {
        tag: 'list-value',
        val: listIndices,
      };
      return idx;
    }

    case 'option': {
      if (value.value !== undefined) {
        const innerIdx = buildNodes(value.value, nodes);
        nodes[idx] = {
          tag: 'option-value',
          val: innerIdx,
        };
      } else {
        nodes[idx] = {
          tag: 'option-value',
          val: undefined,
        };
      }
      return idx;
    }

    case 'result': {
      if ('ok' in value.value) {
        const innerIdx =
          value.value.ok !== undefined ? buildNodes(value.value.ok, nodes) : undefined;
        nodes[idx] = {
          tag: 'result-value',
          val: {
            tag: 'ok',
            val: innerIdx,
          },
        };
      } else {
        const innerIdx =
          value.value.err !== undefined ? buildNodes(value.value.err, nodes) : undefined;
        nodes[idx] = {
          tag: 'result-value',
          val: {
            tag: 'err',
            val: innerIdx,
          },
        };
      }
      return idx;
    }

    case 'u8':
      nodes[idx] = {
        tag: 'prim-u8',
        val: value.value,
      };
      return idx;
    case 'u16':
      nodes[idx] = {
        tag: 'prim-u16',
        val: value.value,
      };
      return idx;
    case 'u32':
      nodes[idx] = {
        tag: 'prim-u32',
        val: value.value,
      };
      return idx;
    case 'u64':
      nodes[idx] = {
        tag: 'prim-u64',
        val: value.value,
      };
      return idx;
    case 's8':
      nodes[idx] = {
        tag: 'prim-s8',
        val: value.value,
      };
      return idx;
    case 's16':
      nodes[idx] = {
        tag: 'prim-s16',
        val: value.value,
      };
      return idx;
    case 's32':
      nodes[idx] = {
        tag: 'prim-s32',
        val: value.value,
      };
      return idx;
    case 's64':
      nodes[idx] = {
        tag: 'prim-s64',
        val: value.value,
      };
      return idx;
    case 'f32':
      nodes[idx] = {
        tag: 'prim-float32',
        val: value.value,
      };
      return idx;
    case 'f64':
      nodes[idx] = {
        tag: 'prim-float64',
        val: value.value,
      };
      return idx;
    case 'char':
      nodes[idx] = {
        tag: 'prim-char',
        val: value.value,
      };
      return idx;
    case 'bool':
      nodes[idx] = {
        tag: 'prim-bool',
        val: value.value,
      };
      return idx;
    case 'string':
      nodes[idx] = {
        tag: 'prim-string',
        val: value.value,
      };
      return idx;

    case 'handle':
      nodes[idx] = {
        tag: 'handle',
        val: [
          {
            value: value.uri,
          },
          value.resourceId,
        ],
      };
      return idx;

    default:
      throw new Error(`Unhandled kind: ${(value as any).kind}`);
  }
}
