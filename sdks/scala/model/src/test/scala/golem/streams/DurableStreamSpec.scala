/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 */
package golem.streams

import golem.schema.{AgentStream, FromSchema, IntoSchema}
import scala.concurrent.{ExecutionContext, Future, Promise}
import scala.collection.mutable
import zio.ZIO
import zio.test._

object DurableStreamSpec extends ZIOSpecDefault {
  private implicit val ec: ExecutionContext = ExecutionContext.parasitic
  private val noRetry                       = DurableStreamRetry(budgetMs = 0)
  private val transient                     = DurableStreamError(DurableStreamErrorKind.Transport, "connection lost")
  private val producer                      = DurableStreamProducer("producer-a", 7, 3)

  private class Host extends DurableStreamReadHost with DurableStreamWriteHost {
    val reads                                                                     = mutable.ArrayBuffer.empty[DurableStreamReadRequest]
    val writes                                                                    = mutable.ArrayBuffer.empty[DurableStreamAppendRequest]
    val batches                                                                   = mutable.Queue.empty[Future[DurableStreamBatch]]
    val receipts                                                                  = mutable.Queue.empty[Future[DurableStreamReceipt]]
    val sleeps                                                                    = mutable.ArrayBuffer.empty[Long]
    var time                                                                      = BigInt(12000)
    var waitFor: Long => Future[Unit]                                             = ms => { time += ms; Future.successful(()) }
    def nowMs(): BigInt                                                           = time
    def sleep(ms: Long): Future[Unit]                                             = { sleeps += ms; waitFor(ms) }
    def dispose(): Future[Unit]                                                   = Future.successful(())
    def read(request: DurableStreamReadRequest): Future[DurableStreamBatch]       = { reads += request; batches.dequeue() }
    def append(request: DurableStreamAppendRequest): Future[DurableStreamReceipt] = {
      writes += request; receipts.dequeue()
    }
  }

  private def batch(values: Int*)(offset: String = "opaque-a", closed: Boolean = false, upToDate: Boolean = true) =
    DurableStreamBatch(
      values.map(_.toByte).toVector,
      "application/octet-stream",
      DurableStreamCheckpoint(offset, Some("transport-cursor")),
      upToDate,
      closed
    )

  private def reader(host: Host, options: DurableStreamReadOptions = DurableStreamReadOptions(retry = noRetry)) =
    DurableStreamReader.create(host, options, (bytes: Vector[Byte]) => bytes, (byte: Byte) => byte)

  private def writer(host: Host, p: DurableStreamProducer = producer, retry: DurableStreamRetry = noRetry) =
    new DurableStreamWriter[Byte](host, p, retry, DurableStreamPayload.Bytes(_))

  private def receipt(sequence: Long = 3, closed: Boolean = false, nextOffset: Option[String] = Some("next-opaque")) =
    DurableStreamReceipt(nextOffset, 7, sequence, closed)

  override def spec = suite("DurableStreamSpec")(
    test("drains one batch before fetching, echoes opaque offset/cursor and delivers closed body") {
      ZIO.fromFuture { _ =>
        val host = new Host
        host.batches.enqueue(
          Future.successful(batch(11, 203)(upToDate = false)),
          Future.successful(batch(7)(offset = "not-a-number", closed = true))
        )
        val stream = reader(host)
        for {
          a     <- stream.pull()
          b     <- stream.pull()
          before = host.reads.size
          c     <- stream.pull()
          eof   <- stream.pull()
        } yield assertTrue(
          a.contains(11.toByte),
          b.contains(203.toByte),
          c.contains(7.toByte),
          eof.isEmpty,
          before == 1,
          host.reads.size == 2,
          host.reads(1).checkpoint == DurableStreamCheckpoint("opaque-a", Some("transport-cursor")),
          host.reads(1).transport == DurableStreamTransport.CatchUp,
          host.reads(1).contentType.contains("application/octet-stream")
        )
      }
    },
    test("now resolves once with catch-up then tails concrete checkpoint using SSE") {
      ZIO.fromFuture { _ =>
        val host = new Host
        host.batches
          .enqueue(Future.successful(batch()(offset = "fixed-tail")), Future.successful(batch(42)(closed = true)))
        val stream = reader(host, DurableStreamReadOptions(DurableStreamCheckpoint("now"), DurableStreamTransport.Sse))
        stream
          .pull()
          .map(value =>
            assertTrue(
              value.contains(42.toByte),
              host.reads.map(_.checkpoint.offset).toVector == Vector("now", "fixed-tail"),
              host.reads.map(_.transport).toVector == Vector(
                DurableStreamTransport.CatchUp,
                DurableStreamTransport.Sse
              ),
              host.sleeps.isEmpty
            )
          )
      }
    },
    test("only empty up-to-date responses wait; retry-after retries the identical request") {
      ZIO.fromFuture { _ =>
        val host = new Host
        host.batches.enqueue(
          Future.successful(batch()(offset = "catch-up-page-2", upToDate = false)),
          Future.successful(batch()()),
          Future.failed(transient.copy(retryAfterMs = Some(BigInt(350)))),
          Future.successful(batch(17)(closed = true))
        )
        val stream = reader(host, DurableStreamReadOptions(idleDelayMs = 19))
        stream
          .pull()
          .map(value =>
            assertTrue(
              value.contains(17.toByte),
              host.sleeps.toVector == Vector(19L, 350L),
              host.reads(2) == host.reads(3),
              host.reads(1).transport == DurableStreamTransport.CatchUp,
              host.reads(2).transport == DurableStreamTransport.LongPoll
            )
          )
      }
    },
    test("reconstructed retries exhaust the same budget") {
      ZIO.fromFuture { _ =>
        def run() = {
          val host = new Host
          host.batches.enqueue(Future.failed(transient), Future.failed(transient), Future.failed(transient))
          reader(host, DurableStreamReadOptions(retry = DurableStreamRetry(250, 100, 100)))
            .pull()
            .failed
            .map(error => (error, host.reads.toVector, host.sleeps.toVector))
        }
        for { first <- run(); replay <- run() } yield assertTrue(
          first == replay,
          first._2.size == 3,
          first._3 == Vector(100L, 100L)
        )
      }
    },
    test("reconstruction inside a batch restores its remainder before using its next offset") {
      ZIO.fromFuture { _ =>
        def setup() = {
          val host = new Host
          host.batches.enqueue(Future.successful(batch(9, 201, 44)()), Future.successful(batch(17)(closed = true)))
          (host, reader(host))
        }
        val (liveHost, live)     = setup()
        val (replayHost, replay) = setup()
        for {
          _              <- live.pull()
          _              <- live.pull()
          _              <- replay.pull()
          _              <- replay.pull()
          remainder      <- replay.pull()
          readsBeforeNext = replayHost.reads.size
          next           <- replay.pull()
          liveRemainder  <- live.pull()
          liveNext       <- live.pull()
        } yield assertTrue(
          remainder.contains(44.toByte),
          next.contains(17.toByte),
          remainder == liveRemainder,
          next == liveNext,
          readsBeforeNext == 1,
          replayHost.reads.toVector == liveHost.reads.toVector
        )
      }
    },
    test("host-accepted equivalent MIME spellings keep the original content type pinned") {
      ZIO.fromFuture { _ =>
        val host     = new Host
        val original = "Application/Octet-Stream; charset=UTF-8; profile=sample"
        host.batches.enqueue(
          Future.successful(batch(1)().copy(contentType = original)),
          Future.successful(batch(2)().copy(contentType = "application/octet-stream; profile=sample; charset=utf-8")),
          Future.successful(
            batch(3)(closed = true).copy(contentType = "application/octet-stream; charset=utf-8; profile=sample")
          )
        )
        val stream = reader(host)
        for {
          first  <- stream.pull()
          second <- stream.pull()
          third  <- stream.pull()
          eof    <- stream.pull()
        } yield assertTrue(
          first.contains(1.toByte),
          second.contains(2.toByte),
          third.contains(3.toByte),
          eof.isEmpty,
          host.reads.map(_.contentType).toVector == Vector(None, Some(original), Some(original))
        )
      }
    },
    test("host-rejected content type changes remain failures") {
      ZIO.fromFuture { _ =>
        val host    = new Host
        val failure = DurableStreamError(DurableStreamErrorKind.ProtocolError, "stream content type changed")
        host.batches.enqueue(
          Future.successful(batch(1)()),
          Future.failed(failure)
        )
        val stream = reader(host)
        for { _ <- stream.pull(); error <- stream.pull().failed } yield assertTrue(
          error == failure,
          host.reads.size == 2,
          host.reads(1).contentType.contains("application/octet-stream")
        )
      }
    },
    test("gone and native failures remain failures, including after schema transfer") {
      ZIO.fromFuture { _ =>
        val host = new Host
        val gone = DurableStreamError(DurableStreamErrorKind.Gone, "retention lost")
        host.batches.enqueue(Future.failed(gone))
        val original    = reader(host)
        val transferred = implicitly[FromSchema[AgentStream[Byte]]]
          .fromValue(implicitly[IntoSchema[AgentStream[Byte]]].toValue(original))
          .toOption
          .get
        for { failure <- transferred.pull().failed; again <- transferred.pull().failed } yield assertTrue(
          failure == gone,
          again == gone,
          host.reads.size == 1,
          host.sleeps.isEmpty
        )
      }
    },
    test("close cancels demand, discards late batch and prevents retries after timer") {
      ZIO.fromFuture { _ =>
        val host = new Host
        val gate = Promise[Unit]()
        host.waitFor = _ => gate.future
        host.batches.enqueue(Future.failed(transient))
        val stream  = reader(host, DurableStreamReadOptions())
        val pulling = stream.pull()
        for {
          _       <- stream.close()
          failure <- pulling.failed
          _        = gate.success(())
          again   <- stream.pull().failed
        } yield assertTrue(failure.getMessage.contains("closed"), again == failure, host.reads.size == 1)
      }
    },
    test("closing an active import discards a late result and forbids another pull") {
      ZIO.fromFuture { _ =>
        val host = new Host
        val gate = Promise[DurableStreamBatch]()
        host.batches.enqueue(gate.future)
        val stream  = reader(host)
        val pulling = stream.pull()
        for {
          concurrent <- stream.pull().failed
          _          <- stream.close()
          _           = gate.success(batch(99)(closed = true))
          cancelled  <- pulling.failed
        } yield assertTrue(
          concurrent.getMessage.contains("active pull"),
          cancelled.getMessage.contains("closed"),
          host.reads.size == 1
        )
      }
    },
    test("uncertain append freezes bytes, tuple and close flag; only acknowledgement advances") {
      ZIO.fromFuture { _ =>
        val host = new Host
        host.receipts.enqueue(
          Future.failed(transient),
          Future.successful(receipt(nextOffset = None)),
          Future.successful(receipt(4, closed = true))
        )
        val stream = writer(host)
        val values = mutable.ArrayBuffer[Byte](3, 99)
        for {
          _            <- stream.append(values).failed
          _             = values(0) = 77
          rejected     <- stream.append(Vector(55.toByte), close = true).failed
          acknowledged <- stream.retryPending()
          _            <- stream.close()
        } yield assertTrue(
          acknowledged.nextOffset.isEmpty,
          rejected.getMessage.contains("pending"),
          host.writes(0) == host.writes(1),
          host.writes(0).payload == DurableStreamPayload.Bytes(Vector(3, 99)),
          host.writes(0).producer == producer,
          host.writes(2).producer.sequence == 4,
          host.writes(2).close,
          host.writes(2).payload.isEmpty,
          stream.isClosed,
          !stream.hasPending
        )
      }
    },
    test("cancelled append ignores late acknowledgement and retries the same tuple") {
      ZIO.fromFuture { _ =>
        val host = new Host
        val gate = Promise[DurableStreamReceipt]()
        host.receipts.enqueue(gate.future, Future.successful(receipt(closed = true, nextOffset = None)))
        val stream  = writer(host)
        val attempt = stream.append(Vector(31.toByte), close = true)
        stream.cancel()
        for {
          cancelled <- attempt.failed
          busy      <- stream.retryPending().failed
          _          = gate.success(receipt(closed = true))
          retained   = stream.hasPending && !stream.isClosed
          _         <- stream.retryPending()
        } yield assertTrue(
          cancelled.getMessage.contains("cancelled"),
          busy.getMessage.contains("active"),
          retained,
          host.writes(0) == host.writes(1),
          stream.isClosed,
          !stream.hasPending
        )
      }
    },
    test("ahead receipt is divergence, behind is protocol failure; neither renumbers uncertain data") {
      ZIO.fromFuture { _ =>
        val host = new Host
        host.receipts
          .enqueue(Future.successful(receipt(5)), Future.successful(receipt(2)), Future.successful(receipt()))
        val stream = writer(host)
        for {
          ahead  <- stream.append(Vector(8.toByte)).failed
          behind <- stream.retryPending().failed
          _      <- stream.retryPending()
        } yield assertTrue(
          ahead.asInstanceOf[DurableStreamError].kind == DurableStreamErrorKind.ProducerDiverged,
          behind.asInstanceOf[DurableStreamError].kind == DurableStreamErrorKind.ProtocolError,
          host.writes.distinct.size == 1,
          !stream.hasPending
        )
      }
    },
    test("writer automatically retries typed transient failures with immutable request") {
      ZIO.fromFuture { _ =>
        val host = new Host
        host.receipts.enqueue(Future.failed(transient), Future.successful(receipt()))
        val stream = writer(host, retry = DurableStreamRetry())
        stream
          .append(Vector(23.toByte))
          .map(_ =>
            assertTrue(
              host.writes.size == 2,
              host.writes.distinct.size == 1,
              host.sleeps.toVector == Vector(100L),
              !stream.hasPending
            )
          )
      }
    },
    test("independent writers acknowledge in either order without sharing pending state") {
      ZIO.fromFuture { _ =>
        def run(reverse: Boolean) = {
          val host   = new Host
          val first  = Promise[DurableStreamReceipt]()
          val second = Promise[DurableStreamReceipt]()
          host.receipts.enqueue(first.future, second.future)
          val a     = writer(host)
          val b     = writer(host, producer.copy(id = "producer-b", sequence = 11))
          val sentA = a.append(Vector(3.toByte))
          val sentB = b.append(Vector(89.toByte), close = true)
          if (reverse) { second.success(receipt(11, closed = true)); first.success(receipt()) }
          else { first.success(receipt()); second.success(receipt(11, closed = true)) }
          for { _ <- sentA; _ <- sentB } yield assertTrue(
            !a.hasPending,
            !a.isClosed,
            !b.hasPending,
            b.isClosed,
            host.writes.map(_.producer.id).toVector == Vector("producer-a", "producer-b"),
            host.writes.map(_.producer.sequence).toVector == Vector(3L, 11L)
          )
        }
        for { forward <- run(false); reverse <- run(true) } yield forward && reverse
      }
    },
    test("empty append rejected before host; max sequence is acknowledged once without overflow") {
      ZIO.fromFuture { _ =>
        val host = new Host
        host.receipts.enqueue(Future.successful(receipt(DurableStreamProducer.MaxInteger)))
        val stream = writer(host, producer.copy(sequence = DurableStreamProducer.MaxInteger))
        for {
          _         <- stream.append(Vector.empty).failed
          _         <- stream.append(Vector(1.toByte))
          exhausted <- stream.append(Vector(2.toByte)).failed
        } yield assertTrue(host.writes.size == 1, exhausted.getMessage.contains("exhausted"), !stream.hasPending)
      }
    }
  )
}
