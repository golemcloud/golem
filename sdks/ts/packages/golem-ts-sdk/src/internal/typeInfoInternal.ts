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

import { Type } from '@golemcloud/golem-ts-types-core';
import { ResolvedGraph } from './mapping/types/resolvedType';

// Runtime type information for a single constructor/method parameter or a
// method return value. Unlike the legacy `TypeInfoInternal` (which mirrored the
// legacy `AnalysedType` / `WitType` / `DataSchema` model), this is schema-native:
// a `schema` carries a self-contained `ResolvedGraph` (the recursion-aware
// in-memory type the value codec thinks in), and the rich/auto-injected cases
// carry exactly the information the runtime boundary needs.
export type RuntimeTypeInfo =
  | { tag: 'schema'; graph: ResolvedGraph; tsType: Type.Type }
  | { tag: 'unstructured-text'; languages: string[]; tsType: Type.Type }
  | { tag: 'unstructured-binary'; mimeTypes: string[]; tsType: Type.Type }
  | { tag: 'multimodal'; cases: MultimodalCase[]; tsType: Type.Type }
  | { tag: 'principal'; tsType: Type.Type }
  | { tag: 'config'; tsType: Type.Type & { kind: 'config' } };

/** A single modality of a multimodal parameter (never itself multimodal/principal/config). */
export interface MultimodalCase {
  name: string;
  type: RuntimeTypeInfo;
}

/** A named constructor/method parameter and its runtime type. */
export interface RuntimeParam {
  name: string;
  type: RuntimeTypeInfo;
}

/** A method's resolved output: either `unit` (no value) or a single value. */
export type RuntimeOutput = { tag: 'unit' } | { tag: 'single'; type: RuntimeTypeInfo };

export function isPrincipal(typeInfo: RuntimeTypeInfo): boolean {
  return typeInfo.tag === 'principal';
}

export function isConfig(
  typeInfo: RuntimeTypeInfo,
): typeInfo is RuntimeTypeInfo & { tag: 'config' } {
  return typeInfo.tag === 'config';
}
