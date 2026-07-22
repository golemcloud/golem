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

import type { Secret as RawSecret } from 'golem:core/types@2.0.0';
import { SECRET_INTERNAL, type SecretInternal } from './secretInternal';

export class GuestSecretHandle {
  #raw: RawSecret | undefined;
  readonly #onTake?: () => void;

  private constructor(raw: RawSecret, onTake?: () => void) {
    this.#raw = raw;
    this.#onTake = onTake;
  }

  static fromRaw(key: SecretInternal, raw: RawSecret): GuestSecretHandle {
    if (key !== SECRET_INTERNAL) {
      throw new Error('GuestSecretHandle.fromRaw is an internal SDK operation');
    }
    return new GuestSecretHandle(raw);
  }

  static fromRawWithTakeCallback(
    key: SecretInternal,
    raw: RawSecret,
    onTake: () => void,
  ): GuestSecretHandle {
    if (key !== SECRET_INTERNAL) {
      throw new Error('GuestSecretHandle.fromRawWithTakeCallback is an internal SDK operation');
    }
    return new GuestSecretHandle(raw, onTake);
  }

  isPresent(): boolean {
    return this.#raw !== undefined;
  }

  take(): RawSecret | undefined {
    const raw = this.#raw;
    this.#raw = undefined;
    if (raw !== undefined) {
      this.#onTake?.();
    }
    return raw;
  }

  withHandle<R>(f: (raw: RawSecret) => R): R | undefined {
    return this.#raw === undefined ? undefined : f(this.#raw);
  }

  toJSON(): never {
    throw new Error(
      'secret handles cannot be serialized; transfer them through a WIT schema-value-tree',
    );
  }
}
