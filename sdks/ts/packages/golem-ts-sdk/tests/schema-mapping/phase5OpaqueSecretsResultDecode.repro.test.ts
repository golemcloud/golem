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

import { describe, expect, it } from 'vitest';
import type { SchemaValueNode as WitSchemaValueNode } from 'golem:core/types@2.0.0';
import { r } from '../../src/internal/mapping/types/resolvedType';
import {
  compileGraphDecoder,
  deserializeGraph,
  deserializeGraphFromWit,
} from '../../src/internal/mapping/values/schemaValue';
import { GuestSecretHandle } from '../../src/internal/schema-model/secretHandle';
import { SECRET_INTERNAL } from '../../src/internal/schema-model/secretInternal';

describe('Phase 5 opaque secrets result decode regressions', () => {
  const graph = {
    defs: new Map(),
    root: r.result(r.u32(), undefined, {
      tag: 'custom' as const,
      okValueName: 'value',
      errValueName: 'error',
    }),
  };

  const rejects = (decode: () => unknown): boolean => {
    try {
      decode();
      return false;
    } catch {
      return true;
    }
  };

  it('rejects an unexpected payload on a custom result side with no declared payload', () => {
    const raw = { id: 'unexpected-secret-payload' } as never;
    const makeWit = () => ({
      valueNodes: [
        { tag: 'secret-value', val: raw } as WitSchemaValueNode,
        { tag: 'result-value', val: { tag: 'err-value', val: 0 } } as WitSchemaValueNode,
      ],
      root: 1,
    });

    expect({
      schemaValue: rejects(() =>
        deserializeGraph(
          {
            tag: 'result',
            result: {
              tag: 'err',
              value: {
                tag: 'secret',
                handle: GuestSecretHandle.fromRaw(SECRET_INTERNAL, raw),
              },
            },
          },
          graph,
        ),
      ),
      wireValue: rejects(() => deserializeGraphFromWit(makeWit(), graph)),
      compiledWireValue: rejects(() => compileGraphDecoder(graph)(makeWit())),
    }).toEqual({
      schemaValue: true,
      wireValue: true,
      compiledWireValue: true,
    });
  });

  it('rejects a missing payload on a custom result side with a declared payload', () => {
    const wit = {
      valueNodes: [
        { tag: 'result-value', val: { tag: 'ok-value', val: undefined } } as WitSchemaValueNode,
      ],
      root: 0,
    };

    expect({
      schemaValue: rejects(() => deserializeGraph({ tag: 'result', result: { tag: 'ok' } }, graph)),
      wireValue: rejects(() => deserializeGraphFromWit(wit, graph)),
      compiledWireValue: rejects(() => compileGraphDecoder(graph)(wit)),
    }).toEqual({
      schemaValue: true,
      wireValue: true,
      compiledWireValue: true,
    });
  });
});
