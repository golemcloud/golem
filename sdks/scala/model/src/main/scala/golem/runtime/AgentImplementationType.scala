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

import golem.Principal
import golem.config.ConfigBuilder
import golem.data.GolemSchema

import scala.concurrent.Future

/**
 * Reflected structure + handlers for an agent implementation.
 *
 * This is produced by macros and consumed by the runtime to wire incoming
 * calls.
 */
final case class AgentImplementationType[Instance, Ctor](
  metadata: AgentMetadata,
  idSchema: GolemSchema[Ctor],
  buildInstance: (Ctor, Principal) => Instance,
  methods: List[ImplementationMethod[Instance]],
  configBuilder: Option[ConfigBuilder[_]] = None,
  configInjectedViaConstructor: Boolean = false,
  principalInjectedViaConstructor: Boolean = false,
  snapshotHandlers: Option[SnapshotHandlers[Instance]] = None
)

sealed trait ImplementationMethod[Instance] {
  def metadata: MethodMetadata
}

final case class SyncImplementationMethod[Instance, In, Out](
  metadata: MethodMetadata,
  inputSchema: GolemSchema[In],
  outputSchema: GolemSchema[Out],
  handler: (Instance, In, Principal) => Out
) extends ImplementationMethod[Instance]

final case class AsyncImplementationMethod[Instance, In, Out](
  metadata: MethodMetadata,
  inputSchema: GolemSchema[In],
  outputSchema: GolemSchema[Out],
  handler: (Instance, In, Principal) => Future[Out]
) extends ImplementationMethod[Instance]
