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

// Guest-side carrier for the opaque, owned `golem:core/types.quota-token`
// resource handle.
//
// A `quota-token` is an affine capability: the guest may hold and transfer it,
// but can never inspect or forge it. Inside a `SchemaValue` the handle lives in
// a take-once cell so the value tree can be shared (e.g. duplicated by a caller)
// without duplicating the underlying capability. Lowering a value that contains
// a token (`schemaValueToWit`) moves the underlying `own<quota-token>` resource
// out of the cell exactly once (first encode wins). A handle that was already
// transferred is `consumed`; the same handle appearing twice in one value tree
// is an alias — both are rejected by the encoder's preflight pass.

import type { QuotaToken as RawQuotaToken } from 'golem:core/types@2.0.0';

export class GuestQuotaTokenHandle {
  // A true ECMAScript private field, not a TypeScript-only `private`: the owned
  // resource is unreachable from guest code even through `as any` / field
  // access, so the handle cannot be inspected, copied, or re-wrapped.
  #raw: RawQuotaToken | undefined;

  private constructor(raw: RawQuotaToken) {
    this.#raw = raw;
  }

  /** Wrap a freshly received owned handle in a take-once cell. */
  static fromRaw(raw: RawQuotaToken): GuestQuotaTokenHandle {
    return new GuestQuotaTokenHandle(raw);
  }

  /** Whether the handle is still present (not yet transferred). */
  isPresent(): boolean {
    return this.#raw !== undefined;
  }

  /**
   * Take the owned handle out of the cell. Returns `undefined` if it was
   * already transferred (consumed) by a previous encode.
   */
  take(): RawQuotaToken | undefined {
    const raw = this.#raw;
    this.#raw = undefined;
    return raw;
  }

  /**
   * Run `f` with the owned handle, if it is still present (i.e. has not been
   * transferred out by an encode). Returns `undefined` if the handle was
   * already consumed.
   *
   * Used by the SDK wrappers to invoke borrowing quota operations (`reserve`,
   * `split`) on the underlying resource without taking ownership of it.
   */
  withHandle<R>(f: (raw: RawQuotaToken) => R): R | undefined {
    return this.#raw === undefined ? undefined : f(this.#raw);
  }

  /**
   * Quota-token handles are unforgeable capabilities, not data: serializing one
   * (e.g. via `JSON.stringify`) is always an error. Transfer them only by
   * passing the owning `QuotaToken` through a WIT `schema-value-tree`.
   */
  toJSON(): never {
    throw new Error(
      'quota-token handles cannot be serialized; transfer them through a WIT schema-value-tree',
    );
  }
}
