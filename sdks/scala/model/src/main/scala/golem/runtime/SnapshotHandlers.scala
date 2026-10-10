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

package golem.runtime

import golem.{Principal, Uuid}
import golem.config.Config

import scala.concurrent.Future

/**
 * Information available while constructing a fresh instance from a snapshot.
 */
final case class SnapshotRestoreContext(
  identityFields: Vector[Any],
  agentId: String,
  phantomId: Option[Uuid],
  restoredPrincipal: Principal,
  private val freshConfig: Option[Config[_]]
) {
  def identity[A](index: Int): A = identityFields(index).asInstanceOf[A]

  def config[A]: Config[A] = freshConfig match {
    case Some(value) => value.asInstanceOf[Config[A]]
    case None        => throw new IllegalStateException("This agent does not declare configuration")
  }
}

/** Decoded snapshot state, before SDK principal envelope encoding. */
sealed trait SnapshotData

final case class SnapshotPayload(bytes: Array[Byte], mimeType: String) extends SnapshotData

final case class MultipartSnapshot(state: zio.blocks.schema.json.Json, parts: Map[String, SnapshotPart])
    extends SnapshotData {
  def decodeState[S: zio.blocks.schema.Schema]: Either[SnapshotError, S] =
    implicitly[zio.blocks.schema.Schema[S]].jsonCodec.decode(state).left.map(error => SnapshotError(error.toString))

  def requirePart(name: String, expectedContentType: String): Either[SnapshotError, Array[Byte]] =
    for {
      expected <- MultipartSnapshot.normalizeContentType(expectedContentType)
      part     <- parts.get(name).toRight(SnapshotError(s"Missing snapshot part '$name'"))
      actual   <- MultipartSnapshot.normalizeContentType(part.contentType)
      bytes    <- if (actual == expected) Right(part.bytes)
               else Left(SnapshotError(s"Snapshot part '$name' has content type '$actual', expected '$expected'"))
    } yield bytes
}

final case class SnapshotPart(bytes: Array[Byte], contentType: String)
final case class SnapshotError(message: String) extends Exception(message)

object MultipartSnapshot {
  def fromState[S: zio.blocks.schema.Schema](
    state: S,
    parts: Map[String, SnapshotPart]
  ): Either[SnapshotError, MultipartSnapshot] =
    scala.util
      .Try(implicitly[zio.blocks.schema.Schema[S]].jsonCodec.encodeToJson(state))
      .toEither
      .left
      .map(error => SnapshotError(error.toString))
      .map(MultipartSnapshot(_, parts))

  private[golem] def validName(name: String): Boolean = name.matches("[A-Za-z0-9_][A-Za-z0-9_.-]*")

  private[golem] def normalizeContentType(value: String): Either[SnapshotError, String] =
    if (value.matches("[A-Za-z0-9!#$%&'*+.^_`|~-]+/[A-Za-z0-9!#$%&'*+.^_`|~-]+"))
      Right(value.toLowerCase(java.util.Locale.ROOT))
    else Left(SnapshotError(s"Invalid snapshot content type '$value'"))
}

/**
 * Snapshot save/load handlers for an agent instance.
 *
 * @tparam Instance
 *   The agent trait type
 * @param save
 *   Projects the current agent state into [[SnapshotData]]
 * @param load
 *   Constructs a fresh agent instance from decoded snapshot data and restore
 *   context.
 */
final case class SnapshotHandlers[Instance](
  save: Instance => Future[SnapshotData],
  load: (SnapshotData, SnapshotRestoreContext) => Future[Instance]
)

object SnapshotHandlers {

  /**
   * Wraps a raw `Instance => Future[Array[Byte]]` save function into the
   * `Instance => Future[SnapshotPayload]` form expected by
   * [[SnapshotHandlers]].
   */
  def wrapSave[Instance](
    raw: Instance => Future[Array[Byte]]
  ): Instance => Future[SnapshotPayload] =
    (instance: Instance) =>
      raw(instance).map(bytes => SnapshotPayload(bytes, "application/octet-stream"))(
        scala.concurrent.ExecutionContext.parasitic
      )

  def wrapLoad[Instance](
    mimeType: String,
    raw: (Array[Byte], SnapshotRestoreContext) => Future[Instance]
  ): (SnapshotData, SnapshotRestoreContext) => Future[Instance] =
    (data, context) =>
      data match {
        case SnapshotPayload(bytes, mime) if mime == mimeType => raw(bytes, context)
        case _                                                => Future.failed(SnapshotError(s"Expected '$mimeType' snapshot"))
      }

}
