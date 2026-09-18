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
import { registerCapabilityAdoption } from './capabilityTransaction';
import { PERMISSION_CARD_INTERNAL, type PermissionCardInternal } from './permissionCardInternal';

interface PermissionCardHandleState {
  raw: RawPermissionCard | undefined;
  readonly tracked: boolean;
}

const states = new WeakMap<GuestPermissionCardHandle, PermissionCardHandleState>();
const owners = new WeakMap<RawPermissionCard, object>();
const transferredOwner = Object.freeze({});

/** Guest-side take-once carrier for an owned, opaque permission-card resource. */
export class GuestPermissionCardHandle {
  constructor(key: PermissionCardInternal, raw: RawPermissionCard, tracked = true) {
    if (key !== PERMISSION_CARD_INTERNAL) {
      throw new Error('GuestPermissionCardHandle construction is an internal SDK operation');
    }
    states.set(this, { raw, tracked });
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
  if (raw !== undefined && state.tracked) owners.set(raw, wireOwner);
  return raw;
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
  const handle = new GuestPermissionCardHandle(key, raw);
  owners.set(raw, handle);
  return handle;
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
