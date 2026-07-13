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

package golem.runtime.autowire

import golem.Principal
import golem.host.js.schema.{JsAgentError, JsAgentType, JsSchemaValueTree}
import golem.runtime.AgentMetadata
import golem.runtime.SnapshotHandlers

import scala.scalajs.js

/**
 * Represents a fully-wired agent definition ready for runtime use.
 *
 * An `AgentDefinition` contains everything needed to:
 *   - Initialize new agent instances
 *   - Invoke methods via RPC
 *   - Export type metadata to the host
 *
 * You typically don't create these directly - use `register` method of
 * `golem.runtime.autowire.AgentImplementation` to generate definitions at
 * compile time.
 *
 * The constructor / method values exchanged at the boundary are
 * `golem:agent@2.0.0` `schema-value-tree`s; method results are an
 * `option<schema-value-tree>` (modelled as a Scala [[Option]]).
 *
 * @tparam Instance
 *   The agent trait type
 * @param typeName
 *   Unique identifier for this agent type
 * @param metadata
 *   Generated metadata describing the agent
 * @param constructor
 *   Handles agent initialization with constructor payloads
 * @param bindings
 *   RPC bindings for each agent method
 * @param mode
 *   The agent's persistence mode
 */
final class AgentDefinition[Instance](
  val typeName: String,
  val metadata: AgentMetadata,
  val constructor: AgentConstructor[Instance],
  bindings: List[MethodBinding[Instance]],
  val mode: AgentMode = AgentMode.Durable,
  val snapshotHandlers: Option[SnapshotHandlers[Instance]] = None
) {

  /**
   * The `golem:agent@2.0.0` type representation of this agent, for host
   * registration. Lazily computed and cached.
   */
  lazy val agentType: JsAgentType =
    AgentTypeEncoderV2.encode(AgentRequestBuilder.fromMetadata(metadata, mode.value))

  private val methodsByName: Map[String, MethodBinding[Instance]] =
    bindings.map(binding => binding.metadata.name -> binding).toMap

  /**
   * Initializes a new agent instance, returning as Any for type-erased
   * contexts.
   */
  def initializeAny(input: JsSchemaValueTree, principal: Principal): js.Promise[Any] =
    initialize(input, principal).asInstanceOf[js.Promise[Any]]

  /**
   * Initializes a new agent instance from a constructor payload.
   */
  def initialize(input: JsSchemaValueTree, principal: Principal): js.Promise[Instance] =
    constructor.initialize(input, principal)

  /**
   * Invokes a method with type-erased instance for dynamic dispatch.
   */
  def invokeAny(
    instance: Any,
    methodName: String,
    input: JsSchemaValueTree,
    principal: Principal
  ): js.Promise[Option[JsSchemaValueTree]] =
    invoke(instance.asInstanceOf[Instance], methodName, input, principal)

  /**
   * Invokes a method on an agent instance.
   */
  def invoke(
    instance: Instance,
    methodName: String,
    input: JsSchemaValueTree,
    principal: Principal
  ): js.Promise[Option[JsSchemaValueTree]] =
    methodsByName
      .get(methodName)
      .map(_.invoke(instance, input, principal))
      .getOrElse(js.Promise.reject(JsAgentError.invalidMethod(s"Unknown method: $methodName")))

  /**
   * Returns the list of method bindings for inspection or testing.
   */
  def methodMetadata: List[MethodBinding[Instance]] = bindings
}
