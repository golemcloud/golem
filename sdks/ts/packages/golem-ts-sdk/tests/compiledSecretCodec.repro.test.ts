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
import { Secret } from '../src/agentConfig';
import {
  compileGraphDecoder,
  compileGraphEncoder,
  deserializeGraphFromWit,
  serializeGraphToWit,
} from '../src/internal/mapping/values/schemaValue';
import { r, type ResolvedGraph } from '../src/internal/mapping/types/resolvedType';
import { GuestSecretHandle } from '../src/internal/schema-model/secretHandle';
import { SECRET_INTERNAL } from '../src/internal/schema-model/secretInternal';

function secretGraph(): ResolvedGraph {
  return { defs: new Map(), root: r.secret(r.string()) };
}

function makeSecret() {
  const raw = { id: 'compiled-secret' } as never;
  const handle = GuestSecretHandle.fromRaw(SECRET_INTERNAL, raw);
  const secret = Secret._fromHandle<string>(SECRET_INTERNAL, handle, {
    defs: new Map(),
    root: r.string(),
  });
  return { raw, secret };
}

describe('compiled opaque secret value codecs', () => {
  it('keeps compiled and interpreted Secret<T> graph support in parity', () => {
    const graph = secretGraph();
    const { secret } = makeSecret();
    const interpreted = serializeGraphToWit(secret, graph);
    const decoded = deserializeGraphFromWit(interpreted, graph);

    expect(decoded).toBeInstanceOf(Secret);
    expect(() => compileGraphEncoder(graph)).not.toThrow();
    expect(() => compileGraphDecoder(graph)).not.toThrow();
  });
});
