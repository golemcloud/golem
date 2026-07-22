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

import type { Pollable } from 'wasi:io/poll@0.2.3';

const disposeSymbol = (Symbol as typeof Symbol & { readonly dispose?: symbol }).dispose;

export function disposeWitResource(resource: unknown): void {
  if (
    !disposeSymbol ||
    resource === null ||
    (typeof resource !== 'object' && typeof resource !== 'function')
  )
    return;

  try {
    const dispose = (resource as Record<symbol, unknown>)[disposeSymbol];
    if (typeof dispose === 'function') dispose.call(resource);
  } catch {
    // Generated WIT resources are always released on a best-effort basis.
  }
}

export function throwIfAborted(signal?: AbortSignal): void {
  if (!signal?.aborted) return;

  if (signal.reason !== undefined) {
    throw signal.reason;
  }

  const err = new Error('The operation was aborted.');
  err.name = 'AbortError';
  throw err;
}

export async function awaitPollable(pollable: Pollable, signal?: AbortSignal): Promise<void> {
  try {
    if (!signal) {
      await pollable.promise();
      return;
    }

    throwIfAborted(signal);
    await pollable.abortablePromise(signal);
  } finally {
    // wasm-rquickjs consumes a pollable while awaiting it, except when an
    // already-aborted signal rejects before that transfer takes place.
    disposeWitResource(pollable);
  }
}
