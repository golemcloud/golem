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

package golem.runtime.autowire

import golem.Principal
import golem.host.js._
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
 * ==Structure==
 * {{{
 * AgentDefinition[MyAgent]
 *   ├── typeName: "my-agent"
 *   ├── metadata: AgentMetadata (name, description, methods)
 *   ├── constructor: AgentConstructor[MyAgent]
 *   ├── bindings: List[MethodBinding[MyAgent]]
 *   └── mode: AgentMode
 * }}}
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
   * The WIT type representation of this agent, for host registration.
   *
   * This is lazily computed and cached. It encodes the agent's schema in a
   * format suitable for the Golem runtime's type system.
   */
  lazy val agentType: JsAgentType =
    AgentTypeEncoder.from(this)
  private val methodsByName: Map[String, MethodBinding[Instance]] =
    bindings.map(binding => binding.metadata.name -> binding).toMap

  /**
   * Initializes a new agent instance, returning as Any for type-erased
   * contexts.
   */
  def initializeAny(payload: JsDataValue, principal: Principal): js.Promise[Any] =
    initialize(payload, principal).asInstanceOf[js.Promise[Any]]

  /**
   * Initializes a new agent instance from a constructor payload.
   *
   * @param payload
   *   The constructor arguments as a JsDataValue
   * @param principal
   *   The principal performing the initialization
   * @return
   *   A Promise resolving to the initialized instance
   */
  def initialize(payload: JsDataValue, principal: Principal): js.Promise[Instance] =
    constructor.initialize(payload, principal)

  /**
   * Invokes a method with type-erased instance for dynamic dispatch.
   */
  def invokeAny(
    instance: Any,
    methodName: String,
    payload: JsDataValue,
    principal: Principal
  ): js.Promise[JsDataValue] =
    invoke(instance.asInstanceOf[Instance], methodName, payload, principal)

  /**
   * Invokes a method on an agent instance.
   *
   * @param instance
   *   The agent instance to invoke on
   * @param methodName
   *   The method to invoke
   * @param payload
   *   The method arguments as a JsDataValue
   * @param principal
   *   The principal performing the invocation
   * @return
   *   A Promise resolving to the method result
   */
  def invoke(
    instance: Instance,
    methodName: String,
    payload: JsDataValue,
    principal: Principal
  ): js.Promise[JsDataValue] = {
    if (!methodsByName.contains(methodName)) {
      scala.scalajs.js.Dynamic.global.console.log(
        s"[AgentDefinition] Unknown method: $methodName, available: ${methodsByName.keySet.mkString(",")}"
      )
    }
    methodsByName
      .get(methodName)
      .map(_.invoke(instance, payload, principal))
      .getOrElse(js.Promise.reject(JsAgentError.invalidMethod(s"Unknown method: $methodName")))
  }

  /**
   * Returns the list of method bindings for inspection or testing.
   */
  def methodMetadata: List[MethodBinding[Instance]] = bindings
}
