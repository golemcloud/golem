/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 */
package golem.streams

import golem.FutureInterop
import golem.config.{ConfigLoader, Secret}
import scala.concurrent.{ExecutionContext, Future, Promise}
import scala.scalajs.js
import scala.scalajs.js.JSConverters._
import scala.scalajs.js.annotation.JSImport
import scala.scalajs.js.typedarray.Uint8Array

private[streams] abstract class DurableStreamHostApi(resource: js.Dynamic) extends DurableStreamHost {
  import DurableStreamHostApi._
  protected implicit val ec: ExecutionContext = ExecutionContext.parasitic
  private var active                          = 0
  private var closing                         = false
  private val released                        = Promise[Unit]()

  override def dispose(): Future[Unit] = {
    closing = true
    releaseIfIdle()
    released.future
  }

  private def releaseIfIdle(): Unit =
    if (closing && active == 0 && !released.isCompleted)
      released.complete(scala.util.Try(drop(resource)))

  override def nowMs(): BigInt               = integer(Clock.now()) / 1000000
  override def sleep(ms: Long): Future[Unit] =
    FutureInterop.fromPromise(Clock.waitFor(js.BigInt((BigInt(ms) * 1000000).toString)))

  protected def call(operation: => js.Dynamic): Future[js.Dynamic] = {
    active += 1
    Future
      .fromTry(scala.util.Try(operation.asInstanceOf[js.Promise[js.Dynamic]]))
      .flatMap(FutureInterop.fromPromise)
      .transform { result =>
        active -= 1
        releaseIfIdle()
        result
      }
      .recoverWith { case exception @ js.JavaScriptException(raw) =>
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
}

private[streams] object DurableStreamHostApi {
  def reader(url: String, json: Boolean, timeoutMs: Long, auth: Option[Secret[String]]): DurableStreamReadHost = {
    val resource = construct(
      Host.DurableStreamReader,
      js.Dynamic.literal(url = url, mode = (if (json) "json" else "bytes"), timeoutMs = big(timeoutMs)),
      auth
    )
    new DurableStreamHostApi(resource) with DurableStreamReadHost {
      override def read(request: DurableStreamReadRequest): Future[DurableStreamBatch] =
        call(
          resource.read(
            js.Dynamic.literal(
              checkpoint =
                js.Dynamic.literal(offset = request.checkpoint.offset, cursor = request.checkpoint.cursor.orUndefined),
              transport = (request.transport match {
                case DurableStreamTransport.CatchUp  => "catch-up"
                case DurableStreamTransport.LongPoll => "long-poll"
                case DurableStreamTransport.Sse      => "sse"
              }),
              contentType = request.contentType.orUndefined
            )
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
    }
  }

  def writer(
    url: String,
    contentType: String,
    producer: DurableStreamProducer,
    timeoutMs: Long,
    auth: Option[Secret[String]]
  ): DurableStreamWriteHost = {
    require(timeoutMs >= 1 && timeoutMs <= 300000)
    val resource = construct(
      Host.DurableStreamWriter,
      js.Dynamic.literal(
        url = url,
        contentType = contentType,
        producerId = producer.id,
        producerEpoch = big(producer.epoch),
        timeoutMs = big(timeoutMs)
      ),
      auth
    )
    new DurableStreamHostApi(resource) with DurableStreamWriteHost {
      override def append(request: DurableStreamAppendRequest): Future[DurableStreamReceipt] = {
        val payload = request.payload match {
          case DurableStreamPayload.Json(values)  => js.Dynamic.literal(tag = "json", `val` = values.toJSArray)
          case DurableStreamPayload.Bytes(values) =>
            val bytes = new Uint8Array(values.size)
            values.indices.foreach(i => bytes(i) = (values(i) & 0xff).toShort)
            js.Dynamic.literal(tag = "bytes", `val` = bytes)
        }
        call(
          resource.append(
            js.Dynamic.literal(payload = payload, sequence = big(request.producer.sequence), close = request.close)
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
    }
  }

  private def construct(constructor: js.Dynamic, options: js.Object, auth: Option[Secret[String]]): js.Dynamic = {
    val handle = auth.map(secret => ConfigLoader.loadSecretHandle[String](secret.path))
    try {
      val borrowed = handle.map(_.withHandle(_.asInstanceOf[js.Any]).get).orUndefined
      js.Dynamic.newInstance(constructor)(options, borrowed)
    } finally handle.foreach(_.take().foreach(raw => drop(raw.asInstanceOf[js.Dynamic])))
  }

  private def drop(resource: js.Dynamic): Unit = {
    val symbol = js.Dynamic.global.Symbol.selectDynamic("dispose")
    js.Dynamic.global.Reflect.get(resource, symbol).applyDynamic("call")(resource)
    ()
  }

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
    val DurableStreamReader: js.Dynamic = js.native
    val DurableStreamWriter: js.Dynamic = js.native
  }

  @js.native
  @JSImport("wasi:clocks/monotonic-clock@0.3.0", JSImport.Namespace)
  private object Clock extends js.Object {
    def now(): js.BigInt                              = js.native
    def waitFor(howLong: js.BigInt): js.Promise[Unit] = js.native
  }
}
