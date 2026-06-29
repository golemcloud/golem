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
 * `secret` resource handle as it travels inside a [[SchemaValue]].
 */
final class GuestSecretHandle private (private var cell: Option[Any]) {

  /** True while the handle has not yet been transferred. */
  def isPresent: Boolean = cell.isDefined

  /** Move the raw handle out, leaving this holder empty. */
  private[golem] def take(): Option[Any] = {
    val current = cell
    cell = None
    current
  }

  /** Borrow the raw handle without consuming it. */
  private[golem] def withHandle[T](f: Any => T): Option[T] = cell.map(f)
}

object GuestSecretHandle {

  /** Wrap a freshly acquired owned raw handle in a take-once holder. */
  private[golem] def fromRaw(raw: Any): GuestSecretHandle = new GuestSecretHandle(Some(raw))
}
