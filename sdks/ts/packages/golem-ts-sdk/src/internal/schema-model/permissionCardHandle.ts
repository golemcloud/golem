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

import type { PermissionCard as RawPermissionCard } from 'golem:core/types@2.0.0';
import { PERMISSION_CARD_INTERNAL, type PermissionCardInternal } from './permissionCardInternal';

/** Guest-side take-once carrier for an owned, opaque permission-card resource. */
export class GuestPermissionCardHandle {
  #raw: RawPermissionCard | undefined;

  private constructor(raw: RawPermissionCard) {
    this.#raw = raw;
  }

  static fromRaw(key: PermissionCardInternal, raw: RawPermissionCard): GuestPermissionCardHandle {
    if (key !== PERMISSION_CARD_INTERNAL) {
      throw new Error('GuestPermissionCardHandle.fromRaw is an internal SDK operation');
    }
    return new GuestPermissionCardHandle(raw);
  }

  /** Whether the card is still present and has not been transferred. */
  isPresent(): boolean {
    return this.#raw !== undefined;
  }

  /** Move the owned card out of this handle at most once. */
  take(): RawPermissionCard | undefined {
    const raw = this.#raw;
    this.#raw = undefined;
    return raw;
  }

  /** Inspect the resource identity without transferring ownership. */
  withHandle<R>(f: (raw: RawPermissionCard) => R): R | undefined {
    return this.#raw === undefined ? undefined : f(this.#raw);
  }

  toJSON(): never {
    throw new Error(
      'permission-card handles cannot be serialized; transfer them through a WIT schema-value-tree',
    );
  }
}
