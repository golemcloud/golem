/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 */
package golem.streams

import golem.FutureInterop
import golem.config.{ConfigLoader, Secret}
import scala.concurrent.{ExecutionContext, Future}
import scala.scalajs.js
import scala.scalajs.js.JSConverters._
import scala.scalajs.js.annotation.JSImport
import scala.scalajs.js.typedarray.Uint8Array

private[streams] final class DurableStreamHostApi(auth: Option[Secret[String]]) extends DurableStreamHost {
  import DurableStreamHostApi._
  private implicit val ec: ExecutionContext = ExecutionContext.parasitic
  // Keep the same capability/revision across all attempts without revealing its value.
  private lazy val handle                  = auth.map(secret => ConfigLoader.loadSecretHandle[String](secret.path))
  private def borrowed: js.UndefOr[js.Any] = handle
    .map(
      _.withHandle(_.asInstanceOf[js.Any])
        .getOrElse(throw new IllegalStateException("secret handle was already transferred"))
    )
    .orUndefined

  override def read(request: DurableStreamReadRequest): Future[DurableStreamBatch] =
    call(
      Host.readDurableStreamBatch(
        js.Dynamic.literal(
          url = request.url,
          checkpoint =
            js.Dynamic.literal(offset = request.checkpoint.offset, cursor = request.checkpoint.cursor.orUndefined),
          mode = (if (request.json) "json" else "bytes"),
          transport = (request.transport match {
            case DurableStreamTransport.CatchUp  => "catch-up"
            case DurableStreamTransport.LongPoll => "long-poll"
            case DurableStreamTransport.Sse      => "sse"
          }),
          contentType = request.contentType.orUndefined,
          timeoutMs = big(request.timeoutMs)
        ),
        borrowed
      )
    ).map { batch =>
      val payload = batch.payload.asInstanceOf[Uint8Array]
      DurableStreamBatch(
        Vector.tabulate(payload.length)(i => payload(i).toByte),
        batch.contentType.asInstanceOf[String],
        DurableStreamCheckpoint(
          batch.next.offset.asInstanceOf[String],
          batch.next.cursor.asInstanceOf[js.UndefOr[String]].toOption
        ),
        batch.upToDate.asInstanceOf[Boolean],
        batch.closed.asInstanceOf[Boolean]
      )
    }

  override def append(request: DurableStreamAppendRequest): Future[DurableStreamReceipt] = {
    val payload = request.payload match {
      case DurableStreamPayload.Json(values)  => js.Dynamic.literal(tag = "json", `val` = values.toJSArray)
      case DurableStreamPayload.Bytes(values) =>
        val bytes = new Uint8Array(values.size)
        values.indices.foreach(i => bytes(i) = (values(i) & 0xff).toShort)
        js.Dynamic.literal(tag = "bytes", `val` = bytes)
    }
    call(
      Host.appendDurableStreamBatch(
        js.Dynamic.literal(
          url = request.url,
          contentType = request.contentType,
          payload = payload,
          producer = js.Dynamic.literal(
            id = request.producer.id,
            epoch = big(request.producer.epoch),
            sequence = big(request.producer.sequence)
          ),
          close = request.close,
          timeoutMs = big(request.timeoutMs)
        ),
        borrowed
      )
    ).map { receipt =>
      DurableStreamReceipt(
        receipt.nextOffset.asInstanceOf[js.UndefOr[String]].toOption,
        integer(receipt.epoch).toLong,
        integer(receipt.sequence).toLong,
        receipt.closed.asInstanceOf[Boolean]
      )
    }
  }

  override def nowMs(): BigInt               = integer(Clock.now()) / 1000000
  override def sleep(ms: Long): Future[Unit] =
    FutureInterop.fromPromise(Clock.waitFor(js.BigInt((BigInt(ms) * 1000000).toString)))

  private def call[A](operation: => js.Promise[A]): Future[A] =
    Future.fromTry(scala.util.Try(operation)).flatMap(FutureInterop.fromPromise).recoverWith {
      case exception @ js.JavaScriptException(raw) =>
        if (raw != null && js.typeOf(raw) == "object" && js.typeOf(raw.asInstanceOf[js.Dynamic].kind) == "string") {
          val error = raw.asInstanceOf[js.Dynamic]
          kinds.get(error.kind.asInstanceOf[String]) match {
            case Some(kind) =>
              Future.failed(
                DurableStreamError(
                  kind,
                  error.message.asInstanceOf[String],
                  optionalInteger(error.retryAfterMs),
                  optionalInteger(error.producerEpoch),
                  optionalInteger(error.expectedSequence)
                )
              )
            case None => Future.failed(exception)
          }
        } else Future.failed(exception)
    }
}

private[streams] object DurableStreamHostApi {
  private def big(value: Long): js.BigInt                    = js.BigInt(value.toString)
  private def integer(value: js.Any): BigInt                 = BigInt(value.asInstanceOf[js.BigInt].toString)
  private def optionalInteger(value: js.Any): Option[BigInt] =
    value.asInstanceOf[js.UndefOr[js.BigInt]].toOption.map(integer)
  private val kinds = Map(
    "invalid-request"   -> DurableStreamErrorKind.InvalidRequest,
    "permission-denied" -> DurableStreamErrorKind.PermissionDenied,
    "not-found"         -> DurableStreamErrorKind.NotFound,
    "gone"              -> DurableStreamErrorKind.Gone,
    "closed"            -> DurableStreamErrorKind.Closed,
    "sequence-conflict" -> DurableStreamErrorKind.SequenceConflict,
    "fenced"            -> DurableStreamErrorKind.Fenced,
    "producer-diverged" -> DurableStreamErrorKind.ProducerDiverged,
    "protocol-error"    -> DurableStreamErrorKind.ProtocolError,
    "payload-too-large" -> DurableStreamErrorKind.PayloadTooLarge,
    "timeout"           -> DurableStreamErrorKind.Timeout,
    "transport"         -> DurableStreamErrorKind.Transport,
    "rate-limited"      -> DurableStreamErrorKind.RateLimited,
    "unavailable"       -> DurableStreamErrorKind.Unavailable
  )

  @js.native
  @JSImport("golem:agent/durable-streams@2.0.0", JSImport.Namespace)
  private object Host extends js.Object {
    def readDurableStreamBatch(request: js.Object, auth: js.UndefOr[js.Any]): js.Promise[js.Dynamic]   = js.native
    def appendDurableStreamBatch(request: js.Object, auth: js.UndefOr[js.Any]): js.Promise[js.Dynamic] = js.native
  }

  @js.native
  @JSImport("wasi:clocks/monotonic-clock@0.3.0", JSImport.Namespace)
  private object Clock extends js.Object {
    def now(): js.BigInt                              = js.native
    def waitFor(howLong: js.BigInt): js.Promise[Unit] = js.native
  }
}
