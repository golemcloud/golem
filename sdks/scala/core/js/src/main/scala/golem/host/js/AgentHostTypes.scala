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
// golem:agent/host@1.5.0  +  golem:api/host@1.5.0  –  JS facade traits
// ---------------------------------------------------------------------------

// --- RpcError  –  tagged union ---

@js.native
sealed trait JsRpcError extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsRpcErrorString extends JsRpcError {
  @JSName("val") def value: String = js.native
}

@js.native
sealed trait JsRpcErrorRemoteAgent extends JsRpcError {
  @JSName("val") def value: JsAgentError = js.native
}

object JsRpcError {
  def protocolError(message: String): JsRpcError =
    JsShape.tagged[JsRpcError]("protocol-error", message.asInstanceOf[js.Any])

  def denied(message: String): JsRpcError =
    JsShape.tagged[JsRpcError]("denied", message.asInstanceOf[js.Any])

  def notFound(message: String): JsRpcError =
    JsShape.tagged[JsRpcError]("not-found", message.asInstanceOf[js.Any])

  def remoteInternalError(message: String): JsRpcError =
    JsShape.tagged[JsRpcError]("remote-internal-error", message.asInstanceOf[js.Any])

  def remoteAgentError(error: JsAgentError): JsRpcError =
    JsShape.tagged[JsRpcError]("remote-agent-error", error)
}

// --- Datetime (wasi:clocks/wall-clock@0.2.3) ---

@js.native
sealed trait JsDatetime extends js.Object {
  def seconds: js.BigInt = js.native
  def nanoseconds: Int   = js.native
}

object JsDatetime {
  def apply(seconds: js.BigInt, nanoseconds: Int): JsDatetime =
    js.Dynamic.literal("seconds" -> seconds, "nanoseconds" -> nanoseconds).asInstanceOf[JsDatetime]
}

// --- Snapshot ---

@js.native
sealed trait JsSnapshot extends js.Object {
  def data: Uint8Array = js.native
  def mimeType: String = js.native
}

object JsSnapshot {
  def apply(data: Uint8Array, mimeType: String): JsSnapshot =
    js.Dynamic.literal("data" -> data, "mimeType" -> mimeType).asInstanceOf[JsSnapshot]
}

// --- PersistenceLevel  –  tagged union ---

@js.native
sealed trait JsPersistenceLevel extends js.Object {
  def tag: String = js.native
}

object JsPersistenceLevel {
  def persistNothing: JsPersistenceLevel =
    JsShape.tagOnly[JsPersistenceLevel]("persist-nothing")

  def persistRemoteSideEffects: JsPersistenceLevel =
    JsShape.tagOnly[JsPersistenceLevel]("persist-remote-side-effects")

  def smart: JsPersistenceLevel =
    JsShape.tagOnly[JsPersistenceLevel]("smart")
}

// --- EnvironmentId ---

@js.native
sealed trait JsEnvironmentId extends js.Object {
  def uuid: JsUuid = js.native
}

object JsEnvironmentId {
  def apply(uuid: JsUuid): JsEnvironmentId =
    js.Dynamic.literal("uuid" -> uuid).asInstanceOf[JsEnvironmentId]
}

// --- AgentMetadata ---

@js.native
sealed trait JsAgentMetadata extends js.Object {
  def agentId: JsAgentId                              = js.native
  def args: js.Array[String]                          = js.native
  def env: js.Array[js.Tuple2[String, String]]        = js.native
  def configVars: js.Array[js.Tuple2[String, String]] = js.native
  def status: JsAgentStatus                           = js.native
  def componentRevision: js.BigInt                    = js.native
  def retryCount: js.BigInt                           = js.native
  def environmentId: JsEnvironmentId                  = js.native
}

@js.native
private[golem] sealed trait JsAgentMetadataRuntime extends JsAgentMetadata {
  def agentType: js.UndefOr[String]          = js.native
  def agentName: js.UndefOr[String]          = js.native
  def componentId: js.UndefOr[JsComponentId] = js.native
}

object JsAgentMetadata {
  def apply(
    agentId: JsAgentId,
    args: js.Array[String],
    env: js.Array[js.Tuple2[String, String]],
    configVars: js.Array[js.Tuple2[String, String]],
    status: JsAgentStatus,
    componentRevision: js.BigInt,
    retryCount: js.BigInt,
    environmentId: JsEnvironmentId
  ): JsAgentMetadata =
    js.Dynamic
      .literal(
        "agentId"           -> agentId,
        "args"              -> args,
        "env"               -> env,
        "configVars"        -> configVars,
        "status"            -> status,
        "componentRevision" -> componentRevision,
        "retryCount"        -> retryCount,
        "environmentId"     -> environmentId
      )
      .asInstanceOf[JsAgentMetadata]
}

// --- ForkResult, ForkDetails ---

@js.native
sealed trait JsForkDetails extends js.Object {
  def forkedPhantomId: JsUuid = js.native
}

object JsForkDetails {
  def apply(forkedPhantomId: JsUuid): JsForkDetails =
    js.Dynamic.literal("forkedPhantomId" -> forkedPhantomId).asInstanceOf[JsForkDetails]
}

@js.native
sealed trait JsForkResult extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsForkResultOriginal extends JsForkResult {
  @JSName("val") def value: JsForkDetails = js.native
}

@js.native
sealed trait JsForkResultForked extends JsForkResult {
  @JSName("val") def value: JsForkDetails = js.native
}

object JsForkResult {
  def original(details: JsForkDetails): JsForkResult =
    JsShape.tagged[JsForkResult]("original", details)

  def forked(details: JsForkDetails): JsForkResult =
    JsShape.tagged[JsForkResult]("forked", details)
}

// --- RevertAgentTarget  –  tagged union ---

@js.native
sealed trait JsRevertAgentTarget extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsRevertToOplogIndex extends JsRevertAgentTarget {
  @JSName("val") def value: js.BigInt = js.native
}

@js.native
sealed trait JsRevertLastInvocations extends JsRevertAgentTarget {
  @JSName("val") def value: js.BigInt = js.native
}

object JsRevertAgentTarget {
  def revertToOplogIndex(index: js.BigInt): JsRevertAgentTarget =
    JsShape.tagged[JsRevertAgentTarget]("revert-to-oplog-index", index)

  def revertLastInvocations(count: js.BigInt): JsRevertAgentTarget =
    JsShape.tagged[JsRevertAgentTarget]("revert-last-invocations", count)
}

// ---------------------------------------------------------------------------
// Filter types
// ---------------------------------------------------------------------------

@js.native
sealed trait JsAgentNameFilter extends js.Object {
  def comparator: JsStringFilterComparator = js.native
  def value: String                        = js.native
}

object JsAgentNameFilter {
  def apply(comparator: JsStringFilterComparator, value: String): JsAgentNameFilter =
    js.Dynamic.literal("comparator" -> comparator, "value" -> value).asInstanceOf[JsAgentNameFilter]
}

@js.native
sealed trait JsAgentStatusFilter extends js.Object {
  def comparator: JsFilterComparator = js.native
  def value: JsAgentStatus           = js.native
}

object JsAgentStatusFilter {
  def apply(comparator: JsFilterComparator, value: JsAgentStatus): JsAgentStatusFilter =
    js.Dynamic.literal("comparator" -> comparator, "value" -> value).asInstanceOf[JsAgentStatusFilter]
}

@js.native
sealed trait JsAgentVersionFilter extends js.Object {
  def comparator: JsFilterComparator = js.native
  def value: js.BigInt               = js.native
}

object JsAgentVersionFilter {
  def apply(comparator: JsFilterComparator, value: js.BigInt): JsAgentVersionFilter =
    js.Dynamic.literal("comparator" -> comparator, "value" -> value).asInstanceOf[JsAgentVersionFilter]
}

@js.native
sealed trait JsAgentCreatedAtFilter extends js.Object {
  def comparator: JsFilterComparator = js.native
  def value: js.BigInt               = js.native
}

object JsAgentCreatedAtFilter {
  def apply(comparator: JsFilterComparator, value: js.BigInt): JsAgentCreatedAtFilter =
    js.Dynamic.literal("comparator" -> comparator, "value" -> value).asInstanceOf[JsAgentCreatedAtFilter]
}

@js.native
sealed trait JsAgentEnvFilter extends js.Object {
  def name: String                         = js.native
  def comparator: JsStringFilterComparator = js.native
  def value: String                        = js.native
}

object JsAgentEnvFilter {
  def apply(name: String, comparator: JsStringFilterComparator, value: String): JsAgentEnvFilter =
    js.Dynamic.literal("name" -> name, "comparator" -> comparator, "value" -> value).asInstanceOf[JsAgentEnvFilter]
}

@js.native
sealed trait JsAgentConfigVarsFilter extends js.Object {
  def name: String                         = js.native
  def comparator: JsStringFilterComparator = js.native
  def value: String                        = js.native
}

object JsAgentConfigVarsFilter {
  def apply(name: String, comparator: JsStringFilterComparator, value: String): JsAgentConfigVarsFilter =
    js.Dynamic
      .literal("name" -> name, "comparator" -> comparator, "value" -> value)
      .asInstanceOf[JsAgentConfigVarsFilter]
}

// --- AgentPropertyFilter  –  tagged union ---

@js.native
sealed trait JsAgentPropertyFilter extends js.Object {
  def tag: String = js.native
}

object JsAgentPropertyFilter {
  def name(filter: JsAgentNameFilter): JsAgentPropertyFilter =
    JsShape.tagged[JsAgentPropertyFilter]("name", filter)

  def status(filter: JsAgentStatusFilter): JsAgentPropertyFilter =
    JsShape.tagged[JsAgentPropertyFilter]("status", filter)

  def version(filter: JsAgentVersionFilter): JsAgentPropertyFilter =
    JsShape.tagged[JsAgentPropertyFilter]("version", filter)

  def createdAt(filter: JsAgentCreatedAtFilter): JsAgentPropertyFilter =
    JsShape.tagged[JsAgentPropertyFilter]("created-at", filter)

  def env(filter: JsAgentEnvFilter): JsAgentPropertyFilter =
    JsShape.tagged[JsAgentPropertyFilter]("env", filter)

  def wasiConfigVars(filter: JsAgentConfigVarsFilter): JsAgentPropertyFilter =
    JsShape.tagged[JsAgentPropertyFilter]("wasi-config-vars", filter)
}

// --- AgentAllFilter, AgentAnyFilter ---

@js.native
sealed trait JsAgentAllFilter extends js.Object {
  def filters: js.Array[JsAgentPropertyFilter] = js.native
}

object JsAgentAllFilter {
  def apply(filters: js.Array[JsAgentPropertyFilter]): JsAgentAllFilter =
    js.Dynamic.literal("filters" -> filters).asInstanceOf[JsAgentAllFilter]
}

@js.native
sealed trait JsAgentAnyFilter extends js.Object {
  def filters: js.Array[JsAgentAllFilter] = js.native
}

object JsAgentAnyFilter {
  def apply(filters: js.Array[JsAgentAllFilter]): JsAgentAnyFilter =
    js.Dynamic.literal("filters" -> filters).asInstanceOf[JsAgentAnyFilter]
}
