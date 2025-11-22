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

import { UnstructuredText } from './textInput';
import { UnstructuredBinary } from './binaryInput';

/**
 * Multimodal type represents a value that holds multiple types of inputs.
 *
 * Example:
 *
 * ```ts
 *
 * import { Multimodal } from '@golemcloud/golem-ts-sdk';
 *
 * type Text = string;
 * type Image = Uint8Array;
 *
 * type Input = Multimodal<Text | Image>;
 *
 * function processInput(input: Input) { }
 *
 * processInput(["text", new Uint8Array([137, 80, 78, 71])]);
 *
 * It is recommended to use tagged unions to distinguish between different types of inputs.
 * This will also ensure the internals of golem can name these inputs based on the value of the `tag` property.
 *
 * Example:
 *
 * ```ts
 * type TaggedInput = Multimodal<{ tag: 'text'; val: string } | { tag: 'image'; val: Uint8Array }>;
 *
 * processInput([{ tag: 'text', val: "hello" }, { tag: 'image', val: new Uint8Array([137, 80, 78, 71]) }]);
 *
 * ```
 */
export type MultimodalAdvanced<T> = T[];

export type Multimodal = MultimodalAdvanced<BasicModality>;

export type MultimodalCustom<T> = MultimodalAdvanced<CustomModality<T>>;

export type BasicModality =
  | { tag: 'text'; val: UnstructuredText }
  | { tag: 'binary'; val: UnstructuredBinary };

export type CustomModality<T> =
  | { tag: 'text'; val: UnstructuredText }
  | { tag: 'binary'; val: UnstructuredBinary }
  | { tag: 'custom'; val: T };
