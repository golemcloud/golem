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

import golem.Principal
import golem.config.ConfigBuilder
import golem.schema.wire.{ConcreteCodec, WitSchemaValueTree}

import scala.concurrent.{ExecutionContext, Future}
import scala.util.control.NonFatal

final case class WireAgentImplementationType[Instance, Ctor](
  metadata: WireAgentMetadata,
  ctorCodec: ConcreteCodec[Ctor],
  buildInstance: (Ctor, Principal) => Instance,
  methods: List[WireImplementationMethod[Instance]],
  configBuilder: Option[ConfigBuilder[?]],
  configInjectedViaConstructor: Boolean,
  snapshotHandlers: Option[SnapshotHandlers[Instance]]
)

trait WireImplementationMethod[Instance] {
  def name: String
  def invoke(instance: Instance, input: WitSchemaValueTree, principal: Principal): Future[Option[WitSchemaValueTree]]
}

final case class WireAgentInputError(message: String) extends RuntimeException(message)

object WireImplementationMethod {
  def apply[Instance, In, Out](methodName: String, input: ConcreteCodec[In], output: Option[ConcreteCodec[Out]])(
    handler: (Instance, In, Principal) => Future[Out]
  ): WireImplementationMethod[Instance] = new WireImplementationMethod[Instance] {
    def name: String = methodName
    def invoke(
      instance: Instance,
      value: WitSchemaValueTree,
      principal: Principal
    ): Future[Option[WitSchemaValueTree]] = {
      val decoded =
        try input.decode(value)
        catch { case NonFatal(error) => throw WireAgentInputError(String.valueOf(error.getMessage)) }
      handler(instance, decoded, principal)
        .map(value => output.map(_.encodeValue(value)))(using ExecutionContext.parasitic)
    }
  }
}
