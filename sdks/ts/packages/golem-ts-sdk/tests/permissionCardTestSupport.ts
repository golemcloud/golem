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

// Test-only helpers that wrap fake host resources in permission cards. They use
// the SDK-internal capability key and are not part of the public surface.

import { PermissionCard as RawPermissionCard } from 'golem:core/types@2.0.0';
import {
  type PermissionCard,
  ScopeCard,
  borrowPermissionCardInternal,
  borrowScopeCardInternal,
  permissionCardFromRawInternal,
} from '../src/host/permissions';
import { PERMISSION_CARD_INTERNAL } from '../src/internal/schema-model/permissionCardInternal';

/** A fake host permission-card resource: an instance of the mocked core resource class. */
export function fakeRawCard<T extends object>(fields?: T): T {
  return Object.assign(Object.create(RawPermissionCard.prototype), fields) as T;
}

export function scopeCardFromRaw(raw: unknown): ScopeCard {
  return new ScopeCard(PERMISSION_CARD_INTERNAL, raw as never);
}

export function rawOfScopeCard(card: ScopeCard): unknown {
  return borrowScopeCardInternal(PERMISSION_CARD_INTERNAL, card);
}

export function permissionCardFromRaw(raw: unknown): PermissionCard {
  return permissionCardFromRawInternal(PERMISSION_CARD_INTERNAL, raw as never);
}

export function rawOfPermissionCard(card: unknown): unknown {
  return borrowPermissionCardInternal(PERMISSION_CARD_INTERNAL, card as PermissionCard);
}
