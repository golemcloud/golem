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

package golem.host

import golem.host.js._
import golem.host.js.schema.JsTypedSchemaValue
import golem.schema.{FromSchema, IntoSchema, TypedSchemaValue}
import golem.schema.wire.SchemaWire

import scala.concurrent.{ExecutionContext, Future}
import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport

/**
 * Scala.js facade for `golem:durability/durability@1.6.0`.
 *
 * WIT interface:
 * {{{
 *   type durable-function-type = wrapped-function-type;
 *   resource live-custom-durable-invocation {
 *     finish: static func(this: live-custom-durable-invocation, response: typed-schema-value, forced-commit: bool)
 *   }
 *   enum oplog-entry-version { v1, v2 }
 *   record persisted-durable-function-invocation {
 *     timestamp: datetime, function-name: string, response: typed-schema-value,
 *     function-type: durable-function-type, entry-version: oplog-entry-version
 *   }
 *   observe-function-call: func(iface: string, function: string)
 *   begin-custom-durable-invocation: func(function-name: string, request: typed-schema-value, function-type: durable-function-type) -> custom-durable-invocation
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
    response: TypedSchemaValue,
    functionType: DurableFunctionType,
    entryVersion: OplogEntryVersion
  )

  private sealed trait CustomDurableInvocation
  private object CustomDurableInvocation {
    final case class Live(invocation: LiveCustomDurableInvocation)            extends CustomDurableInvocation
    final case class Replayed(invocation: PersistedDurableFunctionInvocation) extends CustomDurableInvocation
  }

  private final class LiveCustomDurableInvocation(private val raw: JsLiveCustomDurableInvocation) {
    private var active = true

    def finish(response: TypedSchemaValue, forcedCommit: Boolean): Unit = {
      val jsResponse = typedToJs(response)
      if (!active)
        throw new IllegalStateException("custom durable invocation is no longer active")
      active = false
      JsLiveCustomDurableInvocation.finish(raw, jsResponse, forcedCommit)
    }

    def drop(): Unit =
      if (active) {
        active = false
        val symbolDispose    = js.Dynamic.global.Symbol.selectDynamic("dispose")
        val resourceDisposer = js.Dynamic.global.Reflect.applyDynamic("get")(raw, symbolDispose)
        if (js.isUndefined(resourceDisposer))
          throw new IllegalStateException("live custom durable invocation resource has no disposer")
        js.Dynamic.global.Reflect.applyDynamic("apply")(resourceDisposer, raw, js.Array())
      }
  }

  // --- Native bindings ---

  @js.native
  @JSImport("golem:durability/durability@1.6.0", JSImport.Namespace)
  private object DurabilityModule extends js.Object {
    def observeFunctionCall(iface: String, function: String): Unit = js.native
    def beginCustomDurableInvocation(
      functionName: String,
      request: js.Any,
      functionType: js.Any
    ): JsCustomDurableInvocation = js.native
  }

  @js.native
  @JSImport("golem:durability/durability@1.6.0", "LiveCustomDurableInvocation")
  private object JsLiveCustomDurableInvocation extends js.Object {
    def finish(
      invocation: golem.host.js.JsLiveCustomDurableInvocation,
      response: JsTypedSchemaValue,
      forcedCommit: Boolean
    ): Unit = js.native
  }

  // --- Typed public API ---

  def observeFunctionCall(iface: String, function: String): Unit =
    DurabilityModule.observeFunctionCall(iface, function)

  private def beginCustomDurableInvocation(
    functionName: String,
    request: TypedSchemaValue,
    functionType: DurableFunctionType
  ): CustomDurableInvocation = {
    val raw = DurabilityModule.beginCustomDurableInvocation(
      functionName,
      typedToJs(request),
      DurableFunctionType.toJs(functionType)
    )
    raw.tag match {
      case "live" =>
        val invocation = raw.asInstanceOf[JsCustomDurableInvocationLive].value
        CustomDurableInvocation.Live(new LiveCustomDurableInvocation(invocation))
      case "replayed" =>
        val persisted = raw.asInstanceOf[JsCustomDurableInvocationReplayed].value
        CustomDurableInvocation.Replayed(persistedFromJs(persisted))
      case other => throw new IllegalArgumentException(s"Unknown CustomDurableInvocation tag: $other")
    }
  }

  def durable[Request, Response](
    iface: String,
    function: String,
    functionType: DurableFunctionType,
    request: Request,
    forcedCommit: Boolean = false
  )(body: => Response)(implicit
    requestSchema: IntoSchema[Request],
    responseSchema: IntoSchema[Response],
    responseDecoder: FromSchema[Response]
  ): Response = {
    observeFunctionCall(iface, function)
    val functionName = durableFunctionName(iface, function)
    beginCustomDurableInvocation(functionName, requestSchema.toTyped(request), functionType) match {
      case CustomDurableInvocation.Live(invocation) =>
        try {
          val result = body
          invocation.finish(responseSchema.toTyped(result), forcedCommit)
          result
        } catch {
          case error: Throwable =>
            invocation.drop()
            throw error
        }
      case CustomDurableInvocation.Replayed(invocation) =>
        decodeReplay(invocation, functionName, functionType)
    }
  }

  def durableAsync[Request, Response](
    iface: String,
    function: String,
    functionType: DurableFunctionType,
    request: Request,
    forcedCommit: Boolean = false
  )(body: => Future[Response])(implicit
    requestSchema: IntoSchema[Request],
    responseSchema: IntoSchema[Response],
    responseDecoder: FromSchema[Response],
    executionContext: ExecutionContext
  ): Future[Response] = {
    observeFunctionCall(iface, function)
    val functionName = durableFunctionName(iface, function)
    beginCustomDurableInvocation(functionName, requestSchema.toTyped(request), functionType) match {
      case CustomDurableInvocation.Live(invocation) =>
        try
          body.transform(
            result =>
              try {
                invocation.finish(responseSchema.toTyped(result), forcedCommit)
                result
              } catch {
                case error: Throwable =>
                  invocation.drop()
                  throw error
              },
            error => {
              invocation.drop()
              error
            }
          )
        catch {
          case error: Throwable =>
            invocation.drop()
            throw error
        }
      case CustomDurableInvocation.Replayed(invocation) =>
        Future.successful(decodeReplay(invocation, functionName, functionType))
    }
  }

  private def durableFunctionName(iface: String, function: String): String =
    if (iface.isEmpty) function else s"$iface::$function"

  private def decodeReplay[Response](
    invocation: PersistedDurableFunctionInvocation,
    functionName: String,
    functionType: DurableFunctionType
  )(implicit responseDecoder: FromSchema[Response]): Response = {
    if (invocation.functionName != functionName)
      throw new IllegalStateException(
        s"durable replay mismatch: expected function '$functionName', oplog has '${invocation.functionName}'"
      )
    if (invocation.functionType != functionType)
      throw new IllegalStateException(
        s"durable replay mismatch for '$functionName': expected function type '$functionType', oplog has '${invocation.functionType}'"
      )
    responseDecoder.fromValue(invocation.response.value) match {
      case Right(response) => response
      case Left(error)     =>
        throw new IllegalStateException(s"failed to decode durable response for '$functionName': $error")
    }
  }

  private def persistedFromJs(raw: JsPersistedDurableFunctionInvocation): PersistedDurableFunctionInvocation = {
    val ts        = raw.timestamp
    val timestamp = Datetime(BigInt(ts.seconds.toString), ts.nanoseconds)
    val funcName  = raw.functionName
    val response  = typedFromJs(raw.response)
    val funcType  = DurableFunctionType.fromJs(raw.functionType)
    val entryVer  = OplogEntryVersion.fromString(raw.entryVersion)
    PersistedDurableFunctionInvocation(timestamp, funcName, response, funcType, entryVer)
  }

  private def typedToJs(tv: TypedSchemaValue): JsTypedSchemaValue =
    SchemaWireInterop.typedToJs(SchemaWire.typedSchemaValueToWit(tv))

  private def typedFromJs(value: JsTypedSchemaValue): TypedSchemaValue =
    SchemaWire.typedSchemaValueFromWit(SchemaWireInterop.typedFromJs(value))
}
