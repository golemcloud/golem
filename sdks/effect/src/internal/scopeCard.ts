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

import type { PermissionCard as RawPermissionCard } from "golem:core/types@2.0.0"

// Scope cards never enter schema values, so only the bundle that contains
// `Permissions` and the RPC clients sees them. The state stays module-local.
const SCOPE_CARD_INTERNAL: unique symbol = Symbol("golem:scope-card internal capability")
const states = new WeakMap<object, { raw: RawPermissionCard | undefined }>()

/** Guest-side carrier for an invocation-local scope-card resource. */
export class GuestScopeCard {
  declare private readonly scopeCardBrand: never

  constructor(key: typeof SCOPE_CARD_INTERNAL, raw: RawPermissionCard) {
    if (key !== SCOPE_CARD_INTERNAL) {
      throw new Error("scope cards are created only by the SDK")
    }
    states.set(this, { raw })
  }

  toJSON(): never {
    throw new Error("scope cards cannot be serialized")
  }
}

export const createScopeCard = (raw: RawPermissionCard): GuestScopeCard =>
  new GuestScopeCard(SCOPE_CARD_INTERNAL, raw)

export const isScopeCard = (value: unknown): value is GuestScopeCard =>
  typeof value === "object" && value !== null && states.has(value)

/** The raw resource of a held scope card, or `undefined` when it was released. */
export const peekScopeCard = (card: GuestScopeCard): RawPermissionCard | undefined => {
  const state = states.get(card)
  if (state === undefined) throw new TypeError("not a scope card")
  return state.raw
}

/** Detach the raw resource so the caller can drop it. */
export const releaseScopeCard = (card: GuestScopeCard): RawPermissionCard | undefined => {
  const state = states.get(card)
  if (state === undefined) throw new TypeError("not a scope card")
  const raw = state.raw
  state.raw = undefined
  return raw
}
