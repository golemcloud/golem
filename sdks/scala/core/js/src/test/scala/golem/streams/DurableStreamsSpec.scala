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
import scala.concurrent.{ExecutionContext, Future}
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

  private def install(host: js.Object): Unit = js.Dynamic.global.globalThis.updateDynamic("__golemScalaTestHost")(host)

  override def spec = suite("DurableStreamsSpec")(
    test("JSON reads exact integers and nested arrays, decoding only the demanded message") {
      ZIO.fromFuture { _ =>
        var requests = 0
        install(
          js.Dynamic.literal(readDurableStreamBatch =
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
        } yield assertTrue(
          first.contains(Vector(9007199254740993L, Long.MinValue)),
          second.contains(Vector(13L)),
          error != null,
          requests == 1
        )
      }
    },
    test("JSON appends send individually encoded values, not a flattened or lossy JS array") {
      ZIO.fromFuture { _ =>
        var payload  = Vector.empty[String]
        var sequence = ""
        install(
          js.Dynamic.literal(appendDurableStreamBatch =
            (
              (request: js.Dynamic, auth: js.Any) => {
                payload = request.payload.selectDynamic("val").asInstanceOf[js.Array[String]].toVector
                sequence = request.producer.sequence.toString
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
            readDurableStreamBatch = (
              (request: js.Dynamic, auth: js.Any) => {
                mode = request.mode.asInstanceOf[String]
                val response = batch("")
                response.payload = octets
                response.contentType = "application/octet-stream"
                js.Promise.resolve(response)
              }
            ): js.Function2[js.Dynamic, js.Any, js.Promise[js.Dynamic]],
            appendDurableStreamBatch = (
              (request: js.Dynamic, auth: js.Any) => {
                val payload = request.payload.selectDynamic("val").asInstanceOf[Uint8Array]
                payloads :+= Vector.tabulate(payload.length)(i => payload(i).toInt)
                producers :+= request.producer.id.asInstanceOf[String]
                sequences :+= request.producer.sequence.toString
                js.Promise.resolve(
                  js.Dynamic.literal(
                    epoch = js.BigInt("0"),
                    sequence = request.producer.sequence,
                    closed = request.close
                  )
                )
              }
            ): js.Function2[js.Dynamic, js.Any, js.Promise[js.Dynamic]]
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
            readDurableStreamBatch = (
              (request: js.Dynamic, auth: js.Any) => {
                reads += 1
                if (reads == 1)
                  js.Promise.reject(
                    js.Dynamic.literal(kind = "rate-limited", message = "slow down", retryAfterMs = js.BigInt("700"))
                  )
                else js.Promise.resolve(batch("[29]"))
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
        DurableStreams
          .json[Int](url)
          .pull()
          .map(value => assertTrue(value.contains(29), reads == 2, waits == Vector("700000000")))
      }
    },
    test("auth borrows a pinned config capability without calling Secret.get or reveal") {
      ZIO.fromFuture { _ =>
        var gets     = 0
        var borrowed = false
        val raw      = js.Dynamic.literal(marker = "opaque-secret")
        val tree     = SchemaWireInterop.valueTreeToJs(
          SchemaWire.schemaValueToWit(SchemaValue.SecretValue(GuestSecretHandle.fromRaw(raw)))
        )
        install(
          js.Dynamic.literal(
            getConfigValue =
              ((path: js.Any, graph: js.Any) => { gets += 1; tree }): js.Function2[js.Any, js.Any, js.Any],
            readDurableStreamBatch = (
              (request: js.Dynamic, auth: js.Any) => {
                borrowed = auth == raw
                js.Promise.resolve(batch("[1,2]"))
              }
            ): js.Function2[js.Dynamic, js.Any, js.Promise[js.Dynamic]]
          )
        )
        val secret = new Secret[String](List("token"), () => throw new AssertionError("must not reveal"))
        val stream = DurableStreams.json[Int](url, auth = Some(secret))
        for { first <- stream.pull(); second <- stream.pull() } yield assertTrue(
          first.contains(1),
          second.contains(2),
          gets == 1,
          borrowed
        )
      }
    },
    test("nested schema/RPC forwarding uses ordinary streams and rejects native producer errors") {
      ZIO.fromFuture { _ =>
        install(
          js.Dynamic.literal(readDurableStreamBatch =
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
        } yield assertTrue(!item.done.asInstanceOf[Boolean], error.toString.contains("native failure"))
      }
    }
  ) @@ TestAspect.sequential

  private def iterator(raw: js.Dynamic): js.Dynamic = {
    val iterable = raw.iterable
    val factory  = js.Dynamic.global.Reflect.get(iterable, js.Symbol.asyncIterator)
    factory.applyDynamic("call")(iterable)
  }
}
