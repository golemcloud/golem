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

package golem.runtime.rpc.host

import golem.{Datetime, FutureInterop, InvocationId}
import golem.host.js.{JsAgentId, JsDatetime}
import golem.host.js.schema.{JsAgentError, JsSchemaValueTree, JsTypedAgentConfigValue, JsUuid}
import golem.runtime.rpc.{InvocationMetadata, InvocationResult}

import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
import scala.scalajs.js
import scala.util.control.NonFatal
import scala.scalajs.js.annotation.{JSImport, JSName}

// ---------------------------------------------------------------------------
// `golem:agent/host@2.0.0` `rpc-error` JS facade (schema-native variant).
//
// The `remote-agent-error` payload carries the schema `JsAgentError` (whose
// `custom-error` is a `JsTypedSchemaValue`); the string-carrying cases follow
// the wasm-rquickjs `{ tag, val }` shape.
// ---------------------------------------------------------------------------

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

private[golem] object WasmRpcApi {

  /** A client of the keyed agent of the constructor parameters (`upsert`). */
  def upsert(agentTypeName: String, constructorPayload: JsSchemaValueTree): Either[RpcError, WasmRpcClient] =
    try Right(new WasmRpcClient(RawWasmRpc.upsert(agentTypeName, constructorPayload)))
    catch { case js.JavaScriptException(error) => Left(decodeRpcError(error)) }

  /** A client of the existing agent with the given ID (`get-by-id`). */
  def getById(agentTypeName: String, agentId: JsAgentId): Either[RpcError, WasmRpcClient] =
    try Right(new WasmRpcClient(RawWasmRpc.getById(agentTypeName, agentId)))
    catch { case js.JavaScriptException(error) => Left(decodeRpcError(error)) }

  /** Creates a durable agent and returns its ID and a client (`create`). */
  def create(
    agentTypeName: String,
    constructorPayload: JsSchemaValueTree,
    keyed: Boolean,
    agentConfig: js.Array[JsTypedAgentConfigValue],
    env: Map[String, String]
  ): Future[Either[RpcError, (JsAgentId, WasmRpcClient)]] = {
    val options = js.Dynamic
      .literal(
        "keyed"       -> keyed,
        "agentConfig" -> agentConfig,
        "env"         -> js.Array(env.toSeq.map { case (key, value) => js.Tuple2(key, value) }*)
      )
      .asInstanceOf[RawCreateOptions]
    rpcFuture(RawWasmRpc.create(agentTypeName, constructorPayload, options))
      .map(_.map(created => (created.agentId, new WasmRpcClient(created.client))))
  }

  def bindSelf(): Either[RpcError, (JsAgentId, WasmRpcClient)] =
    try {
      val bound = RawWasmRpc.bindSelf()
      Right((bound._1, new WasmRpcClient(bound._2)))
    } catch { case js.JavaScriptException(error) => Left(decodeRpcError(error)) }

  /**
   * Awaits a host promise whose rejection is an `rpc-error`. A synchronous
   * throw of the host call becomes a `Left` too.
   */
  private def rpcFuture[A](promise: => js.Promise[A]): Future[Either[RpcError, A]] =
    (try FutureInterop.fromPromise(promise)
    catch { case NonFatal(error) => Future.failed(error) })
      .map[Either[RpcError, A]](Right(_))
      .recover { case js.JavaScriptException(error) => Left(decodeRpcError(error)) }

  private def datetimeToJs(datetime: Datetime): JsDatetime = {
    val totalMs            = datetime.epochMillis
    val truncatedWholeMs   = if (totalMs < 0.0) Math.ceil(totalMs) else Math.floor(totalMs)
    val wholeMs            = js.BigInt(truncatedWholeMs)
    val millisPerSecond    = js.BigInt(1000)
    val truncatedSeconds   = wholeMs / millisPerSecond
    val remainingWholeMs   = (wholeMs % millisPerSecond).toString.toInt
    val remainingPartialMs = totalMs  % 1.0
    val truncatedNanos     = remainingWholeMs * 1000000 + (remainingPartialMs * 1e6).toInt
    val (seconds, nanos)   =
      if (truncatedNanos < 0) (truncatedSeconds - js.BigInt(1), truncatedNanos + 1000000000)
      else (truncatedSeconds, truncatedNanos)
    JsDatetime(seconds, nanos)
  }

  private[golem] def decodeRpcError(thrown: Any): RpcError = {
    // Read the discriminator defensively: a thrown value that is not the
    // expected `{ tag, val }` JS object (e.g. a bare string or a foreign error)
    // must degrade to `unknown` rather than triggering a hard cast failure.
    val rawTag =
      try thrown.asInstanceOf[js.Dynamic].selectDynamic("tag")
      catch { case _: Throwable => (js.undefined: js.Any) }

    if (js.typeOf(rawTag) == "string") {
      val value = thrown.asInstanceOf[JsRpcError]
      rawTag.asInstanceOf[String] match {
        case "protocol-error" =>
          RpcError("protocol-error", Some(value.asInstanceOf[JsRpcErrorString].value))
        case "denied" =>
          RpcError("denied", Some(value.asInstanceOf[JsRpcErrorString].value))
        case "not-found" =>
          RpcError("not-found", Some(value.asInstanceOf[JsRpcErrorString].value))
        case "remote-internal-error" =>
          RpcError("remote-internal-error", Some(value.asInstanceOf[JsRpcErrorString].value))
        case "remote-agent-error" =>
          RpcError("remote-agent-error", None, Some(value.asInstanceOf[JsRpcErrorRemoteAgent].value))
        case "conflict" =>
          RpcError("conflict", Some(value.asInstanceOf[JsRpcErrorString].value))
        case "cancelled" =>
          RpcError("cancelled", Some(value.asInstanceOf[JsRpcErrorString].value))
        case "cannot-cancel-running" =>
          RpcError("cannot-cancel-running", Some(value.asInstanceOf[JsRpcErrorString].value))
        case other =>
          RpcError(other, None)
      }
    } else {
      RpcError("unknown", Some(String.valueOf(thrown)))
    }
  }

  final class WasmRpcClient private[host] (private val underlying: js.Object) {
    def drop(): Unit = {
      val symbol = js.Dynamic.global.Symbol.selectDynamic("dispose")
      val drop   = js.Dynamic.global.Reflect.applyDynamic("get")(underlying, symbol)
      drop.applyDynamic("call")(underlying)
      ()
    }

    private def metadata(raw: RawInvocationMetadata): InvocationMetadata = {
      val agentId = JsAgentId.toAgentId(raw.agentId)
      InvocationMetadata(agentId, InvocationId(agentId, JsUuid.toUuid(raw.invocationId.id)))
    }

    /**
     * The ID of the agent this client connects to. On a client of `upsert` the
     * host creates the agent when it is missing.
     */
    def agentId(): Future[Either[RpcError, JsAgentId]] =
      rpcFuture(raw.agentId())

    /**
     * Invokes the method and awaits its result. `scopeCard` is a raw
     * `permission-card` resource that the host borrows for the duration of this
     * call, or `js.undefined`.
     */
    def invokeAndAwait(
      functionName: String,
      input: JsSchemaValueTree,
      scopeCard: js.Any
    ): Future[Either[RpcError, InvocationResult[js.UndefOr[JsSchemaValueTree]]]] =
      rpcFuture(raw.invokeAndAwait(functionName, input, scopeCard))
        .map(_.map(value => InvocationResult(metadata(value.metadata), value.result)))

    /**
     * Triggers the method and returns its handle once the target accepted it.
     */
    def trigger(functionName: String, input: JsSchemaValueTree): Future[Either[RpcError, RawInvocation]] =
      rpcFuture(raw.trigger(functionName, input))

    /**
     * Schedules the method and returns its handle once the target accepted it.
     */
    def schedule(
      datetime: Datetime,
      functionName: String,
      input: JsSchemaValueTree
    ): Future[Either[RpcError, RawInvocation]] =
      rpcFuture(raw.schedule(datetimeToJs(datetime), functionName, input))

    private def raw: RawWasmRpc =
      underlying.asInstanceOf[RawWasmRpc]
  }

  final case class RpcError(
    kind: String,
    message: Option[String] = None,
    agentError: Option[JsAgentError] = None
  ) {
    override def toString: String =
      agentError match {
        case Some(err) =>
          val rendered =
            try js.JSON.stringify(err.asInstanceOf[js.Any])
            catch { case _: Throwable => String.valueOf(err) }
          s"$kind: $rendered"
        case None => message.fold(kind)(text => s"$kind: $text")
      }
  }

  @js.native
  private[rpc] trait RawInvocationMetadata extends js.Object {
    def agentId: JsAgentId                  = js.native
    def invocationId: RawInvocationIdRecord = js.native
  }

  @js.native
  private[golem] trait RawInvocationIdRecord extends js.Object {
    def owner: RawInvocationOwner = js.native
    def id: JsUuid                = js.native
  }

  @js.native
  private[golem] trait RawInvocationOwner extends js.Object {
    def tag: String                  = js.native
    @JSName("val") def value: js.Any = js.native
  }

  /** The invocation ID of a `golem:core` `invocation-id` record. */
  private[golem] def invocationIdFromJs(raw: RawInvocationIdRecord): InvocationId =
    raw.owner.tag match {
      case "agent" =>
        InvocationId(JsAgentId.toAgentId(raw.owner.value.asInstanceOf[JsAgentId]), JsUuid.toUuid(raw.id))
      case other =>
        throw new IllegalStateException(s"protocol-error: invocation $other owner is not an agent")
    }

  private def invocationIdToJs(id: InvocationId): RawInvocationIdRecord =
    js.Dynamic
      .literal(
        "owner" -> js.Dynamic.literal("tag" -> "agent", "val" -> JsAgentId.fromAgentId(id.agentId)),
        "id"    -> JsUuid.fromUuid(id.uuid)
      )
      .asInstanceOf[RawInvocationIdRecord]

  @js.native
  private[rpc] trait RawInvocationResultWithMetadata extends js.Object {
    def metadata: RawInvocationMetadata       = js.native
    def result: js.UndefOr[JsSchemaValueTree] = js.native
  }

  /** The `invocation` resource of `golem:agent/host`. */
  @js.native
  @JSImport("golem:agent/host@2.0.0", "Invocation")
  private[golem] class RawInvocation extends js.Object {
    def id(): RawInvocationIdRecord                         = js.native
    def status(): js.Promise[String]                        = js.native
    def await_(): js.Promise[js.UndefOr[JsSchemaValueTree]] = js.native
    def cancel(): js.Promise[Boolean]                       = js.native
  }

  @js.native
  @JSImport("golem:agent/host@2.0.0", "Invocation")
  private object RawInvocation extends js.Object {
    def get(id: RawInvocationIdRecord): RawInvocation = js.native
  }

  /**
   * Rebuilds the handle of the invocation with the given ID. Makes no remote
   * call.
   */
  private[golem] def getInvocation(id: InvocationId): RawInvocation =
    RawInvocation.get(invocationIdToJs(id))

  /**
   * Awaits a host call of an `invocation` resource whose rejection is an
   * `rpc-error`.
   */
  private[golem] def invocationCall[A](promise: => js.Promise[A]): Future[Either[RpcError, A]] =
    rpcFuture(promise)

  @js.native
  private[rpc] trait RawCreateOptions extends js.Object

  @js.native
  private[rpc] trait RawCreated extends js.Object {
    def client: RawWasmRpc = js.native
    def agentId: JsAgentId = js.native
  }

  @js.native
  @JSImport("golem:agent/host@2.0.0", "WasmRpc")
  private[rpc] final class RawWasmRpc extends js.Object {
    def agentId(): js.Promise[JsAgentId] = js.native
    def invokeAndAwait(
      methodName: String,
      input: JsSchemaValueTree,
      scopeCard: js.Any
    ): js.Promise[RawInvocationResultWithMetadata]                                       = js.native
    def trigger(methodName: String, input: JsSchemaValueTree): js.Promise[RawInvocation] = js.native
    def schedule(
      scheduledTime: JsDatetime,
      methodName: String,
      input: JsSchemaValueTree
    ): js.Promise[RawInvocation] = js.native
  }

  @js.native
  @JSImport("golem:agent/host@2.0.0", "WasmRpc")
  private object RawWasmRpc extends js.Object {
    def upsert(agentTypeName: String, constructorPayload: JsSchemaValueTree): RawWasmRpc = js.native
    def getById(agentTypeName: String, agentId: JsAgentId): RawWasmRpc                   = js.native
    def create(
      agentTypeName: String,
      constructorPayload: JsSchemaValueTree,
      options: RawCreateOptions
    ): js.Promise[RawCreated]                        = js.native
    def bindSelf(): js.Tuple2[JsAgentId, RawWasmRpc] = js.native
  }
}
