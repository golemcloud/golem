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

// Focused edge-case coverage for the schema-native runtime value boundary
// (`boundaryValue.ts`). In particular, multimodal serialization must reject a
// modality element that omits its `val`, *even when the modality's schema is an
// option type* — otherwise a missing payload would be silently encoded as
// `option none` instead of being rejected. Real agents never expose an optional
// modality case, so this is constructed by hand to pin the behaviour.

import { expect, test } from 'vitest';
import type { Type } from '@golemcloud/golem-ts-types-core';
import { serializeRuntimeValue } from '../src/internal/mapping/values/boundaryValue';
import { RuntimeTypeInfo } from '../src/internal/typeInfoInternal';
import { r } from '../src/internal/mapping/types/resolvedType';

// A modality whose value schema is `option<string>` — `serializeGraph(undefined, …)`
// for this schema would otherwise yield `option none` without complaint.
const optionalStringCase: RuntimeTypeInfo = {
  tag: 'schema',
  graph: { defs: new Map(), root: r.option(r.string(), 'undefined') },
  tsType: { kind: 'string', optional: true } as Type.Type,
};

const multimodalType: RuntimeTypeInfo = {
  tag: 'multimodal',
  cases: [{ name: 'maybe', type: optionalStringCase }],
  tsType: { kind: 'others', optional: false, recursive: false } as Type.Type,
};

test('multimodal serialization rejects an element missing its `val` even for an optional case', () => {
  expect(() => serializeRuntimeValue([{ tag: 'maybe' }], multimodalType)).toThrowError(
    /must be an object with a string 'tag' and a 'val'/,
  );
});

test('multimodal serialization rejects an unknown modality', () => {
  expect(() => serializeRuntimeValue([{ tag: 'nope', val: 'x' }], multimodalType)).toThrowError(
    /Unknown multimodal modality 'nope'/,
  );
});

test('multimodal serialization accepts present `val`s, including an explicit undefined for an option case', () => {
  expect(() =>
    serializeRuntimeValue(
      [
        { tag: 'maybe', val: 'hello' },
        { tag: 'maybe', val: undefined },
      ],
      multimodalType,
    ),
  ).not.toThrow();
});
