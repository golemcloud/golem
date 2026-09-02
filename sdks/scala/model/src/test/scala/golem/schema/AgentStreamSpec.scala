/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 */
package golem.schema

import scala.concurrent.{Future, Promise}
import scala.util.Try
import zio.ZIO
import zio.test._

object AgentStreamSpec extends ZIOSpecDefault {
  private val intoSchemaWithoutExecutionContext = implicitly[IntoSchema[AgentStream[String]]]
  private val fromSchemaWithoutExecutionContext = implicitly[FromSchema[AgentStream[String]]]

  override def spec: Spec[TestEnvironment, Any] =
    suite("AgentStreamSpec")(
      test("stream codecs do not require a caller execution context") {
        assertTrue(
          intoSchemaWithoutExecutionContext.graph.root.body.isInstanceOf[SchemaTypeBody.StreamType],
          fromSchemaWithoutExecutionContext ne null
        )
      },
      test("encoding transfers the stream exactly once") {
        ZIO.fromFuture { implicit ec =>
          val stream = AgentStream.fromPull(() => Future.successful(Some("value")))
          val codec  = implicitly[IntoSchema[AgentStream[String]]]
          val first  = codec.toValue(stream)
          val second = Try(codec.toValue(stream))

          stream.pull().failed.map { pullFailure =>
            assertTrue(
              first.isInstanceOf[SchemaValue.StreamValue],
              second.isFailure,
              pullFailure.getMessage.contains("transferred")
            )
          }
        }
      },
      test("a second concurrent pull is rejected") {
        ZIO.fromFuture { implicit ec =>
          val pending = Promise[Option[String]]()
          val stream  = AgentStream.fromPull(() => pending.future)
          val first   = stream.pull()
          val second  = stream.pull()

          second.failed.flatMap { failure =>
            pending.success(None)
            first.map(result => assertTrue(failure.getMessage.contains("active pull"), result.isEmpty))
          }
        }
      },
      test("normal completion is cached and finalizes exactly once") {
        ZIO.fromFuture { implicit ec =>
          var pulls         = 0
          var finalizations = 0
          val stream        = AgentStream.fromPull(
            () => {
              pulls += 1
              Future.successful(None)
            },
            () => {
              finalizations += 1
              Future.successful(())
            }
          )

          for {
            first  <- stream.pull()
            second <- stream.pull()
            _      <- stream.close()
          } yield assertTrue(first.isEmpty, second.isEmpty, pulls == 1, finalizations == 1)
        }
      },
      test("producer failure is cached and finalizes exactly once") {
        ZIO.fromFuture { implicit ec =>
          val failure       = new RuntimeException("producer failed")
          var pulls         = 0
          var finalizations = 0
          val stream        = AgentStream.fromPull[String](
            () => {
              pulls += 1
              Future.failed(failure)
            },
            () => {
              finalizations += 1
              Future.successful(())
            }
          )

          for {
            first  <- stream.pull().failed
            second <- stream.pull().failed
            _      <- stream.close()
          } yield assertTrue(first eq failure, second eq failure, pulls == 1, finalizations == 1)
        }
      },
      test("close fails an active pull and suppresses its late result") {
        ZIO.fromFuture { implicit ec =>
          val source        = Promise[Option[String]]()
          var finalizations = 0
          var pullWasFailed = false
          var pull          = Option.empty[Future[Option[String]]]
          val stream        = AgentStream.fromPull(
            () => source.future,
            () => {
              finalizations += 1
              pullWasFailed = pull.exists(_.value.exists(_.isFailure))
              Future.successful(())
            }
          )
          pull = Some(stream.pull())

          for {
            _           <- stream.close()
            pullFailure <- pull.get.failed
            _            = source.success(Some("late"))
            nextFailure <- stream.pull().failed
            _           <- stream.close()
          } yield assertTrue(
            pullFailure.getMessage.contains("closed"),
            nextFailure.getMessage.contains("closed"),
            pullWasFailed,
            finalizations == 1
          )
        }
      },
      test("map transfers lifecycle ownership") {
        ZIO.fromFuture { implicit ec =>
          var item          = Option(1)
          var finalizations = 0
          val original      = AgentStream.fromPull(
            () => {
              val result = item
              item = None
              Future.successful(result)
            },
            () => {
              finalizations += 1
              Future.successful(())
            }
          )
          val mapped = original.map(_ + 1)

          for {
            originalFailure <- original.close().failed
            first           <- mapped.pull()
            end             <- mapped.pull()
            _               <- mapped.close()
          } yield assertTrue(
            originalFailure.getMessage.contains("transferred"),
            first.contains(2),
            end.isEmpty,
            finalizations == 1
          )
        }
      },
      test("transferred streams own nested acquisitions until the transferred endpoint closes") {
        ZIO.fromFuture { implicit ec =>
          val invocationOwnership = new AgentStreamOwnership
          val outer               = AgentStreamOwnership.capture(invocationOwnership) {
            AgentStream.fromPull[SchemaValue](() => Future.successful(None))
          }
          val handle   = outer.moveToSchemaValueStream(identity)
          val endpoint = handle.take().get
          endpoint.commitTransfer()

          var nestedFinalizations = 0
          val nested              = AgentStreamOwnership.capture(endpoint.activeOwnership) {
            AgentStream.fromPull[String](
              () => Future.successful(None),
              () => {
                nestedFinalizations += 1
                Future.successful(())
              }
            )
          }

          for {
            _       <- invocationOwnership.close()
            before   = nestedFinalizations
            _       <- endpoint.dispose()
            failure <- nested.pull().failed
          } yield assertTrue(
            before == 0,
            nestedFinalizations == 1,
            failure.getMessage.contains("closed")
          )
        }
      },
      test("a synchronous wrapped-stream failure is cached before finalization") {
        ZIO.fromFuture { implicit ec =>
          val failure = new RuntimeException("unwrap failed")
          var unwraps = 0
          val handle  = GuestSchemaValueStreamHandle.wrapped(
            new Object,
            () => {
              unwraps += 1
              throw failure
            }
          )
          val stream = implicitly[FromSchema[AgentStream[String]]]
            .fromValue(SchemaValue.StreamValue(handle))
            .toOption
            .get

          for {
            pullFailure  <- stream.pull().failed
            closeFailure <- stream.close().failed
          } yield assertTrue(pullFailure eq failure, closeFailure eq failure, unwraps == 1)
        }
      },
      test("wrapped disposal closes transferred ownership when unwrapping fails") {
        ZIO.fromFuture { implicit ec =>
          val failure             = new RuntimeException("unwrap failed")
          val invocationOwnership = new AgentStreamOwnership
          val handle              = AgentStreamOwnership.capture(invocationOwnership) {
            GuestSchemaValueStreamHandle.wrapped(new Object, () => Future.failed(failure))
          }
          val endpoint = handle.take().get
          endpoint.commitTransfer()

          var nestedFinalizations = 0
          val nested              = AgentStreamOwnership.capture(endpoint.activeOwnership) {
            AgentStream.fromPull[String](
              () => Future.successful(None),
              () => {
                nestedFinalizations += 1
                Future.successful(())
              }
            )
          }

          for {
            _            <- invocationOwnership.close()
            before        = nestedFinalizations
            disposeError <- endpoint.dispose().failed
            nestedError  <- nested.pull().failed
          } yield assertTrue(
            before == 0,
            disposeError eq failure,
            nestedFinalizations == 1,
            nestedError.getMessage.contains("closed")
          )
        }
      }
    )
}
