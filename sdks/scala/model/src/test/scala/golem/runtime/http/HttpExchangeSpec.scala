// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
package golem.runtime.http

import golem.UShort
import golem.schema._
import golem.schema.SchemaValue._
import scala.concurrent.{Future, Promise}
import zio.ZIO
import zio.test._

object HttpExchangeSpec extends ZIOSpecDefault {
  private def request(body: AgentStream[Array[Byte]]) = HttpRequest(
    "m-CUSTOM",
    "https",
    "[::1]:8443",
    "/web/a%2Bb/",
    Some(""),
    List(HttpHeader("set-cookie", Array(97, 128.toByte)), HttpHeader.ascii("set-cookie", "b=two")),
    body
  )

  def spec = suite("HttpExchangeSpec")(
    test("canonical request fields preserve bytes, duplicates, path, and query without pulling") {
      ZIO.fromFuture { implicit ec =>
        var pulls     = 0
        var remaining = List(Array.emptyByteArray, Array[Byte](0, 128.toByte, 255.toByte), Array[Byte](42))
        val source    = AgentStream.fromPull { () =>
          pulls += 1
          val next = remaining.headOption
          remaining = remaining.drop(1)
          Future.successful(next)
        }
        val wire        = HttpRequest.intoSchema.toValue(request(source))
        val noEagerPull = pulls == 0
        val fields      = wire.asInstanceOf[RecordValue].fields
        val decoded     = HttpRequest.fromSchema.fromValue(wire).toOption.get
        for {
          moved <- source.pull().failed
          empty <- decoded.body.pull()
          chunk <- decoded.body.pull()
          last  <- decoded.body.pull()
          eof   <- decoded.body.pull()
          _     <- decoded.body.close()
        } yield assertTrue(
          noEagerPull,
          moved.getMessage.contains("transferred"),
          fields.take(5) == List(
            StringValue("m-CUSTOM"),
            StringValue("https"),
            StringValue("[::1]:8443"),
            StringValue("/web/a%2Bb/"),
            OptionValue(Some(StringValue("")))
          ),
          fields(5) == ListValue(
            List(
              RecordValue(List(StringValue("set-cookie"), ListValue(List(U8Value(97), U8Value(128))))),
              RecordValue(List(StringValue("set-cookie"), ListValue("b=two".map(c => U8Value(c.toInt)).toList)))
            )
          ),
          empty.exists(_.isEmpty),
          chunk.get.toList == List[Byte](0, 128.toByte, 255.toByte),
          last.get.toList == List[Byte](42),
          eof.isEmpty,
          decoded.headers.map(_.name) == List("set-cookie", "set-cookie"),
          decoded.query.contains(""),
          pulls == 4
        )
      }
    },
    test("response wire status is u16 and bytes are list<u8>, not signed bytes or binary") {
      ZIO.fromFuture { implicit ec =>
        val response =
          HttpResponse(UShort(599), Nil, AgentStream.fromPull(() => Future.successful(Some(Array[Byte](255.toByte)))))
        val wire   = HttpResponse.intoSchema.toValue(response).asInstanceOf[RecordValue]
        val stream = AgentStream
          .fromSchema[SchemaValue](new FromSchema[SchemaValue] {
            def fromValue(value: SchemaValue) = Right(value)
          })
          .fromValue(wire.fields(2))
          .toOption
          .get
        for { item <- stream.pull(); _ <- stream.close() } yield assertTrue(
          wire.fields.head == U16Value(599),
          item.contains(ListValue(List(U8Value(255))))
        )
      }
    },
    test("middleware scope lasts through consumption and closes an unpolled producer exactly once") {
      ZIO.fromFuture { implicit ec =>
        var pulls   = 0
        var closes  = 0
        var cleanup = 0
        val input   = AgentStream.fromPull[Array[Byte]](() => Future.successful(None))
        val handler = HttpHandler.ensuring(_ =>
          Future.successful(
            HttpResponse(
              UShort(200),
              Nil,
              AgentStream.fromPull(
                () => { pulls += 1; Future.successful(Some(Array[Byte](1))) },
                () => { closes += 1; Future.successful(()) }
              )
            )
          )
        ) { () => cleanup += 1; Future.successful(()) }
        for {
          response <- handler(request(input))
          before    = cleanup
          decoded   = HttpResponse.fromSchema.fromValue(HttpResponse.intoSchema.toValue(response)).toOption.get
          _        <- decoded.body.close()
          _        <- decoded.body.close()
          _        <- input.close()
        } yield assertTrue(before == 0, pulls == 0, closes == 1, cleanup == 1)
      }
    },
    test("closing a pending body releases it without waiting for its underlying pull") {
      ZIO.fromFuture { implicit ec =>
        val pending = Promise[Option[Array[Byte]]]()
        var closed  = 0
        val source  = AgentStream.fromPull(() => pending.future, () => { closed += 1; Future.successful(()) })
        val decoded = HttpResponse.fromSchema
          .fromValue(HttpResponse.intoSchema.toValue(HttpResponse(UShort(200), Nil, source)))
          .toOption
          .get
        val pulling = decoded.body.pull()
        for {
          _       <- decoded.body.close()
          failure <- pulling.failed
          _        = pending.success(Some(Array[Byte](9)))
        } yield assertTrue(closed == 1, failure.getMessage.contains("closed"))
      }
    },
    test("errors before the head and after it both finalize middleware without manufacturing EOF") {
      ZIO.fromFuture { implicit ec =>
        var finalized = 0
        def cleanup() = { finalized += 1; Future.successful(()) }
        val input     = AgentStream.fromPull[Array[Byte]](() => Future.successful(None))
        val failure   = new IllegalStateException("test failure")
        val before    = HttpHandler.ensuring(_ => Future.failed(failure))(() => cleanup())
        val after     = HttpHandler.ensuring(_ =>
          Future.successful(
            HttpResponse(UShort(200), Nil, AgentStream.fromPull[Array[Byte]](() => Future.failed(failure)))
          )
        )(() => cleanup())
        for {
          first    <- before(request(input)).failed
          response <- after(request(input))
          second   <- response.body.pull().failed
          _        <- response.body.close()
          _        <- input.close()
        } yield assertTrue(first eq failure, second eq failure, finalized == 2)
      }
    },
    test("strict mapping decoding preserves literal double-encoding and rejects adversarial paths") {
      val bad =
        List("/a//b", "/a/", "/%2f", "/%5c", "/%00", "/%7f", "/.%2e", "/%c0%af", "/%ff", "/%2", "/{x}", "/$name", "//*")
      assertTrue(
        bad.forall(p => FileMappingParser.compile(List(p -> "/ok")).isLeft),
        FileMappingParser.compile(List("/%252e%252e" -> "/%2e%2e")) == Right(
          List(FileMapping.Exact(List("%2e%2e"), "/%2e%2e"))
        ),
        FileMappingParser.compile(List("/*" -> "//$1")).isLeft,
        FileMappingParser.compile(List("/ok" -> "/bad\u0085")).isLeft,
        FileMappingParser.compile(List("/a" -> "/", "/*" -> "/x/$1")).isLeft
      )
    }
  )
}
