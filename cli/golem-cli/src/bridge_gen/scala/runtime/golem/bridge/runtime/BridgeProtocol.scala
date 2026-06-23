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

/** A single agent configuration override entry of a create-agent request. */
final case class AgentConfigEntry(path: List[String], value: SchemaValue)

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
  methodName: String,
  methodParameters: SchemaValue,
  mode: String,
  scheduleAt: Option[String],
  idempotencyKey: Option[String]
)

/** Response of a `POST /v1/agents/invoke-agent` request. */
final case class AgentInvocationResult(
  agentId: AgentId,
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
  agentId: AgentId
)

/**
 * JSON (de)serialization of the bridge REST protocol. The wire shapes mirror
 * the server's OpenAPI `CreateAgentRequest` / `AgentInvocationRequest` /
 * `CreateAgentResponse` / `AgentInvocationResult` (camelCase fields), the same
 * contract the Rust and TypeScript bridges use.
 */
object BridgeProtocol {

  def encodeCreateAgentRequest(request: CreateAgentRequest): Json = {
    val base = Vector[(String, Json)](
      "appName"       -> Json.string(request.appName),
      "envName"       -> Json.string(request.envName),
      "agentTypeName" -> Json.string(request.agentTypeName),
      "parameters"    -> SchemaValueCodec.toJson(request.parameters)
    )
    val withPhantom = request.phantomId match {
      case Some(id) => base :+ ("phantomId" -> Json.string(id))
      case None     => base
    }
    val config = Json.arr(request.config.map(encodeConfigEntry).toVector)
    Json.obj(withPhantom :+ ("config" -> config))
  }

  def encodeAgentInvocationRequest(request: AgentInvocationRequest): Json = {
    var fields = Vector[(String, Json)](
      "appName"          -> Json.string(request.appName),
      "envName"          -> Json.string(request.envName),
      "agentTypeName"    -> Json.string(request.agentTypeName),
      "parameters"       -> SchemaValueCodec.toJson(request.parameters),
      "methodName"       -> Json.string(request.methodName),
      "methodParameters" -> SchemaValueCodec.toJson(request.methodParameters),
      "mode"             -> Json.string(request.mode)
    )
    request.phantomId.foreach(id => fields = fields :+ ("phantomId" -> Json.string(id)))
    request.scheduleAt.foreach(at => fields = fields :+ ("scheduleAt" -> Json.string(at)))
    request.idempotencyKey.foreach(k => fields = fields :+ ("idempotencyKey" -> Json.string(k)))
    Json.obj(fields)
  }

  private def encodeConfigEntry(entry: AgentConfigEntry): Json =
    Json.obj(
      "path"  -> Json.arr(entry.path.map(Json.string).toVector),
      "value" -> SchemaValueCodec.toJson(entry.value)
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
      result      <- decodeResultValue(json)
      revision    <- optionalBigInt(json, "componentRevision")
    } yield AgentInvocationResult(agentId, result, revision)

  /** Extract and decode the `result.value` `SchemaValue` of a `TypedSchemaValue`. */
  private def decodeResultValue(json: Json): Either[String, Option[SchemaValue]] =
    Json.field(json, "result") match {
      case None => Right(None)
      case Some(typed) =>
        Json.field(typed, "value") match {
          case None        => Right(None)
          case Some(value) => SchemaValueCodec.fromJson(value).map(Some(_))
        }
    }

  private def optionalBigInt(json: Json, name: String): Either[String, Option[BigInt]] =
    Json.field(json, name) match {
      case None => Right(None)
      case Some(field) =>
        Json.asNumberLiteral(field).flatMap { literal =>
          try Right(Some(BigDecimal(literal).toBigInt))
          catch { case _: NumberFormatException => Left(s"Invalid number for '$name': $literal") }
        }
    }
}
