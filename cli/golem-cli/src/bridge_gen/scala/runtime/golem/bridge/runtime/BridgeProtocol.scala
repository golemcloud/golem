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

package golem.bridge.runtime

import golem.bridge.runtime.json.Json

/**
 * A single application-JSON configuration override entry of a create-agent
 * request.
 */
final case class AgentConfigEntry(path: List[String], value: Json)

/** Body of a `POST /v1/agents/create-agent` request. */
final case class CreateAgentRequest(
  appName: String,
  envName: String,
  agentTypeName: String,
  parameters: SchemaValue,
  phantomId: Option[String],
  config: List[AgentConfigEntry]
)

/** Response of a `POST /v1/agents/create-agent` request. */
final case class CreateAgentResponse(agentId: AgentId, componentRevision: Option[BigInt])

/** Body of a `POST /v1/agents/invoke-agent` request. */
final case class AgentInvocationRequest(
  appName: String,
  envName: String,
  agentTypeName: String,
  parameters: SchemaValue,
  phantomId: Option[String],
  config: List[AgentConfigEntry],
  methodName: String,
  methodParameters: SchemaValue,
  mode: String,
  scheduleAt: Option[String],
  idempotencyKey: Option[String]
)

/** Response of a `POST /v1/agents/invoke-agent` request. */
final case class AgentInvocationResult(
  agentId: AgentId,
  idempotencyKey: String,
  result: Option[SchemaValue],
  componentRevision: Option[BigInt]
)

/**
 * A resolved agent: everything a generated `XRemote` needs to issue method
 * invocations. The [[Configuration]] is captured at construction time so a
 * remote keeps targeting the server it was created against, even if the global
 * configuration is later changed.
 */
final case class ResolvedAgent(
  configuration: Configuration,
  agentTypeName: String,
  parameters: SchemaValue,
  phantomId: Option[String],
  config: List[AgentConfigEntry],
  agentId: Option[AgentId]
)

final case class InvocationResult[+A](agentId: AgentId, idempotencyKey: String, value: A)
final case class InvocationReceipt(agentId: AgentId, idempotencyKey: String)

/**
 * JSON (de)serialization of the bridge REST protocol. The wire shapes mirror
 * the server's OpenAPI `CreateAgentRequest` / `AgentInvocationRequest` /
 * `CreateAgentResponse` / `AgentInvocationResult` (camelCase fields), the same
 * contract the Rust and TypeScript bridges use.
 */
object BridgeProtocol {

  def encodeCreateAgentRequest(request: CreateAgentRequest): Json =
    Json.obj(createFields(request)(Json.string, SchemaValueCodec.toJson, encodeConfig))

  def encodeAgentInvocationRequest(request: AgentInvocationRequest): Json =
    Json.obj(invocationFields(request)(Json.string, SchemaValueCodec.toJson, encodeConfig))

  def renderCreateAgentRequest(request: CreateAgentRequest): String =
    renderFields(createFields(request)(writeString, writeSchema, writeConfig))

  def renderAgentInvocationRequest(request: AgentInvocationRequest): String =
    renderFields(invocationFields(request)(writeString, writeSchema, writeConfig))

  private def createFields[A](request: CreateAgentRequest)(
    string: String => A,
    schema: SchemaValue => A,
    config: List[AgentConfigEntry] => A
  ): Vector[(String, A)] = {
    val base = Vector(
      "appName"       -> string(request.appName),
      "envName"       -> string(request.envName),
      "agentTypeName" -> string(request.agentTypeName),
      "parameters"    -> schema(request.parameters)
    )
    val withPhantom = request.phantomId match {
      case Some(id) => base :+ ("phantomId" -> string(id))
      case None     => base
    }
    withPhantom :+ ("config" -> config(request.config))
  }

  private def invocationFields[A](request: AgentInvocationRequest)(
    string: String => A,
    schema: SchemaValue => A,
    config: List[AgentConfigEntry] => A
  ): Vector[(String, A)] = {
    var fields = Vector(
      "appName"          -> string(request.appName),
      "envName"          -> string(request.envName),
      "agentTypeName"    -> string(request.agentTypeName),
      "parameters"       -> schema(request.parameters),
      "config"           -> config(request.config),
      "methodName"       -> string(request.methodName),
      "methodParameters" -> schema(request.methodParameters),
      "mode"             -> string(request.mode)
    )
    request.phantomId.foreach(id => fields = fields :+ ("phantomId" -> string(id)))
    request.scheduleAt.foreach(at => fields = fields :+ ("scheduleAt" -> string(at)))
    request.idempotencyKey.foreach(k => fields = fields :+ ("idempotencyKey" -> string(k)))
    fields
  }

  private def writeString(value: String): java.lang.StringBuilder => Unit =
    output => { output.append(Json.string(value).render); () }

  private def writeSchema(value: SchemaValue): java.lang.StringBuilder => Unit =
    output => SchemaValueCodec.writeJson(value, output)

  private def renderFields(fields: Vector[(String, java.lang.StringBuilder => Unit)]): String = {
    val output = new java.lang.StringBuilder("{")
    fields.zipWithIndex.foreach { case ((name, write), index) =>
      if (index != 0) output.append(',')
      writeString(name)(output)
      output.append(':')
      write(output)
    }
    output.append('}').toString
  }

  private def writeConfig(entries: List[AgentConfigEntry]): java.lang.StringBuilder => Unit = output => {
    output.append('[')
    entries.zipWithIndex.foreach { case (entry, index) =>
      if (index != 0) output.append(',')
      output.append("{\"path\":[")
      entry.path.zipWithIndex.foreach { case (part, index) =>
        if (index != 0) output.append(',')
        writeString(part)(output)
      }
      output.append("],\"value\":").append(entry.value.render).append('}')
    }
    output.append(']')
    ()
  }

  private def encodeConfig(entries: List[AgentConfigEntry]): Json =
    Json.arr(entries.map(encodeConfigEntry).toVector)

  private def encodeConfigEntry(entry: AgentConfigEntry): Json =
    Json.obj(
      "path"  -> Json.arr(entry.path.map(Json.string).toVector),
      "value" -> entry.value
    )

  def decodeCreateAgentResponse(json: Json): Either[String, CreateAgentResponse] =
    for {
      agentIdJson <- Json.requireField(json, "agentId")
      agentId     <- AgentId.fromJson(agentIdJson)
      revision    <- optionalBigInt(json, "componentRevision")
    } yield CreateAgentResponse(agentId, revision)

  def decodeAgentInvocationResult(json: Json): Either[String, AgentInvocationResult] =
    for {
      agentIdJson <- Json.requireField(json, "agentId")
      agentId     <- AgentId.fromJson(agentIdJson)
      keyJson     <- Json.requireField(json, "idempotencyKey")
      key         <- Json.asString(keyJson)
      result      <- decodeResultValue(json)
      revision    <- optionalBigInt(json, "componentRevision")
    } yield AgentInvocationResult(agentId, key, result, revision)

  /** Decode a present `TypedSchemaValue` against its returned schema graph. */
  private def decodeResultValue(json: Json): Either[String, Option[SchemaValue]] =
    Json.field(json, "result") match {
      case None        => Right(None)
      case Some(typed) =>
        for {
          _     <- Json.asObject(typed)
          graph <- Json.requireField(typed, "graph")
          value <- Json.requireField(typed, "value")
        } yield Some(PublicValueCodec.fromSchemaGraphJson(graph.render).decode(value))
    }

  private def optionalBigInt(json: Json, name: String): Either[String, Option[BigInt]] =
    Json.field(json, name) match {
      case None        => Right(None)
      case Some(field) =>
        Json.asNumberLiteral(field).flatMap { literal =>
          try Right(Some(BigDecimal(literal).toBigInt))
          catch { case _: NumberFormatException => Left(s"Invalid number for '$name': $literal") }
        }
    }
}
