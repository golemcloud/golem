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

package golem.host.js

import scala.scalajs.js
import scala.scalajs.js.annotation.JSName
import scala.scalajs.js.typedarray.Uint8Array

// ---------------------------------------------------------------------------
// golem:api/oplog@1.5.0  –  JS facade traits
// ---------------------------------------------------------------------------

// --- EnvironmentPluginGrantId ---

@js.native
sealed trait JsEnvironmentPluginGrantId extends js.Object {
  def uuid: JsUuid = js.native
}

// --- LocalAgentConfigEntry ---

@js.native
sealed trait JsLocalAgentConfigEntry extends js.Object {
  def path: js.Array[String] = js.native
  def value: JsValueAndType  = js.native
}

// --- PluginInstallationDescription ---

@js.native
sealed trait JsPluginInstallationDescription extends js.Object {
  def name: String                                    = js.native
  def version: String                                 = js.native
  def parameters: js.Array[js.Tuple2[String, String]] = js.native
}

// --- CreateParameters ---

@js.native
sealed trait JsCreateParameters extends js.Object {
  def timestamp: JsDatetime                                           = js.native
  def agentId: JsAgentId                                              = js.native
  def componentRevision: js.BigInt                                    = js.native
  def env: js.Array[js.Tuple2[String, String]]                        = js.native
  def createdBy: JsAccountId                                          = js.native
  def environmentId: JsEnvironmentId                                  = js.native
  def parent: js.UndefOr[JsAgentId]                                   = js.native
  def componentSize: js.BigInt                                        = js.native
  def initialTotalLinearMemorySize: js.BigInt                         = js.native
  def initialActivePlugins: js.Array[JsPluginInstallationDescription] = js.native
  def configVars: js.Array[js.Tuple2[String, String]]                 = js.native
  def localAgentConfig: js.Array[JsLocalAgentConfigEntry]             = js.native
}

// --- HostCallParameters ---

@js.native
sealed trait JsHostCallParameters extends js.Object {
  def timestamp: JsDatetime                      = js.native
  def functionName: String                       = js.native
  def request: JsValueAndType                    = js.native
  def response: JsValueAndType                   = js.native
  def wrappedFunctionType: JsWrappedFunctionType = js.native
}

// --- SpanData  –  tagged union ---

@js.native
sealed trait JsSpanData extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsSpanDataLocalSpan extends JsSpanData {
  @JSName("val") def value: JsLocalSpanData = js.native
}

@js.native
sealed trait JsSpanDataExternalSpan extends JsSpanData {
  @JSName("val") def value: JsExternalSpanData = js.native
}

object JsSpanData {
  def localSpan(data: JsLocalSpanData): JsSpanData =
    JsShape.tagged[JsSpanData]("local-span", data)

  def externalSpan(data: JsExternalSpanData): JsSpanData =
    JsShape.tagged[JsSpanData]("external-span", data)
}

// --- LocalSpanData ---

@js.native
sealed trait JsLocalSpanData extends js.Object {
  def spanId: String                       = js.native
  def start: JsDatetime                    = js.native
  def parent: js.UndefOr[String]           = js.native
  def linkedContext: js.UndefOr[js.BigInt] = js.native
  def attributes: js.Array[JsAttribute]    = js.native
  def inherited: Boolean                   = js.native
}

// --- ExternalSpanData ---

@js.native
sealed trait JsExternalSpanData extends js.Object {
  def spanId: String = js.native
}

// --- ErrorParameters ---

@js.native
sealed trait JsErrorParameters extends js.Object {
  def timestamp: JsDatetime = js.native
  def error: String         = js.native
  def retryFrom: js.BigInt  = js.native
}

// --- OplogRegion ---

@js.native
sealed trait JsOplogRegion extends js.Object {
  def start: js.BigInt = js.native
  def end: js.BigInt   = js.native
}

// --- JumpParameters ---

@js.native
sealed trait JsJumpParameters extends js.Object {
  def timestamp: JsDatetime = js.native
  def jump: JsOplogRegion   = js.native
}

// --- SetRetryPolicyParameters ---

@js.native
sealed trait JsSetRetryPolicyParameters extends js.Object {
  def timestamp: JsDatetime = js.native
  def name: String          = js.native
  def priority: Int         = js.native
  def predicateJson: String = js.native
  def policyJson: String    = js.native
}

// --- RemoveRetryPolicyParameters ---

@js.native
sealed trait JsRemoveRetryPolicyParameters extends js.Object {
  def timestamp: JsDatetime = js.native
  def name: String          = js.native
}

// --- FilesystemStorageUsageUpdateParameters ---

@js.native
sealed trait JsFilesystemStorageUsageUpdateParameters extends js.Object {
  def timestamp: JsDatetime = js.native
  def delta: js.BigInt      = js.native
}

// --- EndAtomicRegionParameters ---

@js.native
sealed trait JsEndAtomicRegionParameters extends js.Object {
  def timestamp: JsDatetime = js.native
  def beginIndex: js.BigInt = js.native
}

// --- EndRemoteWriteParameters ---

@js.native
sealed trait JsEndRemoteWriteParameters extends js.Object {
  def timestamp: JsDatetime = js.native
  def beginIndex: js.BigInt = js.native
}

// --- TypedDataValue ---

@js.native
sealed trait JsTypedDataValue extends js.Object {
  def value: JsDataValue   = js.native
  def schema: JsDataSchema = js.native
}

// --- AgentInitializationParameters ---

@js.native
sealed trait JsAgentInitializationParameters extends js.Object {
  def idempotencyKey: String                            = js.native
  def constructorParameters: JsTypedDataValue           = js.native
  def traceId: String                                   = js.native
  def traceStates: js.Array[String]                     = js.native
  def invocationContext: js.Array[js.Array[JsSpanData]] = js.native
}

// --- AgentMethodInvocationParameters ---

@js.native
sealed trait JsAgentMethodInvocationParameters extends js.Object {
  def idempotencyKey: String                            = js.native
  def methodName: String                                = js.native
  def functionInput: JsTypedDataValue                   = js.native
  def traceId: String                                   = js.native
  def traceStates: js.Array[String]                     = js.native
  def invocationContext: js.Array[js.Array[JsSpanData]] = js.native
}

// --- LoadSnapshotParameters ---

@js.native
sealed trait JsLoadSnapshotParameters extends js.Object {
  def snapshot: JsSnapshot = js.native
}

// --- ProcessOplogEntriesParameters ---

@js.native
sealed trait JsProcessOplogEntriesParameters extends js.Object {
  def idempotencyKey: String = js.native
}

// --- ManualUpdateParameters ---

@js.native
sealed trait JsManualUpdateParameters extends js.Object {
  def targetRevision: js.BigInt = js.native
}

// --- AgentInvocation  –  tagged union ---

@js.native
sealed trait JsAgentInvocation extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsAgentInvocationWithValue extends JsAgentInvocation {
  @JSName("val") def value: js.Any = js.native
}

object JsAgentInvocation {
  def agentInitialization(params: JsAgentInitializationParameters): JsAgentInvocation =
    JsShape.tagged[JsAgentInvocation]("agent-initialization", params)

  def agentMethodInvocation(params: JsAgentMethodInvocationParameters): JsAgentInvocation =
    JsShape.tagged[JsAgentInvocation]("agent-method-invocation", params)

  def saveSnapshot: JsAgentInvocation =
    JsShape.tagOnly[JsAgentInvocation]("save-snapshot")

  def loadSnapshot(params: JsLoadSnapshotParameters): JsAgentInvocation =
    JsShape.tagged[JsAgentInvocation]("load-snapshot", params)

  def processOplogEntries(params: JsProcessOplogEntriesParameters): JsAgentInvocation =
    JsShape.tagged[JsAgentInvocation]("process-oplog-entries", params)

  def manualUpdate(params: JsManualUpdateParameters): JsAgentInvocation =
    JsShape.tagged[JsAgentInvocation]("manual-update", params)
}

// --- AgentInvocationOutputParameters ---

@js.native
sealed trait JsAgentInvocationOutputParameters extends js.Object {
  def output: JsTypedDataValue = js.native
}

// --- FallibleResultParameters ---

@js.native
sealed trait JsFallibleResultParameters extends js.Object {
  def error: js.UndefOr[String] = js.native
}

// --- SaveSnapshotResultParameters ---

@js.native
sealed trait JsSaveSnapshotResultParameters extends js.Object {
  def snapshot: JsSnapshot = js.native
}

// --- AgentInvocationResult  –  tagged union ---

@js.native
sealed trait JsAgentInvocationResult extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsAgentInvocationResultWithValue extends JsAgentInvocationResult {
  @JSName("val") def value: js.Any = js.native
}

// --- AgentInvocationStartedParameters ---

@js.native
sealed trait JsAgentInvocationStartedParameters extends js.Object {
  def timestamp: JsDatetime         = js.native
  def invocation: JsAgentInvocation = js.native
}

// --- AgentInvocationFinishedParameters ---

@js.native
sealed trait JsAgentInvocationFinishedParameters extends js.Object {
  def timestamp: JsDatetime                     = js.native
  def invocationResult: JsAgentInvocationResult = js.native
  def consumedFuel: js.BigInt                   = js.native
  def componentRevision: js.BigInt              = js.native
}

// --- PendingAgentInvocationParameters ---

@js.native
sealed trait JsPendingAgentInvocationParameters extends js.Object {
  def timestamp: JsDatetime         = js.native
  def invocation: JsAgentInvocation = js.native
}

// --- UpdateDescription  –  tagged union ---

@js.native
sealed trait JsUpdateDescription extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsUpdateDescriptionSnapshotBased extends JsUpdateDescription {
  @JSName("val") def value: JsSnapshot = js.native
}

object JsUpdateDescription {
  def autoUpdate: JsUpdateDescription =
    JsShape.tagOnly[JsUpdateDescription]("auto-update")

  def snapshotBased(snapshot: JsSnapshot): JsUpdateDescription =
    JsShape.tagged[JsUpdateDescription]("snapshot-based", snapshot)
}

// --- PendingUpdateParameters ---

@js.native
sealed trait JsPendingUpdateParameters extends js.Object {
  def timestamp: JsDatetime                  = js.native
  def targetRevision: js.BigInt              = js.native
  def updateDescription: JsUpdateDescription = js.native
}

// --- SuccessfulUpdateParameters ---

@js.native
sealed trait JsSuccessfulUpdateParameters extends js.Object {
  def timestamp: JsDatetime                                       = js.native
  def targetRevision: js.BigInt                                   = js.native
  def newComponentSize: js.BigInt                                 = js.native
  def newActivePlugins: js.Array[JsPluginInstallationDescription] = js.native
}

// --- FailedUpdateParameters ---

@js.native
sealed trait JsFailedUpdateParameters extends js.Object {
  def timestamp: JsDatetime       = js.native
  def targetRevision: js.BigInt   = js.native
  def details: js.UndefOr[String] = js.native
}

// --- GrowMemoryParameters ---

@js.native
sealed trait JsGrowMemoryParameters extends js.Object {
  def timestamp: JsDatetime = js.native
  def delta: js.BigInt      = js.native
}

// --- CreateResourceParameters ---

@js.native
sealed trait JsCreateResourceParameters extends js.Object {
  def timestamp: JsDatetime = js.native
  def resourceId: js.BigInt = js.native
  def name: String          = js.native
  def owner: String         = js.native
}

// --- DropResourceParameters ---

@js.native
sealed trait JsDropResourceParameters extends js.Object {
  def timestamp: JsDatetime = js.native
  def resourceId: js.BigInt = js.native
  def name: String          = js.native
  def owner: String         = js.native
}

// --- LogParameters ---

@js.native
sealed trait JsLogParameters extends js.Object {
  def timestamp: JsDatetime = js.native
  def level: String         = js.native
  def context: String       = js.native
  def message: String       = js.native
}

// --- ActivatePluginParameters ---

@js.native
sealed trait JsActivatePluginParameters extends js.Object {
  def timestamp: JsDatetime                   = js.native
  def plugin: JsPluginInstallationDescription = js.native
}

// --- DeactivatePluginParameters ---

@js.native
sealed trait JsDeactivatePluginParameters extends js.Object {
  def timestamp: JsDatetime                   = js.native
  def plugin: JsPluginInstallationDescription = js.native
}

// --- RevertParameters ---

@js.native
sealed trait JsRevertParameters extends js.Object {
  def timestamp: JsDatetime        = js.native
  def droppedRegion: JsOplogRegion = js.native
}

// --- CancelPendingInvocationParameters ---

@js.native
sealed trait JsCancelPendingInvocationParameters extends js.Object {
  def timestamp: JsDatetime  = js.native
  def idempotencyKey: String = js.native
}

// --- StartSpanParameters ---

@js.native
sealed trait JsStartSpanParameters extends js.Object {
  def timestamp: JsDatetime               = js.native
  def spanId: String                      = js.native
  def parent: js.UndefOr[String]          = js.native
  def linkedContextId: js.UndefOr[String] = js.native
  def attributes: js.Array[JsAttribute]   = js.native
}

// --- FinishSpanParameters ---

@js.native
sealed trait JsFinishSpanParameters extends js.Object {
  def timestamp: JsDatetime = js.native
  def spanId: String        = js.native
}

// --- SetSpanAttributeParameters ---

@js.native
sealed trait JsSetSpanAttributeParameters extends js.Object {
  def timestamp: JsDatetime   = js.native
  def spanId: String          = js.native
  def key: String             = js.native
  def value: JsAttributeValue = js.native
}

// --- ChangePersistenceLevelParameters ---

@js.native
sealed trait JsChangePersistenceLevelParameters extends js.Object {
  def timestamp: JsDatetime                = js.native
  def persistenceLevel: JsPersistenceLevel = js.native
}

// --- BeginRemoteTransactionParameters ---

@js.native
sealed trait JsBeginRemoteTransactionParameters extends js.Object {
  def timestamp: JsDatetime = js.native
  def transactionId: String = js.native
}

// --- RemoteTransactionParameters ---

@js.native
sealed trait JsRemoteTransactionParameters extends js.Object {
  def timestamp: JsDatetime = js.native
  def beginIndex: js.BigInt = js.native
}

// --- SnapshotData ---

@js.native
sealed trait JsSnapshotData extends js.Object {
  def data: Uint8Array = js.native
  def mimeType: String = js.native
}

// --- SnapshotParameters ---

@js.native
sealed trait JsSnapshotParameters extends js.Object {
  def timestamp: JsDatetime  = js.native
  def data: JsSnapshotData   = js.native
}

// --- OplogProcessorCheckpointParameters ---

@js.native
sealed trait JsOplogProcessorCheckpointParameters extends js.Object {
  def timestamp: JsDatetime                   = js.native
  def plugin: JsPluginInstallationDescription = js.native
  def targetAgentId: JsAgentId                = js.native
  def confirmedUpTo: js.BigInt                = js.native
  def sendingUpTo: js.BigInt                  = js.native
  def lastBatchStart: js.BigInt               = js.native
}

// --- OplogTimestamp ---

@js.native
sealed trait JsOplogTimestamp extends js.Object {
  def timestamp: JsDatetime = js.native
}

// ---------------------------------------------------------------------------
// PublicOplogEntry  –  tagged union (37+ variants)
// ---------------------------------------------------------------------------

@js.native
sealed trait JsPublicOplogEntry extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsPublicOplogEntryWithValue extends JsPublicOplogEntry {
  @JSName("val") def value: js.Any = js.native
}
