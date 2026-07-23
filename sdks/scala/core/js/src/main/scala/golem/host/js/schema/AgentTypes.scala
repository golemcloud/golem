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

package golem.host.js.schema

import golem.host.js.{
  JsAgentConfigSource,
  JsAgentMode,
  JsComponentId,
  JsHttpEndpointDetails,
  JsHttpMountDetails,
  JsReadOnlyConfig,
  JsShape,
  JsSnapshotting
}

import scala.scalajs.js

// ---------------------------------------------------------------------------
// `golem:agent@2.0.0` common JS facades (the schema-native subset).
//
// Mirrors `golem_agent_2_0_0_common.d.ts`. The records/variants that touch the
// schema model live here; the HTTP / snapshotting / read-only facades are
// reused from the `golem.host.js` package.
//
// Schema-native shape:
//   - `agent-type` carries ONE merged `schema-graph`; constructor / method /
//     config schema roots are `type-node-index` (Int) into that graph.
//   - `agent-constructor` / `agent-method` use `input-schema` =
//     `parameters(list<named-field>)` with per-field `field-source`, and
//     `output-schema` = `unit | single(type-node-index)`.
//   - `agent-config-declaration.value-type` is a `type-node-index`.
//   - `agent-error.custom-error` carries a `typed-schema-value`.
// ---------------------------------------------------------------------------

/** `auto-injected-kind` is a plain string enum (`"principal"`). */
object JsAutoInjectedKind {
  val principal: String = "principal"
}

@js.native
sealed trait JsFieldSource extends js.Object {
  def tag: String = js.native
}
object JsFieldSource {
  def userSupplied: JsFieldSource               = JsShape.tagOnly[JsFieldSource]("user-supplied")
  def autoInjected(kind: String): JsFieldSource = JsShape.tagged[JsFieldSource]("auto-injected", kind)
  def autoInjectedPrincipal: JsFieldSource      = autoInjected(JsAutoInjectedKind.principal)
}

@js.native
sealed trait JsNamedField extends js.Object {
  def name: String                 = js.native
  def source: JsFieldSource        = js.native
  def schema: Int                  = js.native
  def metadata: JsMetadataEnvelope = js.native
}
object JsNamedField {
  def apply(name: String, source: JsFieldSource, schema: Int, metadata: JsMetadataEnvelope): JsNamedField =
    js.Dynamic
      .literal("name" -> name, "source" -> source, "schema" -> schema, "metadata" -> metadata)
      .asInstanceOf[JsNamedField]
}

@js.native
sealed trait JsInputSchema extends js.Object {
  def tag: String = js.native
}
object JsInputSchema {
  def parameters(fields: js.Array[JsNamedField]): JsInputSchema =
    JsShape.tagged[JsInputSchema]("parameters", fields)
}

@js.native
sealed trait JsOutputSchema extends js.Object {
  def tag: String = js.native
}
object JsOutputSchema {
  def unit: JsOutputSchema               = JsShape.tagOnly[JsOutputSchema]("unit")
  def single(index: Int): JsOutputSchema =
    JsShape.tagged[JsOutputSchema]("single", index.asInstanceOf[js.Any])
}

@js.native
sealed trait JsAgentConstructor extends js.Object {
  def name: js.UndefOr[String]       = js.native
  def description: String            = js.native
  def promptHint: js.UndefOr[String] = js.native
  def inputSchema: JsInputSchema     = js.native
}
object JsAgentConstructor {
  def apply(
    description: String,
    inputSchema: JsInputSchema,
    name: js.UndefOr[String] = js.undefined,
    promptHint: js.UndefOr[String] = js.undefined
  ): JsAgentConstructor = {
    val obj = js.Dynamic.literal("description" -> description, "inputSchema" -> inputSchema)
    name.foreach(n => obj.updateDynamic("name")(n))
    promptHint.foreach(p => obj.updateDynamic("promptHint")(p))
    obj.asInstanceOf[JsAgentConstructor]
  }
}

@js.native
sealed trait JsAgentMethod extends js.Object {
  def name: String                                  = js.native
  def description: String                           = js.native
  def httpEndpoint: js.Array[JsHttpEndpointDetails] = js.native
  def promptHint: js.UndefOr[String]                = js.native
  def inputSchema: JsInputSchema                    = js.native
  def outputSchema: JsOutputSchema                  = js.native
  def readOnly: js.UndefOr[JsReadOnlyConfig]        = js.native
}
object JsAgentMethod {
  def apply(
    name: String,
    description: String,
    httpEndpoint: js.Array[JsHttpEndpointDetails],
    inputSchema: JsInputSchema,
    outputSchema: JsOutputSchema,
    promptHint: js.UndefOr[String] = js.undefined,
    readOnly: js.UndefOr[JsReadOnlyConfig] = js.undefined
  ): JsAgentMethod = {
    val obj = js.Dynamic.literal(
      "name"         -> name,
      "description"  -> description,
      "httpEndpoint" -> httpEndpoint,
      "inputSchema"  -> inputSchema,
      "outputSchema" -> outputSchema
    )
    promptHint.foreach(p => obj.updateDynamic("promptHint")(p))
    readOnly.foreach(r => obj.updateDynamic("readOnly")(r))
    obj.asInstanceOf[JsAgentMethod]
  }
}

@js.native
sealed trait JsAgentConfigDeclaration extends js.Object {
  def source: JsAgentConfigSource = js.native
  def path: js.Array[String]      = js.native
  def valueType: Int              = js.native
}
object JsAgentConfigDeclaration {
  def apply(source: JsAgentConfigSource, path: js.Array[String], valueType: Int): JsAgentConfigDeclaration =
    js.Dynamic
      .literal("source" -> source, "path" -> path, "valueType" -> valueType)
      .asInstanceOf[JsAgentConfigDeclaration]
}

@js.native
sealed trait JsAgentDependency extends js.Object {
  def typeName: String                 = js.native
  def description: js.UndefOr[String]  = js.native
  def schema: JsSchemaGraph            = js.native
  def constructor: JsAgentConstructor  = js.native
  def methods: js.Array[JsAgentMethod] = js.native
}
object JsAgentDependency {
  def apply(
    typeName: String,
    schema: JsSchemaGraph,
    constructor: JsAgentConstructor,
    methods: js.Array[JsAgentMethod],
    description: js.UndefOr[String] = js.undefined
  ): JsAgentDependency = {
    val obj = js.Dynamic.literal(
      "typeName"    -> typeName,
      "schema"      -> schema,
      "constructor" -> constructor,
      "methods"     -> methods
    )
    description.foreach(d => obj.updateDynamic("description")(d))
    obj.asInstanceOf[JsAgentDependency]
  }
}

@js.native
sealed trait JsAgentType extends js.Object {
  def typeName: String                           = js.native
  def description: String                        = js.native
  def sourceLanguage: String                     = js.native
  def schema: JsSchemaGraph                      = js.native
  def constructor: JsAgentConstructor            = js.native
  def methods: js.Array[JsAgentMethod]           = js.native
  def dependencies: js.Array[JsAgentDependency]  = js.native
  def mode: JsAgentMode                          = js.native
  def httpMount: js.UndefOr[JsHttpMountDetails]  = js.native
  def snapshotting: JsSnapshotting               = js.native
  def config: js.Array[JsAgentConfigDeclaration] = js.native
}
object JsAgentType {
  def apply(
    typeName: String,
    description: String,
    sourceLanguage: String,
    schema: JsSchemaGraph,
    constructor: JsAgentConstructor,
    methods: js.Array[JsAgentMethod],
    dependencies: js.Array[JsAgentDependency],
    mode: JsAgentMode,
    snapshotting: JsSnapshotting,
    config: js.Array[JsAgentConfigDeclaration],
    httpMount: js.UndefOr[JsHttpMountDetails] = js.undefined
  ): JsAgentType = {
    val obj = js.Dynamic.literal(
      "typeName"       -> typeName,
      "description"    -> description,
      "sourceLanguage" -> sourceLanguage,
      "schema"         -> schema,
      "constructor"    -> constructor,
      "methods"        -> methods,
      "dependencies"   -> dependencies,
      "mode"           -> mode,
      "snapshotting"   -> snapshotting,
      "config"         -> config
    )
    httpMount.foreach(h => obj.updateDynamic("httpMount")(h))
    obj.asInstanceOf[JsAgentType]
  }
}

@js.native
sealed trait JsTypedAgentConfigValue extends js.Object {
  def path: js.Array[String]    = js.native
  def value: JsTypedSchemaValue = js.native
}
object JsTypedAgentConfigValue {
  def apply(path: js.Array[String], value: JsTypedSchemaValue): JsTypedAgentConfigValue =
    js.Dynamic.literal("path" -> path, "value" -> value).asInstanceOf[JsTypedAgentConfigValue]
}

/**
 * `golem:agent@2.0.0` `registered-agent-type` (the host registry entry). Its
 * `agentType` field is the schema-native [[JsAgentType]].
 */
@js.native
sealed trait JsRegisteredAgentType extends js.Object {
  def agentType: JsAgentType       = js.native
  def implementedBy: JsComponentId = js.native
}
object JsRegisteredAgentType {
  def apply(agentType: JsAgentType, implementedBy: JsComponentId): JsRegisteredAgentType =
    js.Dynamic
      .literal("agentType" -> agentType, "implementedBy" -> implementedBy)
      .asInstanceOf[JsRegisteredAgentType]
}

@js.native
sealed trait JsAgentError extends js.Object {
  def tag: String = js.native
}
object JsAgentError {
  def invalidInput(message: String): JsAgentError          = JsShape.tagged[JsAgentError]("invalid-input", message)
  def invalidMethod(message: String): JsAgentError         = JsShape.tagged[JsAgentError]("invalid-method", message)
  def invalidType(message: String): JsAgentError           = JsShape.tagged[JsAgentError]("invalid-type", message)
  def invalidAgentId(message: String): JsAgentError        = JsShape.tagged[JsAgentError]("invalid-agent-id", message)
  def customError(value: JsTypedSchemaValue): JsAgentError =
    JsShape.tagged[JsAgentError]("custom-error", value)
}
