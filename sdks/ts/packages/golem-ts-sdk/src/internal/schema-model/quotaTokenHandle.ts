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

// Guest-side carrier for the opaque, owned `golem:core/types.quota-token`
// resource handle.
//
// A `quota-token` is an affine capability: the guest may hold and transfer it,
// but can never inspect or forge it. Inside a `SchemaValue` the handle lives in
// a take-once cell so the value tree can be shared (e.g. duplicated by a caller)
// without duplicating the underlying capability. Lowering a value that contains
// a token (`schemaValueToWit`) moves the underlying `own<quota-token>` resource
// out of the cell exactly once (first encode wins). A handle that was already
// transferred is `consumed`; the same handle appearing twice in one value tree
// is an alias — both are rejected by the encoder's preflight pass.

import type { QuotaToken as RawQuotaToken } from 'golem:core/types@2.0.0';
import { registerCapabilityAdoption } from './capabilityTransaction';
import { QUOTA_INTERNAL, type QuotaInternal } from './quotaInternal';

interface QuotaTokenHandleState {
  raw: RawQuotaToken | undefined;
  readonly tracked: boolean;
}

const states = new WeakMap<GuestQuotaTokenHandle, QuotaTokenHandleState>();
const owners = new WeakMap<RawQuotaToken, object>();
const transferredOwner = Object.freeze({});

export class GuestQuotaTokenHandle {
  constructor(key: QuotaInternal, raw: RawQuotaToken, tracked = true) {
    if (key !== QUOTA_INTERNAL) {
      throw new Error('GuestQuotaTokenHandle construction is an internal SDK operation');
    }
    states.set(this, { raw, tracked });
  }

  /**
   * Quota-token handles are unforgeable capabilities, not data: serializing one
   * (e.g. via `JSON.stringify`) is always an error. Transfer them only by
   * passing the owning `QuotaToken` through a WIT `schema-value-tree`.
   */
  toJSON(): never {
    throw new Error(
      'quota-token handles cannot be serialized; transfer them through a WIT schema-value-tree',
    );
  }
}

function requireQuotaInternal(key: QuotaInternal): void {
  if (key !== QUOTA_INTERNAL) {
    throw new Error('this is an internal SDK operation on a quota-token handle');
  }
}

function stateOf(handle: GuestQuotaTokenHandle): QuotaTokenHandleState {
  const state = states.get(handle);
  if (state === undefined) {
    throw new Error('invalid quota-token handle');
  }
  return state;
}

export function createGuestQuotaTokenHandle(
  key: QuotaInternal,
  raw: RawQuotaToken,
): GuestQuotaTokenHandle {
  requireQuotaInternal(key);
  if (owners.has(raw)) {
    throw new Error('quota-token handle is already owned');
  }
  const handle = new GuestQuotaTokenHandle(key, raw);
  owners.set(raw, handle);
  return handle;
}

export function createUntrackedGuestQuotaTokenHandle(
  key: QuotaInternal,
  raw: RawQuotaToken,
): GuestQuotaTokenHandle {
  requireQuotaInternal(key);
  return new GuestQuotaTokenHandle(key, raw, false);
}

export function adoptGuestQuotaTokenHandle(
  key: QuotaInternal,
  raw: RawQuotaToken,
): GuestQuotaTokenHandle {
  const handle = createGuestQuotaTokenHandle(key, raw);
  registerCapabilityAdoption(() => releaseGuestQuotaTokenHandle(key, handle));
  return handle;
}

export function peekGuestQuotaTokenHandle(
  key: QuotaInternal,
  handle: GuestQuotaTokenHandle,
): RawQuotaToken | undefined {
  requireQuotaInternal(key);
  return stateOf(handle).raw;
}

export function takeGuestQuotaTokenHandle(
  key: QuotaInternal,
  handle: GuestQuotaTokenHandle,
): RawQuotaToken | undefined {
  requireQuotaInternal(key);
  const state = stateOf(handle);
  const raw = state.raw;
  if (raw !== undefined && state.tracked && owners.get(raw) !== handle) {
    throw new Error('quota-token handle ownership is invalid');
  }
  state.raw = undefined;
  if (raw !== undefined && state.tracked) owners.set(raw, transferredOwner);
  return raw;
}

export function takeGuestQuotaTokenHandleToWire(
  key: QuotaInternal,
  handle: GuestQuotaTokenHandle,
  wireOwner: object,
): RawQuotaToken | undefined {
  requireQuotaInternal(key);
  const state = stateOf(handle);
  const raw = state.raw;
  if (raw !== undefined && state.tracked && owners.get(raw) !== handle) {
    throw new Error('quota-token handle ownership is invalid');
  }
  state.raw = undefined;
  if (raw !== undefined && state.tracked) owners.set(raw, wireOwner);
  return raw;
}

export function assertGuestQuotaTokenHandleCanLiftFromWire(
  key: QuotaInternal,
  raw: RawQuotaToken,
  wireOwner: object,
): void {
  requireQuotaInternal(key);
  const owner = owners.get(raw);
  if (owner !== undefined && owner !== wireOwner) {
    throw new Error('quota-token handle is already owned');
  }
}

export function liftGuestQuotaTokenHandleFromWire(
  key: QuotaInternal,
  raw: RawQuotaToken,
  wireOwner: object,
): GuestQuotaTokenHandle {
  assertGuestQuotaTokenHandleCanLiftFromWire(key, raw, wireOwner);
  const handle = new GuestQuotaTokenHandle(key, raw);
  owners.set(raw, handle);
  return handle;
}

export function abandonGuestQuotaTokenWireHandle(
  key: QuotaInternal,
  raw: RawQuotaToken,
  wireOwner: object,
): void {
  requireQuotaInternal(key);
  const owner = owners.get(raw);
  if (owner === undefined || owner === wireOwner) {
    owners.set(raw, transferredOwner);
  }
}

export function releaseGuestQuotaTokenHandle(
  key: QuotaInternal,
  handle: GuestQuotaTokenHandle,
): RawQuotaToken | undefined {
  requireQuotaInternal(key);
  const state = stateOf(handle);
  const raw = state.raw;
  if (raw !== undefined && state.tracked && owners.get(raw) !== handle) {
    throw new Error('quota-token handle ownership is invalid');
  }
  state.raw = undefined;
  if (raw !== undefined && state.tracked) owners.delete(raw);
  return raw;
}
