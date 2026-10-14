/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 */

package golem.runtime.tool

import golem.FutureInterop
import golem.host.js.schema.JsTypedSchemaValue
import golem.host.js.tool.JsInvocationResult
import golem.runtime.tool.client.JsWireToolRpcTransport
import golem.runtime.tool.host.ToolHostApi
import golem.schema.wire.WitTypedSchemaValue
import zio.ZIO
import zio.test.*

import scala.concurrent.{Future, Promise}
import scala.scalajs.js

object Gol40TerminalOwnershipAcceptanceSpec extends ZIOSpecDefault {
  private final case class StartedFixture(
    started: golem.tool.WireToolRpcStarted,
    completeOwner: () => Unit,
    observerCancellations: () => Int,
    readerCancellations: () => Int
  )

  private def startedFixture(): StartedFixture = {
    var observerCancellations = 0
    var readerCancellations   = 0
    val owner                 = Promise[JsInvocationResult]()
    val observer              = js.Dynamic.literal(
      "get"    -> js.Any.fromFunction0(() => FutureInterop.toPromise(owner.future)),
      "cancel" -> js.Any.fromFunction0(() => observerCancellations += 1)
    )
    val rpc = js.Dynamic.literal(
      "asyncInvokeAndAwait" -> js.Any.fromFunction5 { (_: js.Any, _: js.Any, _: js.Any, _: js.Any, _: js.Any) =>
        observer
      }
    )
    val done     = js.Dynamic.literal("done" -> true, "value" -> js.undefined)
    val iterator = js.Dynamic.literal(
      "next"   -> js.Any.fromFunction0(() => js.Promise.resolve(done)),
      "return" -> js.Any.fromFunction0 { () =>
        readerCancellations += 1
        js.Promise.resolve(done)
      }
    )
    val rawStream = js.Dynamic.literal()
    js.Dynamic.global.Reflect.set(rawStream, js.Symbol.asyncIterator, js.Any.fromFunction0(() => iterator))
    val transport = new JsWireToolRpcTransport(
      rpc.asInstanceOf[ToolHostApi.RawToolRpc],
      _ => Future.successful(js.Dynamic.literal().asInstanceOf[JsTypedSchemaValue]),
      () =>
        (
          js.Dynamic.literal().asInstanceOf[ToolHostApi.RawToolOutput],
          rawStream.asInstanceOf[ToolHostApi.RawByteStream]
        )
    )
    val started = transport
      .start(Nil, null.asInstanceOf[WitTypedSchemaValue], None, stdout = true, stderr = false)
      .toOption
      .get

    StartedFixture(
      started,
      () => owner.success(JsInvocationResult(js.undefined)),
      () => observerCancellations,
      () => readerCancellations
    )
  }

  override def spec = suite("GOL-40 terminal ownership acceptance")(
    test("explicit invocation cancellation does not cancel the output reader") {
      val fixture = startedFixture()
      fixture.started.cancel()
      assertTrue(fixture.observerCancellations() == 1, fixture.readerCancellations() == 0)
    },
    test("output-reader cancellation preserves the invocation result observer") {
      val fixture = startedFixture()
      for {
        _                    <- ZIO.fromFuture(_ => fixture.started.stdout.get.cancel())
        beforeOwnerCompletion = (fixture.observerCancellations(), fixture.readerCancellations())
        _                     = fixture.completeOwner()
        result               <- ZIO.fromFuture(_ => fixture.started.result)
      } yield assertTrue(
        beforeOwnerCompletion == (0, 1),
        result.isRight,
        fixture.observerCancellations() == 0,
        fixture.readerCancellations() == 1
      )
    },
    test("dropping the raw result observer does not cancel invocation or output") {
      val fixture          = startedFixture()
      val droppedResult    = fixture.started.result
      val beforeCompletion =
        (droppedResult.isCompleted, fixture.observerCancellations(), fixture.readerCancellations())
      fixture.completeOwner()

      ZIO.fromFuture(_ => fixture.started.result).map { result =>
        assertTrue(
          beforeCompletion == (false, 0, 0),
          result.isRight,
          fixture.observerCancellations() == 0,
          fixture.readerCancellations() == 0
        )
      }
    }
  )
}
