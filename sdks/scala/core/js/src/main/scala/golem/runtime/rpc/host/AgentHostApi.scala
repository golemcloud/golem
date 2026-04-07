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

package golem.runtime.rpc.host

import golem.Uuid
import golem.host.js._

import scala.annotation.unused

import scala.scalajs.js
import scala.scalajs.js.BigInt
import scala.scalajs.js.JSConverters._
import scala.scalajs.js.annotation.{JSImport, JSName}
import scala.scalajs.js.typedarray.Uint8Array

object AgentHostApi {
  type OplogIndex       = BigInt
  type ComponentVersion = BigInt

  // --- Type aliases pointing to golem.host.js facades ---
  type AgentMetadata          = JsAgentMetadata
  type RetryPolicy            = JsRetryPolicy
  type PersistenceLevel       = JsPersistenceLevel
  type AgentStatus            = JsAgentStatus
  type UpdateMode             = JsUpdateMode
  type FilterComparator       = JsFilterComparator
  type StringFilterComparator = JsStringFilterComparator
  type AgentPropertyFilter    = JsAgentPropertyFilter
  type RevertAgentTarget      = JsRevertAgentTarget
  type RegisteredAgentType    = JsRegisteredAgentType
  type ComponentIdLiteral     = JsComponentId
  type AgentIdLiteral         = JsAgentId
  type UuidLiteral            = JsUuid
  type PromiseIdLiteral       = JsPromiseId
  type AgentNameFilter        = JsAgentNameFilter
  type AgentStatusFilter      = JsAgentStatusFilter
  type AgentVersionFilter     = JsAgentVersionFilter
  type AgentCreatedAtFilter   = JsAgentCreatedAtFilter
  type AgentEnvFilter         = JsAgentEnvFilter
  type AgentConfigVarsFilter  = JsAgentConfigVarsFilter
  type AgentAllFilter         = JsAgentAllFilter
  type AgentAnyFilter         = JsAgentAnyFilter

  @js.native
  @JSImport("golem:api/host@1.5.0", "GetAgents")
  class GetAgentsHandle(
    @unused componentId: JsComponentId,
    @unused filter: js.UndefOr[JsAgentAnyFilter],
    @unused precise: Boolean
  ) extends js.Object {
    def getNext(): js.UndefOr[js.Array[AgentMetadata]] = js.native
  }

  @js.native
  trait GetPromiseResultHandle extends js.Object {
    def subscribe(): Pollable = js.native

    /**
     * Returns `Uint8Array` if the promise is completed, or `undefined` if not
     * yet.
     */
    def get(): js.UndefOr[Uint8Array] = js.native
  }

  @js.native
  trait Pollable extends js.Object {
    def ready(): Boolean = js.native

    def block(): Unit = js.native

    /**
     * Converts this WASI pollable into a JS Promise that resolves when ready.
     */
    @JSName("promise")
    def promise(): js.Promise[Unit] = js.native
  }

  final case class AgentIdParts(agentTypeName: String, payload: JsDataValue, phantom: Option[Uuid])

  def registeredAgentType(typeName: String): Option[RegisteredAgentType] = {
    val v = AgentRegistryModule.getAgentType(typeName)
    if (v == null || js.isUndefined(v)) None else Some(v)
  }

  def getAllAgentTypes(): List[RegisteredAgentType] =
    AgentRegistryModule.getAllAgentTypes().toList

  def makeAgentId(agentTypeName: String, payload: JsDataValue, phantom: Option[Uuid]): Either[String, String] = {
    val phantomArg = phantom.fold[js.Any](js.undefined)(uuid => toUuidLiteral(uuid))
    try Right(AgentRegistryModule.makeAgentId(agentTypeName, payload, phantomArg))
    catch {
      case js.JavaScriptException(err) => Left(err.toString)
    }
  }

  def parseAgentId(agentId: String): Either[String, AgentIdParts] =
    try {
      val tuple                 = AgentRegistryModule.parseAgentId(agentId)
      val agentType             = tuple(0).asInstanceOf[String]
      val dataValue             = tuple(1).asInstanceOf[JsDataValue]
      val phantomValue          = tuple(2)
      val phantom: Option[Uuid] =
        if (phantomValue == null || js.isUndefined(phantomValue)) None
        else Some(fromUuidLiteral(phantomValue.asInstanceOf[UuidLiteral]))
      Right(AgentIdParts(agentType, dataValue, phantom))
    } catch {
      case js.JavaScriptException(err) => Left(err.toString)
    }

  def resolveComponentId(componentReference: String): Option[ComponentIdLiteral] =
    toOption(HostModule.resolveComponentId(componentReference))

  def resolveAgentId(componentReference: String, agentName: String): Option[AgentIdLiteral] =
    toOption(HostModule.resolveAgentId(componentReference, agentName))

  def resolveAgentIdStrict(componentReference: String, agentName: String): Option[AgentIdLiteral] =
    toOption(HostModule.resolveAgentIdStrict(componentReference, agentName))

  def getSelfMetadata(): AgentMetadata =
    HostModule.getSelfMetadata().asInstanceOf[AgentMetadata]

  def getAgentMetadata(agentId: AgentIdLiteral): Option[AgentMetadata] =
    toOption(HostModule.getAgentMetadata(agentId))

  private def toOption[A](value: js.Any): Option[A] =
    if (value == null || js.isUndefined(value)) None else Some(value.asInstanceOf[A])

  def getAgents(componentId: ComponentIdLiteral, filter: Option[AgentAnyFilter], precise: Boolean): GetAgentsHandle =
    new GetAgentsHandle(componentId, filter.orUndefined, precise)

  def nextAgentBatch(handle: GetAgentsHandle): Option[List[AgentMetadata]] =
    handle.getNext().toOption.map(_.toList)

  def createPromise(): PromiseIdLiteral =
    HostModule.createPromise().asInstanceOf[PromiseIdLiteral]

  def getPromise(promiseId: PromiseIdLiteral): GetPromiseResultHandle =
    HostModule.getPromise(promiseId).asInstanceOf[GetPromiseResultHandle]

  def completePromise(promiseId: PromiseIdLiteral, data: Uint8Array): Boolean =
    HostModule.completePromise(promiseId, data)

  def createWebhook(promiseId: PromiseIdLiteral): String =
    AgentRegistryModule.createWebhook(promiseId)

  def getConfigValue(key: List[String], expectedType: JsWitType): JsWitValue =
    AgentRegistryModule.getConfigValue(js.Array(key: _*), expectedType)

  def getOplogIndex(): OplogIndex =
    HostModule.getOplogIndex()

  def setOplogIndex(index: OplogIndex): Unit =
    HostModule.setOplogIndex(index)

  def markBeginOperation(): OplogIndex =
    HostModule.markBeginOperation()

  def markEndOperation(begin: OplogIndex): Unit =
    HostModule.markEndOperation(begin)

  def oplogCommit(replicas: Int): Unit =
    HostModule.oplogCommit(replicas)

  def getRetryPolicy(): RetryPolicy =
    HostModule.getRetryPolicy().asInstanceOf[RetryPolicy]

  def setRetryPolicy(policy: RetryPolicy): Unit =
    HostModule.setRetryPolicy(policy)

  def getOplogPersistenceLevel(): PersistenceLevel =
    HostModule.getOplogPersistenceLevel().asInstanceOf[PersistenceLevel]

  def setOplogPersistenceLevel(level: PersistenceLevel): Unit =
    HostModule.setOplogPersistenceLevel(level)

  def getIdempotenceMode(): Boolean =
    HostModule.getIdempotenceMode()

  def setIdempotenceMode(flag: Boolean): Unit =
    HostModule.setIdempotenceMode(flag)

  def generateIdempotencyKey(): UuidLiteral =
    HostModule.generateIdempotencyKey()

  def updateAgent(agentId: AgentIdLiteral, targetVersion: ComponentVersion, mode: UpdateMode): Unit =
    HostModule.updateAgent(agentId, targetVersion, mode)

  def forkAgent(sourceAgentId: AgentIdLiteral, targetAgentId: AgentIdLiteral, cutOff: OplogIndex): Unit =
    HostModule.forkAgent(sourceAgentId, targetAgentId, cutOff)

  def revertAgent(agentId: AgentIdLiteral, target: RevertAgentTarget): Unit =
    HostModule.revertAgent(agentId, target)

  def fork(): (String, UuidLiteral) = {
    val raw       = HostModule.fork().asInstanceOf[JsForkResult]
    val tag       = raw.tag
    val details   = raw.asInstanceOf[JsForkResultOriginal].value
    val phantomId = details.forkedPhantomId
    (tag, phantomId)
  }

  private def toUuidLiteral(uuid: Uuid): UuidLiteral =
    JsUuid(
      highBits = js.BigInt(uuid.highBits.toString),
      lowBits = js.BigInt(uuid.lowBits.toString)
    )

  private def fromUuidLiteral(uuid: UuidLiteral): Uuid =
    Uuid(
      highBits = _root_.scala.BigInt(uuid.highBits.toString),
      lowBits = _root_.scala.BigInt(uuid.lowBits.toString)
    )

  object RetryPolicy {
    def apply(
      maxAttempts: Int,
      minDelay: BigInt,
      maxDelay: BigInt,
      multiplier: Double,
      maxJitterFactor: js.UndefOr[Double] = js.undefined
    ): RetryPolicy =
      JsRetryPolicy(maxAttempts, minDelay, maxDelay, multiplier, maxJitterFactor)
  }

  object PersistenceLevel {
    def PersistNothing: PersistenceLevel =
      JsPersistenceLevel.persistNothing

    def PersistRemoteSideEffects: PersistenceLevel =
      JsPersistenceLevel.persistRemoteSideEffects

    def Smart: PersistenceLevel =
      JsPersistenceLevel.smart
  }

  object AgentStatus {
    def Running: AgentStatus = "running"

    def Idle: AgentStatus = "idle"

    def Suspended: AgentStatus = "suspended"

    def Interrupted: AgentStatus = "interrupted"

    def Retrying: AgentStatus = "retrying"

    def Failed: AgentStatus = "failed"

    def Exited: AgentStatus = "exited"
  }

  object UpdateMode {
    def Automatic: UpdateMode = "automatic"

    def SnapshotBased: UpdateMode = "snapshot-based"
  }

  object FilterComparator {
    def Equal: FilterComparator = "equal"

    def NotEqual: FilterComparator = "not-equal"

    def GreaterEqual: FilterComparator = "greater-equal"

    def Greater: FilterComparator = "greater"

    def LessEqual: FilterComparator = "less-equal"

    def Less: FilterComparator = "less"
  }

  object StringFilterComparator {
    def Equal: StringFilterComparator = "equal"

    def NotEqual: StringFilterComparator = "not-equal"

    def Like: StringFilterComparator = "like"

    def NotLike: StringFilterComparator = "not-like"

    def StartsWith: StringFilterComparator = "starts-with"
  }

  object AgentNameFilter {
    def apply(comparator: StringFilterComparator, value: String): AgentNameFilter =
      JsAgentNameFilter(comparator, value)
  }

  object AgentStatusFilter {
    def apply(comparator: FilterComparator, value: AgentStatus): AgentStatusFilter =
      JsAgentStatusFilter(comparator, value)
  }

  object AgentVersionFilter {
    def apply(comparator: FilterComparator, value: BigInt): AgentVersionFilter =
      JsAgentVersionFilter(comparator, value)
  }

  object AgentCreatedAtFilter {
    def apply(comparator: FilterComparator, value: BigInt): AgentCreatedAtFilter =
      JsAgentCreatedAtFilter(comparator, value)
  }

  object AgentEnvFilter {
    def apply(name: String, comparator: StringFilterComparator, value: String): AgentEnvFilter =
      JsAgentEnvFilter(name, comparator, value)
  }

  object AgentConfigVarsFilter {
    def apply(name: String, comparator: StringFilterComparator, value: String): AgentConfigVarsFilter =
      JsAgentConfigVarsFilter(name, comparator, value)
  }

  object AgentPropertyFilter {
    def name(filter: AgentNameFilter): AgentPropertyFilter =
      JsAgentPropertyFilter.name(filter)

    def status(filter: AgentStatusFilter): AgentPropertyFilter =
      JsAgentPropertyFilter.status(filter)

    def version(filter: AgentVersionFilter): AgentPropertyFilter =
      JsAgentPropertyFilter.version(filter)

    def createdAt(filter: AgentCreatedAtFilter): AgentPropertyFilter =
      JsAgentPropertyFilter.createdAt(filter)

    def env(filter: AgentEnvFilter): AgentPropertyFilter =
      JsAgentPropertyFilter.env(filter)

    def wasiConfigVars(filter: AgentConfigVarsFilter): AgentPropertyFilter =
      JsAgentPropertyFilter.wasiConfigVars(filter)
  }

  object AgentAllFilter {
    def apply(filters: List[AgentPropertyFilter]): AgentAllFilter =
      JsAgentAllFilter(filters.toJSArray)
  }

  object AgentAnyFilter {
    def apply(filters: List[AgentAllFilter]): AgentAnyFilter =
      JsAgentAnyFilter(filters.toJSArray)
  }

  object RevertAgentTarget {
    def RevertToOplogIndex(index: OplogIndex): RevertAgentTarget =
      JsRevertAgentTarget.revertToOplogIndex(index)

    def RevertLastInvocations(count: BigInt): RevertAgentTarget =
      JsRevertAgentTarget.revertLastInvocations(count)
  }

  object ComponentIdLiteral {
    def apply(uuid: UuidLiteral): ComponentIdLiteral =
      JsComponentId(uuid)
  }

  object AgentIdLiteral {
    def apply(componentId: ComponentIdLiteral, agentId: String): AgentIdLiteral =
      JsAgentId(componentId, agentId)
  }

  object PromiseIdLiteral {
    def apply(agentId: AgentIdLiteral, oplogIndex: OplogIndex): PromiseIdLiteral =
      JsPromiseId(agentId, oplogIndex)
  }

  object UuidLiteral {
    def apply(highBits: BigInt, lowBits: BigInt): UuidLiteral =
      JsUuid(highBits, lowBits)
  }

  @js.native
  @JSImport("golem:api/host@1.5.0", JSImport.Namespace)
  private object HostModule extends js.Object {
    def resolveComponentId(componentReference: String): js.Any = js.native

    def resolveAgentId(componentReference: String, agentName: String): js.Any = js.native

    def resolveAgentIdStrict(componentReference: String, agentName: String): js.Any = js.native

    def getSelfMetadata(): js.Any = js.native

    def getAgentMetadata(agentId: AgentIdLiteral): js.Any = js.native

    def createPromise(): js.Any = js.native

    def getPromise(promiseId: PromiseIdLiteral): js.Any = js.native

    def completePromise(promiseId: PromiseIdLiteral, data: Uint8Array): Boolean = js.native

    def getOplogIndex(): OplogIndex = js.native

    def setOplogIndex(index: OplogIndex): Unit = js.native

    def markBeginOperation(): OplogIndex = js.native

    def markEndOperation(begin: OplogIndex): Unit = js.native

    def oplogCommit(replicas: Int): Unit = js.native

    def getRetryPolicy(): js.Any = js.native

    def setRetryPolicy(policy: RetryPolicy): Unit = js.native

    def getOplogPersistenceLevel(): js.Any = js.native

    def setOplogPersistenceLevel(level: PersistenceLevel): Unit = js.native

    def getIdempotenceMode(): Boolean = js.native

    def setIdempotenceMode(flag: Boolean): Unit = js.native

    def generateIdempotencyKey(): UuidLiteral = js.native

    def updateAgent(agentId: AgentIdLiteral, targetVersion: ComponentVersion, mode: UpdateMode): Unit =
      js.native

    def forkAgent(sourceAgentId: AgentIdLiteral, targetAgentId: AgentIdLiteral, oplogIdxCutOff: OplogIndex): Unit =
      js.native

    def revertAgent(agentId: AgentIdLiteral, target: RevertAgentTarget): Unit = js.native

    def fork(): js.Any = js.native
  }

  @js.native
  @JSImport("golem:agent/host@1.5.0", JSImport.Namespace)
  private object AgentRegistryModule extends js.Object {
    def getAgentType(typeName: String): RegisteredAgentType = js.native

    def getAllAgentTypes(): js.Array[RegisteredAgentType] = js.native

    def makeAgentId(agentTypeName: String, input: JsDataValue, phantom: js.Any): String = js.native

    def parseAgentId(agentId: String): js.Array[js.Any] = js.native

    def createWebhook(promiseId: PromiseIdLiteral): String = js.native

    def getConfigValue(key: js.Array[String], expectedType: JsWitType): JsWitValue = js.native
  }
}
