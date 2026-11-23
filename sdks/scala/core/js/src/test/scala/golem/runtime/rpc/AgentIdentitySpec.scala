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

import golem.AgentId
import golem.config.ConfigOverride
import golem.host.SchemaWireInterop
import golem.host.js.JsAgentId
import golem.host.js.schema.{JsSchemaValueTree, JsTypedAgentConfigValue}
import golem.reflection.{DynamicAgentClient, GolemReflectError}
import golem.schema.SchemaValue
import golem.schema.wire.SchemaWire
import zio.ZIO
import zio.test._

import java.util.UUID
import scala.concurrent.Future
import scala.scalajs.js

object AgentIdentitySpec extends ZIOSpecDefault {
  private val selfId    = AgentId(UUID.fromString("0a1b2c3d-4e5f-8a6b-9c7d-0e1f2a3b4c5d"))
  private val createdId = AgentId(UUID.fromString("1a1b2c3d-4e5f-8a6b-9c7d-0e1f2a3b4c5d"))
  private val lookedUp  = AgentId(UUID.fromString("2a1b2c3d-4e5f-8a6b-9c7d-0e1f2a3b4c5d"))

  private val value                      = SchemaValue.StringValue("constructor")
  private def payload: JsSchemaValueTree =
    SchemaWireInterop.valueTreeToJs(SchemaWire.schemaValueToWit(value))

  private def rpcError(tag: String, detail: String): js.JavaScriptException =
    js.JavaScriptException(js.Dynamic.literal(tag = tag, `val` = detail))

  private def registeredType(name: String): js.Any =
    js.Dynamic.literal(agentType = js.Dynamic.literal(typeName = name))

  private final class Host {
    var registered: js.Any                = registeredType("DisplayName")
    var typeOfId: js.Any                  = registeredType("DisplayName")
    var bindingError: Option[Throwable]   = None
    var upsertError: Option[Throwable]    = None
    var getByIdError: Option[Throwable]   = None
    var createError: Option[Throwable]    = None
    var agentIdError: Option[Throwable]   = None
    var drops                             = 0
    var calls                             = List.empty[String]
    var upsertPayload: js.Any             = js.undefined
    var createPayload: js.Any             = js.undefined
    var createOptions: js.Dynamic         = js.Dynamic.literal()
    var getByIdArgument: Option[AgentId]  = None
    var typeOfIdArgument: Option[AgentId] = None

    def install(): Unit =
      js.Dynamic.global.globalThis.updateDynamic("__golemScalaTestHost")(
        js.Dynamic.literal(
          bindSelf = (() => {
            calls :+= "bind-self"
            bindingError.foreach(error => throw error)
            val rpc = js.Dynamic.literal()
            js.Dynamic.global.Reflect.set(
              rpc,
              js.Dynamic.global.Symbol.dispose,
              (() => { drops += 1 }): js.Function0[Unit]
            )
            js.Array[js.Any](JsAgentId.fromAgentId(selfId), rpc)
          }): js.Function0[js.Array[js.Any]],
          getAgentType = ((name: String) => {
            calls :+= s"lookup:$name"
            registered
          }): js.Function1[String, js.Any],
          getAgentTypeByAgentId = ((id: JsAgentId) => {
            calls :+= "type-of-id"
            typeOfIdArgument = Some(JsAgentId.toAgentId(id))
            typeOfId
          }): js.Function1[JsAgentId, js.Any],
          wasmRpcUpsert = (
            (name: String, input: js.Any) => {
              calls :+= s"upsert:$name"
              upsertPayload = input
              upsertError.foreach(error => throw error)
            }
          ): js.Function2[String, js.Any, Unit],
          wasmRpcGetById = (
            (name: String, id: JsAgentId) => {
              calls :+= s"get-by-id:$name"
              getByIdArgument = Some(JsAgentId.toAgentId(id))
              getByIdError.foreach(error => throw error)
            }
          ): js.Function2[String, JsAgentId, Unit],
          wasmRpcCreate = (
            (name: String, input: js.Any, options: js.Dynamic) => {
              calls :+= s"create:$name"
              createPayload = input
              createOptions = options
              createError match {
                case Some(error) => js.Promise.reject(error.asInstanceOf[js.JavaScriptException].exception)
                case None        => js.Promise.resolve[js.Any](JsAgentId.fromAgentId(createdId))
              }
            }
          ): js.Function3[String, js.Any, js.Dynamic, js.Promise[js.Any]],
          wasmRpcAgentId = ((_: js.Any) => {
            calls :+= "agent-id"
            agentIdError match {
              case Some(error) => js.Promise.reject(error.asInstanceOf[js.JavaScriptException].exception)
              case None        => js.Promise.resolve[js.Any](JsAgentId.fromAgentId(lookedUp))
            }
          }): js.Function1[js.Any, js.Promise[js.Any]]
        )
      )
  }

  private def failure[A](future: => Future[A]): ZIO[Any, Nothing, Option[AgentRpcError]] =
    ZIO.fromFuture(_ => future).either.map(_.left.toOption.collect { case AgentRpcException(error) => error })

  private def configPaths(options: js.Dynamic): List[List[String]] =
    options.agentConfig.asInstanceOf[js.Array[JsTypedAgentConfigValue]].toList.map(_.path.toList)

  private def env(options: js.Dynamic): List[(String, String)] =
    options.env.asInstanceOf[js.Array[js.Tuple2[String, String]]].toList.map(t => (t._1, t._2))

  override def spec = suite("AgentIdentitySpec")(
    test("self binding keeps the recorded ID and checks its type through the registry") {
      val host = new Host
      host.install()
      val result = AgentIdentity.bindSelf("DisplayName")
      for {
        id <- ZIO.fromFuture(_ => result.toOption.get.agentId())
      } yield assertTrue(
        result.exists(_.agentTypeName == "DisplayName"),
        id == selfId,
        host.typeOfIdArgument.contains(selfId),
        host.calls == List("bind-self", "type-of-id"),
        host.drops == 0
      )
    },
    test("self binding refuses another client type and releases the resource") {
      val host = new Host
      host.install()
      val result = AgentIdentity.bindSelf("Other")
      assertTrue(
        result == Left("invalid-type: self binding expects Other, identity is DisplayName"),
        host.drops == 1
      )
    },
    test("self binding preserves host refusal and releases only an acquired resource") {
      val host = new Host
      host.bindingError = Some(rpcError("denied", "ephemeral owner"))
      host.install()
      val refused = AgentIdentity.bindSelf("DisplayName")
      assertTrue(refused == Left("denied: ephemeral owner"), host.calls == List("bind-self"), host.drops == 0)
    },
    test("upsert calls the upsert host function with the registered name and payload") {
      val host = new Host
      host.install()
      val input  = payload
      val result = AgentIdentity.upsert("lookup-name", input)
      assertTrue(
        result.exists(_.agentTypeName == "DisplayName"),
        host.calls == List("lookup:lookup-name", "upsert:DisplayName"),
        host.upsertPayload == input
      )
    },
    test("the agent ID of an upsert client comes from the agent-id host method once") {
      val host = new Host
      host.install()
      val client = AgentIdentity.upsert("lookup-name", payload).toOption.get
      for {
        first  <- ZIO.fromFuture(_ => client.agentId())
        second <- ZIO.fromFuture(_ => client.agentId())
      } yield assertTrue(first == lookedUp, second == lookedUp, host.calls.count(_ == "agent-id") == 1)
    },
    test("the agent-id host method reports typed RPC errors") {
      val host = new Host
      host.agentIdError = Some(rpcError("conflict", "the key belongs to another agent"))
      host.install()
      val client = AgentIdentity.upsert("lookup-name", payload).toOption.get
      failure(client.agentId()).map(error =>
        assertTrue(error.contains(AgentRpcError.Conflict("the key belongs to another agent")))
      )
    },
    test("upsert of an unregistered type is not-found without a host call") {
      val host = new Host
      host.registered = js.undefined
      host.install()
      val result = AgentIdentity.upsert("missing", payload)
      assertTrue(
        result == Left(AgentRpcError.NotFound("Agent type 'missing' is not registered on this host")),
        host.calls == List("lookup:missing")
      )
    },
    test("upsert reports conflict as a typed RPC error") {
      val host = new Host
      host.upsertError = Some(rpcError("conflict", "agent at the key has another key"))
      host.install()
      assertTrue(
        AgentIdentity.upsert("lookup-name", payload) ==
          Left(AgentRpcError.Conflict("agent at the key has another key"))
      )
    },
    test("keyed create passes keyed, config and env to the create host function") {
      val host = new Host
      host.install()
      val input   = payload
      val options = CreateOptions(keyed = true, List(ConfigOverride(List("limit"), 7)), Map("REGION" -> "eu"))
      for {
        client <- ZIO.fromFuture(_ => AgentIdentity.create("lookup-name", input, options))
        id     <- ZIO.fromFuture(_ => client.agentId())
      } yield assertTrue(
        host.calls == List("lookup:lookup-name", "create:DisplayName"),
        host.createPayload == input,
        host.createOptions.keyed.asInstanceOf[Boolean],
        configPaths(host.createOptions) == List(List("limit")),
        env(host.createOptions) == List("REGION" -> "eu"),
        id == createdId,
        !host.calls.contains("agent-id")
      )
    },
    test("create without keyed passes no key, config or env") {
      val host = new Host
      host.install()
      ZIO.fromFuture(_ => AgentIdentity.create("lookup-name", payload, CreateOptions())).map { _ =>
        assertTrue(
          !host.createOptions.keyed.asInstanceOf[Boolean],
          configPaths(host.createOptions).isEmpty,
          env(host.createOptions).isEmpty
        )
      }
    },
    test("a keyed create of a current key fails with a typed conflict") {
      val host = new Host
      host.createError = Some(rpcError("conflict", "the key is current"))
      host.install()
      failure(AgentIdentity.create("lookup-name", payload, CreateOptions(keyed = true))).map(error =>
        assertTrue(error.contains(AgentRpcError.Conflict("the key is current")))
      )
    },
    test("getById calls the get-by-id host function with the typed ID") {
      val host = new Host
      host.install()
      for {
        client <- ZIO.fromFuture(_ => AgentIdentity.getById("lookup-name", createdId))
        id     <- ZIO.fromFuture(_ => client.agentId())
      } yield assertTrue(
        host.calls == List("lookup:lookup-name", "get-by-id:DisplayName"),
        host.getByIdArgument.contains(createdId),
        id == createdId
      )
    },
    test("getById of an unknown or wrong-type ID fails with a typed not-found") {
      val host = new Host
      host.getByIdError = Some(rpcError("not-found", "no agent of the type has the ID"))
      host.install()
      failure(AgentIdentity.getById("lookup-name", createdId)).map(error =>
        assertTrue(error.contains(AgentRpcError.NotFound("no agent of the type has the ID")))
      )
    },
    test("reflected upsert maps host failures to typed RPC errors") {
      val host = new Host
      host.upsertError = Some(rpcError("denied", "no permission"))
      host.install()
      val denied = DynamicAgentClient.upsert("RawName", value)
      host.upsertError = Some(new IllegalStateException("connection preparation failed"))
      val unknown = DynamicAgentClient.upsert("RawName", value)
      assertTrue(
        denied == Left(GolemReflectError.Remote(AgentRpcError.Denied("no permission"))),
        unknown ==
          Left(GolemReflectError.Remote(AgentRpcError.Unknown("unknown", Some("connection preparation failed"))))
      )
    },
    test("reflected getById reads the agent type from the type tag of the ID") {
      val host = new Host
      host.install()
      ZIO.fromFuture(_ => DynamicAgentClient.getById(createdId)).map { result =>
        assertTrue(
          result.isRight,
          host.typeOfIdArgument.contains(createdId),
          host.calls.contains("get-by-id:DisplayName")
        )
      }
    },
    test("reflected getById of an ID no deployed type tags is not-found without get-by-id") {
      val host = new Host
      host.typeOfId = js.undefined
      host.install()
      ZIO.fromFuture(_ => DynamicAgentClient.getById(createdId)).map { result =>
        assertTrue(
          result.left.toOption.exists {
            case GolemReflectError.Remote(AgentRpcError.NotFound(_)) => true
            case _                                                   => false
          },
          !host.calls.exists(_.startsWith("get-by-id"))
        )
      }
    }
  ) @@ TestAspect.sequential
}
