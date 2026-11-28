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

import golem.host.SchemaWireInterop
import golem.host.js.schema.JsTypedSchemaValue
import golem.runtime.rpc.host.WasmRpcApi
import golem.schema.TypedSchemaValue
import golem.schema.wire.SchemaWire

import scala.scalajs.js

/** A typed `golem:agent/host` `rpc-error`. */
sealed trait AgentRpcError extends Product with Serializable { def message: String }

object AgentRpcError {
  final case class Protocol(detail: String) extends AgentRpcError { val message = s"protocol-error: $detail" }
  final case class Denied(detail: String)   extends AgentRpcError { val message = s"denied: $detail"         }

  /**
   * No agent of the requested type exists at the requested ID, or the target
   * method does not exist.
   */
  final case class NotFound(detail: String)       extends AgentRpcError { val message = s"not-found: $detail" }
  final case class RemoteInternal(detail: String) extends AgentRpcError {
    val message = s"remote-internal-error: $detail"
  }

  /**
   * A keyed create found an agent that already has the key, or the agent at the
   * ID that a key derives has another key.
   */
  final case class Conflict(detail: String) extends AgentRpcError { val message = s"conflict: $detail" }

  /** The awaited invocation was cancelled before it started. */
  final case class Cancelled(detail: String) extends AgentRpcError { val message = s"cancelled: $detail" }

  /**
   * A cancellation found the invocation already running. Cancelling a running
   * invocation is not supported.
   */
  final case class CannotCancelRunning(detail: String) extends AgentRpcError {
    val message = s"cannot-cancel-running: $detail"
  }
  final case class InvalidInput(detail: String)                  extends AgentRpcError { val message = s"invalid-input: $detail"    }
  final case class InvalidMethod(detail: String)                 extends AgentRpcError { val message = s"invalid-method: $detail"   }
  final case class InvalidType(detail: String)                   extends AgentRpcError { val message = s"invalid-type: $detail"     }
  final case class InvalidAgentId(detail: String)                extends AgentRpcError { val message = s"invalid-agent-id: $detail" }
  final case class Custom(payload: TypedSchemaValue)             extends AgentRpcError { val message = "custom-error"               }
  final case class Unknown(kind: String, detail: Option[String]) extends AgentRpcError {
    val message: String = detail.fold(kind)(value => s"$kind: $value")
  }

  private[golem] def fromRpcError(error: WasmRpcApi.RpcError): AgentRpcError =
    error.agentError match {
      case Some(agentError) =>
        val value = agentError.asInstanceOf[js.Dynamic].selectDynamic("val")
        agentError.tag match {
          case "invalid-input"    => InvalidInput(String.valueOf(value))
          case "invalid-method"   => InvalidMethod(String.valueOf(value))
          case "invalid-type"     => InvalidType(String.valueOf(value))
          case "invalid-agent-id" => InvalidAgentId(String.valueOf(value))
          case "custom-error"     =>
            Custom(
              SchemaWire.typedSchemaValueFromWit(SchemaWireInterop.typedFromJs(value.asInstanceOf[JsTypedSchemaValue]))
            )
          case other => Unknown(other, None)
        }
      case None =>
        val detail = error.message.getOrElse(error.kind)
        error.kind match {
          case "protocol-error"        => Protocol(detail)
          case "denied"                => Denied(detail)
          case "not-found"             => NotFound(detail)
          case "remote-internal-error" => RemoteInternal(detail)
          case "conflict"              => Conflict(detail)
          case "cancelled"             => Cancelled(detail)
          case "cannot-cancel-running" => CannotCancelRunning(detail)
          case other                   => Unknown(other, error.message)
        }
    }
}

/**
 * The failure of an RPC client operation that carries the typed
 * [[AgentRpcError]]: making a client (`upsert`, `create`, `getById`),
 * `agentId`, an awaited call, `trigger`, `scheduleAt`, and the operations of an
 * [[Invocation]].
 */
final case class AgentRpcException(error: AgentRpcError) extends RuntimeException(error.message)
