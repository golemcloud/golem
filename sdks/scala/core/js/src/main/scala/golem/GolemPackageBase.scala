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

package golem

private[golem] trait GolemPackageBase {
  // ---------------------------------------------------------------------------
  // Annotations (commonly used on agent traits / methods)
  // ---------------------------------------------------------------------------
  type agentDefinition     = runtime.annotations.agentDefinition
  type agentImplementation = runtime.annotations.agentImplementation
  type description         = runtime.annotations.description
  type prompt              = runtime.annotations.prompt
  type endpoint            = runtime.annotations.endpoint
  type header              = runtime.annotations.header

  type DurabilityMode = runtime.annotations.DurabilityMode
  val DurabilityMode: runtime.annotations.DurabilityMode.type = runtime.annotations.DurabilityMode

  // ---------------------------------------------------------------------------
  // Schema / data model
  // ---------------------------------------------------------------------------
  type GolemSchema[A] = data.GolemSchema[A]
  val GolemSchema: data.GolemSchema.type = data.GolemSchema

  type StructuredSchema = data.StructuredSchema
  val StructuredSchema: data.StructuredSchema.type = data.StructuredSchema

  type StructuredValue = data.StructuredValue
  val StructuredValue: data.StructuredValue.type = data.StructuredValue
}
