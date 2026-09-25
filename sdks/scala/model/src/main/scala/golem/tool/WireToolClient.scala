/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 */

package golem.tool

import golem.schema.wire.{ConcreteCodec, WitSchemaGraph, WitTypedSchemaValue}

import scala.concurrent.Future
import scala.util.control.NonFatal

/** An uninspected custom-error payload returned by the tool host. */
final case class WireCustomToolError(name: String, payload: WitTypedSchemaValue)

sealed trait WireToolRpcFailure extends Product with Serializable
object WireToolRpcFailure {
  final case class ProtocolError(message: String)              extends WireToolRpcFailure
  final case class Denied(message: String)                     extends WireToolRpcFailure
  final case class NotFound(message: String)                   extends WireToolRpcFailure
  final case class RemoteInternalError(message: String)        extends WireToolRpcFailure
  final case class RemoteToolError(error: WireCustomToolError) extends WireToolRpcFailure
  final case class InvalidRemoteToolError(message: String)     extends WireToolRpcFailure
  case object Cancelled                                        extends WireToolRpcFailure
  final case class ResourceExhausted(message: String)          extends WireToolRpcFailure
}

final case class WireToolInvokeResult(result: Option[WitTypedSchemaValue])

trait WireToolRpcTransport {
  def start(
    commandPath: List[String],
    input: WitTypedSchemaValue,
    stdin: Option[ToolInputStream],
    stdout: Boolean
  ): Either[WireToolRpcFailure, WireToolRpcStarted]
}

final case class WireToolRpcStarted(
  stdout: Option[ToolInputStream],
  result: Future[Either[WireToolRpcFailure, WireToolInvokeResult]],
  cancel: () => Unit
)

/** Model-free helpers used by statically generated tool clients. */
object WireToolClientRuntime {
  private implicit val ec: scala.concurrent.ExecutionContext = ToolInvokerRuntime.executionContext

  def input[A](
    codec: ConcreteCodec[A],
    graph: WitSchemaGraph,
    value: A
  ): Either[ToolError[Nothing], WitTypedSchemaValue] =
    try Right(WitTypedSchemaValue(graph, codec.encodeValue(value)))
    catch { case NonFatal(error) => Left(protocol(s"failed to encode tool input: ${message(error)}")) }

  def run[E](
    rpc: WireToolRpcTransport,
    commandPath: List[String],
    input: Either[ToolError[Nothing], WitTypedSchemaValue],
    stdin: Option[ToolInputStream],
    decodeError: WireCustomToolError => Either[String, E]
  ): Future[Either[ToolError[E], WireToolInvokeResult]] =
    input match {
      case Left(error)  => Future.successful(Left(error))
      case Right(value) =>
        rpc
          .start(commandPath, value, stdin, stdout = false)
          .fold(
            f => Future.successful(Left(mapFailure(f, decodeError))),
            _.result.map(_.left.map(mapFailure(_, decodeError)))
          )
    }

  def start[E, T](
    rpc: WireToolRpcTransport,
    commandPath: List[String],
    input: Either[ToolError[Nothing], WitTypedSchemaValue],
    stdin: Option[ToolInputStream],
    decodeError: WireCustomToolError => Either[String, E]
  )(decode: WireToolInvokeResult => Either[ToolError[E], T]): Either[ToolError[E], ToolInvocation[E, T]] =
    input.left.map(identity[ToolError[E]]).flatMap { value =>
      rpc.start(commandPath, value, stdin, stdout = true).left.map(mapFailure(_, decodeError)).flatMap { started =>
        started.stdout.toRight {
          started.cancel()
          protocol("tool invocation did not create declared stdout stream")
        }.map(stream =>
          ToolInvocation(
            stream,
            started.result.map(_.left.map(mapFailure(_, decodeError)).flatMap(decode)),
            started.cancel
          )
        )
      }
    }

  def complete[E, T](call: Future[Either[ToolError[E], WireToolInvokeResult]])(
    decode: WireToolInvokeResult => Either[ToolError[E], T]
  ): Future[Either[ToolError[E], T]] = call.map(_.flatMap(decode))

  def decodeUnit(result: WireToolInvokeResult): Either[ToolError[Nothing], Unit] =
    if (result.result.isEmpty) Right(()) else Left(protocol("tool result unexpectedly contained a value"))

  def decodeValue[A](result: WireToolInvokeResult, codec: ConcreteCodec[A]): Either[ToolError[Nothing], A] =
    result.result match {
      case None        => Left(protocol("tool result did not contain a value"))
      case Some(value) =>
        try Right(codec.decode(value.value))
        catch { case NonFatal(error) => Left(protocol(s"failed to decode tool result: ${message(error)}")) }
    }

  def decodeError[A](error: WireCustomToolError, codec: ConcreteCodec[A]): Either[String, A] =
    try Right(codec.decode(error.payload.value))
    catch { case NonFatal(cause) => Left(s"failed to decode remote tool error: ${message(cause)}") }

  private def mapFailure[E](
    failure: WireToolRpcFailure,
    decode: WireCustomToolError => Either[String, E]
  ): ToolError[E] =
    failure match {
      case WireToolRpcFailure.ProtocolError(m)          => ToolError.Rpc(RpcError.Protocol(m))
      case WireToolRpcFailure.Denied(m)                 => ToolError.Rpc(RpcError.Denied(m))
      case WireToolRpcFailure.NotFound(m)               => ToolError.Rpc(RpcError.NotFound(m))
      case WireToolRpcFailure.RemoteInternalError(m)    => ToolError.Rpc(RpcError.RemoteInternal(m))
      case WireToolRpcFailure.Cancelled                 => ToolError.Rpc(RpcError.Cancelled)
      case WireToolRpcFailure.ResourceExhausted(m)      => ToolError.Rpc(RpcError.ResourceExhausted(m))
      case WireToolRpcFailure.InvalidRemoteToolError(m) => ToolError.Rpc(RpcError.Protocol(m))
      case WireToolRpcFailure.RemoteToolError(error)    =>
        decode(error).fold(m => ToolError.Rpc(RpcError.Protocol(m)), ToolError.Tool(_))
    }

  private def protocol(message: String): ToolError[Nothing] = ToolError.Rpc(RpcError.Protocol(message))
  private def message(error: Throwable): String             = Option(error.getMessage).getOrElse(error.getClass.getName)
}
