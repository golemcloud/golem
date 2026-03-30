/*
 * Copyright 2024-2026 John A. De Goes and the ZIO Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.runtime

import scala.concurrent.Future

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
 *   Deserializes bytes into state and applies them to the agent instance. Takes
 *   the current instance and snapshot bytes, returns the (possibly new)
 *   instance to use going forward.
 */
final case class SnapshotHandlers[Instance](
  save: Instance => Future[SnapshotPayload],
  load: (Instance, Array[Byte]) => Future[Instance]
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

  /**
   * Wraps a raw `(Instance, Array[Byte]) => Future[Unit]` load function into
   * the `(Instance, Array[Byte]) => Future[Instance]` form expected by
   * [[SnapshotHandlers]].
   */
  def wrapLoad[Instance](
    raw: (Instance, Array[Byte]) => Future[Unit]
  ): (Instance, Array[Byte]) => Future[Instance] =
    (instance: Instance, bytes: Array[Byte]) =>
      raw(instance, bytes).map(_ => instance)(scala.concurrent.ExecutionContext.parasitic)
}
