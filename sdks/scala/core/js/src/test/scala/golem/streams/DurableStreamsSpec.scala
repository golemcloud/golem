/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 */
package golem.streams

import golem.FutureInterop
import golem.config.Secret
import golem.host.SchemaWireInterop
import golem.schema._
import golem.schema.wire.SchemaWire
import scala.concurrent.{ExecutionContext, Future, Promise}
import scala.scalajs.js
import scala.scalajs.js.typedarray.Uint8Array
import zio.ZIO
import zio.test._

object DurableStreamsSpec extends ZIOSpecDefault {
  private implicit val ec: ExecutionContext = ExecutionContext.parasitic
  private val url                           = "https://streams.example/events"

  private def bytes(value: String): Uint8Array = {
    val encoded = value.getBytes("UTF-8")
    val result  = new Uint8Array(encoded.length)
    encoded.indices.foreach(i => result(i) = (encoded(i) & 0xff).toShort)
    result
  }

  private def batch(body: String, closed: Boolean = true): js.Dynamic = js.Dynamic.literal(
    payload = bytes(body),
    contentType = "application/json",
    next = js.Dynamic.literal(offset = "opaque-tail", cursor = "cursor-x"),
    upToDate = true,
    closed = closed
  )

  private def resources(kind: String): js.Array[js.Dynamic] =
    js.Dynamic.global.globalThis.__golemDurableStreamResources.selectDynamic(kind).asInstanceOf[js.Array[js.Dynamic]]

  private def install(host: js.Object): Unit = {
    resources("readers").length = 0
    resources("writers").length = 0
    js.Dynamic.global.globalThis.updateDynamic("__golemScalaTestHost")(host)
  }

  override def spec = suite("DurableStreamsSpec")(
    test("JSON reads exact integers and nested arrays, decoding only the demanded message") {
      ZIO.fromFuture { _ =>
        var requests = 0
        install(
          js.Dynamic.literal(readStream =
            (
              (request: js.Dynamic, auth: js.Any) => {
                requests += 1
                js.Promise.resolve(batch("[[9007199254740993,-9223372036854775808],[13],false]"))
              }
            ): js.Function2[js.Dynamic, js.Any, js.Promise[js.Dynamic]]
          )
        )
        val stream = DurableStreams.json[Vector[Long]](url)
        for {
          first  <- stream.pull()
          second <- stream.pull()
          error  <- stream.pull().failed
          _      <- stream.close()
        } yield assertTrue(
          first.contains(Vector(9007199254740993L, Long.MinValue)),
          second.contains(Vector(13L)),
          error != null,
          requests == 1,
          resources("readers").length == 1,
          resources("readers")(0).drops.asInstanceOf[Int] == 1
        )
      }
    },
    test("JSON appends send individually encoded values, not a flattened or lossy JS array") {
      ZIO.fromFuture { _ =>
        var payload  = Vector.empty[String]
        var sequence = ""
        install(
          js.Dynamic.literal(appendStream =
            (
              (request: js.Dynamic, auth: js.Any) => {
                payload = request.payload.selectDynamic("val").asInstanceOf[js.Array[String]].toVector
                sequence = request.sequence.toString
                js.Promise.resolve(
                  js.Dynamic.literal(
                    nextOffset = "end",
                    epoch = js.BigInt("5"),
                    sequence = js.BigInt("2"),
                    closed = true
                  )
                )
              }
            ): js.Function2[js.Dynamic, js.Any, js.Promise[js.Dynamic]]
          )
        )
        val writer = DurableStreams.jsonWriter[Vector[Long]](url, DurableStreamProducer("p", 5, 2))
        writer.append(Vector(Vector(9007199254740993L, 7L), Vector(-23L)), close = true).map { receipt =>
          assertTrue(
            payload == Vector("[9007199254740993,7]", "[-23]"),
            sequence == "2",
            receipt.nextOffset.contains("end"),
            receipt.closed,
            writer.isClosed
          )
        }
      }
    },
    test("byte facade preserves octets and allocates one durable producer ID per writer") {
      ZIO.fromFuture { _ =>
        val octets = new Uint8Array(4)
        Vector(0, 127, 128, 255).zipWithIndex.foreach { case (value, index) => octets(index) = value.toShort }
        var ids       = 0
        var payloads  = Vector.empty[Vector[Int]]
        var producers = Vector.empty[String]
        var sequences = Vector.empty[String]
        var mode      = ""
        install(
          js.Dynamic.literal(
            generateIdempotencyKey = (() => {
              ids += 1; js.Dynamic.literal(highBits = js.BigInt("1"), lowBits = js.BigInt("2"))
            }): js.Function0[js.Dynamic],
            readStream = (
              (request: js.Dynamic, resource: js.Dynamic) => {
                mode = resource.options.mode.asInstanceOf[String]
                val response = batch("")
                response.payload = octets
                response.contentType = "application/octet-stream"
                js.Promise.resolve(response)
              }
            ): js.Function2[js.Dynamic, js.Dynamic, js.Promise[js.Dynamic]],
            appendStream = (
              (request: js.Dynamic, resource: js.Dynamic) => {
                val payload = request.payload.selectDynamic("val").asInstanceOf[Uint8Array]
                payloads :+= Vector.tabulate(payload.length)(i => payload(i).toInt)
                producers :+= resource.options.producerId.asInstanceOf[String]
                sequences :+= request.sequence.toString
                js.Promise.resolve(
                  js.Dynamic.literal(
                    epoch = js.BigInt("0"),
                    sequence = request.sequence,
                    closed = request.close
                  )
                )
              }
            ): js.Function2[js.Dynamic, js.Dynamic, js.Promise[js.Dynamic]]
          )
        )
        val stream = DurableStreams.bytes(url)
        val writer = DurableStreams.byteWriter(url)
        for {
          a        <- stream.pull(); b <- stream.pull(); c <- stream.pull(); d <- stream.pull(); eof <- stream.pull()
          appended <- writer.append(Vector(a.get, b.get, c.get, d.get))
          closed   <- writer.close()
        } yield assertTrue(
          mode == "bytes",
          eof.isEmpty,
          appended.nextOffset.isEmpty,
          closed.nextOffset.isEmpty,
          writer.isClosed,
          !writer.hasPending,
          payloads == Vector(Vector(0, 127, 128, 255), Vector.empty),
          ids == 1,
          resources("writers").length == 1,
          resources("writers")(0).drops.asInstanceOf[Int] == 1,
          producers == Vector.fill(2)("00000000-0000-0001-0000-000000000002"),
          sequences == Vector("0", "1")
        )
      }
    },
    test("host retry errors preserve typed metadata and use durable clock and timer units") {
      ZIO.fromFuture { _ =>
        var reads = 0
        var now   = BigInt(9000000000L)
        var waits = Vector.empty[String]
        install(
          js.Dynamic.literal(
            readStream = (
              (request: js.Dynamic, auth: js.Any) => {
                reads += 1
                if (reads == 1)
                  js.Promise.reject(
                    js.Dynamic.literal(kind = "rate-limited", message = "slow down", retryAfterMs = js.BigInt("700"))
                  )
                else js.Promise.resolve(batch(if (reads == 2) "[29]" else "[31]", closed = reads == 3))
              }
            ): js.Function2[js.Dynamic, js.Any, js.Promise[js.Dynamic]],
            now = (() => js.BigInt(now.toString)): js.Function0[js.BigInt],
            waitFor = ((duration: js.BigInt) => {
              waits :+= duration.toString
              now += BigInt(duration.toString)
              js.Promise.resolve(())
            }): js.Function1[js.BigInt, js.Promise[Unit]]
          )
        )
        val stream = DurableStreams.json[Int](
          url,
          DurableStreamReadOptions(
            checkpoint = DurableStreamCheckpoint("now"),
            liveTransport = DurableStreamTransport.Sse,
            timeoutMs = 12345
          )
        )
        val constructedBeforeRead = resources("readers").length == 1 && reads == 0
        for {
          first  <- stream.pull()
          second <- stream.pull()
          eof    <- stream.pull()
          _      <- stream.close()
        } yield {
          val resource = resources("readers")(0)
          val requests = resource.requests.asInstanceOf[js.Array[js.Dynamic]]
          assertTrue(
            constructedBeforeRead,
            resources("readers").length == 1,
            resource.options.url.asInstanceOf[String] == url,
            resource.options.mode.asInstanceOf[String] == "json",
            resource.options.timeoutMs.toString == "12345",
            first.contains(29),
            second.contains(31),
            eof.isEmpty,
            reads == 3,
            waits == Vector("700000000"),
            requests.map(_.checkpoint.offset.asInstanceOf[String]).toVector == Vector("now", "now", "opaque-tail"),
            requests.map(_.transport.asInstanceOf[String]).toVector == Vector("catch-up", "catch-up", "sse"),
            resource.drops.asInstanceOf[Int] == 1
          )
        }
      }
    },
    test("auth borrows a pinned config capability without calling Secret.get or reveal") {
      ZIO.fromFuture { _ =>
        var gets     = 0
        var borrowed = false
        var drops    = 0
        val raw      = js.Dynamic.literal(marker = "opaque-secret")
        js.Dynamic.global.Reflect
          .set(raw, js.Dynamic.global.Symbol.selectDynamic("dispose"), (() => { drops += 1 }): js.Function0[Unit])
        val tree = SchemaWireInterop.valueTreeToJs(
          SchemaWire.schemaValueToWit(SchemaValue.SecretValue(GuestSecretHandle.fromRaw(raw)))
        )
        install(
          js.Dynamic.literal(
            getConfigValue =
              ((path: js.Any, graph: js.Any) => { gets += 1; tree }): js.Function2[js.Any, js.Any, js.Any],
            readStream = (
              (request: js.Dynamic, resource: js.Dynamic) => {
                borrowed = resource.auth == raw
                js.Promise.resolve(batch("[1,2]"))
              }
            ): js.Function2[js.Dynamic, js.Dynamic, js.Promise[js.Dynamic]]
          )
        )
        val secret = new Secret[String](List("token"), () => throw new AssertionError("must not reveal"))
        val stream = DurableStreams.json[Int](url, auth = Some(secret))
        for { first <- stream.pull(); second <- stream.pull(); _ <- stream.close() } yield assertTrue(
          first.contains(1),
          second.contains(2),
          gets == 1,
          drops == 1,
          borrowed
        )
      }
    },
    test("constructor failure propagates synchronously and releases the borrowed secret handle") {
      var drops = 0
      val raw   = js.Dynamic.literal()
      js.Dynamic.global.Reflect
        .set(raw, js.Dynamic.global.Symbol.selectDynamic("dispose"), (() => { drops += 1 }): js.Function0[Unit])
      val tree = SchemaWireInterop.valueTreeToJs(
        SchemaWire.schemaValueToWit(SchemaValue.SecretValue(GuestSecretHandle.fromRaw(raw)))
      )
      install(
        js.Dynamic.literal(
          getConfigValue = ((path: js.Any, graph: js.Any) => tree): js.Function2[js.Any, js.Any, js.Any],
          constructStream = ((resource: js.Any) => {
            throw js.JavaScriptException(new js.Error("construction failed"))
          }): js.Function1[js.Any, Unit]
        )
      )
      val secret    = new Secret[String](List("token"), () => throw new AssertionError("must not reveal"))
      val attempted =
        scala.util.Try(DurableStreams.byteWriter(url, producer = DurableStreamProducer("p"), auth = Some(secret)))
      assertTrue(
        attempted.failed.get.toString.contains("construction failed"),
        drops == 1,
        resources("writers").length == 1,
        resources("writers")(0).requests.asInstanceOf[js.Array[js.Dynamic]].isEmpty
      )
    },
    test("nested schema/RPC forwarding uses ordinary streams and rejects native producer errors") {
      ZIO.fromFuture { _ =>
        install(
          js.Dynamic.literal(readStream =
            ((request: js.Dynamic, auth: js.Any) => js.Promise.reject(new js.Error("native failure"))): js.Function2[
              js.Dynamic,
              js.Any,
              js.Promise[js.Dynamic]
            ]
          )
        )
        val stream = DurableStreams.json[Int](url)
        val outer  = AgentStream.fromPull(() => Future.successful(Some(stream)))
        val value  = implicitly[IntoSchema[AgentStream[AgentStream[Int]]]].toValue(outer)
        for {
          encoded      <- SchemaWireInterop.valueTreeToJsAsync(SchemaWire.schemaValueToWit(value))
          outerRaw      = encoded.valueNodes(0).asInstanceOf[js.Dynamic].selectDynamic("val")
          outerIterator = iterator(outerRaw)
          item         <- FutureInterop.fromPromise(outerIterator.next().asInstanceOf[js.Promise[js.Dynamic]])
          innerRaw      = item.value.valueNodes.asInstanceOf[js.Array[js.Dynamic]](0).selectDynamic("val")
          error        <- FutureInterop.fromPromise(iterator(innerRaw).next().asInstanceOf[js.Promise[js.Dynamic]]).failed
          _            <- FutureInterop.fromPromise(outerIterator.applyDynamic("return")().asInstanceOf[js.Promise[js.Dynamic]])
        } yield assertTrue(
          !item.done.asInstanceOf[Boolean],
          error.toString.contains("native failure"),
          resources("readers").length == 1,
          resources("readers")(0).drops.asInstanceOf[Int] == 1
        )
      }
    },
    test(
      "construction is synchronous without HTTP, distinct instances own distinct handles and disposal is idempotent"
    ) {
      ZIO.fromFuture { _ =>
        install(js.Dynamic.literal())
        val a       = DurableStreams.bytes(url)
        val b       = DurableStreams.bytes(url + "/other")
        val x       = DurableStreams.byteWriter(url, producer = DurableStreamProducer("a", 4, 9), timeoutMs = 4321)
        val y       = DurableStreams.jsonWriter[Int](url + "/other", DurableStreamProducer("b", 6, 7))
        val readers = resources("readers").toVector
        val writers = resources("writers").toVector
        for {
          _     <- a.close(); _   <- a.close(); _   <- b.close()
          _     <- x.dispose(); _ <- x.dispose(); _ <- y.dispose()
          error <- x.append(Vector(1.toByte)).failed
        } yield assertTrue(
          readers.size == 2,
          writers.size == 2,
          readers(0) != readers(1),
          writers(0) != writers(1),
          readers(1).options.url.asInstanceOf[String] == url + "/other",
          writers(0).options.producerId.asInstanceOf[String] == "a",
          writers(0).options.producerEpoch.toString == "4",
          writers(0).options.timeoutMs.toString == "4321",
          writers(1).options.contentType.asInstanceOf[String] == "application/json",
          (readers ++ writers).forall(_.requests.asInstanceOf[js.Array[js.Dynamic]].isEmpty),
          (readers ++ writers).forall(_.drops.asInstanceOf[Int] == 1),
          error.getMessage.contains("disposed")
        )
      }
    },
    test("uncertain writer retries use the original resource, descriptor, encoded payload and sequence") {
      ZIO.fromFuture { _ =>
        var calls = 0
        install(
          js.Dynamic.literal(
            now = (() => js.BigInt("0")): js.Function0[js.BigInt],
            appendStream = (
              (request: js.Dynamic, resource: js.Dynamic) => {
                calls += 1
                if (calls == 1)
                  js.Promise.reject(js.Dynamic.literal(kind = "transport", message = "lost acknowledgement"))
                else
                  js.Promise.resolve(
                    js.Dynamic.literal(epoch = js.BigInt("8"), sequence = request.sequence, closed = request.close)
                  )
              }
            ): js.Function2[js.Dynamic, js.Dynamic, js.Promise[js.Dynamic]]
          )
        )
        val writer = DurableStreams.jsonWriter[Long](
          url,
          DurableStreamProducer("fixed", 8, 13),
          timeoutMs = 2468,
          retry = DurableStreamRetry(budgetMs = 0)
        )
        val constructedBeforeAppend = resources("writers").length == 1 && calls == 0
        val values                  = scala.collection.mutable.ArrayBuffer(9007199254740993L, 17L)
        for {
          _        <- writer.append(values).failed
          _         = values(0) = 3L
          rejected <- writer.append(Vector(99L)).failed
          retried  <- writer.retryPending()
          closed   <- writer.close()
          _        <- writer.dispose()
        } yield {
          val resource = resources("writers")(0)
          val requests = resource.requests.asInstanceOf[js.Array[js.Dynamic]].toVector
          assertTrue(
            constructedBeforeAppend,
            resources("writers").length == 1,
            resource.options.producerId.asInstanceOf[String] == "fixed",
            resource.options.producerEpoch.toString == "8",
            resource.options.timeoutMs.toString == "2468",
            resource.options.url.asInstanceOf[String] == url,
            requests.map(_.sequence.toString) == Vector("13", "13", "14"),
            requests
              .take(2)
              .forall(
                _.payload.selectDynamic("val").asInstanceOf[js.Array[String]].toVector == Vector(
                  "9007199254740993",
                  "17"
                )
              ),
            requests.map(_.close.asInstanceOf[Boolean]) == Vector(false, false, true),
            requests(2).payload.selectDynamic("val").asInstanceOf[js.Array[String]].isEmpty,
            retried.nextOffset.isEmpty,
            closed.nextOffset.isEmpty,
            writer.isClosed,
            !writer.hasPending,
            resource.drops.asInstanceOf[Int] == 1,
            rejected.getMessage.contains("pending")
          )
        }
      }
    },
    test("reader close and writer disposal defer resource drop until borrowed native calls settle") {
      ZIO.fromFuture { _ =>
        def run(fail: Boolean) = {
          val reading                                                    = Promise[js.Dynamic]()
          val writing                                                    = Promise[js.Dynamic]()
          val read: js.Function2[js.Any, js.Any, js.Promise[js.Dynamic]] =
            (request, resource) => FutureInterop.toPromise(reading.future)
          val write: js.Function2[js.Any, js.Any, js.Promise[js.Dynamic]] =
            (request, resource) => FutureInterop.toPromise(writing.future)
          install(js.Dynamic.literal(readStream = read, appendStream = write))
          val reader   = DurableStreams.json[Int](url)
          val writer   = DurableStreams.jsonWriter[Int](url, DurableStreamProducer("a"))
          val pull     = reader.pull()
          val append   = writer.append(Vector(5))
          val close    = reader.close()
          val dispose  = writer.dispose()
          val deferred =
            !close.isCompleted && !dispose.isCompleted && resources("readers")(0).drops.asInstanceOf[Int] == 0 &&
              resources("writers")(0).drops.asInstanceOf[Int] == 0
          if (fail) {
            reading.failure(new RuntimeException("read failed after close"))
            writing.failure(new RuntimeException("append failed after disposal"))
          } else {
            reading.success(batch("[99]"))
            writing.success(js.Dynamic.literal(epoch = js.BigInt("0"), sequence = js.BigInt("0"), closed = true))
          }
          for {
            readError <- pull.failed; writeError <- append.failed
            _         <- close; _                <- dispose
            retry     <- writer.retryPending().failed
          } yield assertTrue(
            deferred,
            readError.getMessage.contains("closed"),
            writeError.getMessage.contains("cancelled"),
            retry.getMessage.contains("disposed"),
            writer.hasPending,
            !writer.isClosed,
            resources("readers")(0).drops.asInstanceOf[Int] == 1,
            resources("writers")(0).drops.asInstanceOf[Int] == 1
          )
        }
        for { success <- run(false); failure <- run(true) } yield success && failure
      }
    }
  ) @@ TestAspect.sequential

  private def iterator(raw: js.Dynamic): js.Dynamic = {
    val iterable = raw.iterable
    val factory  = js.Dynamic.global.Reflect.get(iterable, js.Symbol.asyncIterator)
    factory.applyDynamic("call")(iterable)
  }
}
