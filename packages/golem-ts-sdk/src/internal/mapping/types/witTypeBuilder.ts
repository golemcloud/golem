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

// Copied from wasm-rpc rust implementation
import {NamedWitTypeNode, NodeIndex, ResourceMode, WitTypeNode} from "golem:rpc/types@0.2.2";
import {AnalysedType, getNameFromAnalysedType, getOwnerFromAnalysedType, NameOptionTypePair, NameTypePair} from "./AnalysedType";
import {WitType} from "golem:agent/common";

export class WitTypeBuilder {
    private nodes: NamedWitTypeNode[] = [];
    private mapping = new Map<string, number>();

    add(typ: AnalysedType): NodeIndex {
        const hash = JSON.stringify(typ);
        if (this.mapping.has(hash)) {
            return this.mapping.get(hash)!;
        }

        const idx = this.nodes.length;
        const boolType: WitTypeNode = { tag: 'prim-bool-type' };
        this.nodes.push({  name: undefined, owner: undefined,  type: boolType });

        const node: WitTypeNode = this.convert(typ);
        const name = getNameFromAnalysedType(typ);
        const owner = getOwnerFromAnalysedType(typ);
        this.nodes[idx] = { name, owner, type: node };
        this.mapping.set(hash, idx);
        return idx;
    }

    build(): WitType {
        return { nodes: this.nodes };
    }

    private convert(typ: AnalysedType): WitTypeNode {
        switch (typ.kind) {
            case 'variant': {
                const cases: [string, NodeIndex | undefined][] = typ.value.cases.map(
                    (c: NameOptionTypePair) => [c.name, c.typ ? this.add(c.typ) : undefined],
                );
                return { tag: 'variant-type', val: cases };
            }

            case 'result': {
                const ok = typ.value.ok ? this.add(typ.value.ok) : undefined;
                const err = typ.value.err ? this.add(typ.value.err) : undefined;
                return { tag: 'result-type', val: [ok, err] };
            }

            case 'option': {
                const inner = this.add(typ.value.inner);
                return { tag: 'option-type', val: inner };
            }

            case 'enum':
                return { tag: 'enum-type', val: typ.value.cases };

            case 'flags':
                return { tag: 'flags-type', val: typ.value.names };

            case 'record': {
                const fields: [string, NodeIndex][] = typ.value.fields.map(
                    (f: NameTypePair) => [f.name, this.add(f.typ)],
                );
                return { tag: 'record-type', val: fields };
            }

            case 'tuple': {
                const elements = typ.value.items.map((item) => this.add(item));
                return { tag: 'tuple-type', val: elements };
            }

            case 'list': {
                const inner = this.add(typ.value.inner);
                return { tag: 'list-type', val: inner };
            }

            case 'string':
                return { tag: 'prim-string-type' };
            case 'chr':
                return { tag: 'prim-char-type' };
            case 'f64':
                return { tag: 'prim-f64-type' };
            case 'f32':
                return { tag: 'prim-f32-type' };
            case 'u64':
                return { tag: 'prim-u64-type' };
            case 's64':
                return { tag: 'prim-s64-type' };
            case 'u32':
                return { tag: 'prim-u32-type' };
            case 's32':
                return { tag: 'prim-s32-type' };
            case 'u16':
                return { tag: 'prim-u16-type' };
            case 's16':
                return { tag: 'prim-s16-type' };
            case 'u8':
                return { tag: 'prim-u8-type' };
            case 's8':
                return { tag: 'prim-s8-type' };
            case 'bool':
                return { tag: 'prim-bool-type' };

            // FIXME: Why? typ.value.resourceId is a number and the handle-type takes a bigint
            case 'handle': {
                const resId: number = typ.value.resourceId;
                const mode: ResourceMode =
                    typ.value.mode === 'owned' ? 'owned' : 'borrowed';
                return { tag: 'handle-type', val: [BigInt(resId), mode] };
            }

            default:
                throw new Error(`Unhandled AnalysedType kind: ${(typ as any).kind}`);
        }
    }
}
