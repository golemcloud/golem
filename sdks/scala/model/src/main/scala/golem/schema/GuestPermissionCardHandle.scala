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

import golem.schema.wire.{ConcreteCodec, WitSchemaTypeBody, WitSchemaValueNode}

import scala.collection.immutable.ListMap

/**
 * Opaque, affine holder for an owned `permission-card` resource handle.
 *
 * This is the schema-level carrier. Application code uses the card types of
 * `golem.host.PermissionsApi`, such as `PermissionCard`; each wraps one holder.
 * Guest code cannot build a holder from data: holders come only from values the
 * host supplies. `intoSchema(spec)` chooses the schema metadata declared for
 * the value; it never creates or widens authority, because the host checks
 * every use of the handle.
 */
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

  private var claimedByCard = false

  /** True once a typed card object wraps this holder, even after it moved. */
  private[golem] def isClaimedByCard: Boolean = claimedByCard

  /**
   * Mark this holder as wrapped by a typed card object. Returns `false` when a
   * card object already wraps it, also when that card moved its handle, so a
   * holder never backs two card objects.
   */
  private[golem] def claimForCard(): Boolean =
    if (claimedByCard) false
    else {
      claimedByCard = true
      true
    }

  private var releaseRequested = false

  /**
   * Take the raw resource to drop it. When the holder is empty because its
   * handle is lowered into a call that has not reached the host yet, the
   * release is recorded so that [[giveBack]] does not restore the handle.
   */
  private[golem] def takeForRelease(): Option[Any] = {
    val current = take()
    if (current.isEmpty) releaseRequested = true
    current
  }

  /**
   * Return a raw handle that was taken for a call the host never received.
   * Restricted to the SDK; the holder must be empty. Returns `false` without
   * restoring the handle when a release was requested meanwhile; the caller
   * must then drop the raw resource.
   */
  private[golem] def giveBack(raw: Any): Boolean = {
    if (cell.isDefined) throw new IllegalStateException("cannot give back a handle to a holder that is not empty")
    if (releaseRequested) {
      releaseRequested = false
      false
    } else {
      cell = Some(raw)
      true
    }
  }
}

object GuestPermissionCardHandle {

  val nonPolymorphicSpec: PermissionCardSpec = PermissionCardSpec(polymorphic = false)

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

  implicit val nonPolymorphicIntoSchema: IntoSchema[GuestPermissionCardHandle] =
    intoSchema(nonPolymorphicSpec)

  def concreteCodec(spec: PermissionCardSpec): ConcreteCodec[GuestPermissionCardHandle] =
    ConcreteCodec.scalar[GuestPermissionCardHandle](WitSchemaTypeBody.PermissionCardType(spec))(
      WitSchemaValueNode.PermissionCardHandle.apply
    ) { case WitSchemaValueNode.PermissionCardHandle(handle) => handle }

  implicit val nonPolymorphicConcreteCodec: ConcreteCodec[GuestPermissionCardHandle] =
    concreteCodec(nonPolymorphicSpec)

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
