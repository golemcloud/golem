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

// Lazy, log-safe handle for a secret config field. A `Secret<T>` never holds the
// plaintext: `get()` reveals + decodes the LIVE value on every call (config may
// change between invocations; the caller caches by keeping the returned value),
// and `toJSON()` throws so the handle can never be accidentally serialized into
// a log line or a snapshot.

import { getConfigValue } from 'golem:agent/host@2.0.0';
import { reveal } from 'golem:secrets/reveal@0.1.0';
import type { Secret as RawSecret } from 'golem:core/types@2.0.0';
import { SchemaValue, schemaGraphToWit, schemaValueFromWit } from './internal/schema-model';
import { SECRET_INTERNAL } from './internal/schema-model/secretInternal';
import {
  peekGuestSecretHandle,
  releaseGuestSecretHandle,
} from './internal/schema-model/secretHandle';
import type { ConfigDeclaration } from './config';

const declarations = new WeakMap<Secret<unknown>, ConfigDeclaration>();

/**
 * A lazy, log-safe handle over a `secret<inner>` config field. Obtained from
 * `this.config.<field>` (and `InitContext.config.<field>`) for any field
 * declared with `s.secret(inner)`.
 *
 * - {@link get} performs the host read + reveal + decode fresh on every call, so
 *   it always reflects the current value; cache the result yourself if you need
 *   a stable snapshot within a single operation.
 * - {@link toJSON} throws, so `JSON.stringify(this.config)` (or a stray log of
 *   the whole config object) can never leak the plaintext.
 */
export class Secret<T> {
  constructor(declaration: ConfigDeclaration) {
    declarations.set(this, declaration);
  }

  /**
   * Reveal and decode the current plaintext value.
   *
   * The host returns an opaque secret handle for a `secret<inner>` field; this
   * reveals it (capability-gated via `golem:secrets/reveal`) against the inner
   * type graph and decodes the resulting value tree with the inner codec.
   *
   * Only resolves inside the Golem guest (the host bindings are unavailable in
   * plain Node), so it is exercised at invocation time, never at import time.
   */
  get(): T {
    const d = declarationOf(this);
    const tree = getConfigValue(d.path, schemaGraphToWit(d.graph));
    const sv = schemaValueFromWit(tree);
    if (sv.tag !== 'secret') {
      throw new Error(`Expected a secret config value at '${d.path.join('.')}', got '${sv.tag}'`);
    }
    const handle = (sv as Extract<SchemaValue, { tag: 'secret' }>).handle;
    const raw = peekGuestSecretHandle(SECRET_INTERNAL, handle);
    if (raw === undefined) {
      throw new Error(`Secret config handle at '${d.path.join('.')}' was already transferred`);
    }
    const revealedTree = reveal(raw, schemaGraphToWit(d.codec.graph));
    return d.codec.fromValue(schemaValueFromWit(revealedTree)) as T;
  }

  /** Refuse serialization so secrets never leak through logs / JSON / snapshots. */
  toJSON(): never {
    throw new Error(
      'Secret values are not serializable; call .get() to read the plaintext (and avoid logging it)',
    );
  }
}

function declarationOf(secret: Secret<unknown>): ConfigDeclaration {
  const declaration = declarations.get(secret);
  if (declaration === undefined) {
    throw new TypeError('Invalid config Secret');
  }
  return declaration;
}

export function withConfigSecretHandle<R>(
  secret: Secret<unknown>,
  use: (handle: RawSecret) => R,
): R {
  const d = declarationOf(secret);
  const tree = getConfigValue(d.path, schemaGraphToWit(d.graph));
  const sv = schemaValueFromWit(tree);
  if (sv.tag !== 'secret') {
    throw new Error(`Expected a secret config value at '${d.path.join('.')}', got '${sv.tag}'`);
  }
  const raw = releaseGuestSecretHandle(SECRET_INTERNAL, sv.handle);
  if (raw === undefined) {
    throw new Error(`Secret config handle at '${d.path.join('.')}' was already transferred`);
  }
  try {
    return use(raw);
  } finally {
    (raw as unknown as { [Symbol.dispose](): void })[Symbol.dispose]();
  }
}
