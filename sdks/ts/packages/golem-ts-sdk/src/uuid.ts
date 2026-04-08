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

import { parseUuid, uuidToString, Uuid as RawUuid } from 'golem:core/types@1.5.0';

/**
 * Rich UUID class that is structurally compatible with the WIT binding type
 * `{ highBits: bigint; lowBits: bigint }` from `golem:core/types@1.5.0`.
 */
export class Uuid {
  readonly highBits: bigint;
  readonly lowBits: bigint;

  constructor(highBits: bigint, lowBits: bigint) {
    this.highBits = highBits;
    this.lowBits = lowBits;
  }

  /**
   * Formats the UUID as a standard hyphenated string.
   */
  toString(): string {
    return uuidToString(this);
  }

  /**
   * Parses a UUID string.
   */
  static parse(value: string): Uuid {
    const raw = parseUuid(value);
    return new Uuid(raw.highBits, raw.lowBits);
  }

  /**
   * Generates a random UUID.
   */
  static generate(): Uuid {
    return Uuid.parse(crypto.randomUUID());
  }

  /**
   * Wraps a raw WIT UUID object in a Uuid instance.
   * If the value is already a Uuid, it is returned directly.
   */
  static from(value: RawUuid): Uuid {
    if (value instanceof Uuid) {
      return value;
    }
    return new Uuid(value.highBits, value.lowBits);
  }
}
