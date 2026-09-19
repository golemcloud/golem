/* Copyright 2024-2026 Golem Cloud. Licensed under the Golem Source License v1.1. */
package golem.bridge.runtime

import java.util.ArrayDeque
import java.util.concurrent.atomic.AtomicLong
import scala.concurrent.{Future, Promise}

private[runtime] object StreamSessionState {
  sealed trait PendingInput {
    def first: BigInt
    def count: BigInt
    def bytes: Int
    def terminal: Boolean
    def send(channel: Long): Future[Unit]
    def after(highWater: BigInt): PendingInput
    final def highWater: BigInt = first + count
  }
  final case class PendingData(
    first: BigInt,
    count: BigInt,
    bytes: Int,
    sendFn: Long => Future[Unit],
    trimFn: Option[BigInt => PendingInput] = None
  ) extends PendingInput {
    val terminal = false
    def send(channel: Long): Future[Unit] = sendFn(channel)
    def after(highWater: BigInt): PendingInput =
      if (highWater == first) this
      else trimFn.map(_(highWater)).getOrElse(throw BridgeException("input high-water split an atomic item"))
  }
  final case class PendingTerminal(first: BigInt, sendFn: Long => Future[Unit]) extends PendingInput {
    val count = BigInt(0); val bytes = 0; val terminal = true
    def send(channel: Long): Future[Unit] = sendFn(channel)
    def after(highWater: BigInt): PendingInput = this
  }

  /** Durable client-side input state. Exactly one pending range is retained until its cumulative ACK. */
  final class InputState(totalReplayBytes: AtomicLong = new AtomicLong(0L)) {
    private var channel = 0L
    private var next = BigInt(0)
    private var ended = false
    private var cancelled = false
    private var pending: Option[PendingInput] = None
    def detach(): Unit = synchronized { channel = 0L }
    def remap(newChannel: Long, highWater: BigInt, terminal: Boolean): Option[(PendingInput, Long)] = synchronized {
      if (newChannel <= 0) throw BridgeException("invalid input channel")
      if (highWater < 0 || highWater > next) throw BridgeException("input high-water is beyond sent data")
      pending.foreach { p =>
        if (highWater < p.first || highWater > p.highWater)
          throw BridgeException("input high-water conflicts with pending range")
      }
      if (terminal && !ended) throw BridgeException("server reported an unsent input terminal")
      if (!terminal && ended && !cancelled && pending.forall(!_.terminal)) throw BridgeException("server lost the input terminal")
      channel = newChannel
      pending = pending.flatMap { p =>
        if ((p.terminal && terminal) || (!p.terminal && highWater == p.highWater)) {
          release(p)
          None
        } else if (!p.terminal && highWater > p.first) {
          val trimmed = p.after(highWater)
          release(p)
          charge(trimmed)
          Some(trimmed)
        } else Some(p)
      }
      pending.map(_ -> channel)
    }
    def reserve(data: PendingInput): (PendingInput, Long) = synchronized {
      if (ended || pending.nonEmpty || data.first != next || data.count < 0)
        throw BridgeException("conflicting input stream state")
      charge(data)
      pending = Some(data)
      next += data.count
      if (data.terminal) ended = true
      data -> channel
    }
    def acknowledge(highWater: BigInt, terminal: Boolean): Boolean = synchronized {
      if (highWater < 0 || highWater > next) throw BridgeException("input ACK is beyond sent data")
      val p = pending.getOrElse {
        if (highWater != next || terminal != ended) throw BridgeException("conflicting duplicate input ACK")
        return false
      }
      if (highWater < p.first || highWater > p.highWater)
        throw BridgeException("input ACK conflicts with pending range")
      if (terminal && !ended) throw BridgeException("input ACK reports an unsent terminal")
      val accepted = (p.terminal && terminal && highWater == next) || (!p.terminal && highWater == p.highWater)
      if (accepted) {
        release(p)
        pending = None
      } else if (!p.terminal && highWater > p.first) {
        val trimmed = p.after(highWater)
        release(p)
        charge(trimmed)
        pending = Some(trimmed)
      }
      accepted
    }
    def cancel(): Unit = synchronized { pending.foreach(release); pending = None; ended = true }
    def cancelLocal(): Unit = synchronized { cancel(); cancelled = true }
    private def charge(data: PendingInput): Unit = {
      if (data.bytes > StreamSessionProtocol.MaxReplayBytes || totalReplayBytes.addAndGet(data.bytes) > StreamSessionProtocol.MaxSessionQueuedBytes) {
        totalReplayBytes.addAndGet(-data.bytes)
        throw BridgeException("unacknowledged input exceeds the protocol limit")
      }
    }
    private def release(data: PendingInput): Unit = totalReplayBytes.addAndGet(-data.bytes)
    def currentChannel: Long = synchronized(channel)
    def nextSequence: BigInt = synchronized(next)
    def isTerminal: Boolean = synchronized(ended)
    def pendingBytes: Int = synchronized(pending.fold(0)(_.bytes))
  }

  final case class Delivery[A](bytes: Int, value: Either[Throwable, () => AgentStreamStep[A]])

  /** Byte/item bounded affine output queue. Charges are released on delivery or cancellation. */
  final class OutputQueue[A](release: Int => Unit) extends AgentStream.Consumer[A] {
    private val queued = new ArrayDeque[Delivery[A]]()
    private val waiters = new ArrayDeque[Promise[AgentStreamStep[A]]]()
    private var bytes = 0
    private var terminal = false
    private var cancelled = false
    @volatile var cancelRemote: String => Future[Unit] = _ => Future.successful(())
    def pull(): Future[AgentStreamStep[A]] = synchronized {
      if (!queued.isEmpty) deliver(queued.remove())
      else if (terminal || cancelled) Future.successful(AgentStreamStep.End)
      else { val p = Promise[AgentStreamStep[A]](); waiters.add(p); p.future }
    }
    private def deliver(d: Delivery[A]): Future[AgentStreamStep[A]] = {
      bytes -= d.bytes; release(d.bytes)
      d.value.fold(Future.failed, thunk => Future.successful(thunk()))
    }
    def offer(d: Delivery[A], isTerminal: Boolean = false): Unit = synchronized {
      if (terminal || cancelled) throw BridgeException("message received after output stream terminal")
      if (isTerminal) terminal = true
      if (!waiters.isEmpty) {
        release(d.bytes)
        d.value.fold(waiters.remove().failure, thunk => waiters.remove().success(thunk()))
      } else {
        if (queued.size >= StreamSessionProtocol.MaxQueuedItems || bytes + d.bytes > StreamSessionProtocol.MaxQueuedBytes)
          throw BridgeException("output stream queue exceeded the protocol limit")
        queued.add(d); bytes += d.bytes
      }
    }
    private def stop(reason: String): Future[Unit] = synchronized {
      if (cancelled || terminal) Future.successful(())
      else {
        cancelled = true
        while (!queued.isEmpty) { val d = queued.remove(); bytes -= d.bytes; release(d.bytes) }
        while (!waiters.isEmpty) waiters.remove().success(AgentStreamStep.End)
        cancelRemote(reason)
      }
    }
    def cancel(): Future[Unit] = stop("cancelled")
    def drop(): Future[Unit] = stop("consumer-drop")
    def queuedBytes: Int = synchronized(bytes)
  }
}
