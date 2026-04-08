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

import { Uuid } from './uuid';
import { ComponentId as RawComponentId, AccountId as RawAccountId } from 'golem:core/types@1.5.0';
import { EnvironmentId as RawEnvironmentId } from 'golem:api/host@1.5.0';

export class ComponentId {
  readonly uuid: Uuid;

  constructor(uuid: Uuid) {
    this.uuid = uuid;
  }

  toString(): string {
    return this.uuid.toString();
  }

  static from(value: RawComponentId): ComponentId {
    if (value instanceof ComponentId) {
      return value;
    }
    return new ComponentId(Uuid.from(value.uuid));
  }
}

export class AccountId {
  readonly uuid: Uuid;

  constructor(uuid: Uuid) {
    this.uuid = uuid;
  }

  toString(): string {
    return this.uuid.toString();
  }

  static from(value: RawAccountId): AccountId {
    if (value instanceof AccountId) {
      return value;
    }
    return new AccountId(Uuid.from(value.uuid));
  }
}

export class EnvironmentId {
  readonly uuid: Uuid;

  constructor(uuid: Uuid) {
    this.uuid = uuid;
  }

  toString(): string {
    return this.uuid.toString();
  }

  static from(value: RawEnvironmentId): EnvironmentId {
    if (value instanceof EnvironmentId) {
      return value;
    }
    return new EnvironmentId(Uuid.from(value.uuid));
  }
}
