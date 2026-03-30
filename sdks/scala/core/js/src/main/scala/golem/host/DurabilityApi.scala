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

import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport

/**
 * Scala.js facade for `golem:durability/durability@1.5.0`.
 *
 * WIT interface:
 * {{{
 *   type durable-function-type = wrapped-function-type;
 *   record durable-execution-state { is-live: bool, persistence-level: persistence-level }
 *   enum oplog-entry-version { v1, v2 }
 *   record persisted-durable-function-invocation {
 *     timestamp: datetime, function-name: string, response: value-and-type,
 *     function-type: durable-function-type, entry-version: oplog-entry-version
 *   }
 *   observe-function-call: func(iface: string, function: string)
 *   begin-durable-function: func(function-type: durable-function-type) -> oplog-index
 *   end-durable-function: func(function-type: durable-function-type, begin-index: oplog-index, forced-commit: bool)
 *   current-durable-execution-state: func() -> durable-execution-state
 *   persist-durable-function-invocation: func(function-name: string, request: value-and-type, response: value-and-type, function-type: durable-function-type)
 *   read-persisted-durable-function-invocation: func() -> persisted-durable-function-invocation
 * }}}
 */
object DurabilityApi {

  type OplogIndex = BigInt

  // --- WIT: wrapped-function-type variant (aliased as durable-function-type) ---

  sealed trait DurableFunctionType extends Product with Serializable {
    def tag: String
  }

  object DurableFunctionType {
    case object ReadLocal                                          extends DurableFunctionType { val tag = "read-local"   }
    case object WriteLocal                                         extends DurableFunctionType { val tag = "write-local"  }
    case object ReadRemote                                         extends DurableFunctionType { val tag = "read-remote"  }
    case object WriteRemote                                        extends DurableFunctionType { val tag = "write-remote" }
    final case class WriteRemoteBatched(begin: Option[OplogIndex]) extends DurableFunctionType {
      val tag = "write-remote-batched"
    }
    final case class WriteRemoteTransaction(begin: Option[OplogIndex]) extends DurableFunctionType {
      val tag = "write-remote-transaction"
    }

    def fromJs(raw: JsWrappedFunctionType): DurableFunctionType =
      raw.tag match {
        case "read-local"           => ReadLocal
        case "write-local"          => WriteLocal
        case "read-remote"          => ReadRemote
        case "write-remote"         => WriteRemote
        case "write-remote-batched" =>
          val v   = raw.asInstanceOf[JsWrappedFunctionTypeBatched].value
          val idx = v.toOption.map(bi => BigInt(bi.toString))
          WriteRemoteBatched(idx)
        case "write-remote-transaction" =>
          val v   = raw.asInstanceOf[JsWrappedFunctionTypeTransaction].value
          val idx = v.toOption.map(bi => BigInt(bi.toString))
          WriteRemoteTransaction(idx)
        case other => throw new IllegalArgumentException(s"Unknown DurableFunctionType tag: $other")
      }

    def toJs(ft: DurableFunctionType): JsWrappedFunctionType = ft match {
      case ReadLocal               => JsWrappedFunctionType.readLocal
      case WriteLocal              => JsWrappedFunctionType.writeLocal
      case ReadRemote              => JsWrappedFunctionType.readRemote
      case WriteRemote             => JsWrappedFunctionType.writeRemote
      case WriteRemoteBatched(idx) =>
        JsWrappedFunctionType.writeRemoteBatched(
          idx.fold[js.UndefOr[js.BigInt]](js.undefined)(i => js.BigInt(i.toString))
        )
      case WriteRemoteTransaction(idx) =>
        JsWrappedFunctionType.writeRemoteTransaction(
          idx.fold[js.UndefOr[js.BigInt]](js.undefined)(i => js.BigInt(i.toString))
        )
    }

  }

  // --- WIT: durable-execution-state record ---

  final case class DurableExecutionState(
    isLive: Boolean,
    persistenceLevel: HostApi.PersistenceLevel
  )

  // --- WIT: oplog-entry-version enum ---

  sealed trait OplogEntryVersion extends Product with Serializable
  object OplogEntryVersion {
    case object V1 extends OplogEntryVersion
    case object V2 extends OplogEntryVersion

    def fromString(s: String): OplogEntryVersion = s match {
      case "v1" => V1
      case "v2" => V2
      case _    => V1
    }
  }

  // --- WIT: persisted-durable-function-invocation record ---

  final case class Datetime(seconds: BigInt, nanoseconds: Int)

  final case class PersistedDurableFunctionInvocation(
    timestamp: Datetime,
    functionName: String,
    response: WitValueTypes.ValueAndType,
    functionType: DurableFunctionType,
    entryVersion: OplogEntryVersion
  )

  // --- Native bindings ---

  @js.native
  @JSImport("golem:durability/durability@1.5.0", JSImport.Namespace)
  private object DurabilityModule extends js.Object {
    def observeFunctionCall(iface: String, function: String): Unit                                   = js.native
    def beginDurableFunction(functionType: js.Any): js.BigInt                                        = js.native
    def endDurableFunction(functionType: js.Any, beginIndex: js.BigInt, forcedCommit: Boolean): Unit = js.native
    def currentDurableExecutionState(): js.Any                                                       = js.native
    def persistDurableFunctionInvocation(
      functionName: String,
      request: js.Any,
      response: js.Any,
      functionType: js.Any
    ): Unit                                              = js.native
    def readPersistedDurableFunctionInvocation(): js.Any = js.native
  }

  // --- Typed public API ---

  def observeFunctionCall(iface: String, function: String): Unit =
    DurabilityModule.observeFunctionCall(iface, function)

  def beginDurableFunction(functionType: DurableFunctionType): OplogIndex =
    BigInt(DurabilityModule.beginDurableFunction(DurableFunctionType.toJs(functionType)).toString)

  def endDurableFunction(functionType: DurableFunctionType, beginIndex: OplogIndex, forcedCommit: Boolean): Unit =
    DurabilityModule.endDurableFunction(
      DurableFunctionType.toJs(functionType),
      js.BigInt(beginIndex.toString),
      forcedCommit
    )

  def currentDurableExecutionState(): DurableExecutionState = {
    val raw  = DurabilityModule.currentDurableExecutionState().asInstanceOf[JsDurableExecutionState]
    val live = raw.isLive
    val pl   = HostApi.PersistenceLevel.fromTag(raw.persistenceLevel.tag)
    DurableExecutionState(live, pl)
  }

  def persistDurableFunctionInvocation(
    functionName: String,
    request: WitValueTypes.ValueAndType,
    response: WitValueTypes.ValueAndType,
    functionType: DurableFunctionType
  ): Unit =
    DurabilityModule.persistDurableFunctionInvocation(
      functionName,
      WitValueTypes.ValueAndType.toJs(request),
      WitValueTypes.ValueAndType.toJs(response),
      DurableFunctionType.toJs(functionType)
    )

  def readPersistedDurableFunctionInvocation(): PersistedDurableFunctionInvocation = {
    val raw =
      DurabilityModule.readPersistedDurableFunctionInvocation().asInstanceOf[JsPersistedDurableFunctionInvocation]
    val ts        = raw.timestamp
    val timestamp = Datetime(BigInt(ts.seconds.toString), ts.nanoseconds)
    val funcName  = raw.functionName
    val response  = WitValueTypes.ValueAndType.fromJs(raw.response)
    val funcType  = DurableFunctionType.fromJs(raw.functionType)
    val entryVer  = OplogEntryVersion.fromString(raw.entryVersion)
    PersistedDurableFunctionInvocation(timestamp, funcName, response, funcType, entryVer)
  }

  def raw: Any = DurabilityModule
}
