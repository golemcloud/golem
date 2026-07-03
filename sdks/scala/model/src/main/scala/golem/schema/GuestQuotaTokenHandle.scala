/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.schema

/**
 * Opaque, affine (take-once) holder for an owned `golem:core/types@2.0.0`
 * `quota-token` resource handle as it travels inside a [[SchemaValue]].
 *
 * The raw handle is a host resource that may only be transferred once. The
 * holder hides the raw value (stored as an opaque `Any`) so guest code cannot
 * read or forge a quota token; it can only move the handle to a single
 * destination (an RPC argument, a return value, ...). The raw value is only
 * ever meaningful on the JS (Scala.js) side, where it is the live host
 * resource; the cross-compiled model treats it as opaque.
 *
 * Equality is by identity (the default for a plain class): two holders are
 * equal only if they are the same instance. This makes a [[SchemaValue]] that
 * carries a handle comparable by handle identity rather than by any structural
 * content, which an opaque capability does not have.
 */
final class GuestQuotaTokenHandle private (private var cell: Option[Any]) {

  /** True while the handle has not yet been transferred. */
  def isPresent: Boolean = cell.isDefined

  /**
   * Move the raw handle out, leaving this holder empty. Returns `None` if the
   * handle was already transferred.
   *
   * Restricted to the SDK: the raw value is the live host capability, so guest
   * code must never be able to extract it.
   */
  private[golem] def take(): Option[Any] = {
    val current = cell
    cell = None
    current
  }

  /**
   * Borrow the raw handle without consuming it, e.g. for a
   * `borrow<quota-token>` host call. Returns `None` if the handle was already
   * transferred.
   *
   * Restricted to the SDK for the same reason as [[take]].
   */
  private[golem] def withHandle[T](f: Any => T): Option[T] = cell.map(f)
}

object GuestQuotaTokenHandle {

  /**
   * Wrap a freshly acquired owned raw handle in a take-once holder. Restricted
   * to the SDK so guest code cannot re-wrap (and thus duplicate) a raw host
   * capability it should never be able to read.
   */
  private[golem] def fromRaw(raw: Any): GuestQuotaTokenHandle = new GuestQuotaTokenHandle(Some(raw))
}
