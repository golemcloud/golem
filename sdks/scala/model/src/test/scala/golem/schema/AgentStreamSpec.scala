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
  override def spec: Spec[TestEnvironment, Any] =
    suite("AgentStreamSpec")(
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
      }
    )
}
