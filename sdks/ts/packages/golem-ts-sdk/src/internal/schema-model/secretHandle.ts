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
import { registerCapabilityAdoption } from './capabilityTransaction';
import { SECRET_INTERNAL, type SecretInternal } from './secretInternal';

interface SecretHandleState {
  raw: RawSecret | undefined;
  readonly onTake?: () => void;
  readonly tracked: boolean;
}

const states = new WeakMap<GuestSecretHandle, SecretHandleState>();
const owners = new WeakMap<RawSecret, object>();
const transferredOwner = Object.freeze({});

export class GuestSecretHandle {
  constructor(key: SecretInternal, raw: RawSecret, onTake?: () => void, tracked = true) {
    if (key !== SECRET_INTERNAL) {
      throw new Error('GuestSecretHandle construction is an internal SDK operation');
    }
    states.set(this, { raw, onTake, tracked });
  }

  toJSON(): never {
    throw new Error(
      'secret handles cannot be serialized; transfer them through a WIT schema-value-tree',
    );
  }
}

function requireSecretInternal(key: SecretInternal): void {
  if (key !== SECRET_INTERNAL) {
    throw new Error('this is an internal SDK operation on a secret handle');
  }
}

function stateOf(handle: GuestSecretHandle): SecretHandleState {
  const state = states.get(handle);
  if (state === undefined) {
    throw new Error('invalid secret handle');
  }
  return state;
}

export function createGuestSecretHandle(
  key: SecretInternal,
  raw: RawSecret,
  onTake?: () => void,
): GuestSecretHandle {
  requireSecretInternal(key);
  if (owners.has(raw)) {
    throw new Error('secret handle is already owned');
  }
  const handle = new GuestSecretHandle(key, raw, onTake);
  owners.set(raw, handle);
  return handle;
}

export function createUntrackedGuestSecretHandle(
  key: SecretInternal,
  raw: RawSecret,
): GuestSecretHandle {
  requireSecretInternal(key);
  return new GuestSecretHandle(key, raw, undefined, false);
}

export function adoptGuestSecretHandle(key: SecretInternal, raw: RawSecret): GuestSecretHandle {
  const handle = createGuestSecretHandle(key, raw);
  registerCapabilityAdoption(() => releaseGuestSecretHandle(key, handle));
  return handle;
}

export function peekGuestSecretHandle(
  key: SecretInternal,
  handle: GuestSecretHandle,
): RawSecret | undefined {
  requireSecretInternal(key);
  return stateOf(handle).raw;
}

export function takeGuestSecretHandle(
  key: SecretInternal,
  handle: GuestSecretHandle,
): RawSecret | undefined {
  requireSecretInternal(key);
  const state = stateOf(handle);
  const raw = state.raw;
  if (raw !== undefined && state.tracked && owners.get(raw) !== handle) {
    throw new Error('secret handle ownership is invalid');
  }
  state.raw = undefined;
  if (raw !== undefined) {
    if (state.tracked) owners.set(raw, transferredOwner);
    state.onTake?.();
  }
  return raw;
}

export function takeGuestSecretHandleToWire(
  key: SecretInternal,
  handle: GuestSecretHandle,
  wireOwner: object,
): RawSecret | undefined {
  requireSecretInternal(key);
  const state = stateOf(handle);
  const raw = state.raw;
  if (raw !== undefined && state.tracked && owners.get(raw) !== handle) {
    throw new Error('secret handle ownership is invalid');
  }
  state.raw = undefined;
  if (raw !== undefined) {
    if (state.tracked) owners.set(raw, wireOwner);
    state.onTake?.();
  }
  return raw;
}

export function assertGuestSecretHandleCanLiftFromWire(
  key: SecretInternal,
  raw: RawSecret,
  wireOwner: object,
): void {
  requireSecretInternal(key);
  const owner = owners.get(raw);
  if (owner !== undefined && owner !== wireOwner) {
    throw new Error('secret handle is already owned');
  }
}

export function liftGuestSecretHandleFromWire(
  key: SecretInternal,
  raw: RawSecret,
  wireOwner: object,
): GuestSecretHandle {
  assertGuestSecretHandleCanLiftFromWire(key, raw, wireOwner);
  const handle = new GuestSecretHandle(key, raw);
  owners.set(raw, handle);
  return handle;
}

export function releaseGuestSecretHandle(
  key: SecretInternal,
  handle: GuestSecretHandle,
): RawSecret | undefined {
  requireSecretInternal(key);
  const state = stateOf(handle);
  const raw = state.raw;
  if (raw !== undefined && state.tracked && owners.get(raw) !== handle) {
    throw new Error('secret handle ownership is invalid');
  }
  state.raw = undefined;
  if (raw !== undefined) {
    if (state.tracked) owners.delete(raw);
    state.onTake?.();
  }
  return raw;
}
