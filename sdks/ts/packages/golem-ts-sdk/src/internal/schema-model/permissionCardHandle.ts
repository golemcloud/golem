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

import { PermissionCard as RawPermissionCard } from 'golem:core/types@2.0.0';
import { registerCapabilityAdoption } from './capabilityTransaction';
import { PERMISSION_CARD_INTERNAL, type PermissionCardInternal } from './permissionCardInternal';

interface PermissionCardHandleState {
  raw: RawPermissionCard | undefined;
  readonly tracked: boolean;
  readonly lifted: boolean;
  // A public `PermissionCard` owns the handle, so relinquishing a schema value
  // that references it leaves the card in place.
  boundToCard: boolean;
}

const states = new WeakMap<GuestPermissionCardHandle, PermissionCardHandleState>();
const owners = new WeakMap<RawPermissionCard, object>();
const wireOrigins = new WeakMap<RawPermissionCard, GuestPermissionCardHandle>();
const transferredOwner = Object.freeze({});
// Public card objects (`PermissionCard`, `ScopeCard`). They carry no data, so
// structural comparisons must compare them by identity.
const cardObjects = new WeakSet<object>();
const cardHandles = new WeakMap<object, GuestPermissionCardHandle>();

/** Register a public card object so that structural equality compares it by identity. */
export function registerPermissionCardObject(key: PermissionCardInternal, card: object): void {
  requirePermissionCardInternal(key);
  cardObjects.add(card);
}

export function isPermissionCardObject(value: unknown): boolean {
  return typeof value === 'object' && value !== null && cardObjects.has(value);
}

/**
 * `true` when `raw` is an instance of the host `golem:core/types` permission-card
 * resource class. Wire lifting accepts only such values, so a plain object or
 * other data does not pass. This is a JavaScript `instanceof` check, not proof
 * that the value is a live host resource: an object made with
 * `Object.create(PermissionCard.prototype)` passes it. Such an object grants no
 * authority, because the host validates the resource when the SDK lowers it
 * into a host call or a wire value.
 */
export function isHostPermissionCardResource(raw: unknown): boolean {
  return (
    typeof RawPermissionCard === 'function' &&
    typeof raw === 'object' &&
    raw !== null &&
    raw instanceof RawPermissionCard
  );
}

/** Guest-side take-once carrier for an owned, opaque permission-card resource. */
export class GuestPermissionCardHandle {
  constructor(key: PermissionCardInternal, raw: RawPermissionCard, tracked = true, lifted = false) {
    if (key !== PERMISSION_CARD_INTERNAL) {
      throw new Error('GuestPermissionCardHandle construction is an internal SDK operation');
    }
    states.set(this, { raw, tracked, lifted, boundToCard: false });
  }

  toJSON(): never {
    throw new Error(
      'permission-card handles cannot be serialized; transfer them through a WIT schema-value-tree',
    );
  }
}

function requirePermissionCardInternal(key: PermissionCardInternal): void {
  if (key !== PERMISSION_CARD_INTERNAL) {
    throw new Error('this is an internal SDK operation on a permission-card handle');
  }
}

function stateOf(handle: GuestPermissionCardHandle): PermissionCardHandleState {
  const state = states.get(handle);
  if (state === undefined) {
    throw new Error('invalid permission-card handle');
  }
  return state;
}

export function createGuestPermissionCardHandle(
  key: PermissionCardInternal,
  raw: RawPermissionCard,
): GuestPermissionCardHandle {
  requirePermissionCardInternal(key);
  if (owners.has(raw)) {
    throw new Error('permission-card handle is already owned');
  }
  const handle = new GuestPermissionCardHandle(key, raw);
  owners.set(raw, handle);
  return handle;
}

/** `true` when the SDK tracks `raw` as held by a handle, a wire value or the host. */
export function guestPermissionCardResourceIsTracked(
  key: PermissionCardInternal,
  raw: RawPermissionCard,
): boolean {
  requirePermissionCardInternal(key);
  return owners.has(raw);
}

export function createUntrackedGuestPermissionCardHandle(
  key: PermissionCardInternal,
  raw: RawPermissionCard,
): GuestPermissionCardHandle {
  requirePermissionCardInternal(key);
  return new GuestPermissionCardHandle(key, raw, false);
}

export function adoptGuestPermissionCardHandle(
  key: PermissionCardInternal,
  raw: RawPermissionCard,
): GuestPermissionCardHandle {
  const handle = createGuestPermissionCardHandle(key, raw);
  registerCapabilityAdoption(() => releaseGuestPermissionCardHandle(key, handle));
  return handle;
}

export function peekGuestPermissionCardHandle(
  key: PermissionCardInternal,
  handle: GuestPermissionCardHandle,
): RawPermissionCard | undefined {
  requirePermissionCardInternal(key);
  return stateOf(handle).raw;
}

export function takeGuestPermissionCardHandle(
  key: PermissionCardInternal,
  handle: GuestPermissionCardHandle,
): RawPermissionCard | undefined {
  requirePermissionCardInternal(key);
  const state = stateOf(handle);
  const raw = state.raw;
  if (raw !== undefined && state.tracked && owners.get(raw) !== handle) {
    throw new Error('permission-card handle ownership is invalid');
  }
  state.raw = undefined;
  if (raw !== undefined && state.tracked) owners.set(raw, transferredOwner);
  return raw;
}

export function takeGuestPermissionCardHandleToWire(
  key: PermissionCardInternal,
  handle: GuestPermissionCardHandle,
  wireOwner: object,
): RawPermissionCard | undefined {
  requirePermissionCardInternal(key);
  const state = stateOf(handle);
  const raw = state.raw;
  if (raw !== undefined && state.tracked && owners.get(raw) !== handle) {
    throw new Error('permission-card handle ownership is invalid');
  }
  state.raw = undefined;
  if (raw !== undefined && state.tracked) {
    owners.set(raw, wireOwner);
    wireOrigins.set(raw, handle);
  }
  return raw;
}

/**
 * Give a resource that was lowered into a wire tree but never sent back to the
 * handle it came from. Returns `false` when the resource has no such handle.
 */
export function restoreGuestPermissionCardHandleFromWire(
  key: PermissionCardInternal,
  raw: RawPermissionCard,
  wireOwner: object,
): boolean {
  requirePermissionCardInternal(key);
  const origin = wireOrigins.get(raw);
  if (origin === undefined || owners.get(raw) !== wireOwner) return false;
  const state = stateOf(origin);
  if (state.raw !== undefined) return false;
  state.raw = raw;
  owners.set(raw, origin);
  wireOrigins.delete(raw);
  return true;
}

/**
 * Detach a resource that is about to be disposed from the handle that holds it,
 * so that the card wrapping the handle reports itself as moved.
 */
export function detachGuestPermissionCardResource(
  key: PermissionCardInternal,
  raw: RawPermissionCard,
): void {
  requirePermissionCardInternal(key);
  const owner = owners.get(raw);
  if (owner instanceof GuestPermissionCardHandle) {
    const state = states.get(owner);
    if (state !== undefined && state.raw === raw) state.raw = undefined;
  }
  owners.set(raw, transferredOwner);
  wireOrigins.delete(raw);
}

export function assertGuestPermissionCardHandleCanLiftFromWire(
  key: PermissionCardInternal,
  raw: RawPermissionCard,
  wireOwner: object,
): void {
  requirePermissionCardInternal(key);
  const owner = owners.get(raw);
  if (owner !== undefined && owner !== wireOwner) {
    throw new Error('permission-card handle is already owned');
  }
}

export function liftGuestPermissionCardHandleFromWire(
  key: PermissionCardInternal,
  raw: RawPermissionCard,
  wireOwner: object,
): GuestPermissionCardHandle {
  assertGuestPermissionCardHandleCanLiftFromWire(key, raw, wireOwner);
  const handle = new GuestPermissionCardHandle(key, raw, true, true);
  owners.set(raw, handle);
  return handle;
}

export function permissionCardHandleWasLiftedFromWire(
  key: PermissionCardInternal,
  handle: GuestPermissionCardHandle,
): boolean {
  requirePermissionCardInternal(key);
  return stateOf(handle).lifted;
}

export function abandonGuestPermissionCardWireHandle(
  key: PermissionCardInternal,
  raw: RawPermissionCard,
  wireOwner: object,
): void {
  requirePermissionCardInternal(key);
  const owner = owners.get(raw);
  if (owner === undefined || owner === wireOwner) {
    owners.set(raw, transferredOwner);
  }
}

/** Bind a handle to the public card that wraps it. A handle has at most one card. */
export function bindGuestPermissionCardHandleToCard(
  key: PermissionCardInternal,
  handle: GuestPermissionCardHandle,
  card: object,
): void {
  requirePermissionCardInternal(key);
  const state = stateOf(handle);
  if (state.boundToCard) {
    throw new Error('permission-card handle is already held by another card');
  }
  state.boundToCard = true;
  cardObjects.add(card);
  cardHandles.set(card, handle);
}

/** `true` when a public card object already wraps `handle`. */
export function guestPermissionCardHandleIsBoundToCard(
  key: PermissionCardInternal,
  handle: GuestPermissionCardHandle,
): boolean {
  requirePermissionCardInternal(key);
  return stateOf(handle).boundToCard;
}

/** The resource currently held by a public card object, or `undefined` for any other value. */
export function peekPermissionCardObjectResource(
  key: PermissionCardInternal,
  value: unknown,
): RawPermissionCard | undefined {
  requirePermissionCardInternal(key);
  if (typeof value !== 'object' || value === null) return undefined;
  const handle = cardHandles.get(value);
  return handle === undefined ? undefined : stateOf(handle).raw;
}

/**
 * Give up a schema value's claim on a handle after a failed conversion. A
 * handle that a `PermissionCard` owns stays with the card.
 */
export function relinquishGuestPermissionCardHandle(
  key: PermissionCardInternal,
  handle: GuestPermissionCardHandle,
): void {
  requirePermissionCardInternal(key);
  if (stateOf(handle).boundToCard) return;
  releaseGuestPermissionCardHandle(key, handle);
}

export function releaseGuestPermissionCardHandle(
  key: PermissionCardInternal,
  handle: GuestPermissionCardHandle,
): RawPermissionCard | undefined {
  requirePermissionCardInternal(key);
  const state = stateOf(handle);
  const raw = state.raw;
  if (raw !== undefined && state.tracked && owners.get(raw) !== handle) {
    throw new Error('permission-card handle ownership is invalid');
  }
  state.raw = undefined;
  if (raw !== undefined && state.tracked) owners.delete(raw);
  return raw;
}
