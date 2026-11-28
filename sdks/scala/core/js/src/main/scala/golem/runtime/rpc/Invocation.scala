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

import golem.InvocationId
import golem.concurrent.GolemExecutionContext
import golem.host.js.schema.JsSchemaValueTree
import golem.runtime.autowire.SchemaPayload
import golem.runtime.rpc.host.WasmRpcApi
import golem.schema.{FromSchema, SchemaValue}

import scala.concurrent.Future
import scala.scalajs.js
import scala.util.control.NonFatal

/** The lifecycle state of an accepted invocation. */
sealed trait InvocationStatus extends Product with Serializable

object InvocationStatus {

  /** Accepted and waiting for its turn. */
  case object Pending extends InvocationStatus

  /** Accepted and waiting for its scheduled time. */
  case object Scheduled extends InvocationStatus

  /** Started and executing. */
  case object Running extends InvocationStatus

  /** Started and waiting, for example for a promise or a timer. */
  case object Waiting extends InvocationStatus

  /** Started and waiting to retry after a failure. */
  case object Retrying extends InvocationStatus

  /** Started, and its agent was interrupted by an operator. */
  case object Interrupted extends InvocationStatus

  /** Finished with a result. */
  case object Completed extends InvocationStatus

  /** Finished with a failure. */
  case object Failed extends InvocationStatus

  /** Withdrawn before it started. */
  case object Cancelled extends InvocationStatus

  /**
   * Recorded its result, then its agent failed during the invocation's
   * background work.
   */
  case object BackgroundFailed extends InvocationStatus

  private[golem] def fromHost(tag: String): InvocationStatus =
    tag match {
      case "pending"           => Pending
      case "scheduled"         => Scheduled
      case "running"           => Running
      case "waiting"           => Waiting
      case "retrying"          => Retrying
      case "interrupted"       => Interrupted
      case "completed"         => Completed
      case "failed"            => Failed
      case "cancelled"         => Cancelled
      case "background-failed" => BackgroundFailed
      case other               => throw new IllegalStateException(s"protocol-error: unknown invocation status '$other'")
    }
}

/**
 * The handle of an accepted invocation of an agent method, which `trigger` and
 * `scheduleAt` return.
 *
 * The ID only locates the invocation; it is not authority. Only the caller that
 * started the invocation, or a caller with the applicable management
 * permission, can observe or cancel it; any other caller fails with
 * [[AgentRpcError.Denied]]. Store [[id]] to await the invocation from a later
 * invocation of the same agent, and rebuild the handle with [[Invocation.get]].
 * Dropping the handle, or abandoning the `Future` of [[awaitResult]], only
 * detaches: the invocation keeps running.
 *
 * `trigger` and `scheduleAt` reject methods with stream inputs or outputs, and
 * never lend a scope card. Cancelling a running invocation is not supported:
 * [[cancel]] fails with [[AgentRpcError.CannotCancelRunning]].
 *
 * Every operation fails with an [[AgentRpcException]] when the host reports an
 * `rpc-error`.
 */
final class Invocation[+Out] private (
  val id: InvocationId,
  backend: Invocation.Backend,
  decode: Option[JsSchemaValueTree] => Future[Out]
) {

  /**
   * The current lifecycle state of the invocation. Fails with
   * [[AgentRpcError.NotFound]] when its agent or the invocation no longer
   * exists.
   */
  def status(): Future[InvocationStatus] = backend.status()

  /**
   * Awaits the result of the invocation, without starting, resuming or
   * resubmitting it. Fails with [[AgentRpcError.Cancelled]] when the invocation
   * was cancelled before it started.
   */
  def awaitResult(): Future[Out] =
    backend.awaitRaw().flatMap(decode)(GolemExecutionContext.global)

  /**
   * Withdraws the invocation if it has not started yet, and returns `true`.
   * Returns `false` when the invocation already finished or was already
   * cancelled, and fails with [[AgentRpcError.CannotCancelRunning]] when it is
   * running.
   */
  def cancel(): Future[Boolean] = backend.cancel()

  /** The handle of the same invocation, with `f` applied to its result. */
  private[golem] def mapResult[B](f: Out => B): Invocation[B] =
    new Invocation[B](id, backend, raw => decode(raw).map(f)(GolemExecutionContext.global))

  override def toString: String = s"Invocation($id)"
}

object Invocation {

  /** The host operations of a handle; results are raw value trees. */
  private[golem] trait Backend {
    def status(): Future[InvocationStatus]
    def awaitRaw(): Future[Option[JsSchemaValueTree]]
    def cancel(): Future[Boolean]
  }

  /**
   * Rebuilds the handle of the invocation with the given ID. Makes no remote
   * call. `Out` is the output type of the invoked method; a `Unit` method has
   * the `Unit` output.
   */
  def get[Out](id: InvocationId)(implicit decoder: FromSchema[Out]): Invocation[Out] =
    new Invocation(id, hostBackend(WasmRpcApi.getInvocation(id)), decodeWith(decoder))

  private[golem] def apply(id: InvocationId, backend: Backend): Invocation[Option[JsSchemaValueTree]] =
    new Invocation(id, backend, Future.successful)

  /** The handle of a host `invocation` resource. */
  private[golem] def fromHost(raw: WasmRpcApi.RawInvocation): Invocation[Option[JsSchemaValueTree]] =
    apply(WasmRpcApi.invocationIdFromJs(raw.id()), hostBackend(raw))

  private def hostBackend(raw: WasmRpcApi.RawInvocation): Backend =
    new Backend {
      def status(): Future[InvocationStatus] =
        call(raw.status()).map(InvocationStatus.fromHost)(GolemExecutionContext.global)
      def awaitRaw(): Future[Option[JsSchemaValueTree]] =
        call(raw.await_()).map(_.toOption)(GolemExecutionContext.global)
      def cancel(): Future[Boolean] = call(raw.cancel())
    }

  private def call[A](promise: => js.Promise[A]): Future[A] = {
    implicit val ec = GolemExecutionContext.global
    WasmRpcApi.invocationCall(promise).flatMap {
      case Right(value) => Future.successful(value)
      case Left(error)  => Future.failed(AgentRpcException(AgentRpcError.fromRpcError(error)))
    }
  }

  private def decodeWith[Out](decoder: FromSchema[Out]): Option[JsSchemaValueTree] => Future[Out] = { result =>
    val decoded =
      try
        result match {
          case Some(tree) => SchemaPayload.decode[Out](tree)(decoder).left.map(_.message)
          case None       =>
            decoder
              .fromValue(SchemaValue.TupleValue(Nil))
              .left
              .map(_ => "Expected a return value for a non-unit method output, got none")
        }
      catch { case NonFatal(error) => Left(String.valueOf(error.getMessage)) }
    decoded.fold(error => Future.failed(new IllegalArgumentException(error)), Future.successful)
  }
}
