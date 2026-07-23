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

import golem.schema.{MetadataEnvelope, SchemaGraph}

/**
 * Where a constructor/method parameter's value comes from at the
 * `golem:agent@2.0.0` boundary.
 *
 * `UserSupplied` parameters consume one field of the input record (in
 * declaration order). `AutoInjectedPrincipal` parameters are injected by the
 * runtime and never consume a record field.
 *
 * Note: the Scala SDK currently emits only `UserSupplied` named fields,
 * omitting the (internally injected) principal from the input schema;
 * `AutoInjectedPrincipal` is reserved for future cross-SDK metadata parity.
 */
sealed trait FieldSource extends Product with Serializable

object FieldSource {
  case object UserSupplied          extends FieldSource
  case object AutoInjectedPrincipal extends FieldSource
}

/**
 * A single named parameter of a constructor/method input, with its own
 * self-contained [[SchemaGraph]] (the v2 `named-field`).
 */
final case class ParameterMetadata(
  name: String,
  source: FieldSource,
  graph: SchemaGraph,
  metadata: MetadataEnvelope = MetadataEnvelope.empty
)

/**
 * The schema-native description of a constructor's or method's input: the
 * ordered list of named parameters (the v2 `input-schema = parameters`).
 */
final case class InputMetadata(parameters: List[ParameterMetadata]) {

  /**
   * The user-supplied parameters (those that consume an input-record field).
   */
  def userSupplied: List[ParameterMetadata] =
    parameters.filter(_.source == FieldSource.UserSupplied)
}

object InputMetadata {
  val empty: InputMetadata = InputMetadata(Nil)
}

/**
 * The schema-native description of a method's output (the v2 `output-schema`):
 * `Unit` (the host returns `none`) or `Single` (the host returns `some(tree)`).
 */
sealed trait OutputMetadata extends Product with Serializable

object OutputMetadata {
  case object Unit                            extends OutputMetadata
  final case class Single(graph: SchemaGraph) extends OutputMetadata
}
