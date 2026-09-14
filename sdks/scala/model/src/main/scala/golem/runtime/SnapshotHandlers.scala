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

/**
 * Payload returned by a snapshot save operation.
 *
 * @param bytes
 *   The serialized state
 * @param mimeType
 *   The MIME type of the serialized data (e.g. "application/json" or
 *   "application/octet-stream")
 */
final case class SnapshotPayload(bytes: Array[Byte], mimeType: String)

/**
 * Snapshot save/load handlers for an agent instance.
 *
 * @tparam Instance
 *   The agent trait type
 * @param save
 *   Serializes the current agent state into a [[SnapshotPayload]]
 * @param load
 *   Constructs a fresh agent instance from the snapshot bytes and restore
 *   context.
 */
final case class SnapshotHandlers[Instance](
  save: Instance => Future[SnapshotPayload],
  load: (Array[Byte], SnapshotRestoreContext) => Future[Instance]
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

}
