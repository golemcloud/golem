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

package golem

import golem.host.js.{JsAgentId, JsEnvironmentId, JsUuid}
import golem.runtime.rpc.host.AgentHostApi
import zio.blocks.schema.Schema
import zio.blocks.schema.json.{JsonCodec, JsonCodecDeriver}
import java.util.UUID
import scala.concurrent.Future
import scala.scalajs.js
import scala.scalajs.js.Dictionary
import scala.scalajs.js.typedarray.Uint8Array

/**
 * Public Scala.js SDK access to Golem's runtime host API.
 *
 * Scala.js-only API (delegates to `golem:api/host@1.5.0`).
 */
object HostApi {

  // ----- Oplog persistence -----------------------------------------------------------------

  type OplogIndex = BigInt

  /**
   * Unconditionally traps the current invocation with the given reason.
   *
   * This call never returns: it surfaces as an uncatchable wasm trap on the
   * host side and fails the invocation. Handle recoverable failures in guest
   * code.
   */
  def trap(reason: String): Nothing = {
    AgentHostApi.trap(reason)
    // The host call must not return; if Scala.js reaches this point the host
    // implementation is broken. Throw a fatal error so the worker terminates
    // even in that pathological case.
    throw new Error(s"trap host call returned unexpectedly: $reason")
  }

  def oplogCommit(replicas: Int): Unit =
    AgentHostApi.oplogCommit(replicas)

  // ----- Agent management / registry ------------------------------------------------------

  type ComponentVersion   = AgentHostApi.ComponentVersion
  type ComponentIdLiteral = AgentHostApi.ComponentIdLiteral
  type AgentStatus        = AgentHostApi.AgentStatus

  /**
   * Metadata about an agent.
   *
   * @param agentTypeName
   *   The name of the agent's type. Owners without an agent type have none.
   * @param fingerprint
   *   The incarnation of the agent: a deleted and re-created agent keeps its ID
   *   but gets a new fingerprint
   */
  final case class AgentMetadata(
    agentId: AgentId,
    agentTypeName: Option[String],
    fingerprint: UUID,
    args: List[String],
    env: Map[String, String],
    config: Map[String, String],
    status: AgentStatus,
    componentRevision: BigInt,
    retryCount: BigInt,
    environmentId: JsEnvironmentId
  )
  type UpdateMode        = AgentHostApi.UpdateMode
  type RevertAgentTarget = AgentHostApi.RevertAgentTarget

  /**
   * An agent type deployed in the current environment, as reported by the Golem
   * host registry.
   *
   * @param typeName
   *   The type name (from `@agentDefinition`)
   */
  final case class RegisteredAgentType(typeName: String)
  type FilterComparator       = AgentHostApi.FilterComparator
  type StringFilterComparator = AgentHostApi.StringFilterComparator
  type AgentPropertyFilter    = AgentHostApi.AgentPropertyFilter
  type AgentKeyFilter         = AgentHostApi.AgentKeyFilter
  type AgentStatusFilter      = AgentHostApi.AgentStatusFilter
  type AgentVersionFilter     = AgentHostApi.AgentVersionFilter
  type AgentCreatedAtFilter   = AgentHostApi.AgentCreatedAtFilter
  type AgentEnvFilter         = AgentHostApi.AgentEnvFilter
  type AgentConfigFilter      = AgentHostApi.AgentConfigFilter
  type AgentAllFilter         = AgentHostApi.AgentAllFilter
  type AgentAnyFilter         = AgentHostApi.AgentAnyFilter

  /**
   * Which agent the code runs on after [[fork]]. Both sides get the ID of the
   * forked agent, which has the same type, a new ID and no key.
   */
  sealed trait ForkResult {
    def forkedAgentId: AgentId
  }

  object ForkResult {
    final case class Original(forkedAgentId: AgentId) extends ForkResult
    final case class Forked(forkedAgentId: AgentId)   extends ForkResult
  }
  type GetAgentsHandle        = AgentHostApi.GetAgentsHandle
  type GetPromiseResultHandle = AgentHostApi.GetPromiseResultHandle
  type UuidLiteral            = AgentHostApi.UuidLiteral
  val UuidLiteral: AgentHostApi.UuidLiteral.type               = AgentHostApi.UuidLiteral
  val ComponentIdLiteral: AgentHostApi.ComponentIdLiteral.type = AgentHostApi.ComponentIdLiteral

  val PromiseIdLiteral: AgentHostApi.PromiseIdLiteral.type = AgentHostApi.PromiseIdLiteral

  def registeredAgentType(typeName: String): Option[RegisteredAgentType] =
    AgentHostApi.registeredAgentType(typeName).map(fromHostRegisteredAgentType)

  def getAllAgentTypes(): List[RegisteredAgentType] =
    AgentHostApi.getAllAgentTypes().map(fromHostRegisteredAgentType)

  /**
   * The deployed agent type that the type tag of an agent ID names, or `None`
   * when no deployed agent type has that tag.
   */
  def registeredAgentTypeFor(agentId: AgentId): Option[RegisteredAgentType] =
    AgentHostApi.registeredAgentTypeFor(JsAgentId.fromAgentId(agentId)).map(fromHostRegisteredAgentType)

  def resolveComponentId(componentReference: String): Option[ComponentIdLiteral] =
    AgentHostApi.resolveComponentId(componentReference)

  def getSelfMetadata(): AgentMetadata =
    fromHostMetadata(AgentHostApi.getSelfMetadata())

  def getAgentMetadata(agentId: AgentId): Option[AgentMetadata] =
    AgentHostApi.getAgentMetadata(JsAgentId.fromAgentId(agentId)).map(fromHostMetadata)

  /** Enumerates the agents of `agentTypeName` in the current environment. */
  def getAgents(agentTypeName: String, filter: Option[AgentAnyFilter], precise: Boolean): GetAgentsHandle =
    AgentHostApi.getAgents(agentTypeName, filter, precise)

  def nextAgentBatch(handle: GetAgentsHandle): Option[List[AgentMetadata]] =
    AgentHostApi.nextAgentBatch(handle).map(_.map(fromHostMetadata))

  def generateIdempotencyKey(): UUID =
    JsUuid.toUuid(AgentHostApi.generateIdempotencyKey())

  def updateAgent(agentId: AgentId, targetVersion: BigInt, mode: UpdateMode): Unit =
    AgentHostApi.updateAgent(JsAgentId.fromAgentId(agentId), toJsBigInt(targetVersion), mode)

  def updateAgentRaw(agentId: AgentId, targetVersion: ComponentVersion, mode: UpdateMode): Unit =
    AgentHostApi.updateAgent(JsAgentId.fromAgentId(agentId), targetVersion, mode)

  /**
   * Forks an agent at a given oplog index into a new agent of the same type,
   * and returns the ID that Golem issued for the new agent. The new agent has
   * no key.
   */
  def forkAgent(sourceAgentId: AgentId, cutOff: OplogIndex): AgentId =
    JsAgentId.toAgentId(AgentHostApi.forkAgent(JsAgentId.fromAgentId(sourceAgentId), toJsBigInt(cutOff)))

  def revertAgent(agentId: AgentId, target: RevertAgentTarget): Unit =
    AgentHostApi.revertAgent(JsAgentId.fromAgentId(agentId), target)

  def fork(): ForkResult = {
    val (tag, forkedAgentId) = AgentHostApi.fork()
    val agentId              = JsAgentId.toAgentId(forkedAgentId)
    tag match {
      case "original" => ForkResult.Original(agentId)
      case "forked"   => ForkResult.Forked(agentId)
      case other      => throw new IllegalStateException(s"Unknown fork result tag: $other")
    }
  }

  object AgentStatus {
    val Running: AgentStatus     = AgentHostApi.AgentStatus.Running
    val Idle: AgentStatus        = AgentHostApi.AgentStatus.Idle
    val Suspended: AgentStatus   = AgentHostApi.AgentStatus.Suspended
    val Interrupted: AgentStatus = AgentHostApi.AgentStatus.Interrupted
    val Retrying: AgentStatus    = AgentHostApi.AgentStatus.Retrying
    val Failed: AgentStatus      = AgentHostApi.AgentStatus.Failed
    val Exited: AgentStatus      = AgentHostApi.AgentStatus.Exited
    val Sandboxed: AgentStatus   = AgentHostApi.AgentStatus.Sandboxed
  }

  object UpdateMode {
    val Automatic: UpdateMode     = AgentHostApi.UpdateMode.Automatic
    val SnapshotBased: UpdateMode = AgentHostApi.UpdateMode.SnapshotBased
  }

  object FilterComparator {
    val Equal: FilterComparator        = AgentHostApi.FilterComparator.Equal
    val NotEqual: FilterComparator     = AgentHostApi.FilterComparator.NotEqual
    val GreaterEqual: FilterComparator = AgentHostApi.FilterComparator.GreaterEqual
    val Greater: FilterComparator      = AgentHostApi.FilterComparator.Greater
    val LessEqual: FilterComparator    = AgentHostApi.FilterComparator.LessEqual
    val Less: FilterComparator         = AgentHostApi.FilterComparator.Less
  }

  object StringFilterComparator {
    val Equal: StringFilterComparator      = AgentHostApi.StringFilterComparator.Equal
    val NotEqual: StringFilterComparator   = AgentHostApi.StringFilterComparator.NotEqual
    val Like: StringFilterComparator       = AgentHostApi.StringFilterComparator.Like
    val NotLike: StringFilterComparator    = AgentHostApi.StringFilterComparator.NotLike
    val StartsWith: StringFilterComparator = AgentHostApi.StringFilterComparator.StartsWith
  }

  /**
   * A filter on the canonical key of keyed agents. An agent without a key never
   * matches it.
   */
  object AgentKeyFilter {
    def apply(comparator: StringFilterComparator, value: String): AgentKeyFilter =
      AgentHostApi.AgentKeyFilter(comparator, value)
  }

  object AgentStatusFilter {
    def apply(comparator: FilterComparator, value: AgentStatus): AgentStatusFilter =
      AgentHostApi.AgentStatusFilter(comparator, value)
  }

  object AgentVersionFilter {
    def apply(comparator: FilterComparator, value: BigInt): AgentVersionFilter =
      AgentHostApi.AgentVersionFilter(comparator, toJsBigInt(value))
  }

  object AgentCreatedAtFilter {
    def apply(comparator: FilterComparator, value: BigInt): AgentCreatedAtFilter =
      AgentHostApi.AgentCreatedAtFilter(comparator, toJsBigInt(value))
  }

  object AgentEnvFilter {
    def apply(name: String, comparator: StringFilterComparator, value: String): AgentEnvFilter =
      AgentHostApi.AgentEnvFilter(name, comparator, value)
  }

  object AgentConfigFilter {
    def apply(name: String, comparator: StringFilterComparator, value: String): AgentConfigFilter =
      AgentHostApi.AgentConfigFilter(name, comparator, value)
  }

  object AgentPropertyFilter {
    def key(filter: AgentKeyFilter): AgentPropertyFilter =
      AgentHostApi.AgentPropertyFilter.key(filter)
    def status(filter: AgentStatusFilter): AgentPropertyFilter =
      AgentHostApi.AgentPropertyFilter.status(filter)
    def version(filter: AgentVersionFilter): AgentPropertyFilter =
      AgentHostApi.AgentPropertyFilter.version(filter)
    def createdAt(filter: AgentCreatedAtFilter): AgentPropertyFilter =
      AgentHostApi.AgentPropertyFilter.createdAt(filter)
    def env(filter: AgentEnvFilter): AgentPropertyFilter =
      AgentHostApi.AgentPropertyFilter.env(filter)
    def config(filter: AgentConfigFilter): AgentPropertyFilter =
      AgentHostApi.AgentPropertyFilter.config(filter)
  }

  object AgentAllFilter {
    def apply(filters: List[AgentPropertyFilter]): AgentAllFilter =
      AgentHostApi.AgentAllFilter(filters)
  }

  object AgentAnyFilter {
    def apply(filters: List[AgentAllFilter]): AgentAnyFilter =
      AgentHostApi.AgentAnyFilter(filters)
  }

  object RevertAgentTarget {
    def RevertToOplogIndex(index: OplogIndex): RevertAgentTarget =
      AgentHostApi.RevertAgentTarget.RevertToOplogIndex(toJsBigInt(index))
    def RevertLastInvocations(count: BigInt): RevertAgentTarget =
      AgentHostApi.RevertAgentTarget.RevertLastInvocations(toJsBigInt(count))
  }

  // ----- Promises --------------------------------------------------------------------------

  type PromiseId = AgentHostApi.PromiseIdLiteral

  def createPromise(): PromiseId =
    AgentHostApi.createPromise()

  /** Completes a promise with a binary payload. */
  def completePromise(promiseId: PromiseId, data: Array[Byte]): Boolean =
    AgentHostApi.completePromise(promiseId, toUint8Array(data))

  /**
   * Low-level completion using `Uint8Array` (internal; prefer `Array[Byte]`).
   */
  private[golem] def completePromiseRaw(promiseId: PromiseId, data: Uint8Array): Boolean =
    AgentHostApi.completePromise(promiseId, data)

  /**
   * Awaits a promise completion and returns the payload bytes.
   */
  def awaitPromise(promiseId: PromiseId): Future[Array[Byte]] =
    awaitPromiseRaw(promiseId).map(fromUint8Array)(golem.concurrent.GolemExecutionContext.global)

  /** Low-level await using `Uint8Array` (internal; prefer `Array[Byte]`). */
  private[golem] def awaitPromiseRaw(promiseId: PromiseId): Future[Uint8Array] =
    golem.FutureInterop.fromPromise(AgentHostApi.getPromise(promiseId).get())

  /**
   * Await a promise and decode the payload as JSON using
   * `zio.blocks.schema.json`.
   *
   * By default, decoding is lenient (extra JSON fields are ignored). If you
   * want strict decoding, set `rejectExtraFields = true`.
   */
  def awaitPromiseJson[A](promiseId: PromiseId, rejectExtraFields: Boolean = false)(implicit
    schema: Schema[A]
  ): Future[A] =
    awaitPromise(promiseId).map { bytes =>
      val codec = jsonCodec[A](rejectExtraFields)
      codec.decode(bytes) match {
        case Right(value) => value
        case Left(err)    => throw new IllegalArgumentException(err.toString)
      }
    }(golem.concurrent.GolemExecutionContext.global)

  /**
   * Encode a value as JSON and complete the promise with the encoded bytes.
   *
   * Encoding uses `zio.blocks.schema.json` and is deterministic w.r.t. the
   * derived schema.
   */
  def completePromiseJson[A](
    promiseId: PromiseId,
    value: A
  )(implicit schema: Schema[A]): Boolean = {
    val codec = jsonCodec[A](rejectExtraFields = false)
    completePromise(promiseId, codec.encode(value))
  }

  // ----- Webhooks --------------------------------------------------------------------------

  /**
   * Creates a webhook that can be used to integrate with webhook-driven APIs.
   *
   * This creates a promise and registers a webhook URL for it. When an external
   * service POSTs to the returned URL, the underlying promise is completed with
   * the request body. The agent type must be deployed via an HTTP API for this
   * to work.
   *
   * Usage:
   * {{{
   *   val webhook = HostApi.createWebhook()
   *   // Send webhook.url to an external service
   *   val data = webhook.await().map(_.json[MyType]())
   * }}}
   */
  def createWebhook(): WebhookHandler = {
    val promiseId  = AgentHostApi.createPromise()
    val webhookUrl = AgentHostApi.createWebhook(promiseId)
    new WebhookHandler(webhookUrl, promiseId)
  }

  /**
   * A handle to a pending webhook. Provides the webhook URL and methods to
   * await the incoming POST payload.
   */
  final class WebhookHandler private[HostApi] (val url: String, private val promiseId: PromiseId) {

    /**
     * Awaits the webhook POST payload asynchronously.
     */
    def await(): Future[WebhookRequestPayload] =
      awaitPromiseRaw(promiseId).map(new WebhookRequestPayload(_))(
        golem.concurrent.GolemExecutionContext.global
      )
  }

  /**
   * The payload received from a webhook POST request.
   */
  final class WebhookRequestPayload private[HostApi] (private val payload: Uint8Array) {

    /** Returns the raw payload as a byte array. */
    def bytes: Array[Byte] = fromUint8Array(payload)

    /**
     * Decodes the payload as JSON using `zio.blocks.schema.json`.
     *
     * By default, decoding is lenient (extra JSON fields are ignored). Set
     * `rejectExtraFields = true` for strict decoding.
     */
    def json[A](rejectExtraFields: Boolean = false)(implicit schema: Schema[A]): A = {
      val codec = jsonCodec[A](rejectExtraFields)
      codec.decode(fromUint8Array(payload)) match {
        case Right(value) => value
        case Left(err)    => throw new IllegalArgumentException(s"Failed to decode webhook payload: $err")
      }
    }
  }

  // ----- Helpers ---------------------------------------------------------------------------

  private def toJsBigInt(value: BigInt): js.BigInt =
    js.BigInt(value.toString)

  private def fromJsBigInt(value: js.BigInt): BigInt =
    BigInt(value.toString)

  private def jsonCodec[A](rejectExtraFields: Boolean)(implicit schema: Schema[A]): JsonCodec[A] = {
    val deriver =
      if (rejectExtraFields) JsonCodecDeriver.withRejectExtraFields(true)
      else JsonCodecDeriver
    schema.derive(deriver)
  }

  private def toUint8Array(bytes: Array[Byte]): Uint8Array = {
    val array = new Uint8Array(bytes.length)
    var i     = 0
    while (i < bytes.length) {
      array(i) = (bytes(i) & 0xff).toShort
      i += 1
    }
    array
  }

  private def fromUint8Array(bytes: Uint8Array): Array[Byte] = {
    val out = new Array[Byte](bytes.length)
    var i   = 0
    while (i < bytes.length) {
      out(i) = bytes(i).toByte
      i += 1
    }
    out
  }

  private def fromHostRegisteredAgentType(raw: AgentHostApi.RegisteredAgentType): RegisteredAgentType =
    RegisteredAgentType(typeName = raw.agentType.typeName)

  private def fromHostMetadata(m: AgentHostApi.AgentMetadata): AgentMetadata = {
    val args    = if (m.args == null || js.isUndefined(m.args)) Nil else m.args.toList
    val env     = tuplesToMap(m.env)
    val config  = tuplesToMap(m.config)
    val compRev =
      if (js.isUndefined(m.componentRevision.asInstanceOf[js.Any])) BigInt(0) else fromJsBigInt(m.componentRevision)
    val retry = if (js.isUndefined(m.retryCount.asInstanceOf[js.Any])) BigInt(0) else fromJsBigInt(m.retryCount)
    AgentMetadata(
      agentId = JsAgentId.toAgentId(m.agentId),
      agentTypeName = m.agentTypeName.toOption,
      fingerprint = JsUuid.toUuid(m.fingerprint),
      args = args,
      env = env,
      config = config,
      status = m.status,
      componentRevision = compRev,
      retryCount = retry,
      environmentId = m.environmentId
    )
  }

  private def tuplesToMap(arr: js.Array[js.Tuple2[String, String]]): Map[String, String] =
    if (arr == null || js.isUndefined(arr)) Map.empty
    else arr.map(t => (t._1, t._2)).toMap

  @js.native
  private trait HasTag extends js.Object {
    def tag: js.UndefOr[String] = js.native
  }
}
