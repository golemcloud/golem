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

package golem.host.js

import golem.host.js.schema.JsTypedSchemaValue

import scala.scalajs.js
import scala.scalajs.js.annotation.JSName

// ---------------------------------------------------------------------------
// golem:durability/durability@1.5.0  –  JS facade traits
// ---------------------------------------------------------------------------

// --- WrappedFunctionType / DurableFunctionType  –  tagged union ---

@js.native
sealed trait JsWrappedFunctionType extends js.Object {
  def tag: String = js.native
}

@js.native
sealed trait JsWrappedFunctionTypeBatched extends JsWrappedFunctionType {
  @JSName("val") def value: js.UndefOr[js.BigInt] = js.native
}

@js.native
sealed trait JsWrappedFunctionTypeTransaction extends JsWrappedFunctionType {
  @JSName("val") def value: js.UndefOr[js.BigInt] = js.native
}

object JsWrappedFunctionType {
  def readLocal: JsWrappedFunctionType   = JsShape.tagOnly[JsWrappedFunctionType]("read-local")
  def writeLocal: JsWrappedFunctionType  = JsShape.tagOnly[JsWrappedFunctionType]("write-local")
  def readRemote: JsWrappedFunctionType  = JsShape.tagOnly[JsWrappedFunctionType]("read-remote")
  def writeRemote: JsWrappedFunctionType = JsShape.tagOnly[JsWrappedFunctionType]("write-remote")

  def writeRemoteBatched(beginIndex: js.UndefOr[js.BigInt]): JsWrappedFunctionType =
    JsShape.taggedOptional[JsWrappedFunctionType]("write-remote-batched", beginIndex.map(_.asInstanceOf[js.Any]))

  def writeRemoteTransaction(beginIndex: js.UndefOr[js.BigInt]): JsWrappedFunctionType =
    JsShape.taggedOptional[JsWrappedFunctionType]("write-remote-transaction", beginIndex.map(_.asInstanceOf[js.Any]))
}

// --- DurableExecutionState ---

@js.native
sealed trait JsDurableExecutionState extends js.Object {
  def isLive: Boolean                      = js.native
  def persistenceLevel: JsPersistenceLevel = js.native
}

object JsDurableExecutionState {
  def apply(isLive: Boolean, persistenceLevel: JsPersistenceLevel): JsDurableExecutionState =
    js.Dynamic.literal("isLive" -> isLive, "persistenceLevel" -> persistenceLevel).asInstanceOf[JsDurableExecutionState]
}

// --- PersistedDurableFunctionInvocation ---

@js.native
sealed trait JsPersistedDurableFunctionInvocation extends js.Object {
  def timestamp: JsDatetime               = js.native
  def functionName: String                = js.native
  def response: JsTypedSchemaValue        = js.native
  def functionType: JsDurableFunctionType = js.native
  def entryVersion: JsOplogEntryVersion   = js.native
}

object JsPersistedDurableFunctionInvocation {
  def apply(
    timestamp: JsDatetime,
    functionName: String,
    response: JsTypedSchemaValue,
    functionType: JsDurableFunctionType,
    entryVersion: JsOplogEntryVersion
  ): JsPersistedDurableFunctionInvocation =
    js.Dynamic
      .literal(
        "timestamp"    -> timestamp,
        "functionName" -> functionName,
        "response"     -> response,
        "functionType" -> functionType,
        "entryVersion" -> entryVersion
      )
      .asInstanceOf[JsPersistedDurableFunctionInvocation]
}
