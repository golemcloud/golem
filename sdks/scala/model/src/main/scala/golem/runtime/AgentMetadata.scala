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

import golem.config.AgentConfigDeclaration
import golem.runtime.http.{HttpEndpointDetails, HttpMountDetails}

/**
 * Describes a single method on an agent.
 *
 * This metadata is generated at compile time by the autowiring macros and used
 * for WIT type generation and runtime dispatch.
 *
 * @param name
 *   The method name as it appears in the trait
 * @param description
 *   Human-readable description (from `@description` annotation)
 * @param prompt
 *   Optional LLM prompt (from `@prompt` annotation)
 * @param mode
 *   Optional method mode override
 * @param input
 *   Schema-native description of the method's input parameters
 * @param output
 *   Schema-native description of the method's return type (unit/single)
 */
final case class MethodMetadata(
  name: String,
  description: Option[String],
  prompt: Option[String],
  mode: Option[String],
  input: InputMetadata,
  output: OutputMetadata,
  httpEndpoints: List[HttpEndpointDetails] = Nil,
  readOnly: Option[ReadOnlyConfig] = None
)

/**
 * Describes an agent's complete interface.
 *
 * This metadata is generated at compile time from the agent trait and includes
 * all information needed for:
 *   - WIT type generation
 *   - Host registration
 *   - Runtime method dispatch
 *
 * @param name
 *   The agent's type name (from the trait name)
 * @param description
 *   Human-readable description (from `@description` annotation)
 * @param mode
 *   The agent's persistence mode (from `@agentDefinition(mode = ...)`)
 * @param methods
 *   Metadata for each method in the agent trait
 * @param constructor
 *   Metadata (and schema-native input) for the agent's constructor parameters
 */
final case class AgentMetadata(
  name: String,
  description: Option[String],
  mode: Option[String],
  methods: List[MethodMetadata],
  constructor: ConstructorMetadata,
  httpMount: Option[HttpMountDetails] = None,
  config: List[AgentConfigDeclaration] = Nil,
  snapshotting: Snapshotting = Snapshotting.Disabled
)
