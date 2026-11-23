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

/**
 * Body of a `POST /v1/runtime/envs/{env}/agent-types/{type}/agents` request.
 * With `keyed`, it creates the keyed agent of the parameters, which fails with
 * HTTP 409 when that agent exists; otherwise each create makes a new agent with
 * a new ID.
 */
final case class CreateAgentRequest(
  parameters: SchemaValue,
  config: List[AgentConfigEntry],
  env: Map[String, String],
  keyed: Boolean
)

/**
 * An agent and the incarnation of it that a create, upsert or lookup reached.
 * `agentFingerprint` identifies the incarnation.
 */
final case class AgentReference(agentId: AgentId, agentFingerprint: String)

/**
 * Body of a `POST /v1/runtime/envs/{env}/agent-types/{type}/invoke` request.
 * It names the agent by exactly one of `agentId` or `parameters`; with
 * `expectedFingerprint`, a missing agent or another incarnation fails with
 * HTTP 404. The idempotency key travels in the `Idempotency-Key` header.
 */
final case class AgentInvocationRequest(
  agentId: Option[AgentId],
  parameters: Option[SchemaValue],
  method: String,
  methodParameters: SchemaValue,
  mode: String,
  scheduleAt: Option[String],
  expectedFingerprint: Option[String]
) {
  require(agentId.isDefined != parameters.isDefined, "an invocation names its agent by exactly one of agentId or parameters")
}

/** Response of an invoke request. */
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
 *
 * A remote of `upsert` has the constructor `parameters` and no `target`; its
 * invocations name the keyed agent by them. A remote of `create` or `getById`
 * has a `target` and its invocations name the agent by its ID and fingerprint.
 */
final case class ResolvedAgent(
  configuration: Configuration,
  agentTypeName: String,
  parameters: Option[SchemaValue],
  target: Option[AgentReference]
) {
  private[runtime] val knownAgentId =
    new java.util.concurrent.atomic.AtomicReference[Option[AgentId]](target.map(_.agentId))

  /** The constructor parameters that name an upsert remote's keyed agent. */
  private[runtime] def keyedParameters: SchemaValue =
    parameters.getOrElse(
      throw BridgeException(s"Missing constructor parameters for keyed $agentTypeName agent")
    )
}

final case class InvocationResult[+A](agentId: AgentId, idempotencyKey: String, value: A)
final case class InvocationReceipt(agentId: AgentId, idempotencyKey: String)

/**
 * JSON (de)serialization of the bridge REST protocol. The wire shapes mirror
 * the server's OpenAPI `CreateAgentRequest` / `AgentKeyRequest` /
 * `AgentInvocationRequest` / `AgentReference` / `AgentInvocationResult`
 * (camelCase fields), the same contract the Rust and TypeScript bridges use.
 */
object BridgeProtocol {

  def encodeCreateAgentRequest(request: CreateAgentRequest): Json =
    Json.obj(createFields(request)(SchemaValueCodec.toJson, encodeConfig, encodeEnv, Json.bool))

  def encodeAgentInvocationRequest(request: AgentInvocationRequest): Json =
    Json.obj(invocationFields(request)(Json.string, SchemaValueCodec.toJson))

  def renderCreateAgentRequest(request: CreateAgentRequest): String =
    renderFields(createFields(request)(writeSchema, writeConfig, writeEnv, writeBool))

  def renderAgentInvocationRequest(request: AgentInvocationRequest): String =
    renderFields(invocationFields(request)(writeString, writeSchema))

  /** Body of an upsert or lookup request: the constructor parameters. */
  def renderAgentKeyRequest(parameters: SchemaValue): String =
    renderFields(Vector("parameters" -> writeSchema(parameters)))

  private def createFields[A](request: CreateAgentRequest)(
    schema: SchemaValue => A,
    config: List[AgentConfigEntry] => A,
    env: Map[String, String] => A,
    bool: Boolean => A
  ): Vector[(String, A)] = Vector(
    "parameters" -> schema(request.parameters),
    "config"     -> config(request.config),
    "env"        -> env(request.env),
    "keyed"      -> bool(request.keyed)
  )

  private def invocationFields[A](request: AgentInvocationRequest)(
    string: String => A,
    schema: SchemaValue => A
  ): Vector[(String, A)] = {
    var fields = Vector.empty[(String, A)]
    request.agentId.foreach(id => fields = fields :+ ("agentId" -> string(id.uuid)))
    request.parameters.foreach(p => fields = fields :+ ("parameters" -> schema(p)))
    fields = fields ++ Vector(
      "method"           -> string(request.method),
      "methodParameters" -> schema(request.methodParameters),
      "mode"             -> string(request.mode)
    )
    request.scheduleAt.foreach(at => fields = fields :+ ("scheduleAt" -> string(at)))
    request.expectedFingerprint.foreach(f => fields = fields :+ ("expectedFingerprint" -> string(f)))
    fields
  }

  private def writeString(value: String): java.lang.StringBuilder => Unit =
    output => { output.append(Json.string(value).render); () }

  private def writeBool(value: Boolean): java.lang.StringBuilder => Unit =
    output => { output.append(value); () }

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

  private def writeEnv(env: Map[String, String]): java.lang.StringBuilder => Unit = output => {
    output.append(encodeEnv(env).render)
    ()
  }

  private def encodeEnv(env: Map[String, String]): Json =
    Json.obj(env.toVector.sortBy(_._1).map { case (name, value) => name -> Json.string(value) })

  private def encodeConfig(entries: List[AgentConfigEntry]): Json =
    Json.arr(entries.map(encodeConfigEntry).toVector)

  private def encodeConfigEntry(entry: AgentConfigEntry): Json =
    Json.obj(
      "path"  -> Json.arr(entry.path.map(Json.string).toVector),
      "value" -> entry.value
    )

  def decodeAgentReference(json: Json): Either[String, AgentReference] =
    for {
      agentIdJson     <- Json.requireField(json, "agentId")
      agentId         <- AgentId.fromJson(agentIdJson)
      fingerprintJson <- Json.requireField(json, "agentFingerprint")
      fingerprint     <- Json.asString(fingerprintJson)
    } yield AgentReference(agentId, fingerprint)

  /**
   * The agent type name and the fingerprint in the server's `AgentMetadataDto`
   * of `GET /v1/runtime/envs/{env}/agents/{id}`.
   */
  def decodeAgentMetadata(json: Json): Either[String, (Option[String], String)] =
    for {
      typeName        <- Json.field(json, "agentTypeName") match {
                           case None        => Right(None)
                           case Some(value) => Json.asString(value).map(Some(_))
                         }
      fingerprintJson <- Json.requireField(json, "fingerprint")
      fingerprint     <- Json.asString(fingerprintJson)
    } yield (typeName, fingerprint)

  /** The account ID of the token of `GET /v1/me/token`. */
  def decodeTokenAccountId(json: Json): Either[String, String] =
    Json.requireField(json, "accountId").flatMap(Json.asString)

  /**
   * The environment ID of the page of `GET /v1/me/visible-environments` whose
   * entry is in `accountId` and has the application and environment names.
   */
  def decodeVisibleEnvironmentId(
    json: Json,
    accountId: String,
    appName: String,
    envName: String
  ): Either[String, Option[String]] = {
    def string(json: Json, path: String*): Either[String, String] =
      path.foldLeft[Either[String, Json]](Right(json))((at, name) => at.flatMap(Json.requireField(_, name))).flatMap(Json.asString)
    for {
      values <- Json.requireField(json, "values").flatMap(Json.asArray)
      ids    <- values.foldLeft[Either[String, Vector[String]]](Right(Vector.empty)) { (acc, entry) =>
                  for {
                    ids     <- acc
                    account <- string(entry, "account", "id")
                    app     <- string(entry, "application", "name")
                    env     <- string(entry, "environment", "name")
                    id      <- string(entry, "environment", "id")
                  } yield if (account == accountId && app == appName && env == envName) ids :+ id else ids
                }
    } yield ids.headOption
  }

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
