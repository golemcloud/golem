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

import golem.config.{ConfigOverride, ConfigOverrideEncoder}
import golem.host.js.schema.{JsSchemaValueTree, JsTypedAgentConfigValue, JsUuid}
import golem.runtime.rpc.host.{AgentHostApi, WasmRpcApi}

import java.util.UUID
import scala.scalajs.js

private[golem] object AgentIdentity {
  def make(
    agentTypeName: String,
    payload: JsSchemaValueTree,
    phantom: Option[UUID]
  ): Either[String, String] =
    AgentHostApi.makeAgentId(agentTypeName, payload, phantom)

  def parse(agentId: String): Either[String, AgentHostApi.AgentIdParts] =
    AgentHostApi.parseAgentId(agentId)

  def createConnection(
    agentTypeName: String,
    constructorPayload: JsSchemaValueTree,
    phantom: Option[UUID],
    configOverrides: List[ConfigOverride]
  ): Either[WasmRpcApi.RpcError, WasmRpcApi.WasmRpcClient] = {
    val phantomArg = phantom.fold[js.UndefOr[JsUuid]](js.undefined)(JsUuid.fromUuid)
    WasmRpcApi.createClient(
      agentTypeName,
      constructorPayload,
      phantomArg,
      ConfigOverrideEncoder.encode(configOverrides)
    )
  }

  def resolve(
    agentTypeName: String,
    constructorPayload: JsSchemaValueTree,
    phantom: Option[UUID],
    configOverrides: List[ConfigOverride]
  ): Either[String, RemoteAgentClient] =
    AgentHostApi
      .registeredAgentType(agentTypeName)
      .toRight(s"Agent type '$agentTypeName' is not registered on this host")
      .flatMap { agentType =>
        val displayTypeName = agentType.agentType.typeName
        make(displayTypeName, constructorPayload, phantom).map { id =>
          val phantomArg: js.UndefOr[JsUuid] = phantom.fold[js.UndefOr[JsUuid]](js.undefined)(JsUuid.fromUuid)
          val jsConfig                       =
            if (configOverrides.isEmpty) js.Array[JsTypedAgentConfigValue]()
            else ConfigOverrideEncoder.encode(configOverrides)
          val rpcClient = WasmRpcApi.newClient(displayTypeName, constructorPayload, phantomArg, jsConfig)
          RemoteAgentClient(displayTypeName, id, agentType, new RemoteAgentClient.WasmRpcInvoker(rpcClient))
        }
      }
}
