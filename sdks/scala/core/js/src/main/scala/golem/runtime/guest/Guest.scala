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

package golem.runtime.guest

import golem.host.js._
import golem.runtime.autowire.{AgentRegistry, WitValueBuilder}
import golem.FutureInterop
import zio.blocks.schema.json.Json

import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
import scala.scalajs.js
import scala.scalajs.js.annotation.{JSExport, JSExportTopLevel}
import scala.scalajs.js.typedarray.Uint8Array

/**
 * Scala.js implementation of the mandatory Golem JS guest exports.
 *
 * The Scala application code is responsible for registering agent definitions
 * into AgentRegistry at module initialization time (typically via
 * `AgentImplementation.registerClass[...]` calls in a small exported value
 * whose initializer runs on module load).
 */
object Guest {
  private var resolved: js.UndefOr[Resolved]                   = js.undefined
  private var initializationPrincipal: Option[golem.Principal] = None
  private final case class Resolved(defn: golem.runtime.autowire.AgentDefinition[Any], instance: Any)

  private def invalidType(message: String): JsAgentError =
    JsAgentError.invalidType(message)

  private def invalidAgentId(message: String): JsAgentError =
    JsAgentError.invalidAgentId(message)

  private def customError(message: String): JsAgentError = {
    val witValue: JsWitValue = WitValueBuilder.build(
      golem.data.DataType.StringType,
      golem.data.DataValue.StringValue(message)
    ) match {
      case Left(_)  => JsWitValue(js.Array(JsWitNode.primString(message)))
      case Right(v) => v
    }

    val witType: JsWitType = JsWitType(
      js.Array(
        JsNamedWitTypeNode(JsWitTypeNode.primStringType)
      )
    )

    JsAgentError.customError(JsValueAndType(witValue, witType))
  }

  private def asAgentError(err: Any, fallbackTag: String): JsAgentError =
    if (err == null) customError("null")
    else {
      val dyn       = err.asInstanceOf[js.Dynamic]
      val hasTagVal =
        try !js.isUndefined(dyn.selectDynamic("tag")) && !js.isUndefined(dyn.selectDynamic("val"))
        catch { case _: Throwable => false }

      if (hasTagVal) dyn.asInstanceOf[JsAgentError]
      else
        err match {
          case s: String =>
            fallbackTag match {
              case "invalid-input"    => JsAgentError.invalidInput(s)
              case "invalid-method"   => JsAgentError.invalidMethod(s)
              case "invalid-type"     => JsAgentError.invalidType(s)
              case "invalid-agent-id" => JsAgentError.invalidAgentId(s)
              case _                  => customError(s)
            }
          case other => customError(String.valueOf(other))
        }
    }

  private def isJsAgentError(err: Any): Boolean =
    try {
      val dyn = err.asInstanceOf[js.Dynamic]
      !js.isUndefined(dyn.selectDynamic("tag")) && !js.isUndefined(dyn.selectDynamic("val"))
    } catch { case _: Throwable => false }

  private def normalizeMethodName(methodName: String): String =
    if (methodName.contains(".{") && methodName.endsWith("}")) {
      val start = methodName.indexOf(".{") + 2
      methodName.substring(start, methodName.length - 1)
    } else methodName

  private def initialize(agentTypeName: String, input: js.Dynamic, principal: js.Dynamic): js.Promise[Unit] =
    if (!js.isUndefined(resolved)) {
      js.Promise.reject(customError("Agent is already initialized in this container")).asInstanceOf[js.Promise[Unit]]
    } else {
      AgentRegistry.get(agentTypeName) match {
        case None =>
          js.Promise.reject(invalidType("Invalid agent '" + agentTypeName + "'")).asInstanceOf[js.Promise[Unit]]
        case Some(defnAny) =>
          val scalaPrincipal = PrincipalConverter.fromJs(principal)
          initializationPrincipal = Some(scalaPrincipal)
          // Avoid calling `.then` directly (Scala 3 scaladoc / TASTy reader can error on it during `doc`).
          val initPromise              = defnAny.initializeAny(input.asInstanceOf[JsDataValue], scalaPrincipal)
          val initFuture: Future[Unit] =
            FutureInterop
              .fromPromise(initPromise)
              .map { inst =>
                resolved = Resolved(defnAny, inst)
                ()
              }
              .recoverWith {
                // Only recover SDK-level errors (JsAgentError with {tag, val} shape).
                // User code errors must propagate as WASM traps.
                case js.JavaScriptException(err) if isJsAgentError(err) =>
                  Future.failed(scala.scalajs.js.JavaScriptException(err))
              }
          FutureInterop.toPromise(initFuture).asInstanceOf[js.Promise[Unit]]
      }
    }

  private def invoke(methodName: String, input: js.Dynamic, principal: js.Dynamic): js.Promise[js.Dynamic] =
    if (js.isUndefined(resolved)) {
      js.Promise.reject(invalidAgentId("Agent is not initialized")).asInstanceOf[js.Promise[js.Dynamic]]
    } else {
      val r              = resolved.asInstanceOf[Resolved]
      val mn             = normalizeMethodName(methodName)
      val scalaPrincipal = PrincipalConverter.fromJs(principal)
      val onRejected: js.Function1[Any, js.Thenable[js.Dynamic]] =
        js.Any.fromFunction1 { (err: Any) =>
          // Only catch SDK-level errors (JsAgentError). User code errors must propagate
          // as unhandled rejections so they become WASM traps, enabling atomic block retries.
          if (isJsAgentError(err))
            js.Promise.reject(err).asInstanceOf[js.Thenable[js.Dynamic]]
          else
            throw (err match {
              case t: Throwable => t
              case other        => js.JavaScriptException(other)
            })
        }
      r.defn
        .invokeAny(r.instance, mn, input.asInstanceOf[JsDataValue], scalaPrincipal)
        .asInstanceOf[js.Promise[js.Dynamic]]
        .`catch`[js.Dynamic](onRejected)
    }

  private def getDefinition(): js.Promise[js.Any] =
    if (js.isUndefined(resolved)) {
      js.Promise.reject(invalidAgentId("Agent is not initialized")).asInstanceOf[js.Promise[js.Any]]
    } else {
      js.Promise.resolve[js.Any](resolved.asInstanceOf[Resolved].defn.agentType.asInstanceOf[js.Any])
    }

  private def discoverAgentTypes(): js.Promise[js.Array[js.Any]] =
    try {
      val arr = new js.Array[js.Any]()
      AgentRegistry.all.foreach(d => arr.push(d.agentType.asInstanceOf[js.Any]))
      js.Promise.resolve[js.Array[js.Any]](arr)
    } catch {
      case t: Throwable =>
        js.Promise.reject(asAgentError(t.toString, "custom-error")).asInstanceOf[js.Promise[js.Array[js.Any]]]
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

  @JSExportTopLevel("guest")
  val guest: js.Dynamic =
    js.Dynamic.literal(
      "initialize" -> ((agentTypeName: String, input: js.Dynamic, principal: js.Dynamic) =>
        initialize(agentTypeName, input, principal)
      ),
      "invoke" -> ((methodName: String, input: js.Dynamic, principal: js.Dynamic) =>
        invoke(methodName, input, principal)
      ),
      "getDefinition"      -> (() => getDefinition()),
      "discoverAgentTypes" -> (() => discoverAgentTypes())
    )

  @JSExportTopLevel("saveSnapshot")
  object SaveSnapshot {
    @JSExport
    def save(): js.Promise[JsSnapshot] =
      if (js.isUndefined(resolved)) {
        FutureInterop.toPromise(Future.successful(JsSnapshot(new Uint8Array(0), "application/octet-stream")))
      } else {
        val r = resolved.asInstanceOf[Resolved]
        r.defn.snapshotHandlers match {
          case Some(handlers) =>
            FutureInterop.toPromise(
              handlers.save(r.instance).map { payload =>
                val principal      = initializationPrincipal.getOrElse(golem.Principal.Anonymous)
                val principalBytes = PrincipalConverter.toJson(principal)
                if (payload.mimeType == "application/json") {
                  val stateJson        = new String(payload.bytes, "UTF-8")
                  val principalJsonStr = new String(principalBytes, "UTF-8")
                  val envelope         = s"""{"version":1,"principal":$principalJsonStr,"state":$stateJson}"""
                  JsSnapshot(toUint8Array(envelope.getBytes("UTF-8")), "application/json")
                } else {
                  val totalLength  = 1 + 4 + principalBytes.length + payload.bytes.length
                  val fullSnapshot = new Array[Byte](totalLength)
                  fullSnapshot(0) = 2.toByte
                  fullSnapshot(1) = ((principalBytes.length >>> 24) & 0xff).toByte
                  fullSnapshot(2) = ((principalBytes.length >>> 16) & 0xff).toByte
                  fullSnapshot(3) = ((principalBytes.length >>> 8) & 0xff).toByte
                  fullSnapshot(4) = (principalBytes.length & 0xff).toByte
                  System.arraycopy(principalBytes, 0, fullSnapshot, 5, principalBytes.length)
                  System.arraycopy(payload.bytes, 0, fullSnapshot, 5 + principalBytes.length, payload.bytes.length)
                  JsSnapshot(toUint8Array(fullSnapshot), "application/octet-stream")
                }
              }
            )
          case None =>
            FutureInterop.toPromise(Future.successful(JsSnapshot(new Uint8Array(0), "application/octet-stream")))
        }
      }
  }

  @JSExportTopLevel("loadSnapshot")
  object LoadSnapshot {
    @JSExport
    def load(snapshot: JsSnapshot): js.Promise[Unit] =
      if (js.isUndefined(resolved)) {
        FutureInterop.toPromise(Future.successful(()))
      } else {
        val r = resolved.asInstanceOf[Resolved]
        r.defn.snapshotHandlers match {
          case Some(handlers) =>
            val bytes                   = fromUint8Array(snapshot.payload)
            val (principal, agentState) =
              if (snapshot.mimeType == "application/json") {
                Json.parse(bytes) match {
                  case Right(envelope) =>
                    val p = envelope
                      .get("principal")
                      .one
                      .toOption
                      .flatMap(pJson => PrincipalConverter.fromJson(pJson.printBytes).toOption)
                      .getOrElse(initializationPrincipal.getOrElse(golem.Principal.Anonymous))
                    val stateBytes = envelope
                      .get("state")
                      .one
                      .toOption
                      .map(_.printBytes)
                      .getOrElse(bytes)
                    (p, stateBytes)
                  case Left(_) =>
                    (initializationPrincipal.getOrElse(golem.Principal.Anonymous), bytes)
                }
              } else if (bytes.nonEmpty) {
                val version = bytes(0) & 0xff
                version match {
                  case 1 =>
                    val p = initializationPrincipal.getOrElse(golem.Principal.Anonymous)
                    (p, bytes.drop(1))
                  case 2 =>
                    if (bytes.length < 5)
                      throw new RuntimeException("Version 2 snapshot too short for principal length")
                    val principalLen =
                      ((bytes(1) & 0xff) << 24) | ((bytes(2) & 0xff) << 16) |
                        ((bytes(3) & 0xff) << 8) | (bytes(4) & 0xff)
                    val principalEnd = 5 + principalLen
                    if (bytes.length < principalEnd)
                      throw new RuntimeException("Version 2 snapshot too short for principal data")
                    val principalBytes = java.util.Arrays.copyOfRange(bytes, 5, principalEnd)
                    val p              = PrincipalConverter.fromJson(principalBytes) match {
                      case Right(v)  => v
                      case Left(err) => throw new RuntimeException(s"Failed to deserialize principal: $err")
                    }
                    (p, bytes.drop(principalEnd))
                  case other =>
                    throw new RuntimeException(s"Unsupported snapshot version: $other")
                }
              } else {
                (initializationPrincipal.getOrElse(golem.Principal.Anonymous), bytes)
              }
            initializationPrincipal = Some(principal)
            FutureInterop.toPromise(
              handlers.load(r.instance, agentState).map { newInstance =>
                resolved = Resolved(r.defn, newInstance)
              }
            )
          case None =>
            FutureInterop.toPromise(Future.successful(()))
        }
      }
  }
}
