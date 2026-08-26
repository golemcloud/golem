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

import scala.collection.immutable.ListMap

/** Opaque, affine holder for an owned `permission-card` resource handle. */
final class GuestPermissionCardHandle private (private var cell: Option[Any]) {

  /** True while the card has not yet been transferred. */
  def isPresent: Boolean = cell.isDefined

  /** Move the raw host resource out of this holder at most once. */
  private[golem] def take(): Option[Any] = {
    val current = cell
    cell = None
    current
  }

  /** Borrow the raw resource identity without transferring it. */
  private[golem] def withHandle[T](f: Any => T): Option[T] = cell.map(f)
}

object GuestPermissionCardHandle {

  /** Wrap a freshly received card. Guest code cannot forge or re-wrap cards. */
  private[golem] def fromRaw(raw: Any): GuestPermissionCardHandle =
    new GuestPermissionCardHandle(Some(raw))

  /**
   * Build an encoder for a permission-card schema with the required static
   * polymorphism. Callers must choose this explicitly because the opaque handle
   * cannot be inspected to derive the schema.
   */
  def intoSchema(spec: PermissionCardSpec): IntoSchema[GuestPermissionCardHandle] =
    new IntoSchema[GuestPermissionCardHandle] {
      override lazy val graph: SchemaGraph =
        SchemaGraph(ListMap.empty, SchemaType(SchemaTypeBody.PermissionCardType(spec)))

      override def toValue(handle: GuestPermissionCardHandle): SchemaValue =
        SchemaValue.PermissionCardHandle(handle)
    }

  implicit val fromSchema: FromSchema[GuestPermissionCardHandle] =
    new FromSchema[GuestPermissionCardHandle] {
      override def fromValue(
        value: SchemaValue
      ): Either[FromSchemaError, GuestPermissionCardHandle] =
        value match {
          case SchemaValue.PermissionCardHandle(handle) => Right(handle)
          case other                                    => Left(FromSchemaError(s"expected permission-card handle, got $other"))
        }
    }
}
