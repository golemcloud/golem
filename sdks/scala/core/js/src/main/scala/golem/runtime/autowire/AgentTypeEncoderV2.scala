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

import golem.host.SchemaWireInterop
import golem.host.js.{JsHttpEndpointDetails, JsHttpMountDetails, JsReadOnlyConfig, JsSnapshotting}
import golem.host.js.schema._
import golem.schema.{MetadataEnvelope, SchemaGraph}
import golem.schema.wire.GraphEncoder
import golem.schema.SchemaBuilder

import scala.collection.mutable
import scala.scalajs.js
import scala.scalajs.js.JSConverters._

/**
 * v2 (`golem:agent@2.0.0`) agent-type encoder.
 *
 * Builds ONE merged per-agent `schema-graph` from the self-contained graphs of
 * every constructor parameter, method parameter, method output, and config
 * value, and emits each schema root as a `type-node-index` into that single
 * graph (§4.22). Definitions are merged by stable `type-id`; conflicting
 * same-id bodies raise [[golem.schema.SchemaConflictError]].
 *
 * The encoder is a pure function of an [[AgentRequest]] (the schema-native
 * surface description). Wiring `AgentDefinition` -> `AgentRequest` is done by
 * the runtime/macro layer in the Slice 4d atomic flip; keeping the encoder
 * surface-driven makes the graph-merge logic independently testable.
 */
object AgentTypeEncoderV2 {

  // --- Surface IR (schema-native description of an agent) -------------------

  sealed trait FieldSource
  object FieldSource {
    case object UserSupplied          extends FieldSource
    case object AutoInjectedPrincipal extends FieldSource
  }

  /** A single named parameter with its own self-contained schema graph. */
  final case class Param(
    name: String,
    source: FieldSource,
    graph: SchemaGraph,
    metadata: MetadataEnvelope = MetadataEnvelope.empty
  )

  final case class Constructor(
    description: String,
    params: List[Param],
    name: Option[String] = None,
    promptHint: Option[String] = None
  )

  final case class Method(
    name: String,
    description: String,
    params: List[Param],
    /** `None` => `output-schema = unit`; `Some(graph)` => `single`. */
    output: Option[SchemaGraph],
    httpEndpoints: js.Array[JsHttpEndpointDetails] = new js.Array[JsHttpEndpointDetails](),
    promptHint: Option[String] = None,
    readOnly: js.UndefOr[JsReadOnlyConfig] = js.undefined
  )

  final case class ConfigDecl(source: String, path: List[String], graph: SchemaGraph)

  final case class AgentRequest(
    typeName: String,
    description: String,
    mode: String,
    constructor: Constructor,
    methods: List[Method],
    snapshotting: JsSnapshotting,
    config: List[ConfigDecl] = Nil,
    httpMount: js.UndefOr[JsHttpMountDetails] = js.undefined,
    sourceLanguage: String = "scala"
  )

  // --- Encoder --------------------------------------------------------------

  def encode(req: AgentRequest): JsAgentType = {
    // 1. Collect every self-contained graph that contributes to this agent, in a
    //    deterministic order (constructor params, then per-method params+output,
    //    then config values).
    val allGraphs = mutable.ListBuffer.empty[SchemaGraph]
    req.constructor.params.foreach(p => allGraphs += p.graph)
    req.methods.foreach { m =>
      m.params.foreach(p => allGraphs += p.graph)
      m.output.foreach(g => allGraphs += g)
    }
    req.config.foreach(c => allGraphs += c.graph)

    // 2. Merge all defs by stable id (conflict-checked) and build a single
    //    incremental encoder over the merged def pool.
    val mergedDefs = SchemaBuilder.mergeGraphDefs(allGraphs.toList)
    val encoder    = new GraphEncoder(mergedDefs)

    // 3. Encode each contributing root into the shared pool, recording its
    //    type-node-index.
    def rootIndex(g: SchemaGraph): Int = encoder.encodeType(g.root)

    val constructorJs = JsAgentConstructor(
      description = req.constructor.description,
      inputSchema = encodeParams(req.constructor.params, rootIndex),
      name = req.constructor.name.orUndefined,
      promptHint = req.constructor.promptHint.orUndefined
    )

    val methodsJs: js.Array[JsAgentMethod] =
      req.methods.map { m =>
        val output = m.output match {
          case None    => JsOutputSchema.unit
          case Some(g) => JsOutputSchema.single(rootIndex(g))
        }
        JsAgentMethod(
          name = m.name,
          description = m.description,
          httpEndpoint = m.httpEndpoints,
          inputSchema = encodeParams(m.params, rootIndex),
          outputSchema = output,
          promptHint = m.promptHint.orUndefined,
          readOnly = m.readOnly
        )
      }.toJSArray

    val configJs: js.Array[JsAgentConfigDeclaration] =
      req.config.map { c =>
        JsAgentConfigDeclaration(c.source, c.path.toJSArray, rootIndex(c.graph))
      }.toJSArray

    // 4. Finish the single graph (placeholder structural root) and convert to JS.
    val mergedGraph = SchemaWireInterop.graphToJs(encoder.finish())

    JsAgentType(
      typeName = req.typeName,
      description = req.description,
      sourceLanguage = req.sourceLanguage,
      schema = mergedGraph,
      constructor = constructorJs,
      methods = methodsJs,
      dependencies = new js.Array[JsAgentDependency](),
      mode = req.mode,
      snapshotting = req.snapshotting,
      config = configJs,
      httpMount = req.httpMount
    )
  }

  private def encodeParams(params: List[Param], rootIndex: SchemaGraph => Int): JsInputSchema = {
    val fields = params.map { p =>
      JsNamedField(
        name = p.name,
        source = encodeSource(p.source),
        schema = rootIndex(p.graph),
        metadata = SchemaWireInterop.metadataToJs(p.metadata)
      )
    }.toJSArray
    JsInputSchema.parameters(fields)
  }

  private def encodeSource(source: FieldSource): JsFieldSource =
    source match {
      case FieldSource.UserSupplied          => JsFieldSource.userSupplied
      case FieldSource.AutoInjectedPrincipal => JsFieldSource.autoInjectedPrincipal
    }
}
