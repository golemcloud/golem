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

import {
  type FailedReservation,
  newToken,
  reserve as rawReserve,
  split as rawSplit,
  merge as rawMerge,
  Reservation as RawReservation,
} from 'golem:quota/types@1.5.0';
import { GuestQuotaTokenHandle, type SchemaValue, v } from '../internal/schema-model';
import { QUOTA_INTERNAL, type QuotaInternal } from '../internal/schema-model/quotaInternal';
import { isPromiseLike } from './guard';
import { Result } from './result';

export type { FailedReservation };

/**
 * A committed or in-flight resource-consumption handle.
 *
 * Dropping a `Reservation` without calling `commit` is equivalent to
 * committing zero usage — unused capacity is returned to the pool.
 */
export class Reservation {
  constructor(private readonly raw: RawReservation) {}

  /**
   * Commit actual usage.
   *
   * - If `used` < reserved — unused capacity is returned to the pool.
   * - If `used` > reserved — the excess is deducted from the token's
   *   remaining allocation as "debt".
   */
  commit(used: bigint): void {
    RawReservation.commit(this.raw, used);
  }
}

/**
 * An unforgeable capability granting the right to consume a named resource.
 *
 * Dropping the token releases the underlying lease back to the executor pool.
 *
 * Typical usage:
 * ```ts
 * const token = acquireQuotaToken('openai-tokens', 1000n);
 * const result = token.reserve(500n);
 * if (result.isOk()) {
 *   // ... do work ...
 *   result.unwrap().commit(actualUsed);
 * }
 * ```
 *
 * Or use the RAII helper {@link withQuotaToken} to commit automatically.
 */
export class QuotaToken {
  // True ECMAScript private field: the opaque owned handle is unreachable from
  // guest code, so a `QuotaToken` cannot be forged or have its capability
  // extracted by reaching into the instance.
  readonly #handle: GuestQuotaTokenHandle;

  private constructor(handle: GuestQuotaTokenHandle) {
    if (!(handle instanceof GuestQuotaTokenHandle)) {
      throw new Error('QuotaToken can only be constructed from an opaque quota-token handle');
    }
    this.#handle = handle;
  }

  /**
   * Reserve `amount` units from the local allocation.
   *
   * Returns `Result.ok(reservation)` when capacity is available, or
   * `Result.err(failedReservation)` when the resource's enforcement policy
   * is `reject`.  For `throttle` / `terminate` policies the call suspends or
   * terminates the agent before returning.
   *
   * Traps if this token has already been transferred (for example, sent to
   * another agent through an RPC call or returned from a method). Split the
   * token first if you need to both keep and send a capability.
   */
  reserve(amount: bigint): Result<Reservation, FailedReservation> {
    const result = this.#handle.withHandle((raw): Result<Reservation, FailedReservation> => {
      try {
        return Result.ok(new Reservation(rawReserve(raw, amount)));
      } catch (e) {
        return Result.err(e as FailedReservation);
      }
    });
    if (result === undefined) {
      throw new Error(TOKEN_CONSUMED);
    }
    return result;
  }

  /**
   * Split off a child token that receives `childExpectedUse` units of
   * expected-use from this token.
   *
   * - The parent's `expectedUse` is reduced by `childExpectedUse`.
   * - Credits are divided proportionally between parent and child.
   *
   * Traps if `childExpectedUse` exceeds the parent's current expected-use, or
   * if this token has already been transferred.
   */
  split(childExpectedUse: bigint): QuotaToken {
    const raw = this.#handle.withHandle((h) => rawSplit(h, childExpectedUse));
    if (raw === undefined) {
      throw new Error(TOKEN_CONSUMED);
    }
    return new QuotaToken(GuestQuotaTokenHandle.fromRaw(QUOTA_INTERNAL, raw));
  }

  /**
   * Merge `other` back into this token, combining expected-use and credits.
   *
   * Both tokens must refer to the same resource (same resource-name and
   * environment).  `other` is consumed by this call.
   *
   * Traps if the tokens refer to different resources, or if either token has
   * already been transferred.
   */
  merge(other: QuotaToken): void {
    // Reject merging a token into itself before taking any handle, so a shared
    // handle is not consumed by the receiver and then read again as `other`.
    if (other.#handle === this.#handle) {
      throw new Error('cannot merge a quota token with itself');
    }
    // Check this token first so a consumed receiver does not consume `other`.
    if (!this.#handle.isPresent()) {
      throw new Error(TOKEN_CONSUMED);
    }
    const otherRaw = other.#handle.take();
    if (otherRaw === undefined) {
      throw new Error(TOKEN_CONSUMED);
    }
    this.#handle.withHandle((h) => rawMerge(h, otherRaw));
  }

  /**
   * Lower the token into a schema value by sharing its opaque owned handle. The
   * handle is not transferred here; it is moved out of the cell only when the
   * resulting `SchemaValue` is encoded into a WIT `schema-value-tree`.
   *
   * This exposes the opaque handle, so it is gated behind the unexported
   * {@link QUOTA_INTERNAL} key: only SDK-internal code (the value mapping layer)
   * may extract a token's handle. A guest cannot, so it cannot reach the raw
   * owned resource to forge or duplicate the capability.
   */
  _toSchemaValue(key: QuotaInternal): SchemaValue {
    requireQuotaInternal(key);
    return v.quotaToken(this.#handle);
  }

  /**
   * Reconstruct a token from a decoded schema value's opaque handle. Gated
   * behind {@link QUOTA_INTERNAL} so only SDK-internal code can wrap a handle
   * back into a token.
   */
  static _fromSchemaValue(key: QuotaInternal, value: SchemaValue): QuotaToken {
    requireQuotaInternal(key);
    if (value.tag !== 'quota-token') {
      throw new Error(`Expected a quota-token schema value, got '${value.tag}'`);
    }
    return new QuotaToken(value.handle);
  }

  /**
   * Wrap a freshly acquired owned handle. Gated behind {@link QUOTA_INTERNAL} so
   * only SDK-internal code can construct a token from a raw handle.
   */
  static _fromHandle(key: QuotaInternal, handle: GuestQuotaTokenHandle): QuotaToken {
    requireQuotaInternal(key);
    return new QuotaToken(handle);
  }

  /**
   * Quota tokens are unforgeable capabilities, not data: serializing one (e.g.
   * via `JSON.stringify`) is always an error. Pass them directly to RPC / agent
   * boundaries instead, where they travel as an opaque owned handle.
   */
  toJSON(): never {
    throw new Error(
      'quota tokens cannot be serialized; pass them directly to RPC or agent boundaries',
    );
  }
}

function requireQuotaInternal(key: QuotaInternal): void {
  if (key !== QUOTA_INTERNAL) {
    throw new Error('this is an internal SDK operation on a quota token');
  }
}

const TOKEN_CONSUMED =
  'quota token has already been transferred and can no longer be used; split the token first if ' +
  'you need to both keep and send a capability';

/**
 * Construct a `QuotaToken` for the named resource.
 *
 * @param resourceName - The resource name as declared in the manifest.
 * @param expectedUse  - Expected units per reservation; used to derive the
 *                       credit rate and max-credit for fair scheduling.
 */
export function acquireQuotaToken(resourceName: string, expectedUse: bigint): QuotaToken {
  return QuotaToken._fromHandle(
    QUOTA_INTERNAL,
    GuestQuotaTokenHandle.fromRaw(QUOTA_INTERNAL, newToken(resourceName, expectedUse)),
  );
}

export function withReservation<R>(
  token: QuotaToken,
  amount: bigint,
  fn: (reservation: Reservation) => Promise<{ used: bigint; value: R }>,
): Promise<Result<R, FailedReservation>>;

export function withReservation<R>(
  token: QuotaToken,
  amount: bigint,
  fn: (reservation: Reservation) => { used: bigint; value: R },
): Result<R, FailedReservation>;

/**
 * Reserve `amount` units, run `fn`, then commit the actual usage returned
 * by `fn`. Commits zero if `fn` throws or the returned Promise rejects.
 * Supports both sync and async callbacks.
 *
 * Returns `Result.ok(value)` on success, or `Result.err(failedReservation)`
 * if the reservation could not be granted.
 *
 * @param token  - The token to reserve against.
 * @param amount - Units to reserve.
 * @param fn     - Work to perform; must return the actual units consumed.
 */ export function withReservation<R>(
  token: QuotaToken,
  amount: bigint,
  fn: (
    reservation: Reservation,
  ) => { used: bigint; value: R } | Promise<{ used: bigint; value: R }>,
): Result<R, FailedReservation> | Promise<Result<R, FailedReservation>> {
  const reserveResult = token.reserve(amount);
  if (reserveResult.isErr()) {
    return Result.err(reserveResult.unwrapErr());
  }
  const reservation = reserveResult.unwrap();
  try {
    const result = fn(reservation);
    if (isPromiseLike(result)) {
      return result.then(
        ({ used, value }) => {
          reservation.commit(used);
          return Result.ok(value);
        },
        (e) => {
          reservation.commit(0n);
          throw e;
        },
      );
    }
    const { used, value } = result;
    reservation.commit(used);
    return Result.ok(value);
  } catch (e) {
    reservation.commit(0n);
    throw e;
  }
}
