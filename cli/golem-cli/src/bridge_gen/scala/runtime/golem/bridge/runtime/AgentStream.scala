/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at http://license.golem.cloud/LICENSE
 */
package golem.bridge.runtime

import java.util.concurrent.atomic.AtomicBoolean
import scala.concurrent.{ExecutionContext, Future}

/** A binary schema value. MIME is deliberately carried beside the bytes. */
final case class AgentBinary(bytes: Vector[Byte], mimeType: Option[String] = None)

sealed trait AgentStreamStep[+A]
object AgentStreamStep {
  final case class Item[A](value: A) extends AgentStreamStep[A]
  case object End extends AgentStreamStep[Nothing]
}

sealed abstract class AgentStreamTerminal(message: String) extends RuntimeException(message)
final case class AgentStreamError(code: String, override val getMessage: String)
    extends AgentStreamTerminal(getMessage)
final case class AgentStreamCancelled(reason: String)
    extends AgentStreamTerminal(s"stream cancelled: $reason")

/**
 * Dependency-free, demand-driven stream. `consume` is affine: a second call
 * deterministically returns a failed Future. Pulling one item at a time is the
 * back-pressure boundary. `cancel` is explicit; `drop` means an unread output
 * was abandoned and is propagated as `consumer-drop` by remote streams.
 */
final class AgentStream[A] private[runtime] (
  private val acquire: () => Future[AgentStream.Consumer[A]]
) {
  private val consumed = new AtomicBoolean(false)

  private def claim(): Unit =
    if (!consumed.compareAndSet(false, true))
      throw BridgeException("AgentStream already has a consumer")

  def consume()(implicit ec: ExecutionContext): Future[AgentStream.Consumer[A]] =
    try {
      claim()
      acquire()
    } catch {
      case error: Throwable => Future.failed(error)
    }
}

object AgentStream {
  trait Consumer[A] {
    def pull(): Future[AgentStreamStep[A]]
    def cancel(): Future[Unit]
    def drop(): Future[Unit]
  }

  /** Creates an input stream. Source failures become `source-unavailable`. */
  def fromPull[A](next: () => Future[AgentStreamStep[A]], onCancel: () => Future[Unit] = () => Future.successful(())): AgentStream[A] =
    new AgentStream(() => Future.successful(new Consumer[A] {
      private val terminal = new AtomicBoolean(false)
      def pull(): Future[AgentStreamStep[A]] =
        if (terminal.get()) Future.successful(AgentStreamStep.End)
        else next().map { step =>
          if (step == AgentStreamStep.End) terminal.set(true)
          step
        }(scala.concurrent.ExecutionContext.parasitic)
      def cancel(): Future[Unit] = if (terminal.compareAndSet(false, true)) onCancel() else Future.successful(())
      def drop(): Future[Unit] = cancel()
    }))

  private[runtime] def remote[A](consumer: Consumer[A]): AgentStream[A] =
    new AgentStream(() => Future.successful(consumer))

  private[runtime] def claimAny(stream: AgentStream[Any]): Future[Consumer[Any]] = {
    stream.claim()
    stream.acquire()
  }
}
