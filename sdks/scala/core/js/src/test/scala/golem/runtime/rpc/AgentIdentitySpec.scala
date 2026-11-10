/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 */

package golem.runtime.rpc

import golem.HostApi
import golem.config.ConfigOverride
import golem.host.SchemaWireInterop
import golem.host.js.schema.{JsSchemaValueTree, JsTypedAgentConfigValue, JsUuid}
import golem.reflection.{AgentRpcError, DynamicAgentClient, GolemReflectError, ParsedAgentId, ParsedAgentIdParts}
import golem.schema.SchemaValue
import golem.schema.wire.SchemaWire
import zio.test._

import java.util.UUID
import scala.scalajs.js
import scala.scalajs.js.JSConverters._
import scala.util.Try

object AgentIdentitySpec extends ZIOSpecDefault {
  private val phantom                    = UUID.fromString("e29089a4-1c5c-45ac-a003-d87b85f13e6c")
  private val value                      = SchemaValue.StringValue("constructor")
  private def payload: JsSchemaValueTree =
    SchemaWireInterop.valueTreeToJs(SchemaWire.schemaValueToWit(value))

  private final class Host {
    var registered: js.Any                 = js.Dynamic.literal(agentType = js.Dynamic.literal(typeName = "DisplayName"))
    var makeError: Option[String]          = None
    var parseError: Option[String]         = None
    var connectionError: Option[Throwable] = None
    var calls                              = List.empty[String]
    var madePayload: js.Any                = js.undefined
    var madePhantom: js.Any                = js.undefined
    var rpcPayload: js.Any                 = js.undefined
    var rpcPhantom: js.Any                 = js.undefined
    var config                             = js.Array[JsTypedAgentConfigValue]()
    val parsedPayload                      = payload

    def install(): Unit =
      js.Dynamic.global.globalThis.updateDynamic("__golemScalaTestHost")(
        js.Dynamic.literal(
          getAgentType = ((name: String) => {
            calls :+= s"lookup:$name"
            registered
          }): js.Function1[String, js.Any],
          makeAgentId = (
            (name: String, input: js.Any, id: js.Any) => {
              calls :+= s"make:$name"
              madePayload = input
              madePhantom = id
              makeError.foreach(error => throw js.JavaScriptException(error))
              "server-id"
            }
          ): js.Function3[String, js.Any, js.Any, String],
          parseAgentId = ((id: String) => {
            calls :+= s"parse:$id"
            parseError.foreach(error => throw js.JavaScriptException(error))
            js.Array[js.Any]("DisplayName", js.Dynamic.literal(value = parsedPayload), JsUuid.fromUuid(phantom))
          }): js.Function1[String, js.Array[js.Any]],
          constructWasmRpc = (
            (name: String, input: js.Any, id: js.Any, overrides: js.Array[JsTypedAgentConfigValue]) => {
              calls :+= s"rpc:$name"
              rpcPayload = input
              rpcPhantom = id
              config = overrides
              connectionError.foreach(error => throw error)
            }
          ): js.Function4[String, js.Any, js.Any, js.Array[JsTypedAgentConfigValue], Unit]
        )
      )
  }

  override def spec = suite("AgentIdentitySpec")(
    test("resolution uses the registered display name and preserves payload, phantom and config") {
      val host = new Host
      host.install()
      val input  = payload
      val result =
        RemoteAgentClient.resolve("lookup-name", input, Some(phantom), List(ConfigOverride(List("limit"), 7)))
      assertTrue(
        result.exists(client => client.agentTypeName == "DisplayName" && client.agentId == "server-id"),
        result.exists(_.metadata.asInstanceOf[js.Any] == host.registered),
        host.calls == List("lookup:lookup-name", "make:DisplayName", "rpc:DisplayName"),
        host.madePayload == input,
        host.rpcPayload == input,
        JsUuid.toUuid(host.madePhantom.asInstanceOf[JsUuid]) == phantom,
        JsUuid.toUuid(host.rpcPhantom.asInstanceOf[JsUuid]) == phantom,
        host.config.length == 1,
        host.config(0).path.toList == List("limit")
      )
    },
    test("resolution without a phantom keeps it absent and passes empty config") {
      val host = new Host
      host.install()
      val result = RemoteAgentClient.resolve("lookup-name", payload)
      assertTrue(result.isRight, js.isUndefined(host.madePhantom), js.isUndefined(host.rpcPhantom), host.config.isEmpty)
    },
    test("missing registered types stop before identity and RPC creation") {
      val host = new Host
      host.registered = js.undefined
      host.install()
      val result = RemoteAgentClient.resolve("missing", payload)
      assertTrue(
        result == Left("Agent type 'missing' is not registered on this host"),
        host.calls == List("lookup:missing")
      )
    },
    test("identity failures stop RPC creation and keep facade error mappings") {
      val host = new Host
      host.makeError = Some("invalid constructor")
      host.install()
      val result = RemoteAgentClient.resolve("lookup-name", payload)
      val calls  = host.calls
      assertTrue(
        result == Left("invalid constructor"),
        calls == List("lookup:lookup-name", "make:DisplayName"),
        HostApi.makeAgentId("DisplayName", payload, None) == Left("invalid constructor"),
        ParsedAgentId.create("DisplayName", value) == Left(GolemReflectError.Identity("invalid constructor"))
      )
    },
    test("parse facades preserve the constructor payload and phantom") {
      val host = new Host
      host.install()
      assertTrue(
        HostApi.parseAgentId("server-id") == Right(HostApi.AgentIdParts("DisplayName", Some(phantom))),
        HostApi.parseAgentIdRaw("server-id").exists(_.payload.value == host.parsedPayload),
        ParsedAgentId("server-id").parts == Right(ParsedAgentIdParts("DisplayName", value, Some(phantom)))
      )
    },
    test("parse failures keep host and reflection error mappings") {
      val host = new Host
      host.parseError = Some("invalid id")
      host.install()
      assertTrue(
        HostApi.parseAgentId("invalid") == Left("invalid id"),
        HostApi.parseAgentIdRaw("invalid") == Left("invalid id"),
        ParsedAgentId("invalid").parts == Left(GolemReflectError.Identity("invalid id"))
      )
    },
    test("fallible connection prepares phantom and config without registry lookup or ID creation") {
      val host = new Host
      host.install()
      val input  = payload
      val result =
        AgentIdentity.createConnection("RawName", input, Some(phantom), List(ConfigOverride(List("limit"), 7)))
      assertTrue(
        result.isRight,
        host.calls == List("rpc:RawName"),
        host.rpcPayload == input,
        JsUuid.toUuid(host.rpcPhantom.asInstanceOf[JsUuid]) == phantom,
        host.config.length == 1,
        host.config(0).path.toList == List("limit")
      )
    },
    test("dynamic binding parses identity and connects without registry lookup or ID creation") {
      val host = new Host
      host.install()
      val id     = ParsedAgentId("server-id")
      val result = DynamicAgentClient.fromAgentId(id)
      assertTrue(
        result.exists(_.agentId.contains(id)),
        host.calls == List("parse:server-id", "rpc:DisplayName"),
        JsUuid.toUuid(host.rpcPhantom.asInstanceOf[JsUuid]) == phantom,
        host.config.isEmpty
      )
    },
    test("reflected connection maps host failure while typed connection still throws") {
      val host  = new Host
      val error = js.JavaScriptException(js.Dynamic.literal(tag = "denied", `val` = "no permission"))
      host.connectionError = Some(error)
      host.install()
      val reflected      = DynamicAgentClient.ephemeral("RawName", value)
      val reflectedCalls = host.calls
      val typed          = Try(RemoteAgentClient.resolve("lookup-name", payload))
      assertTrue(
        reflected == Left(GolemReflectError.Remote(AgentRpcError.Denied("no permission"))),
        reflectedCalls == List("rpc:RawName"),
        typed.failed.toOption.contains(error),
        js.isUndefined(host.rpcPhantom)
      )
    },
    test("reflected connection retains NonFatal unknown-error mapping") {
      val host = new Host
      host.connectionError = Some(new IllegalStateException("connection preparation failed"))
      host.install()
      assertTrue(
        DynamicAgentClient.ephemeral("RawName", value) ==
          Left(GolemReflectError.Remote(AgentRpcError.Unknown("unknown", Some("connection preparation failed"))))
      )
    }
  ) @@ TestAspect.sequential
}
