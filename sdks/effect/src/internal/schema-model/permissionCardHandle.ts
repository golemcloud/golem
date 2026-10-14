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
import { assertCapabilityReady, type CapabilityTransaction } from "./capabilityTransaction.js"
import { PERMISSION_CARD_INTERNAL, type PermissionCardInternal } from "./permissionCardInternal.js"

interface PermissionCardHandleState {
  raw: RawPermissionCard | undefined
  readonly tracked: boolean
  /** Set by release, including while the resource is on the wire, so a restore cannot revive it. */
  released?: boolean
}

/** The outcome of giving a wire resource back to the handle it was taken from. */
export type WireRestoreResult = "restored" | "released" | "unknown"

interface PermissionCardHandleRegistry {
  readonly states: WeakMap<object, PermissionCardHandleState>
  readonly owners: WeakMap<RawPermissionCard, object>
  readonly wireOrigins: WeakMap<object, GuestPermissionCardHandle>
  readonly transferredOwner: object
}

// The package ships the main entry and the middleware entry as separate bundles, and both lift
// and lower permission cards. They must recognise each other's cards, so the registry lives on
// `globalThis` under a well-known symbol instead of in module scope. Code in the same component
// can therefore reach the registry and alias one raw resource into several card objects, or
// read the raw resource of a card. It cannot create authority: the raw resource comes only from
// the host, and the host checks every use of it. Aliasing makes the second use of a moved
// resource fail on the host.
const registryKey = Symbol.for("@effect-golem/permission-card-handle-registry")
const globals = globalThis as typeof globalThis & {
  [registryKey]?: PermissionCardHandleRegistry
}
const registry = (globals[registryKey] ??= {
  states: new WeakMap<object, PermissionCardHandleState>(),
  owners: new WeakMap<RawPermissionCard, object>(),
  wireOrigins: new WeakMap<object, GuestPermissionCardHandle>(),
  transferredOwner: Object.freeze({}),
})
const { states, owners, wireOrigins, transferredOwner } = registry

/** Guest-side take-once carrier for an owned, opaque permission-card resource. */
export class GuestPermissionCardHandle {
  declare private readonly permissionCardBrand: never

  constructor(key: PermissionCardInternal, raw: RawPermissionCard, tracked = true) {
    if (key !== PERMISSION_CARD_INTERNAL) {
      throw new Error("GuestPermissionCardHandle construction is an internal SDK operation")
    }
    states.set(this, { raw, tracked })
  }

  toJSON(): never {
    throw new Error(
      "permission-card handles cannot be serialized; transfer them through a WIT schema-value-tree",
    )
  }
}

function requirePermissionCardInternal(key: PermissionCardInternal): void {
  if (key !== PERMISSION_CARD_INTERNAL) {
    throw new Error("this is an internal SDK operation on a permission-card handle")
  }
}

function stateOf(handle: GuestPermissionCardHandle): PermissionCardHandleState {
  const state = states.get(handle)
  if (state === undefined) {
    throw new Error("invalid permission-card handle")
  }
  return state
}

export function isGuestPermissionCardHandle(value: unknown): value is GuestPermissionCardHandle {
  return (
    typeof value === "object" && value !== null && states.has(value as GuestPermissionCardHandle)
  )
}

export function createGuestPermissionCardHandle(
  key: PermissionCardInternal,
  raw: RawPermissionCard,
): GuestPermissionCardHandle {
  requirePermissionCardInternal(key)
  if (owners.has(raw)) {
    throw new Error("permission-card handle is already owned")
  }
  const handle = new GuestPermissionCardHandle(key, raw)
  owners.set(raw, handle)
  return handle
}

export function createUntrackedGuestPermissionCardHandle(
  key: PermissionCardInternal,
  raw: RawPermissionCard,
): GuestPermissionCardHandle {
  requirePermissionCardInternal(key)
  return new GuestPermissionCardHandle(key, raw, false)
}

export function adoptGuestPermissionCardHandle(
  key: PermissionCardInternal,
  raw: RawPermissionCard,
  transaction: CapabilityTransaction,
): GuestPermissionCardHandle {
  const handle = createGuestPermissionCardHandle(key, raw)
  transaction.register(() => releaseGuestPermissionCardHandle(key, handle))
  return handle
}

export function peekGuestPermissionCardHandle(
  key: PermissionCardInternal,
  handle: GuestPermissionCardHandle,
): RawPermissionCard | undefined {
  requirePermissionCardInternal(key)
  return stateOf(handle).raw
}

export function takeGuestPermissionCardHandle(
  key: PermissionCardInternal,
  handle: GuestPermissionCardHandle,
): RawPermissionCard | undefined {
  requirePermissionCardInternal(key)
  assertCapabilityReady(handle)
  const state = stateOf(handle)
  const raw = state.raw
  if (raw !== undefined && state.tracked && owners.get(raw) !== handle) {
    throw new Error("permission-card handle ownership is invalid")
  }
  state.raw = undefined
  if (raw !== undefined && state.tracked) owners.set(raw, transferredOwner)
  return raw
}

export function takeGuestPermissionCardHandleToWire(
  key: PermissionCardInternal,
  handle: GuestPermissionCardHandle,
  wireOwner: object,
): RawPermissionCard | undefined {
  requirePermissionCardInternal(key)
  assertCapabilityReady(handle)
  const state = stateOf(handle)
  const raw = state.raw
  if (raw !== undefined && state.tracked && owners.get(raw) !== handle) {
    throw new Error("permission-card handle ownership is invalid")
  }
  state.raw = undefined
  if (raw !== undefined && state.tracked) {
    owners.set(raw, wireOwner)
    wireOrigins.set(wireOwner, handle)
  }
  return raw
}

/**
 * Give a raw resource that `wireOwner` still holds back to the card it was taken from. Use it
 * only when the host never received the wire value. Returns `"released"` when the card was
 * released while the resource was on the wire; the caller must then drop the resource. Returns
 * `"unknown"` when the resource has no known origin or the origin card holds another resource.
 */
export function restoreGuestPermissionCardHandleFromWire(
  key: PermissionCardInternal,
  raw: RawPermissionCard,
  wireOwner: object,
): WireRestoreResult {
  requirePermissionCardInternal(key)
  const handle = wireOrigins.get(wireOwner)
  if (handle === undefined || owners.get(raw) !== wireOwner) return "unknown"
  const state = stateOf(handle)
  if (state.raw !== undefined) return "unknown"
  wireOrigins.delete(wireOwner)
  if (state.released) {
    owners.delete(raw)
    return "released"
  }
  state.raw = raw
  owners.set(raw, handle)
  return "restored"
}

export function assertGuestPermissionCardHandleCanLiftFromWire(
  key: PermissionCardInternal,
  raw: RawPermissionCard,
  wireOwner: object,
): void {
  requirePermissionCardInternal(key)
  const owner = owners.get(raw)
  if (owner !== undefined && owner !== wireOwner) {
    throw new Error("permission-card handle is already owned")
  }
}

export function liftGuestPermissionCardHandleFromWire(
  key: PermissionCardInternal,
  raw: RawPermissionCard,
  wireOwner: object,
): GuestPermissionCardHandle {
  assertGuestPermissionCardHandleCanLiftFromWire(key, raw, wireOwner)
  const handle = new GuestPermissionCardHandle(key, raw)
  owners.set(raw, handle)
  return handle
}

export function abandonGuestPermissionCardWireHandle(
  key: PermissionCardInternal,
  raw: RawPermissionCard,
  wireOwner: object,
): void {
  requirePermissionCardInternal(key)
  const owner = owners.get(raw)
  if (owner === undefined || owner === wireOwner) {
    owners.set(raw, transferredOwner)
  }
}

export function releaseGuestPermissionCardHandle(
  key: PermissionCardInternal,
  handle: GuestPermissionCardHandle,
): RawPermissionCard | undefined {
  requirePermissionCardInternal(key)
  const state = stateOf(handle)
  const raw = state.raw
  if (raw !== undefined && state.tracked && owners.get(raw) !== handle) {
    throw new Error("permission-card handle ownership is invalid")
  }
  state.raw = undefined
  state.released = true
  if (raw !== undefined && state.tracked) owners.delete(raw)
  return raw
}
