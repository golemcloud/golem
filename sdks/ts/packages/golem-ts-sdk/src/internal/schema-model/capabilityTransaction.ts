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

type Rollback = () => void;

let activeRollbacks: Rollback[] | undefined;

/**
 * Atomically build a schema value that may adopt owned capability handles.
 * Nested conversions share the outer journal, while a failed nested conversion
 * rolls back only the handles it adopted.
 */
export function withCapabilityAdoptionTransaction<T>(encode: () => T): T {
  const journal = activeRollbacks ?? [];
  const start = journal.length;
  const outermost = activeRollbacks === undefined;
  if (outermost) activeRollbacks = journal;

  try {
    return encode();
  } catch (error) {
    for (let i = journal.length - 1; i >= start; i--) {
      try {
        journal[i]!();
      } catch {
        // Preserve the conversion failure while still attempting every rollback.
      }
    }
    journal.length = start;
    throw error;
  } finally {
    if (outermost) activeRollbacks = undefined;
  }
}

/**
 * Run a public root conversion in a journal isolated from any conversion that
 * synchronously invoked it. Successful adoptions belong only to this call.
 */
export function withIsolatedCapabilityAdoptionTransaction<T>(encode: () => T): T {
  const previous = activeRollbacks;
  activeRollbacks = [];
  try {
    return withCapabilityAdoptionTransaction(encode);
  } finally {
    activeRollbacks = previous;
  }
}

type Encoder = (value: unknown) => unknown;

const rootChildren = new WeakMap<Encoder, Encoder>();

/** Make an encoder an isolated public root while retaining its joining child encoder. */
export function isolateCapabilityRoot<T>(encode: (value: unknown) => T): (value: unknown) => T {
  const root = (value: unknown): T =>
    withIsolatedCapabilityAdoptionTransaction(() => encode(value));
  rootChildren.set(root, encode);
  return root;
}

/** Encode one structural child without re-entering that child's public root boundary. */
export function encodeChild<T>(
  codec: { readonly toValue: (value: unknown) => T },
  value: unknown,
): T {
  const child = rootChildren.get(codec.toValue) as ((value: unknown) => T) | undefined;
  return (child ?? codec.toValue)(value);
}

/** Register a newly adopted handle with the active synchronous conversion. */
export function registerCapabilityAdoption(rollback: Rollback): void {
  activeRollbacks?.push(rollback);
}
