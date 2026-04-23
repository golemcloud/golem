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

package golem.host

import golem.HostApi
import golem.host.js._
import golem.runtime.rpc.host.AgentHostApi

import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport

/**
 * Scala.js facade for `golem:api/oplog@1.5.0`.
 *
 * Provides typed access to the oplog via `GetOplog` and `SearchOplog`
 * resources. Each `OplogEntry` variant is a full Scala sealed trait case,
 * matching the WIT definition.
 */
object OplogApi {

  type OplogIndex = BigInt

  // ---------------------------------------------------------------------------
  // Supporting record types
  // ---------------------------------------------------------------------------

  final case class PluginInstallationDescription(
    name: String,
    version: String,
    parameters: Map[String, String]
  )

  final case class CreateParameters(
    timestamp: ContextApi.DateTime,
    agentId: AgentHostApi.AgentIdLiteral,
    componentRevision: BigInt,
    env: Map[String, String],
    createdBy: String,
    environmentId: String,
    parent: Option[AgentHostApi.AgentIdLiteral],
    componentSize: BigInt,
    initialTotalLinearMemorySize: BigInt,
    initialActivePlugins: List[PluginInstallationDescription]
  )

  final case class HostCallParameters(
    timestamp: ContextApi.DateTime,
    functionName: String,
    request: WitValueTypes.ValueAndType,
    response: WitValueTypes.ValueAndType,
    wrappedFunctionType: DurabilityApi.DurableFunctionType
  )

  final case class LocalSpanData(
    spanId: String,
    start: ContextApi.DateTime,
    parent: Option[String],
    linkedContext: Option[BigInt],
    attributes: List[ContextApi.Attribute],
    inherited: Boolean
  )

  final case class ExternalSpanData(spanId: String)

  sealed trait SpanData extends Product with Serializable
  object SpanData {
    final case class LocalSpan(data: LocalSpanData)       extends SpanData
    final case class ExternalSpan(data: ExternalSpanData) extends SpanData
  }

  final case class TypedDataValue(value: String, schema: String)

  object TypedDataValue {
    def fromJs(raw: JsTypedDataValue): TypedDataValue =
      TypedDataValue(
        value = js.JSON.stringify(raw.value.asInstanceOf[js.Any]),
        schema = js.JSON.stringify(raw.schema.asInstanceOf[js.Any])
      )
  }

  final case class AgentInvocationStartedParameters(
    timestamp: ContextApi.DateTime,
    functionName: String,
    request: List[TypedDataValue],
    idempotencyKey: String,
    traceId: String,
    traceStates: List[String],
    invocationContext: List[List[SpanData]]
  )

  final case class AgentInvocationFinishedParameters(
    timestamp: ContextApi.DateTime,
    response: Option[TypedDataValue],
    consumedFuel: Long
  )

  final case class ErrorParameters(
    timestamp: ContextApi.DateTime,
    error: String,
    retryFrom: OplogIndex
  )

  final case class OplogRegion(start: OplogIndex, end: OplogIndex)

  final case class JumpParameters(
    timestamp: ContextApi.DateTime,
    jump: OplogRegion
  )

  final case class SetRetryPolicyParameters(
    timestamp: ContextApi.DateTime,
    name: String,
    priority: Int,
    predicateJson: String,
    policyJson: String
  )

  final case class RemoveRetryPolicyParameters(
    timestamp: ContextApi.DateTime,
    name: String
  )

  final case class FilesystemStorageUsageUpdateParameters(
    timestamp: ContextApi.DateTime,
    delta: BigInt
  )

  final case class EndAtomicRegionParameters(
    timestamp: ContextApi.DateTime,
    beginIndex: OplogIndex
  )

  final case class EndRemoteWriteParameters(
    timestamp: ContextApi.DateTime,
    beginIndex: OplogIndex
  )

  final case class AgentMethodInvocationParameters(
    idempotencyKey: String,
    functionName: String,
    input: Option[List[TypedDataValue]],
    traceId: String,
    traceStates: List[String],
    invocationContext: List[List[SpanData]]
  )

  sealed trait AgentInvocation extends Product with Serializable
  object AgentInvocation {
    final case class ExportedFunction(params: AgentMethodInvocationParameters) extends AgentInvocation
    final case class AgentInitialization(idempotencyKey: String)               extends AgentInvocation
    case object SaveSnapshot                                                   extends AgentInvocation
    case object LoadSnapshot                                                   extends AgentInvocation
    final case class ProcessOplogEntries(idempotencyKey: String)               extends AgentInvocation
    final case class ManualUpdate(componentRevision: BigInt)                   extends AgentInvocation
  }

  final case class PendingAgentInvocationParameters(
    timestamp: ContextApi.DateTime,
    invocation: AgentInvocation
  )

  sealed trait UpdateDescription extends Product with Serializable
  object UpdateDescription {
    case object AutoUpdate                            extends UpdateDescription
    final case class SnapshotBased(data: Array[Byte]) extends UpdateDescription
  }

  final case class PendingUpdateParameters(
    timestamp: ContextApi.DateTime,
    targetRevision: BigInt,
    updateDescription: UpdateDescription
  )

  final case class SuccessfulUpdateParameters(
    timestamp: ContextApi.DateTime,
    targetRevision: BigInt,
    newComponentSize: BigInt,
    newActivePlugins: List[PluginInstallationDescription]
  )

  final case class FailedUpdateParameters(
    timestamp: ContextApi.DateTime,
    targetRevision: BigInt,
    details: Option[String]
  )

  final case class GrowMemoryParameters(
    timestamp: ContextApi.DateTime,
    delta: BigInt
  )

  final case class CreateResourceParameters(
    timestamp: ContextApi.DateTime,
    resourceId: BigInt,
    name: String,
    owner: String
  )

  final case class DropResourceParameters(
    timestamp: ContextApi.DateTime,
    resourceId: BigInt,
    name: String,
    owner: String
  )

  sealed trait LogLevel extends Product with Serializable
  object LogLevel {
    case object Stdout   extends LogLevel
    case object Stderr   extends LogLevel
    case object Trace    extends LogLevel
    case object Debug    extends LogLevel
    case object Info     extends LogLevel
    case object Warn     extends LogLevel
    case object Error    extends LogLevel
    case object Critical extends LogLevel

    def fromString(s: String): LogLevel = s match {
      case "stdout"   => Stdout
      case "stderr"   => Stderr
      case "trace"    => Trace
      case "debug"    => Debug
      case "info"     => Info
      case "warn"     => Warn
      case "error"    => Error
      case "critical" => Critical
      case _          => Info
    }
  }

  final case class LogParameters(
    timestamp: ContextApi.DateTime,
    level: LogLevel,
    context: String,
    message: String
  )

  final case class ActivatePluginParameters(
    timestamp: ContextApi.DateTime,
    plugin: PluginInstallationDescription
  )

  final case class DeactivatePluginParameters(
    timestamp: ContextApi.DateTime,
    plugin: PluginInstallationDescription
  )

  final case class RevertParameters(
    timestamp: ContextApi.DateTime,
    start: OplogIndex,
    end: OplogIndex
  )

  final case class CancelPendingInvocationParameters(
    timestamp: ContextApi.DateTime,
    idempotencyKey: String
  )

  final case class StartSpanParameters(
    timestamp: ContextApi.DateTime,
    spanId: String,
    parent: Option[String],
    linkedContext: Option[String],
    attributes: List[ContextApi.Attribute]
  )

  final case class FinishSpanParameters(
    timestamp: ContextApi.DateTime,
    spanId: String
  )

  final case class SetSpanAttributeParameters(
    timestamp: ContextApi.DateTime,
    spanId: String,
    key: String,
    value: ContextApi.AttributeValue
  )

  final case class ChangePersistenceLevelParameters(
    timestamp: ContextApi.DateTime,
    persistenceLevel: HostApi.PersistenceLevel
  )

  final case class BeginRemoteTransactionParameters(
    timestamp: ContextApi.DateTime,
    transactionId: String
  )

  final case class RemoteTransactionParameters(
    timestamp: ContextApi.DateTime,
    beginIndex: OplogIndex
  )

  final case class OplogProcessorCheckpointParameters(
    timestamp: ContextApi.DateTime,
    plugin: PluginInstallationDescription,
    targetAgentId: AgentHostApi.AgentIdLiteral,
    confirmedUpTo: OplogIndex,
    sendingUpTo: OplogIndex,
    lastBatchStart: OplogIndex
  )

  // ---------------------------------------------------------------------------
  // OplogEntry sealed trait — 37+ variants
  // ---------------------------------------------------------------------------

  sealed trait OplogEntry extends Product with Serializable {
    def timestamp: ContextApi.DateTime
  }

  object OplogEntry {
    final case class Create(params: CreateParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class HostCall(params: HostCallParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class AgentInvocationStarted(params: AgentInvocationStartedParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class AgentInvocationFinished(params: AgentInvocationFinishedParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class Suspend(ts: ContextApi.DateTime) extends OplogEntry {
      def timestamp: ContextApi.DateTime = ts
    }
    final case class Error(params: ErrorParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class NoOp(ts: ContextApi.DateTime) extends OplogEntry {
      def timestamp: ContextApi.DateTime = ts
    }
    final case class Jump(params: JumpParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class Interrupted(ts: ContextApi.DateTime) extends OplogEntry {
      def timestamp: ContextApi.DateTime = ts
    }
    final case class Exited(ts: ContextApi.DateTime) extends OplogEntry {
      def timestamp: ContextApi.DateTime = ts
    }
    final case class SetRetryPolicy(params: SetRetryPolicyParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class RemoveRetryPolicy(params: RemoveRetryPolicyParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class FilesystemStorageUsageUpdate(params: FilesystemStorageUsageUpdateParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class BeginAtomicRegion(ts: ContextApi.DateTime) extends OplogEntry {
      def timestamp: ContextApi.DateTime = ts
    }
    final case class EndAtomicRegion(params: EndAtomicRegionParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class BeginRemoteWrite(ts: ContextApi.DateTime) extends OplogEntry {
      def timestamp: ContextApi.DateTime = ts
    }
    final case class EndRemoteWrite(params: EndRemoteWriteParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class PendingAgentInvocation(params: PendingAgentInvocationParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class PendingUpdate(params: PendingUpdateParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class SuccessfulUpdate(params: SuccessfulUpdateParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class FailedUpdate(params: FailedUpdateParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class GrowMemory(params: GrowMemoryParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class CreateResource(params: CreateResourceParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class DropResource(params: DropResourceParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class Log(params: LogParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class Restart(ts: ContextApi.DateTime) extends OplogEntry {
      def timestamp: ContextApi.DateTime = ts
    }
    final case class ActivatePlugin(params: ActivatePluginParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class DeactivatePlugin(params: DeactivatePluginParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class Revert(params: RevertParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class CancelPendingInvocation(params: CancelPendingInvocationParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class StartSpan(params: StartSpanParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class FinishSpan(params: FinishSpanParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class SetSpanAttribute(params: SetSpanAttributeParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class ChangePersistenceLevel(params: ChangePersistenceLevelParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class BeginRemoteTransaction(params: BeginRemoteTransactionParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class PreCommitRemoteTransaction(params: RemoteTransactionParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class PreRollbackRemoteTransaction(params: RemoteTransactionParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class CommittedRemoteTransaction(params: RemoteTransactionParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class RolledBackRemoteTransaction(params: RemoteTransactionParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }
    final case class Snapshot(ts: ContextApi.DateTime, data: Array[Byte], mimeType: String) extends OplogEntry {
      def timestamp: ContextApi.DateTime = ts
    }
    final case class OplogProcessorCheckpoint(params: OplogProcessorCheckpointParameters) extends OplogEntry {
      def timestamp: ContextApi.DateTime = params.timestamp
    }

    // --- Parsing ---

    def fromJs(raw: js.Any): OplogEntry = {
      val entry = raw.asInstanceOf[JsPublicOplogEntry]
      val tag   = entry.tag
      def v     = entry.asInstanceOf[JsPublicOplogEntryWithValue].value
      tag match {
        case "create"                                  => Create(parseCreateParameters(v.asInstanceOf[JsCreateParameters]))
        case "host-call" | "imported-function-invoked" =>
          HostCall(parseHostCallParameters(v.asInstanceOf[JsHostCallParameters]))
        case "agent-invocation-started" | "exported-function-invoked" =>
          AgentInvocationStarted(
            parseAgentInvocationStartedParameters(v.asInstanceOf[JsAgentInvocationStartedParameters])
          )
        case "agent-invocation-finished" | "exported-function-completed" =>
          AgentInvocationFinished(
            parseAgentInvocationFinishedParameters(v.asInstanceOf[JsAgentInvocationFinishedParameters])
          )
        case "suspend"          => Suspend(parseTimestamp(v.asInstanceOf[JsOplogTimestamp]))
        case "error"            => Error(parseErrorParameters(v.asInstanceOf[JsErrorParameters]))
        case "no-op"            => NoOp(parseTimestamp(v.asInstanceOf[JsOplogTimestamp]))
        case "jump"             => Jump(parseJumpParameters(v.asInstanceOf[JsJumpParameters]))
        case "interrupted"      => Interrupted(parseTimestamp(v.asInstanceOf[JsOplogTimestamp]))
        case "exited"           => Exited(parseTimestamp(v.asInstanceOf[JsOplogTimestamp]))
        case "set-retry-policy" =>
          SetRetryPolicy(parseSetRetryPolicyParameters(v.asInstanceOf[JsSetRetryPolicyParameters]))
        case "remove-retry-policy" =>
          RemoveRetryPolicy(parseRemoveRetryPolicyParameters(v.asInstanceOf[JsRemoveRetryPolicyParameters]))
        case "filesystem-storage-usage-update" =>
          FilesystemStorageUsageUpdate(
            parseFilesystemStorageUsageUpdateParameters(v.asInstanceOf[JsFilesystemStorageUsageUpdateParameters])
          )
        case "begin-atomic-region" => BeginAtomicRegion(parseTimestamp(v.asInstanceOf[JsOplogTimestamp]))
        case "end-atomic-region"   =>
          EndAtomicRegion(parseEndAtomicRegionParameters(v.asInstanceOf[JsEndAtomicRegionParameters]))
        case "begin-remote-write" => BeginRemoteWrite(parseTimestamp(v.asInstanceOf[JsOplogTimestamp]))
        case "end-remote-write"   =>
          EndRemoteWrite(parseEndRemoteWriteParameters(v.asInstanceOf[JsEndRemoteWriteParameters]))
        case "pending-agent-invocation" =>
          PendingAgentInvocation(
            parsePendingAgentInvocationParameters(v.asInstanceOf[JsPendingAgentInvocationParameters])
          )
        case "pending-update"    => PendingUpdate(parsePendingUpdateParameters(v.asInstanceOf[JsPendingUpdateParameters]))
        case "successful-update" =>
          SuccessfulUpdate(parseSuccessfulUpdateParameters(v.asInstanceOf[JsSuccessfulUpdateParameters]))
        case "failed-update"   => FailedUpdate(parseFailedUpdateParameters(v.asInstanceOf[JsFailedUpdateParameters]))
        case "grow-memory"     => GrowMemory(parseGrowMemoryParameters(v.asInstanceOf[JsGrowMemoryParameters]))
        case "create-resource" =>
          CreateResource(parseCreateResourceParameters(v.asInstanceOf[JsCreateResourceParameters]))
        case "drop-resource"   => DropResource(parseDropResourceParameters(v.asInstanceOf[JsDropResourceParameters]))
        case "log"             => Log(parseLogParameters(v.asInstanceOf[JsLogParameters]))
        case "restart"         => Restart(parseTimestamp(v.asInstanceOf[JsOplogTimestamp]))
        case "activate-plugin" =>
          ActivatePlugin(parseActivatePluginParameters(v.asInstanceOf[JsActivatePluginParameters]))
        case "deactivate-plugin" =>
          DeactivatePlugin(parseDeactivatePluginParameters(v.asInstanceOf[JsDeactivatePluginParameters]))
        case "revert"                                          => Revert(parseRevertParameters(v.asInstanceOf[JsRevertParameters]))
        case "cancel-pending-invocation" | "cancel-invocation" =>
          CancelPendingInvocation(
            parseCancelPendingInvocationParameters(v.asInstanceOf[JsCancelPendingInvocationParameters])
          )
        case "start-span"         => StartSpan(parseStartSpanParameters(v.asInstanceOf[JsStartSpanParameters]))
        case "finish-span"        => FinishSpan(parseFinishSpanParameters(v.asInstanceOf[JsFinishSpanParameters]))
        case "set-span-attribute" =>
          SetSpanAttribute(parseSetSpanAttributeParameters(v.asInstanceOf[JsSetSpanAttributeParameters]))
        case "change-persistence-level" =>
          ChangePersistenceLevel(
            parseChangePersistenceLevelParameters(v.asInstanceOf[JsChangePersistenceLevelParameters])
          )
        case "begin-remote-transaction" =>
          BeginRemoteTransaction(
            parseBeginRemoteTransactionParameters(v.asInstanceOf[JsBeginRemoteTransactionParameters])
          )
        case "pre-commit-remote-transaction" =>
          PreCommitRemoteTransaction(parseRemoteTransactionParameters(v.asInstanceOf[JsRemoteTransactionParameters]))
        case "pre-rollback-remote-transaction" =>
          PreRollbackRemoteTransaction(parseRemoteTransactionParameters(v.asInstanceOf[JsRemoteTransactionParameters]))
        case "committed-remote-transaction" =>
          CommittedRemoteTransaction(parseRemoteTransactionParameters(v.asInstanceOf[JsRemoteTransactionParameters]))
        case "rolled-back-remote-transaction" =>
          RolledBackRemoteTransaction(parseRemoteTransactionParameters(v.asInstanceOf[JsRemoteTransactionParameters]))
        case "snapshot" =>
          val sp = v.asInstanceOf[JsSnapshotParameters]
          Snapshot(
            ts = parseDateTime(sp.timestamp),
            data = new scala.scalajs.js.typedarray.Int8Array(sp.data.data.buffer).toArray,
            mimeType = sp.data.mimeType
          )
        case "oplog-processor-checkpoint" =>
          val cp = v.asInstanceOf[JsOplogProcessorCheckpointParameters]
          OplogProcessorCheckpoint(
            OplogProcessorCheckpointParameters(
              timestamp = parseDateTime(cp.timestamp),
              plugin = parsePluginInstallationDescription(cp.plugin),
              targetAgentId = parseAgentId(cp.targetAgentId),
              confirmedUpTo = BigInt(cp.confirmedUpTo.toString),
              sendingUpTo = BigInt(cp.sendingUpTo.toString),
              lastBatchStart = BigInt(cp.lastBatchStart.toString)
            )
          )
        case other =>
          throw new IllegalArgumentException(s"Unknown oplog entry tag: $other")
      }
    }
  }

  // ---------------------------------------------------------------------------
  // Parsing helpers
  // ---------------------------------------------------------------------------

  private def parseDateTime(raw: JsDatetime): ContextApi.DateTime = {
    val secs  = BigInt(raw.seconds.toString)
    val nanos = raw.nanoseconds.toInt
    ContextApi.DateTime(secs, nanos)
  }

  private def parseTimestamp(raw: JsOplogTimestamp): ContextApi.DateTime =
    parseDateTime(raw.timestamp)

  private def parsePluginInstallationDescription(raw: JsPluginInstallationDescription): PluginInstallationDescription =
    PluginInstallationDescription(
      name = raw.pluginName,
      version = raw.pluginVersion,
      parameters = raw.parameters.toSeq.map(t => t._1 -> t._2).toMap
    )

  private def parseAgentId(raw: JsAgentId): AgentHostApi.AgentIdLiteral =
    raw.asInstanceOf[AgentHostApi.AgentIdLiteral]

  private def parseCreateParameters(raw: JsCreateParameters): CreateParameters =
    CreateParameters(
      timestamp = parseDateTime(raw.timestamp),
      agentId = parseAgentId(raw.agentId),
      componentRevision = BigInt(raw.componentRevision.toString),
      env = raw.env.toSeq.map(t => t._1 -> t._2).toMap,
      createdBy = raw.createdBy.toString,
      environmentId = raw.environmentId.toString,
      parent = raw.parent.toOption.map(parseAgentId),
      componentSize = BigInt(raw.componentSize.toString),
      initialTotalLinearMemorySize = BigInt(raw.initialTotalLinearMemorySize.toString),
      initialActivePlugins = raw.initialActivePlugins.toList.map(parsePluginInstallationDescription)
    )

  private def parseHostCallParameters(raw: JsHostCallParameters): HostCallParameters =
    HostCallParameters(
      timestamp = parseDateTime(raw.timestamp),
      functionName = raw.functionName,
      request = WitValueTypes.ValueAndType.fromJs(raw.request),
      response = WitValueTypes.ValueAndType.fromJs(raw.response),
      wrappedFunctionType = DurabilityApi.DurableFunctionType.fromJs(raw.durableFunctionType)
    )

  private def parseSpanData(raw: JsSpanData): SpanData =
    raw.tag match {
      case "local-span" =>
        val v = raw.asInstanceOf[JsSpanDataLocalSpan].value
        SpanData.LocalSpan(
          LocalSpanData(
            spanId = v.spanId,
            start = parseDateTime(v.start),
            parent = v.parent.toOption,
            linkedContext = v.linkedContext.toOption.map(bi => BigInt(bi.toString)),
            attributes = v.attributes.toList.map { a =>
              ContextApi.Attribute(a.key, ContextApi.AttributeValue.fromJs(a.value))
            },
            inherited = v.inherited
          )
        )
      case "external-span" =>
        val v = raw.asInstanceOf[JsSpanDataExternalSpan].value
        SpanData.ExternalSpan(ExternalSpanData(v.spanId))
      case other =>
        throw new IllegalArgumentException(s"Unknown SpanData tag: $other")
    }

  private def parseSpanDataLists(raw: js.Array[js.Array[JsSpanData]]): List[List[SpanData]] =
    raw.toList.map(_.toList.map(parseSpanData))

  private def parseAgentInvocationStartedParameters(
    raw: JsAgentInvocationStartedParameters
  ): AgentInvocationStartedParameters = {
    val inv = raw.invocation
    inv.tag match {
      case "agent-method-invocation" | "exported-function" =>
        val p = inv.asInstanceOf[JsAgentInvocationWithValue].value.asInstanceOf[JsAgentMethodInvocationParameters]
        AgentInvocationStartedParameters(
          timestamp = parseDateTime(raw.timestamp),
          functionName = p.methodName,
          request = {
            val fi = p.functionInput.asInstanceOf[js.Any]
            if (js.isUndefined(fi) || fi == null) Nil
            else List(TypedDataValue.fromJs(fi.asInstanceOf[JsTypedDataValue]))
          },
          idempotencyKey = p.idempotencyKey,
          traceId = p.traceId,
          traceStates = p.traceStates.toList,
          invocationContext = parseSpanDataLists(p.invocationContext)
        )
      case "agent-initialization" =>
        val p = inv.asInstanceOf[JsAgentInvocationWithValue].value.asInstanceOf[JsAgentInitializationParameters]
        AgentInvocationStartedParameters(
          timestamp = parseDateTime(raw.timestamp),
          functionName = "<agent-initialization>",
          request = Nil,
          idempotencyKey = p.idempotencyKey,
          traceId = p.traceId,
          traceStates = p.traceStates.toList,
          invocationContext = parseSpanDataLists(p.invocationContext)
        )
      case "save-snapshot" =>
        AgentInvocationStartedParameters(
          timestamp = parseDateTime(raw.timestamp),
          functionName = "<save-snapshot>",
          request = Nil,
          idempotencyKey = "",
          traceId = "",
          traceStates = Nil,
          invocationContext = Nil
        )
      case "load-snapshot" =>
        AgentInvocationStartedParameters(
          timestamp = parseDateTime(raw.timestamp),
          functionName = "<load-snapshot>",
          request = Nil,
          idempotencyKey = "",
          traceId = "",
          traceStates = Nil,
          invocationContext = Nil
        )
      case "process-oplog-entries" =>
        val p = inv.asInstanceOf[JsAgentInvocationWithValue].value.asInstanceOf[JsProcessOplogEntriesParameters]
        AgentInvocationStartedParameters(
          timestamp = parseDateTime(raw.timestamp),
          functionName = "<process-oplog-entries>",
          request = Nil,
          idempotencyKey = p.idempotencyKey,
          traceId = "",
          traceStates = Nil,
          invocationContext = Nil
        )
      case other =>
        AgentInvocationStartedParameters(
          timestamp = parseDateTime(raw.timestamp),
          functionName = s"<$other>",
          request = Nil,
          idempotencyKey = "",
          traceId = "",
          traceStates = Nil,
          invocationContext = Nil
        )
    }
  }

  private def parseAgentInvocationFinishedParameters(
    raw: JsAgentInvocationFinishedParameters
  ): AgentInvocationFinishedParameters = {
    val result   = raw.result
    val response = result.tag match {
      case "agent-initialization" | "agent-method" =>
        val p =
          result.asInstanceOf[JsAgentInvocationResultWithValue].value.asInstanceOf[JsAgentInvocationOutputParameters]
        Some(TypedDataValue.fromJs(p.output))
      case _ => None
    }
    AgentInvocationFinishedParameters(
      timestamp = parseDateTime(raw.timestamp),
      response = response,
      consumedFuel = BigInt(raw.consumedFuel.toString).toLong
    )
  }

  private def parseErrorParameters(raw: JsErrorParameters): ErrorParameters =
    ErrorParameters(
      timestamp = parseDateTime(raw.timestamp),
      error = raw.error,
      retryFrom = BigInt(raw.retryFrom.toString)
    )

  private def parseJumpParameters(raw: JsJumpParameters): JumpParameters =
    JumpParameters(
      timestamp = parseDateTime(raw.timestamp),
      jump = OplogRegion(BigInt(raw.jump.start.toString), BigInt(raw.jump.end.toString))
    )

  private def parseSetRetryPolicyParameters(raw: JsSetRetryPolicyParameters): SetRetryPolicyParameters = {
    val p = raw.policy
    SetRetryPolicyParameters(
      timestamp = parseDateTime(raw.timestamp),
      name = p.name,
      priority = p.priority,
      predicateJson = js.JSON.stringify(p.predicate.asInstanceOf[js.Any]),
      policyJson = js.JSON.stringify(p.policy.asInstanceOf[js.Any])
    )
  }

  private def parseRemoveRetryPolicyParameters(raw: JsRemoveRetryPolicyParameters): RemoveRetryPolicyParameters =
    RemoveRetryPolicyParameters(
      timestamp = parseDateTime(raw.timestamp),
      name = raw.name
    )

  private def parseFilesystemStorageUsageUpdateParameters(
    raw: JsFilesystemStorageUsageUpdateParameters
  ): FilesystemStorageUsageUpdateParameters =
    FilesystemStorageUsageUpdateParameters(
      timestamp = parseDateTime(raw.timestamp),
      delta = BigInt(raw.delta.toString)
    )

  private def parseEndAtomicRegionParameters(raw: JsEndAtomicRegionParameters): EndAtomicRegionParameters =
    EndAtomicRegionParameters(
      timestamp = parseDateTime(raw.timestamp),
      beginIndex = BigInt(raw.beginIndex.toString)
    )

  private def parseEndRemoteWriteParameters(raw: JsEndRemoteWriteParameters): EndRemoteWriteParameters =
    EndRemoteWriteParameters(
      timestamp = parseDateTime(raw.timestamp),
      beginIndex = BigInt(raw.beginIndex.toString)
    )

  private def parsePendingAgentInvocationParameters(
    raw: JsPendingAgentInvocationParameters
  ): PendingAgentInvocationParameters = {
    val inv      = raw.invocation
    val agentInv = inv.tag match {
      case "agent-method-invocation" | "exported-function" =>
        val p = inv.asInstanceOf[JsAgentInvocationWithValue].value.asInstanceOf[JsAgentMethodInvocationParameters]
        AgentInvocation.ExportedFunction(
          AgentMethodInvocationParameters(
            idempotencyKey = p.idempotencyKey,
            functionName = p.methodName,
            input = Some(List(TypedDataValue.fromJs(p.functionInput))),
            traceId = p.traceId,
            traceStates = p.traceStates.toList,
            invocationContext = parseSpanDataLists(p.invocationContext)
          )
        )
      case "agent-initialization" =>
        val p = inv.asInstanceOf[JsAgentInvocationWithValue].value.asInstanceOf[JsAgentInitializationParameters]
        AgentInvocation.AgentInitialization(idempotencyKey = p.idempotencyKey)
      case "save-snapshot" =>
        AgentInvocation.SaveSnapshot
      case "load-snapshot" =>
        AgentInvocation.LoadSnapshot
      case "process-oplog-entries" =>
        val p = inv.asInstanceOf[JsAgentInvocationWithValue].value.asInstanceOf[JsProcessOplogEntriesParameters]
        AgentInvocation.ProcessOplogEntries(idempotencyKey = p.idempotencyKey)
      case "manual-update" =>
        val p = inv.asInstanceOf[JsAgentInvocationWithValue].value.asInstanceOf[JsManualUpdateParameters]
        AgentInvocation.ManualUpdate(BigInt(p.targetRevision.toString))
      case other =>
        throw new IllegalArgumentException(s"Unknown AgentInvocation tag: $other")
    }
    PendingAgentInvocationParameters(timestamp = parseDateTime(raw.timestamp), invocation = agentInv)
  }

  private def parsePendingUpdateParameters(raw: JsPendingUpdateParameters): PendingUpdateParameters = {
    val desc = raw.description
    val ud   = desc.tag match {
      case "auto-update"    => UpdateDescription.AutoUpdate
      case "snapshot-based" =>
        val snapshot = desc.asInstanceOf[JsUpdateDescriptionSnapshotBased].value
        UpdateDescription.SnapshotBased(new scala.scalajs.js.typedarray.Int8Array(snapshot.payload.buffer).toArray)
      case other =>
        throw new IllegalArgumentException(s"Unknown UpdateDescription tag: $other")
    }
    PendingUpdateParameters(
      timestamp = parseDateTime(raw.timestamp),
      targetRevision = BigInt(raw.targetRevision.toString),
      updateDescription = ud
    )
  }

  private def parseSuccessfulUpdateParameters(raw: JsSuccessfulUpdateParameters): SuccessfulUpdateParameters =
    SuccessfulUpdateParameters(
      timestamp = parseDateTime(raw.timestamp),
      targetRevision = BigInt(raw.targetRevision.toString),
      newComponentSize = BigInt(raw.newComponentSize.toString),
      newActivePlugins = raw.newActivePlugins.toList.map(parsePluginInstallationDescription)
    )

  private def parseFailedUpdateParameters(raw: JsFailedUpdateParameters): FailedUpdateParameters =
    FailedUpdateParameters(
      timestamp = parseDateTime(raw.timestamp),
      targetRevision = BigInt(raw.targetRevision.toString),
      details = raw.details.toOption
    )

  private def parseGrowMemoryParameters(raw: JsGrowMemoryParameters): GrowMemoryParameters =
    GrowMemoryParameters(
      timestamp = parseDateTime(raw.timestamp),
      delta = BigInt(raw.delta.toString)
    )

  private def parseCreateResourceParameters(raw: JsCreateResourceParameters): CreateResourceParameters =
    CreateResourceParameters(
      timestamp = parseDateTime(raw.timestamp),
      resourceId = BigInt(raw.id.toString),
      name = raw.name,
      owner = raw.owner
    )

  private def parseDropResourceParameters(raw: JsDropResourceParameters): DropResourceParameters =
    DropResourceParameters(
      timestamp = parseDateTime(raw.timestamp),
      resourceId = BigInt(raw.id.toString),
      name = raw.name,
      owner = raw.owner
    )

  private def parseLogParameters(raw: JsLogParameters): LogParameters =
    LogParameters(
      timestamp = parseDateTime(raw.timestamp),
      level = LogLevel.fromString(raw.level),
      context = raw.context,
      message = raw.message
    )

  private def parseActivatePluginParameters(raw: JsActivatePluginParameters): ActivatePluginParameters =
    ActivatePluginParameters(
      timestamp = parseDateTime(raw.timestamp),
      plugin = parsePluginInstallationDescription(raw.plugin)
    )

  private def parseDeactivatePluginParameters(raw: JsDeactivatePluginParameters): DeactivatePluginParameters =
    DeactivatePluginParameters(
      timestamp = parseDateTime(raw.timestamp),
      plugin = parsePluginInstallationDescription(raw.plugin)
    )

  private def parseRevertParameters(raw: JsRevertParameters): RevertParameters =
    RevertParameters(
      timestamp = parseDateTime(raw.timestamp),
      start = BigInt(raw.droppedRegion.start.toString),
      end = BigInt(raw.droppedRegion.end.toString)
    )

  private def parseCancelPendingInvocationParameters(
    raw: JsCancelPendingInvocationParameters
  ): CancelPendingInvocationParameters =
    CancelPendingInvocationParameters(
      timestamp = parseDateTime(raw.timestamp),
      idempotencyKey = raw.idempotencyKey
    )

  private def parseStartSpanParameters(raw: JsStartSpanParameters): StartSpanParameters =
    StartSpanParameters(
      timestamp = parseDateTime(raw.timestamp),
      spanId = raw.spanId,
      parent = raw.parent.toOption,
      linkedContext = raw.linkedContextId.toOption,
      attributes = raw.attributes.toList.map { a =>
        ContextApi.Attribute(a.key, ContextApi.AttributeValue.fromJs(a.value))
      }
    )

  private def parseFinishSpanParameters(raw: JsFinishSpanParameters): FinishSpanParameters =
    FinishSpanParameters(
      timestamp = parseDateTime(raw.timestamp),
      spanId = raw.spanId
    )

  private def parseSetSpanAttributeParameters(raw: JsSetSpanAttributeParameters): SetSpanAttributeParameters =
    SetSpanAttributeParameters(
      timestamp = parseDateTime(raw.timestamp),
      spanId = raw.spanId,
      key = raw.key,
      value = ContextApi.AttributeValue.fromJs(raw.value)
    )

  private def parseChangePersistenceLevelParameters(
    raw: JsChangePersistenceLevelParameters
  ): ChangePersistenceLevelParameters =
    ChangePersistenceLevelParameters(
      timestamp = parseDateTime(raw.timestamp),
      persistenceLevel = HostApi.PersistenceLevel.fromTag(raw.persistenceLevel.tag)
    )

  private def parseBeginRemoteTransactionParameters(
    raw: JsBeginRemoteTransactionParameters
  ): BeginRemoteTransactionParameters =
    BeginRemoteTransactionParameters(
      timestamp = parseDateTime(raw.timestamp),
      transactionId = raw.transactionId
    )

  private def parseRemoteTransactionParameters(raw: JsRemoteTransactionParameters): RemoteTransactionParameters =
    RemoteTransactionParameters(
      timestamp = parseDateTime(raw.timestamp),
      beginIndex = BigInt(raw.beginIndex.toString)
    )

  // ---------------------------------------------------------------------------
  // GetOplog resource
  // ---------------------------------------------------------------------------

  final class GetOplog private (private val handle: JsGetOplog) {

    def getNext(): Option[List[OplogEntry]] = {
      val batch = handle.getNext()
      if (js.isUndefined(batch) || batch == null) None
      else {
        val arr = batch.asInstanceOf[js.Array[js.Any]]
        Some(arr.toList.map(OplogEntry.fromJs))
      }
    }
  }

  object GetOplog {
    def apply(agentId: AgentHostApi.AgentIdLiteral, start: OplogIndex): GetOplog = {
      val ctor   = OplogModule.asInstanceOf[js.Dynamic].selectDynamic("GetOplog")
      val handle = js.Dynamic.newInstance(ctor)(agentId, js.BigInt(start.toString))
      new GetOplog(handle.asInstanceOf[JsGetOplog])
    }
  }

  // ---------------------------------------------------------------------------
  // SearchOplog resource
  // ---------------------------------------------------------------------------

  final class SearchOplog private (private val handle: JsSearchOplog) {

    def getNext(): Option[List[(OplogIndex, OplogEntry)]] = {
      val batch = handle.getNext()
      if (js.isUndefined(batch) || batch == null) None
      else {
        val arr = batch.asInstanceOf[js.Array[js.Tuple2[js.Any, js.Any]]]
        Some(arr.toList.map { t =>
          val idx   = BigInt(t._1.toString)
          val entry = OplogEntry.fromJs(t._2)
          (idx, entry)
        })
      }
    }
  }

  object SearchOplog {
    def apply(agentId: AgentHostApi.AgentIdLiteral, text: String): SearchOplog = {
      val ctor   = OplogModule.asInstanceOf[js.Dynamic].selectDynamic("SearchOplog")
      val handle = js.Dynamic.newInstance(ctor)(agentId, text)
      new SearchOplog(handle.asInstanceOf[JsSearchOplog])
    }
  }

  // ---------------------------------------------------------------------------
  // Native bindings
  // ---------------------------------------------------------------------------

  @js.native
  private sealed trait JsGetOplog extends js.Object {
    def getNext(): js.Any = js.native
  }

  @js.native
  private sealed trait JsSearchOplog extends js.Object {
    def getNext(): js.Any = js.native
  }

  @js.native
  @JSImport("golem:api/oplog@1.5.0", JSImport.Namespace)
  private object OplogModule extends js.Object

  def raw: Any = OplogModule
}
