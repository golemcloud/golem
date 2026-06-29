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

import type { QuantitySpec, QuantityValue } from './internal/schema-model';

export class Path {
  private readonly _pathBrand!: void;

  constructor(public readonly path: string) {}

  toString(): string {
    return this.path;
  }
}

export class Duration {
  private readonly _durationBrand!: void;

  constructor(public readonly nanoseconds: bigint) {}
}

export class Quantity<U extends QuantitySpec = QuantitySpec> {
  private readonly _spec!: U;

  constructor(public readonly value: QuantityValue) {}

  static spec<U extends QuantitySpec>(spec: U): U {
    return spec;
  }
}
