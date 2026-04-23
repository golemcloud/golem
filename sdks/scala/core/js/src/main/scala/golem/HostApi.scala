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

import golem.host.js.{JsAgentMetadataRuntime, JsComponentId, JsDataValue, JsEnvironmentId}
import golem.runtime.rpc.host.AgentHostApi
import golem.Uuid
import zio.blocks.schema.Schema
import zio.blocks.schema.json.{JsonCodec, JsonCodecDeriver}
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

  // ----- Core oplog / atomic region ---------------------------------------------------------

  type OplogIndex = BigInt

  def getOplogIndex(): OplogIndex =
    fromJsBigInt(AgentHostApi.getOplogIndex())

  def setOplogIndex(index: OplogIndex): Unit =
    AgentHostApi.setOplogIndex(toJsBigInt(index))

  def markBeginOperation(): OplogIndex =
    fromJsBigInt(AgentHostApi.markBeginOperation())

  def markEndOperation(begin: OplogIndex): Unit =
    AgentHostApi.markEndOperation(toJsBigInt(begin))

  def oplogCommit(replicas: Int): Unit =
    AgentHostApi.oplogCommit(replicas)

  // ----- Persistence level -----------------------------------------------------------------

  sealed trait PersistenceLevel extends Product with Serializable {
    def tag: String
  }

  object PersistenceLevel {
    case object PersistNothing           extends PersistenceLevel { override val tag: String = "persist-nothing" }
    case object PersistRemoteSideEffects extends PersistenceLevel {
      override val tag: String = "persist-remote-side-effects"
    }
    case object Smart extends PersistenceLevel { override val tag: String = "smart" }

    /** Forward-compatible wrapper for unknown host values. */
    final case class Unknown(tag: String) extends PersistenceLevel

    def fromTag(tag: String): PersistenceLevel =
      tag match {
        case "persist-nothing"             => PersistNothing
        case "persist-remote-side-effects" => PersistRemoteSideEffects
        case "smart"                       => Smart
        case other                         => Unknown(other)
      }
  }

  def getOplogPersistenceLevel(): PersistenceLevel =
    fromHostPersistenceLevel(AgentHostApi.getOplogPersistenceLevel())

  def setOplogPersistenceLevel(level: PersistenceLevel): Unit =
    AgentHostApi.setOplogPersistenceLevel(toHostPersistenceLevel(level))

  // ----- Idempotence -----------------------------------------------------------------------

  def getIdempotenceMode(): Boolean =
    AgentHostApi.getIdempotenceMode()

  def setIdempotenceMode(flag: Boolean): Unit =
    AgentHostApi.setIdempotenceMode(flag)

  // ----- Agent management / registry ------------------------------------------------------

  type ComponentVersion   = AgentHostApi.ComponentVersion
  type ComponentIdLiteral = AgentHostApi.ComponentIdLiteral
  type AgentIdLiteral     = AgentHostApi.AgentIdLiteral
  type AgentStatus        = AgentHostApi.AgentStatus

  final case class AgentMetadata(
    agentId: AgentIdLiteral,
    args: List[String],
    env: Map[String, String],
    config: Map[String, String],
    status: AgentStatus,
    componentRevision: BigInt,
    retryCount: BigInt,
    agentType: String,
    agentName: String,
    componentId: ComponentIdLiteral,
    environmentId: JsEnvironmentId
  )
  type UpdateMode        = AgentHostApi.UpdateMode
  type RevertAgentTarget = AgentHostApi.RevertAgentTarget

  /**
   * A registered agent type as reported by the Golem host registry.
   *
   * @param typeName
   *   The type name (from `@agentDefinition`)
   * @param implementedBy
   *   The component that implements this agent type
   */
  final case class RegisteredAgentType(typeName: String, implementedBy: ComponentIdLiteral)
  type FilterComparator       = AgentHostApi.FilterComparator
  type StringFilterComparator = AgentHostApi.StringFilterComparator
  type AgentPropertyFilter    = AgentHostApi.AgentPropertyFilter
  type AgentNameFilter        = AgentHostApi.AgentNameFilter
  type AgentStatusFilter      = AgentHostApi.AgentStatusFilter
  type AgentVersionFilter     = AgentHostApi.AgentVersionFilter
  type AgentCreatedAtFilter   = AgentHostApi.AgentCreatedAtFilter
  type AgentEnvFilter         = AgentHostApi.AgentEnvFilter
  type AgentConfigFilter      = AgentHostApi.AgentConfigFilter
  type AgentAllFilter         = AgentHostApi.AgentAllFilter
  type AgentAnyFilter         = AgentHostApi.AgentAnyFilter
  sealed trait ForkResult {
    def forkedPhantomId: Uuid
  }

  object ForkResult {
    final case class Original(forkedPhantomId: Uuid) extends ForkResult
    final case class Forked(forkedPhantomId: Uuid)   extends ForkResult
  }
  type GetAgentsHandle        = AgentHostApi.GetAgentsHandle
  type GetPromiseResultHandle = AgentHostApi.GetPromiseResultHandle
  type Pollable               = AgentHostApi.Pollable
  type UuidLiteral            = AgentHostApi.UuidLiteral
  val UuidLiteral: AgentHostApi.UuidLiteral.type               = AgentHostApi.UuidLiteral
  val ComponentIdLiteral: AgentHostApi.ComponentIdLiteral.type = AgentHostApi.ComponentIdLiteral

  /**
   * The parsed components of a Golem agent ID string.
   *
   * @param agentTypeName
   *   The agent type name
   * @param phantom
   *   Optional phantom UUID (for pre-provisioned instances)
   */
  final case class AgentIdParts(agentTypeName: String, phantom: Option[Uuid])
  val AgentIdLiteral: AgentHostApi.AgentIdLiteral.type     = AgentHostApi.AgentIdLiteral
  val PromiseIdLiteral: AgentHostApi.PromiseIdLiteral.type = AgentHostApi.PromiseIdLiteral

  def registeredAgentType(typeName: String): Option[RegisteredAgentType] =
    AgentHostApi.registeredAgentType(typeName).map(fromHostRegisteredAgentType)

  def getAllAgentTypes(): List[RegisteredAgentType] =
    AgentHostApi.getAllAgentTypes().map(fromHostRegisteredAgentType)

  private[golem] def makeAgentId(
    agentTypeName: String,
    payload: JsDataValue,
    phantom: Option[Uuid]
  ): Either[String, String] =
    AgentHostApi.makeAgentId(agentTypeName, payload, phantom)

  private[golem] def parseAgentIdRaw(agentId: String): Either[String, AgentHostApi.AgentIdParts] =
    AgentHostApi.parseAgentId(agentId)

  /**
   * Parses a Golem agent ID string into its components.
   *
   * @return
   *   Either an error or the parsed parts (type name, phantom UUID)
   */
  def parseAgentId(agentId: String): Either[String, AgentIdParts] =
    AgentHostApi.parseAgentId(agentId).map { raw =>
      AgentIdParts(agentTypeName = raw.agentTypeName, phantom = raw.phantom)
    }

  def resolveComponentId(componentReference: String): Option[ComponentIdLiteral] =
    AgentHostApi.resolveComponentId(componentReference)

  def resolveAgentId(componentReference: String, agentName: String): Option[AgentIdLiteral] =
    AgentHostApi.resolveAgentId(componentReference, agentName)

  def resolveAgentIdStrict(componentReference: String, agentName: String): Option[AgentIdLiteral] =
    AgentHostApi.resolveAgentIdStrict(componentReference, agentName)

  def getSelfMetadata(): AgentMetadata =
    fromHostMetadata(AgentHostApi.getSelfMetadata())

  def getAgentMetadata(agentId: AgentIdLiteral): Option[AgentMetadata] =
    AgentHostApi.getAgentMetadata(agentId).map(fromHostMetadata)

  def getAgents(componentId: ComponentIdLiteral, filter: Option[AgentAnyFilter], precise: Boolean): GetAgentsHandle =
    AgentHostApi.getAgents(componentId, filter, precise)

  def nextAgentBatch(handle: GetAgentsHandle): Option[List[AgentMetadata]] =
    AgentHostApi.nextAgentBatch(handle).map(_.map(fromHostMetadata))

  def generateIdempotencyKey(): Uuid =
    fromUuidLiteral(AgentHostApi.generateIdempotencyKey())

  def updateAgent(agentId: AgentIdLiteral, targetVersion: BigInt, mode: UpdateMode): Unit =
    AgentHostApi.updateAgent(agentId, toJsBigInt(targetVersion), mode)

  def updateAgentRaw(agentId: AgentIdLiteral, targetVersion: ComponentVersion, mode: UpdateMode): Unit =
    AgentHostApi.updateAgent(agentId, targetVersion, mode)

  def forkAgent(sourceAgentId: AgentIdLiteral, targetAgentId: AgentIdLiteral, cutOff: OplogIndex): Unit =
    AgentHostApi.forkAgent(sourceAgentId, targetAgentId, toJsBigInt(cutOff))

  def revertAgent(agentId: AgentIdLiteral, target: RevertAgentTarget): Unit =
    AgentHostApi.revertAgent(agentId, target)

  def fork(): ForkResult = {
    val (tag, phantomIdLit) = AgentHostApi.fork()
    val phantomId           = fromUuidLiteral(phantomIdLit)
    tag match {
      case "original" => ForkResult.Original(phantomId)
      case "forked"   => ForkResult.Forked(phantomId)
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

  object AgentNameFilter {
    def apply(comparator: StringFilterComparator, value: String): AgentNameFilter =
      AgentHostApi.AgentNameFilter(comparator, value)
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
    def name(filter: AgentNameFilter): AgentPropertyFilter =
      AgentHostApi.AgentPropertyFilter.name(filter)
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
   *
   * This is implemented in a non-blocking way (polling `pollable.ready()`), so
   * it can be safely composed with other async work using `Future`.
   *
   * If you want the explicit blocking behavior, use `awaitPromiseBlocking`.
   */
  def awaitPromise(promiseId: PromiseId): Future[Array[Byte]] =
    awaitPromiseRaw(promiseId).map(fromUint8Array)(scala.scalajs.concurrent.JSExecutionContext.Implicits.queue)

  /**
   * Blocks until a promise is completed, then returns the payload bytes.
   *
   * Under the hood this calls WIT `subscribe` / `pollable.block`.
   */
  def awaitPromiseBlocking(promiseId: PromiseId): Array[Byte] =
    fromUint8Array(awaitPromiseBlockingRaw(promiseId))

  /** Low-level await using `Uint8Array` (internal; prefer `Array[Byte]`). */
  private[golem] def awaitPromiseRaw(promiseId: PromiseId): Future[Uint8Array] = {
    val handle   = AgentHostApi.getPromise(promiseId)
    val pollable = handle.subscribe()
    golem.FutureInterop
      .fromPromise(pollable.promise())
      .map { _ =>
        handle.get().toOption match {
          case Some(bytes) => bytes
          case None        => throw new IllegalStateException("Promise completed but result is empty")
        }
      }(scala.scalajs.concurrent.JSExecutionContext.Implicits.queue)
  }

  /**
   * Low-level blocking await using `Uint8Array` (internal; prefer
   * `Array[Byte]`).
   *
   * Uses WASI `pollable.block()` for synchronous blocking. This should only be
   * used from synchronous (non-async) code paths. For async code, use
   * `awaitPromiseRaw` which uses `pollable.promise()`.
   */
  private[golem] def awaitPromiseBlockingRaw(promiseId: PromiseId): Uint8Array = {
    val handle   = AgentHostApi.getPromise(promiseId)
    val pollable = handle.subscribe()
    pollable.block()
    handle.get().toOption match {
      case Some(bytes) => bytes
      case None        => throw new IllegalStateException("Promise completed but result is empty")
    }
  }

  /**
   * Await a promise and decode the payload as JSON using
   * `zio.blocks.schema.json`.
   *
   * This is the '''non-blocking''' variant that polls via `setTimeout`. For
   * fork+join patterns (where the Golem runtime must know the worker is
   * suspended), use [[awaitPromiseBlockingJson]] instead.
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
    }(scala.scalajs.concurrent.JSExecutionContext.Implicits.queue)

  /**
   * Blocks until a promise is completed, then decodes the payload as JSON.
   *
   * Uses WASI `pollable.block()` under the hood, which properly signals to the
   * Golem runtime that this worker is suspended. This is required for fork+join
   * patterns where the forked worker must complete the promise before the
   * original worker can proceed.
   *
   * @see
   *   [[awaitPromiseJson]] for the non-blocking variant
   */
  def awaitPromiseBlockingJson[A](promiseId: PromiseId, rejectExtraFields: Boolean = false)(implicit
    schema: Schema[A]
  ): A = {
    val bytes = awaitPromiseBlocking(promiseId)
    val codec = jsonCodec[A](rejectExtraFields)
    codec.decode(bytes) match {
      case Right(value) => value
      case Left(err)    => throw new IllegalArgumentException(err.toString)
    }
  }

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
   *   val payload = webhook.awaitBlocking()
   *   val data = payload.json[MyType]
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
     *
     * This is non-blocking (polls via `setTimeout`). For fork+join patterns
     * where the Golem runtime must know the worker is suspended, use
     * [[awaitBlocking]] instead.
     */
    def await(): Future[WebhookRequestPayload] =
      awaitPromiseRaw(promiseId).map(new WebhookRequestPayload(_))(
        scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
      )

    /**
     * Blocks until the webhook POST payload arrives.
     *
     * Uses WASI `pollable.block()` under the hood. Use this from synchronous
     * code paths or fork+join patterns.
     */
    def awaitBlocking(): WebhookRequestPayload =
      new WebhookRequestPayload(awaitPromiseBlockingRaw(promiseId))
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

  private def fromUuidLiteral(uuid: UuidLiteral): Uuid =
    Uuid(
      highBits = BigInt(uuid.highBits.toString),
      lowBits = BigInt(uuid.lowBits.toString)
    )

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
    RegisteredAgentType(
      typeName = raw.agentType.typeName,
      implementedBy = raw.implementedBy
    )

  private def fromHostMetadata(m: AgentHostApi.AgentMetadata): AgentMetadata = {
    val rt         = m.asInstanceOf[JsAgentMetadataRuntime]
    val args       = if (m.args == null || js.isUndefined(m.args)) Nil else m.args.toList
    val env        = tuplesToMap(m.env)
    val config = tuplesToMap(m.config)
    val compRev    =
      if (js.isUndefined(m.componentRevision.asInstanceOf[js.Any])) BigInt(0) else fromJsBigInt(m.componentRevision)
    val retry                      = if (js.isUndefined(m.retryCount.asInstanceOf[js.Any])) BigInt(0) else fromJsBigInt(m.retryCount)
    val agentType                  = rt.agentType.getOrElse("")
    val agentName                  = rt.agentName.getOrElse("")
    val componentId: JsComponentId = rt.componentId.getOrElse(m.agentId.componentId)
    val envId                      = m.environmentId
    AgentMetadata(
      agentId = m.agentId,
      args = args,
      env = env,
      config = config,
      status = m.status,
      componentRevision = compRev,
      retryCount = retry,
      agentType = agentType,
      agentName = agentName,
      componentId = componentId,
      environmentId = envId
    )
  }

  private def tuplesToMap(arr: js.Array[js.Tuple2[String, String]]): Map[String, String] =
    if (arr == null || js.isUndefined(arr)) Map.empty
    else arr.map(t => (t._1, t._2)).toMap

  private def fromHostPersistenceLevel(level: AgentHostApi.PersistenceLevel): PersistenceLevel = {
    val tag = level.asInstanceOf[HasTag].tag.toOption.getOrElse(level.toString)
    PersistenceLevel.fromTag(tag)
  }

  private def toHostPersistenceLevel(level: PersistenceLevel): AgentHostApi.PersistenceLevel =
    Dictionary[js.Any]("tag" -> level.tag).asInstanceOf[AgentHostApi.PersistenceLevel]

  @js.native
  private trait HasTag extends js.Object {
    def tag: js.UndefOr[String] = js.native
  }
}
