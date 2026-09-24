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

import golem.config.{AgentConfigDeclaration, AgentConfigSource}
import golem.runtime.http.{HttpEndpointDetails, HttpMountDetails}
import golem.schema.{MetadataEnvelope, SchemaBuilder, SchemaGraph}
import golem.schema.wire.{GraphEncoder, SchemaWire, WitSchemaGraph}

final case class WireParameterMetadata(name: String, source: FieldSource, schema: Int, metadata: MetadataEnvelope)
final case class WireConstructorMetadata(
  name: Option[String],
  description: String,
  promptHint: Option[String],
  parameters: List[WireParameterMetadata]
)
final case class WireMethodMetadata(
  name: String,
  description: Option[String],
  prompt: Option[String],
  mode: Option[String],
  parameters: List[WireParameterMetadata],
  output: Option[Int],
  httpEndpoints: List[HttpEndpointDetails],
  readOnly: Option[ReadOnlyConfig]
)
final case class WireConfigDeclaration(source: AgentConfigSource, path: List[String], schema: Int)

/**
 * Generated metadata contains one compiler-emitted pool and indices into it.
 */
final case class WireAgentMetadata(
  name: String,
  kind: AgentTypeKind,
  description: Option[String],
  mode: Option[String],
  methods: List[WireMethodMetadata],
  constructor: WireConstructorMetadata,
  httpMount: Option[HttpMountDetails],
  config: List[WireConfigDeclaration],
  snapshotting: Snapshotting,
  schema: WitSchemaGraph
) {
  def reflectedInput(parameters: List[WireParameterMetadata]): InputMetadata =
    InputMetadata(parameters.map(p => ParameterMetadata(p.name, p.source, graph(p.schema), p.metadata)))

  def reflectedMethod(method: WireMethodMetadata): MethodMetadata =
    MethodMetadata(
      method.name,
      method.description,
      method.prompt,
      method.mode,
      reflectedInput(method.parameters),
      method.output.fold[OutputMetadata](OutputMetadata.Unit)(root => OutputMetadata.Single(graph(root))),
      method.httpEndpoints,
      method.readOnly
    )

  def reflectedConstructor: ConstructorMetadata =
    ConstructorMetadata(
      constructor.name,
      constructor.description,
      constructor.promptHint,
      reflectedInput(constructor.parameters)
    )

  /**
   * Explicit reflection is the only generated-path consumer of owned graphs.
   */
  def reflected: AgentMetadata =
    AgentMetadata(
      name,
      kind,
      description,
      mode,
      methods.map(reflectedMethod),
      reflectedConstructor,
      httpMount,
      config.map(c => AgentConfigDeclaration(c.source, c.path, graph(c.schema))),
      snapshotting
    )

  private def graph(root: Int): SchemaGraph = SchemaWire.schemaGraphFromWit(schema.copy(root = root))
}

object WireAgentMetadata {

  /** Used by the compiler and by explicit dynamic registration. */
  def fromModel(metadata: AgentMetadata): WireAgentMetadata = {
    val graphs = metadata.constructor.input.parameters.map(_.graph) ++ metadata.methods.flatMap { method =>
      method.input.parameters.map(_.graph) ++ (method.output match {
        case OutputMetadata.Unit          => Nil
        case OutputMetadata.Single(graph) => List(graph)
      })
    } ++ metadata.config.map(_.valueType)
    val encoder                                                       = new GraphEncoder(SchemaBuilder.mergeGraphDefs(graphs))
    def parameters(input: InputMetadata): List[WireParameterMetadata] = input.parameters.map { p =>
      WireParameterMetadata(p.name, p.source, encoder.encodeType(p.graph.root), p.metadata)
    }
    val constructor = WireConstructorMetadata(
      metadata.constructor.name,
      metadata.constructor.description,
      metadata.constructor.promptHint,
      parameters(metadata.constructor.input)
    )
    val methods = metadata.methods.map { m =>
      val input  = parameters(m.input)
      val output = m.output match {
        case OutputMetadata.Unit          => None
        case OutputMetadata.Single(graph) => Some(encoder.encodeType(graph.root))
      }
      WireMethodMetadata(m.name, m.description, m.prompt, m.mode, input, output, m.httpEndpoints, m.readOnly)
    }
    val config = metadata.config.map(c => WireConfigDeclaration(c.source, c.path, encoder.encodeType(c.valueType.root)))
    WireAgentMetadata(
      metadata.name,
      metadata.kind,
      metadata.description,
      metadata.mode,
      methods,
      constructor,
      metadata.httpMount,
      config,
      metadata.snapshotting,
      encoder.finish()
    )
  }
}
