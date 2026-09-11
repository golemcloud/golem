/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.runtime.rpc

import golem.host.SchemaWireInterop
import golem.host.js.schema.JsSchemaValueTree
import golem.runtime.autowire.SchemaPayload
import golem.schema._
import golem.schema.wire.SchemaWire
import scala.concurrent.{Future, Promise}
import zio.ZIO
import zio.blocks.schema.Schema
import zio.test._

/**
 * Slice 4b — [[SchemaRpcCodec]] is the v2 (`golem:agent/host@2.0.0`)
 * RPC-boundary codec: it encodes the parameter-list `schema-value-tree`,
 * decodes the optional result tree (`unit` => `none`, `single` => `some`), and
 * carries `typed-schema-value`s for config / custom errors. These tests pin the
 * round-trips and the option/unit policy against the TS client semantics.
 */
object SchemaRpcCodecSpec extends ZIOSpecDefault {

  // Parameter-list records (the macro shapes method/constructor `In` this way).
  final case class Args2(a: Int, b: String)
  object Args2 {
    implicit val schema: Schema[Args2] = Schema.derived
  }

  final case class NoArgs()
  object NoArgs {
    implicit val schema: Schema[NoArgs] = Schema.derived
  }

  final case class Result(value: Int, label: String)
  object Result {
    implicit val schema: Schema[Result] = Schema.derived
  }

  final case class Config(host: String, port: Int)
  object Config {
    implicit val schema: Schema[Config] = Schema.derived
  }

  override def spec: Spec[TestEnvironment, Any] =
    suite("SchemaRpcCodecSpec")(
      suite("generated stream boundaries")(
        test("encoding does not prefetch and a pending read cannot request another item") {
          ZIO.fromFuture { implicit ec =>
            var pulls     = 0
            val requested = Promise[Unit]()
            val next      = Promise[Option[String]]()
            val source    = AgentStream.fromPull[String] { () =>
              pulls += 1
              requested.trySuccess(())
              next.future
            }
            for {
              tree   <- SchemaRpcCodec.encodeValueAsync(AgentStream.intoSchema[String].toValue(source))
              stream <-
                SchemaRpcCodec.decodeResultAsync {
                  AgentStream.fromSchema[String].fromValue(SchemaRpcCodec.decodeValue(tree)).fold(throw _, identity)
                }
              lazyBeforeRead = pulls == 0
              pending        = stream.pull()
              _             <- requested.future
              rejected      <- stream.pull().failed
              _              = next.success(Some("item"))
              item          <- pending
              _             <- stream.close()
            } yield assertTrue(
              lazyBeforeRead,
              pulls == 1,
              item.contains("item"),
              rejected.isInstanceOf[IllegalStateException]
            )
          }
        },
        test("failed nested item decode closes acquired and unvisited stream siblings") {
          ZIO.fromFuture { implicit ec =>
            var closed   = 0
            val original = new IllegalArgumentException("invalid nested sibling")
            val rawItems = new IntoSchema[SchemaValue] {
              def graph: SchemaGraph = {
                val stream = AgentStream.intoSchema[String].graph
                stream.copy(root = SchemaType(SchemaTypeBody.TupleType(List(stream.root, stream.root))))
              }
              def toValue(value: SchemaValue): SchemaValue = value
            }
            val source = AgentStream.fromPull(() =>
              Future.successful(
                Some(
                  SchemaValue.TupleValue(List.fill(2) {
                    AgentStream
                      .intoSchema[String]
                      .toValue(
                        AgentStream.fromPull[String](
                          () => Future.successful(Some("inner")),
                          () => { closed += 1; Future.failed(new RuntimeException("cleanup failed")) }
                        )
                      )
                  })
                )
              )
            )
            val itemDecoder = new FromSchema[AgentStream[String]] {
              def fromValue(value: SchemaValue): Either[FromSchemaError, AgentStream[String]] = {
                val SchemaValue.TupleValue(items) = value: @unchecked
                AgentStream.fromSchema[String].fromValue(items.head).fold(throw _, identity)
                throw original
              }
            }
            for {
              tree  <- SchemaRpcCodec.encodeValueAsync(AgentStream.intoSchema[SchemaValue](rawItems).toValue(source))
              outer <- SchemaRpcCodec.decodeResultAsync {
                         AgentStream
                           .fromSchema[AgentStream[String]](itemDecoder)
                           .fromValue(SchemaRpcCodec.decodeValue(tree))
                           .fold(throw _, identity)
                       }
              error <- outer.pull().failed
              _     <- outer.close().recover { case _ => () }
            } yield assertTrue(error eq original, closed == 2)
          }
        },
        test("successfully returned inner stream survives outer EOF") {
          ZIO.fromFuture { implicit ec =>
            var closed   = 0
            var produced = false
            val source   = AgentStream.fromPull[AgentStream[String]](() =>
              if (produced) Future.successful(None)
              else {
                produced = true
                Future.successful(
                  Some(
                    AgentStream.fromPull[String](
                      () => Future.successful(Some("inner")),
                      () => { closed += 1; Future.successful(()) }
                    )
                  )
                )
              }
            )
            for {
              tree  <- SchemaRpcCodec.encodeValueAsync(AgentStream.intoSchema[AgentStream[String]].toValue(source))
              outer <- SchemaRpcCodec.decodeResultAsync {
                         AgentStream
                           .fromSchema[AgentStream[String]]
                           .fromValue(SchemaRpcCodec.decodeValue(tree))
                           .fold(throw _, identity)
                       }
              inner <- outer.pull().map(_.get)
              end   <- outer.pull()
              _     <- outer.close()
              alive  = closed == 0
              item  <- inner.pull()
              _     <- inner.close()
            } yield assertTrue(end.isEmpty, alive, item.contains("inner"), closed == 1)
          }
        },
        test("by-name encoding rolls back an earlier sibling and preserves the original error") {
          ZIO.fromFuture { implicit ec =>
            var closed   = 0
            val original = new IllegalArgumentException("invalid sibling")
            val stream   = AgentStream.fromPull[String](
              () => Future.successful(Some("item")),
              () => { closed += 1; Future.failed(new RuntimeException("cleanup failed")) }
            )
            SchemaRpcCodec.encodeValueAsync {
              AgentStream.intoSchema[String].toValue(stream)
              throw original
            }.failed.map(error => assertTrue(error eq original, closed == 1))
          }
        },
        test("result decoding closes partial sibling acquisitions exactly once") {
          ZIO.fromFuture { implicit ec =>
            var closed   = 0
            val original = new IllegalArgumentException("invalid result sibling")
            val source   = AgentStream.fromPull[String](
              () => Future.successful(Some("item")),
              () => { closed += 1; Future.failed(new RuntimeException("cleanup failed")) }
            )
            for {
              tree  <- SchemaRpcCodec.encodeValueAsync(AgentStream.intoSchema[String].toValue(source))
              error <- SchemaRpcCodec.decodeResultAsync {
                         val raw = SchemaRpcCodec.decodeValue(tree)
                         AgentStream.fromSchema[String].fromValue(raw).fold(throw _, identity)
                         throw original
                       }.failed
            } yield assertTrue(error eq original, closed == 1)
          }
        },
        test("successful output survives decoding and forwards without invoking item codecs") {
          ZIO.fromFuture { implicit ec =>
            var closed = 0
            var pulls  = 0
            val source = AgentStream.fromPull[String](
              () => { pulls += 1; Future.successful(Some("item")) },
              () => { closed += 1; Future.successful(()) }
            )
            val neverDecode = new FromSchema[String] {
              def fromValue(value: SchemaValue): Either[FromSchemaError, String] =
                throw new AssertionError("forwarding decoded an item")
            }
            for {
              tree            <- SchemaRpcCodec.encodeValueAsync(AgentStream.intoSchema[String].toValue(source))
              originalEndpoint =
                tree.valueNodes(0).asInstanceOf[scala.scalajs.js.Dynamic].selectDynamic("val").asInstanceOf[AnyRef]
              stream <- SchemaRpcCodec.decodeResultAsync {
                          AgentStream
                            .fromSchema[String](neverDecode)
                            .fromValue(SchemaRpcCodec.decodeValue(tree))
                            .fold(throw _, identity)
                        }
              alive      = closed == 0 && pulls == 0
              forwarded <- SchemaRpcCodec.encodeValueAsync(AgentStream.intoSchema[String].toValue(stream))
              same       =
                originalEndpoint eq
                  forwarded
                    .valueNodes(0)
                    .asInstanceOf[scala.scalajs.js.Dynamic]
                    .selectDynamic("val")
                    .asInstanceOf[AnyRef]
              received <- SchemaRpcCodec.decodeResultAsync {
                            AgentStream
                              .fromSchema[String]
                              .fromValue(SchemaRpcCodec.decodeValue(forwarded))
                              .fold(throw _, identity)
                          }
              item <- received.pull()
              _    <- received.close()
              _    <- received.close()
            } yield assertTrue(alive, same, item.contains("item"), pulls == 1, closed == 1)
          }
        }
      ),
      suite("arguments (parameter-list value tree)")(
        test("encodeArgs/decodeArgs round-trip for a multi-field param list") {
          val in   = Args2(7, "hello")
          val tree = SchemaRpcCodec.encodeArgs(in)
          assertTrue(SchemaRpcCodec.decodeArgs[Args2](tree) == Right(in))
        },
        test("encodeArgs/decodeArgs round-trip for an empty param list") {
          val in   = NoArgs()
          val tree = SchemaRpcCodec.encodeArgs(in)
          assertTrue(SchemaRpcCodec.decodeArgs[NoArgs](tree) == Right(in))
        },
        test("encodeArgs equals SchemaPayload.encode (single value-tree hub)") {
          val in       = Args2(1, "x")
          val viaCodec = SchemaWireInterop.valueTreeFromJs(SchemaRpcCodec.encodeArgs(in))
          val viaHub   = SchemaWireInterop.valueTreeFromJs(SchemaPayload.encode(in))
          assertTrue(viaCodec == viaHub)
        }
      ),
      suite("results (option<schema-value-tree>)")(
        test("encodeUnitResult is absent (none on the wire)") {
          assertTrue(SchemaRpcCodec.encodeUnitResult.isEmpty)
        },
        test("decodeUnitResult is always () regardless of presence") {
          val absent: Option[JsSchemaValueTree]  = None
          val present: Option[JsSchemaValueTree] = Some(SchemaPayload.encode(Result(1, "a")))
          assertTrue(
            SchemaRpcCodec.decodeUnitResult(absent) == Right(()),
            SchemaRpcCodec.decodeUnitResult(present) == Right(())
          )
        },
        test("encodeSingleResult/decodeSingleResult round-trip") {
          val out  = Result(42, "answer")
          val some = SchemaRpcCodec.encodeSingleResult(out)
          assertTrue(
            some.isDefined,
            SchemaRpcCodec.decodeSingleResult[Result](some) == Right(out)
          )
        },
        test("decodeSingleResult on absent result is an error") {
          val decoded = SchemaRpcCodec.decodeSingleResult[Result](None)
          assertTrue(decoded.isLeft)
        }
      ),
      suite("typed-schema-value (config values, custom errors)")(
        test("encodeTyped/decodeTyped round-trip") {
          val cfg   = Config("localhost", 5432)
          val typed = SchemaRpcCodec.encodeTyped(cfg)
          assertTrue(SchemaRpcCodec.decodeTyped[Config](typed) == Right(cfg))
        },
        test("encodeTyped carries the self-contained graph of A") {
          val cfg      = Config("h", 1)
          val typed    = SchemaRpcCodec.encodeTyped(cfg)
          val graph    = SchemaWireInterop.graphFromJs(typed.graph)
          val expected = SchemaWire.schemaGraphToWit(IntoSchema[Config].graph)
          assertTrue(graph == expected)
        },
        test("typedConfigValue carries path + typed-schema-value") {
          val entry = SchemaRpcCodec.typedConfigValue(List("db", "primary"), Config("h", 2))
          assertTrue(
            entry.path.toList == List("db", "primary"),
            SchemaRpcCodec.decodeTyped[Config](entry.value) == Right(Config("h", 2))
          )
        }
      )
    )
}
