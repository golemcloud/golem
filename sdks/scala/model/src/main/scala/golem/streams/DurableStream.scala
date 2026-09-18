/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 */
package golem.streams

import golem.schema.AgentStream
import scala.concurrent.{ExecutionContext, Future, Promise}
import scala.util.Try

/** Server-generated opaque offset and independent transport cursor. */
final case class DurableStreamCheckpoint(offset: String = "-1", cursor: Option[String] = None)

enum DurableStreamTransport {
  case CatchUp, LongPoll, Sse
}

final case class DurableStreamRetry(
  budgetMs: Long = 60000,
  initialDelayMs: Long = 100,
  maxDelayMs: Long = 5000
) {
  require(budgetMs >= 0 && initialDelayMs > 0 && maxDelayMs >= initialDelayMs)
}

final case class DurableStreamReadOptions(
  checkpoint: DurableStreamCheckpoint = DurableStreamCheckpoint(),
  liveTransport: DurableStreamTransport = DurableStreamTransport.LongPoll,
  timeoutMs: Long = 30000,
  idleDelayMs: Long = 100,
  retry: DurableStreamRetry = DurableStreamRetry()
) {
  require(timeoutMs >= 1 && timeoutMs <= 300000 && idleDelayMs > 0)
}

enum DurableStreamErrorKind {
  case InvalidRequest, PermissionDenied, NotFound, Gone, Closed, SequenceConflict, Fenced,
    ProducerDiverged, ProtocolError, PayloadTooLarge, Timeout, Transport, RateLimited, Unavailable
}

final case class DurableStreamError(
  kind: DurableStreamErrorKind,
  message: String,
  retryAfterMs: Option[BigInt] = None,
  producerEpoch: Option[BigInt] = None,
  expectedSequence: Option[BigInt] = None
) extends RuntimeException(message) {
  def retryable: Boolean = kind match {
    case DurableStreamErrorKind.Timeout | DurableStreamErrorKind.Transport | DurableStreamErrorKind.RateLimited |
        DurableStreamErrorKind.Unavailable =>
      true
    case _ => false
  }
}

/**
 * A new epoch starts at sequence zero. Forks retain this identity unchanged.
 */
final case class DurableStreamProducer(id: String, epoch: Long = 0, sequence: Long = 0) {
  require(id.nonEmpty && epoch >= 0 && epoch <= DurableStreamProducer.MaxInteger)
  require(sequence >= 0 && sequence <= DurableStreamProducer.MaxInteger)
}

object DurableStreamProducer {
  private[streams] val MaxInteger = 9007199254740991L
}

final case class DurableStreamReceipt(
  nextOffset: Option[String],
  epoch: Long,
  sequence: Long,
  closed: Boolean
)

private[golem] enum DurableStreamPayload {
  case Json(values: Vector[String])
  case Bytes(values: Vector[Byte])

  def isEmpty: Boolean = this match {
    case Json(values)  => values.isEmpty
    case Bytes(values) => values.isEmpty
  }
}

private[golem] final case class DurableStreamReadRequest(
  url: String,
  checkpoint: DurableStreamCheckpoint,
  json: Boolean,
  transport: DurableStreamTransport,
  contentType: Option[String],
  timeoutMs: Long
)

private[golem] final case class DurableStreamBatch(
  payload: Vector[Byte],
  contentType: String,
  next: DurableStreamCheckpoint,
  upToDate: Boolean,
  closed: Boolean
)

private[golem] final case class DurableStreamAppendRequest(
  url: String,
  contentType: String,
  payload: DurableStreamPayload,
  producer: DurableStreamProducer,
  close: Boolean,
  timeoutMs: Long
)

private[golem] trait DurableStreamHost {
  def read(request: DurableStreamReadRequest): Future[DurableStreamBatch]
  def append(request: DurableStreamAppendRequest): Future[DurableStreamReceipt]
  def nowMs(): BigInt
  def sleep(ms: Long): Future[Unit]
}

/**
 * Retry state is reconstructed by the same durable host results and clock
 * reads.
 */
private[streams] final class DurableStreamRetries(host: DurableStreamHost, options: DurableStreamRetry) {
  private implicit val ec: ExecutionContext = ExecutionContext.parasitic
  private var started: Option[BigInt]       = None
  private var delay                         = options.initialDelayMs

  def reset(): Unit = {
    started = None
    delay = options.initialDelayMs
  }

  def run[A](cancelled: () => Boolean)(operation: => Future[A]): Future[A] = {
    def attempt(): Future[A] =
      if (cancelled()) Future.failed(new IllegalStateException("durable stream operation cancelled"))
      else
        Future.fromTry(Try(operation)).flatten.recoverWith {
          case error: DurableStreamError if error.retryable =>
            val now       = host.nowMs()
            val start     = started.getOrElse { started = Some(now); now }
            val remaining = BigInt(options.budgetMs) - (now - start).max(BigInt(0))
            val wait      = BigInt(delay).max(error.retryAfterMs.getOrElse(BigInt(0)))
            if (cancelled() || wait >= remaining) Future.failed(error)
            else {
              delay = (BigInt(delay) * 2).min(BigInt(options.maxDelayMs)).toLong
              host.sleep(wait.toLong).flatMap { _ =>
                if (host.nowMs() - start >= options.budgetMs) Future.failed(error)
                else attempt()
              }
            }
        }
    attempt()
  }
}

/**
 * One buffered external batch, exposed as an ordinary affine AgentStream.
 * Checkpoints advance only after every element of the batch has been consumed.
 */
private[golem] object DurableStreamReader {
  def create[A, Raw](
    host: DurableStreamHost,
    url: String,
    json: Boolean,
    options: DurableStreamReadOptions,
    unpack: Vector[Byte] => Vector[Raw],
    decode: Raw => A
  ): AgentStream[A] = {
    implicit val ec: ExecutionContext = ExecutionContext.parasitic
    var checkpoint                    = options.checkpoint
    var contentType                   = Option.empty[String]
    var transport                     = DurableStreamTransport.CatchUp
    var pending                       = Option.empty[DurableStreamBatch]
    var buffer                        = Vector.empty[Raw]
    var index                         = 0
    var cancelled                     = false
    var closed                        = false
    val retries                       = new DurableStreamRetries(host, options.retry)

    def pull(): Future[Option[A]] =
      if (cancelled) Future.failed(new IllegalStateException("durable stream reader closed"))
      else if (index < buffer.size) {
        val value = decode(buffer(index))
        index += 1
        Future.successful(Some(value))
      } else {
        pending.foreach { batch =>
          checkpoint = batch.next
          closed = batch.closed
          transport = if (batch.upToDate) options.liveTransport else DurableStreamTransport.CatchUp
        }
        pending = None
        buffer = Vector.empty
        index = 0
        if (closed) Future.successful(None)
        else {
          val request = DurableStreamReadRequest(url, checkpoint, json, transport, contentType, options.timeoutMs)
          retries.run(() => cancelled)(host.read(request)).flatMap { batch =>
            if (cancelled) Future.failed(new IllegalStateException("durable stream reader closed"))
            else {
              if (checkpoint.offset == "now" && batch.next.offset == "now")
                throw DurableStreamError(
                  DurableStreamErrorKind.ProtocolError,
                  "now did not resolve to a concrete offset"
                )
              if (contentType.isEmpty) contentType = Some(batch.contentType)
              buffer = unpack(batch.payload)
              pending = Some(batch)
              retries.reset()
              if (buffer.isEmpty && batch.upToDate && !batch.closed && checkpoint.offset != "now")
                host.sleep(options.idleDelayMs).flatMap(_ => pull())
              else pull()
            }
          }
        }
      }

    AgentStream.fromPull(
      () => pull(),
      () => {
        cancelled = true
        buffer = Vector.empty
        pending = None
        Future.successful(())
      }
    )
  }
}

/**
 * Serialized, protocol-idempotent appends. A failed or cancelled attempt
 * retains its encoded payload and tuple; call retryPending before supplying new
 * data. Dropping a writer does not undo an external append. Golem forks do not
 * create independent producers; the server's deduplication guarantees still
 * apply.
 */
final class DurableStreamWriter[A] private[golem] (
  host: DurableStreamHost,
  url: String,
  contentType: String,
  initialProducer: DurableStreamProducer,
  timeoutMs: Long,
  retry: DurableStreamRetry,
  encode: Vector[A] => DurableStreamPayload
) {
  require(timeoutMs >= 1 && timeoutMs <= 300000)
  private implicit val ec: ExecutionContext = ExecutionContext.parasitic
  private var producer                      = initialProducer
  private var pending                       = Option.empty[DurableStreamAppendRequest]
  private var active                        = Option.empty[Promise[DurableStreamReceipt]]
  private var cancelled                     = false
  private var closed                        = false
  private var exhausted                     = false
  private val retries                       = new DurableStreamRetries(host, retry)

  def hasPending: Boolean = pending.isDefined
  def isClosed: Boolean   = closed

  def append(values: Iterable[A], close: Boolean = false): Future[DurableStreamReceipt] =
    if (pending.isDefined || active.isDefined)
      Future.failed(
        new IllegalStateException("resolve the pending append with retryPending before submitting new data")
      )
    else if (closed || exhausted)
      Future.failed(new IllegalStateException("durable stream writer is closed or its sequence is exhausted"))
    else
      Future
        .fromTry(Try {
          val payload = encode(values.iterator.toVector)
          require(close || !payload.isEmpty, "an empty append must close the stream")
          pending = Some(DurableStreamAppendRequest(url, contentType, payload, producer, close, timeoutMs))
        })
        .flatMap(_ => retryPending())

  def close(): Future[DurableStreamReceipt] = append(Vector.empty, close = true)

  /** Retry the exact retained tuple, encoded payload and close flag. */
  def retryPending(): Future[DurableStreamReceipt] =
    if (active.isDefined)
      Future.failed(new IllegalStateException("durable stream writer already has an active attempt"))
    else
      pending match {
        case None          => Future.failed(new IllegalStateException("durable stream writer has no pending append"))
        case Some(request) =>
          cancelled = false
          val result = Promise[DurableStreamReceipt]()
          active = Some(result)
          retries.run(() => cancelled)(host.append(request)).onComplete { outcome =>
            active = None
            if (!cancelled) {
              val checked = outcome.flatMap { receipt =>
                Try {
                  if (receipt.epoch != request.producer.epoch || receipt.sequence < request.producer.sequence)
                    throw DurableStreamError(DurableStreamErrorKind.ProtocolError, "invalid producer acknowledgement")
                  if (receipt.sequence > request.producer.sequence)
                    throw DurableStreamError(
                      DurableStreamErrorKind.ProducerDiverged,
                      "producer acknowledgement is ahead of this writer"
                    )
                  if (request.close && !receipt.closed)
                    throw DurableStreamError(DurableStreamErrorKind.ProtocolError, "close was not acknowledged")
                  exhausted = producer.sequence == DurableStreamProducer.MaxInteger
                  if (!exhausted) producer = producer.copy(sequence = producer.sequence + 1)
                  closed = receipt.closed
                  pending = None
                  retries.reset()
                  receipt
                }
              }
              result.tryComplete(checked)
            }
          }
          result.future
      }

  /**
   * Fail the caller immediately without discarding the uncertain append. The
   * bounded native import may continue; retryPending is available once it
   * settles.
   */
  def cancel(): Unit = active.foreach { result =>
    cancelled = true
    result.tryFailure(new IllegalStateException("durable stream append cancelled"))
  }
}
