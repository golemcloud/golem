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
import golem.config.ConfigOverrideEncoder
import golem.host.js.JsAgentId
import golem.host.js.schema.JsSchemaValueTree
import golem.runtime.rpc.host.{AgentHostApi, WasmRpcApi}

import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
import scala.util.control.NonFatal

/**
 * Makes RPC clients through the `wasm-rpc` static functions. Every client uses
 * the agent type name that the host registry reports for the requested name.
 */
private[golem] object AgentIdentity {

  /**
   * Binds the current agent. The host registry names the agent type that the
   * type tag of the returned ID belongs to; it must be `expectedTypeName`.
   */
  def bindSelf(expectedTypeName: String): Either[String, RemoteAgentClient] =
    WasmRpcApi.bindSelf().left.map(_.toString).flatMap { case (id, rpc) =>
      val actual = AgentHostApi.registeredAgentTypeFor(id).map(_.agentType.typeName)
      if (actual.contains(expectedTypeName))
        Right(client(expectedTypeName, rpc, Some(JsAgentId.toAgentId(id))))
      else {
        rpc.drop()
        Left(
          s"invalid-type: self binding expects $expectedTypeName, identity is ${actual.getOrElse("an unknown agent type")}"
        )
      }
    }

  /** A client of the keyed agent of the constructor parameters. */
  def upsert(agentTypeName: String, constructorPayload: JsSchemaValueTree): Either[AgentRpcError, RemoteAgentClient] =
    registeredTypeName(agentTypeName).flatMap { typeName =>
      WasmRpcApi
        .upsert(typeName, constructorPayload)
        .left
        .map(AgentRpcError.fromRpcError)
        .map(client(typeName, _, None))
    }

  /**
   * Creates a durable agent. Fails with [[AgentRpcException]]; a keyed create
   * of an existing key fails with [[AgentRpcError.Conflict]].
   */
  def create(
    agentTypeName: String,
    constructorPayload: JsSchemaValueTree,
    options: CreateOptions
  ): Future[RemoteAgentClient] =
    registeredTypeName(agentTypeName) match {
      case Left(error)     => Future.failed(AgentRpcException(error))
      case Right(typeName) =>
        (try
          WasmRpcApi.create(
            typeName,
            constructorPayload,
            options.keyed,
            ConfigOverrideEncoder.encode(options.config),
            options.env
          )
        catch { case NonFatal(error) => Future.failed(error) }).flatMap {
          case Right((id, rpc)) => Future.successful(client(typeName, rpc, Some(JsAgentId.toAgentId(id))))
          case Left(error)      => Future.failed(AgentRpcException(AgentRpcError.fromRpcError(error)))
        }
    }

  /**
   * A client of the existing agent with the given ID. An unknown ID, or the ID
   * of an agent of another type, fails with [[AgentRpcError.NotFound]].
   */
  def getById(agentTypeName: String, agentId: AgentId): Future[RemoteAgentClient] =
    Future.fromTry(scala.util.Try {
      registeredTypeName(agentTypeName)
        .flatMap(typeName =>
          WasmRpcApi
            .getById(typeName, JsAgentId.fromAgentId(agentId))
            .left
            .map(AgentRpcError.fromRpcError)
            .map(client(typeName, _, Some(agentId)))
        )
        .fold(error => throw AgentRpcException(error), identity)
    })

  private def registeredTypeName(agentTypeName: String): Either[AgentRpcError, String] =
    AgentHostApi
      .registeredAgentType(agentTypeName)
      .map(_.agentType.typeName)
      .toRight(AgentRpcError.NotFound(s"Agent type '$agentTypeName' is not registered on this host"))

  private def client(typeName: String, rpc: WasmRpcApi.WasmRpcClient, knownId: Option[AgentId]): RemoteAgentClient =
    RemoteAgentClient(typeName, new RemoteAgentClient.WasmRpcInvoker(rpc, knownId))
}
