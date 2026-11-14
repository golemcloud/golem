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

import golem.concurrent.{CustomInvocationContext, CustomInvocationToken, GolemExecutionContext}

import scala.collection.mutable
import scala.concurrent.{ExecutionContext, Future, Promise}
import scala.scalajs.js
import scala.util.control.NonFatal
import scala.util.{Failure, Success, Try}
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

  // --- WIT: wrapped-function-type variant (aliased as durable-function-type) ---

  sealed trait DurableFunctionType extends Product with Serializable {
    def tag: String
  }

  object DurableFunctionType {
    case object ReadLocal   extends DurableFunctionType { val tag = "read-local"   }
    case object WriteLocal  extends DurableFunctionType { val tag = "write-local"  }
    case object ReadRemote  extends DurableFunctionType { val tag = "read-remote"  }
    case object WriteRemote extends DurableFunctionType { val tag = "write-remote" }

    def fromJs(raw: JsWrappedFunctionType): DurableFunctionType =
      raw.tag match {
        case "read-local"   => ReadLocal
        case "write-local"  => WriteLocal
        case "read-remote"  => ReadRemote
        case "write-remote" => WriteRemote
        case other          => throw new IllegalArgumentException(s"Unknown DurableFunctionType tag: $other")
      }

    def toJs(ft: DurableFunctionType): JsWrappedFunctionType = ft match {
      case ReadLocal   => JsWrappedFunctionType.readLocal
      case WriteLocal  => JsWrappedFunctionType.writeLocal
      case ReadRemote  => JsWrappedFunctionType.readRemote
      case WriteRemote => JsWrappedFunctionType.writeRemote
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

  private final class LiveCustomDurableInvocation(val raw: JsLiveCustomDurableInvocation) {
    private var active = true

    def finish(response: TypedSchemaValue, forcedCommit: Boolean): Unit = {
      val jsResponse = typedToJs(response)
      if (!active)
        throw new IllegalStateException("custom durable invocation is no longer active")
      active = false
      JsLiveCustomDurableInvocation.finish(raw, jsResponse, forcedCommit)
    }

    /**
     * Persists the response; the invocation stays owned and must be dropped
     * afterwards.
     */
    def finishAsync(response: TypedSchemaValue, forcedCommit: Boolean): Future[Unit] = {
      val jsResponse = typedToJs(response)
      if (!active)
        throw new IllegalStateException("custom durable invocation is no longer active")
      JsLiveCustomDurableInvocation.finishAsync(raw, jsResponse, forcedCommit).toFuture
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
    def beginCustomDurableInvocationAsync(
      functionName: String,
      request: js.Any,
      functionType: js.Any
    ): js.Promise[JsCustomDurableInvocation] = js.native
  }

  @js.native
  @JSImport("golem:durability/durability@1.6.0", "LiveCustomDurableInvocation")
  private object JsLiveCustomDurableInvocation extends js.Object {
    def finish(
      invocation: golem.host.js.JsLiveCustomDurableInvocation,
      response: JsTypedSchemaValue,
      forcedCommit: Boolean
    ): Unit = js.native
    def finishAsync(
      invocation: golem.host.js.JsLiveCustomDurableInvocation,
      response: JsTypedSchemaValue,
      forcedCommit: Boolean
    ): js.Promise[Unit] = js.native
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

  /**
   * Runs a custom operation whose body is synchronous. The body may open nested
   * synchronous operations; it must not start [[durableAsync]] operations,
   * which fail with `IllegalStateException`. Futures and callbacks that the
   * body starts continue in the caller's scope, so their host calls are not
   * part of this operation.
   */
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
          synchronousBodies += 1
          val result =
            try body
            finally synchronousBodies -= 1
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

  /**
   * Runs a custom operation whose body is asynchronous.
   *
   * The body, every `Future` callback it registers through a
   * [[golem.concurrent.GolemExecutionContext]], and every host call made from
   * those callbacks belong to this operation, also while sibling operations or
   * other root-level work run concurrently. The body receives such an execution
   * context as a context parameter; it takes precedence over an imported one.
   * The returned `Future` completes in the caller's scope.
   *
   * Operations started in the body are nested. The operation finishes, or is
   * dropped on failure, only after all of them have finished, so a nested
   * operation that never completes keeps this one open. Callbacks run by other
   * execution contexts, including `ExecutionContext.parasitic`, are not
   * attributed to this operation. Work that is still running after the
   * operation closed continues in the nearest open enclosing operation, or at
   * the root; host calls from work that escapes an outermost operation are not
   * supported, because replay skips the completed body.
   */
  def durableAsync[Request, Response](
    iface: String,
    function: String,
    functionType: DurableFunctionType,
    request: Request,
    forcedCommit: Boolean = false
  )(body: ExecutionContext ?=> Future[Response])(implicit
    requestSchema: IntoSchema[Request],
    responseSchema: IntoSchema[Response],
    responseDecoder: FromSchema[Response]
  ): Future[Response] =
    if (synchronousBodies > 0)
      Future.failed(
        new IllegalStateException(
          s"durableAsync operation '${durableFunctionName(iface, function)}' cannot run inside a synchronous durable body"
        )
      )
    else {
      implicit val ec: ExecutionContext = GolemExecutionContext.global
      val parent                        = openScope(CustomInvocationContext.capture())
      parent.foreach(_.childStarted())
      val functionName = durableFunctionName(iface, function)
      val admitted     = attempt {
        observeFunctionCall(iface, function)
        DurabilityModule
          .beginCustomDurableInvocationAsync(
            functionName,
            typedToJs(requestSchema.toTyped(request)),
            DurableFunctionType.toJs(functionType)
          )
          .toFuture
      }
      admitted.transformWith { outcome =>
        val live =
          outcome.toOption.map(raw => raw: js.Any).filter(_.asInstanceOf[JsCustomDurableInvocation].tag == "live")
        live match {
          case Some(raw) =>
            val invocation = new LiveCustomDurableInvocation(raw.asInstanceOf[JsCustomDurableInvocationLive].value)
            runLive(invocation, parent, forcedCommit)(body)
          case None =>
            parent.foreach(_.childClosed())
            Future.fromTry(outcome.map { raw =>
              raw.tag match {
                case "replayed" =>
                  val persisted = raw.asInstanceOf[JsCustomDurableInvocationReplayed].value
                  decodeReplay(persistedFromJs(persisted), functionName, functionType)
                case other => throw new IllegalArgumentException(s"Unknown CustomDurableInvocation tag: $other")
              }
            })
        }
      }
    }

  // Runs in the parent's scope. Every callback registered here captures the operation's scope.
  private def runLive[Response](
    invocation: LiveCustomDurableInvocation,
    parent: Option[OpenScope],
    forcedCommit: Boolean
  )(body: ExecutionContext ?=> Future[Response])(implicit
    responseSchema: IntoSchema[Response],
    ec: ExecutionContext
  ): Future[Response] = {
    val token =
      try CustomInvocationContext.enterLive(invocation.raw)
      catch {
        case error: Throwable =>
          invocation.drop()
          parent.foreach(_.childClosed())
          throw error
      }
    val scope = new OpenScope(token)
    openScopes += scope
    CustomInvocationContext.run(
      token,
      () =>
        attempt(body)
          .transformWith(result => scope.bodyFinished().transformWith(_ => complete(invocation, result, forcedCommit)))
          .transform(result => close(invocation, scope, parent, result))
    )
  }

  private def complete[Response](
    invocation: LiveCustomDurableInvocation,
    result: Try[Response],
    forcedCommit: Boolean
  )(implicit responseSchema: IntoSchema[Response], ec: ExecutionContext): Future[Response] =
    result match {
      case Success(value) =>
        attempt(invocation.finishAsync(responseSchema.toTyped(value), forcedCommit)).map(_ => value)
      case Failure(error) => Future.failed(error)
    }

  // Drops the invocation in its own scope, then closes the scope and releases the parent.
  private def close[Response](
    invocation: LiveCustomDurableInvocation,
    scope: OpenScope,
    parent: Option[OpenScope],
    result: Try[Response]
  ): Try[Response] = {
    val closed = Try {
      try invocation.drop()
      finally {
        openScopes -= scope
        CustomInvocationContext.close(scope.token)
      }
    }
    parent.foreach(_.childClosed())
    (result, closed) match {
      case (Success(_), Failure(error)) => Failure(error)
      case _                            => result
    }
  }

  /** An open asynchronous operation and the nested operations it waits for. */
  private final class OpenScope(val token: CustomInvocationToken) {
    private var openChildren = 0
    private var finished     = Option.empty[Promise[Unit]]

    def childStarted(): Unit = openChildren += 1

    def childClosed(): Unit = {
      openChildren -= 1
      settle()
    }

    def bodyFinished(): Future[Unit] = {
      val done = Promise[Unit]()
      finished = Some(done)
      settle()
      done.future
    }

    private def settle(): Unit =
      if (openChildren == 0) finished.foreach(_.trySuccess(()))
  }

  private val openScopes        = mutable.ArrayBuffer.empty[OpenScope]
  private var synchronousBodies = 0

  private def openScope(token: CustomInvocationToken): Option[OpenScope] =
    openScopes.find(_.token eq token)

  private def attempt[A](future: => Future[A]): Future[A] =
    try future
    catch { case NonFatal(error) => Future.failed(error) }

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
