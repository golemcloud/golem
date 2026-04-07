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
  QuotaToken as RawQuotaToken,
  Reservation as RawReservation,
} from 'golem:quota/host@1.5.0';
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
  constructor(private readonly raw: RawQuotaToken) {}

  /**
   * Reserve `amount` units from the local allocation.
   *
   * Returns `Result.ok(reservation)` when capacity is available, or
   * `Result.err(failedReservation)` when the resource's enforcement policy
   * is `reject`.  For `throttle` / `terminate` policies the call suspends or
   * terminates the agent before returning.
   */
  reserve(amount: bigint): Result<Reservation, FailedReservation> {
    try {
      const raw = this.raw.reserve(amount);
      return Result.ok(new Reservation(raw));
    } catch (e) {
      return Result.err(e as FailedReservation);
    }
  }

  /**
   * Split off a child token that receives `childExpectedUse` units of
   * expected-use from this token.
   *
   * - The parent's `expectedUse` is reduced by `childExpectedUse`.
   * - Credits are divided proportionally between parent and child.
   *
   * Traps if `childExpectedUse` exceeds the parent's current expected-use.
   */
  split(childExpectedUse: bigint): QuotaToken {
    return new QuotaToken(this.raw.split(childExpectedUse));
  }

  /**
   * Merge `other` back into this token, combining expected-use and credits.
   *
   * Both tokens must refer to the same resource (same resource-name and
   * environment).  `other` is consumed by this call.
   *
   * Traps if the tokens refer to different resources.
   */
  merge(other: QuotaToken): void {
    this.raw.merge(other.raw);
  }
}

/**
 * Construct a `QuotaToken` for the named resource.
 *
 * @param resourceName - The resource name as declared in the manifest.
 * @param expectedUse  - Expected units per reservation; used to derive the
 *                       credit rate and max-credit for fair scheduling.
 */
export function acquireQuotaToken(resourceName: string, expectedUse: bigint): QuotaToken {
  return new QuotaToken(new RawQuotaToken(resourceName, expectedUse));
}

/**
 * Reserve `amount` units, run `fn`, then commit the actual usage returned
 * by `fn`.  Commits zero if `fn` throws.
 *
 * Returns `Result.ok(value)` on success, or `Result.err(failedReservation)`
 * if the reservation could not be granted.
 *
 * @param token  - The token to reserve against.
 * @param amount - Units to reserve.
 * @param fn     - Work to perform; must return the actual units consumed.
 */
export function withReservation<T>(
  token: QuotaToken,
  amount: bigint,
  fn: (reservation: Reservation) => { used: bigint; value: T },
): Result<T, FailedReservation> {
  const reserveResult = token.reserve(amount);
  if (reserveResult.isErr()) {
    return Result.err(reserveResult.unwrapErr());
  }
  const reservation = reserveResult.unwrap();
  try {
    const { used, value } = fn(reservation);
    reservation.commit(used);
    return Result.ok(value);
  } catch (e) {
    reservation.commit(0n);
    throw e;
  }
}
