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
import golem.host.SchemaWireInterop
import golem.host.js.schema.JsTypedSchemaValue
import golem.host.js.tool.JsInvocationResult
import golem.runtime.tool.client.JsWireToolRpcTransport
import golem.runtime.tool.host.ToolHostApi
import golem.schema.{AgentStream, IntoSchema}
import golem.schema.wire.{SchemaWire, WitTypedSchemaValue}
import zio.ZIO
import zio.blocks.async.*
import zio.test.*

import scala.concurrent.{Future, Promise}
import scala.scalajs.js

object Gol40TerminalOwnershipAcceptanceSpec extends ZIOSpecDefault {
  private implicit val ec: scala.concurrent.ExecutionContext = golem.tool.ToolInvokerRuntime.executionContext

  private final case class StartedFixture(
    started: golem.tool.WireToolRpcStarted,
    completeOwner: () => Unit,
    observerCancellations: () => Int,
    readerCancellations: () => Int,
    observerDisposals: () => Int,
    getting: Future[Unit],
    disposed: Future[Unit],
    admit: () => Unit,
    completeWith: JsInvocationResult => Unit,
    rejectEncoding: () => Unit
  )

  private def startedFixture(deferEncoding: Boolean = false, getFailure: Option[Throwable] = None): StartedFixture = {
    var observerCancellations = 0
    var readerCancellations   = 0
    var observerDisposals     = 0
    val getting               = Promise[Unit]()
    val disposed              = Promise[Unit]()
    val encoded               = Promise[JsTypedSchemaValue]()
    val jsInput               = js.Dynamic.literal().asInstanceOf[JsTypedSchemaValue]
    if (!deferEncoding) encoded.success(jsInput)
    val owner    = Promise[JsInvocationResult]()
    val observer = js.Dynamic.literal(
      "get" -> js.Any.fromFunction0 { () =>
        getting.trySuccess(())
        getFailure.foreach(throw _)
        FutureInterop.toPromise(owner.future)
      },
      "cancel" -> js.Any.fromFunction0(() => observerCancellations += 1)
    )
    js.Dynamic.global.Reflect.set(
      observer,
      js.Dynamic.global.Symbol.selectDynamic("dispose"),
      js.Any.fromFunction0 { () =>
        observerDisposals += 1
        disposed.trySuccess(())
        ()
      }
    )
    val rpc = js.Dynamic.literal(
      "asyncInvokeAndAwait" -> js.Any.fromFunction5 { (_: js.Any, _: js.Any, _: js.Any, _: js.Any, _: js.Any) =>
        observer
      }
    )
    var outputs                                                          = 0
    def output(): (ToolHostApi.RawToolOutput, ToolHostApi.RawByteStream) = {
      outputs += 1
      val bytes    = if (outputs == 1) js.Array[Short](7, 11) else js.Array[Short](42, 43, 44)
      var ended    = false
      val done     = js.Dynamic.literal("done" -> true, "value" -> js.undefined)
      val iterator = js.Dynamic.literal(
        "next" -> js.Any.fromFunction0 { () =>
          val next =
            if (ended) done
            else
              js.Dynamic.literal(
                "done"  -> false,
                "value" -> js.Dynamic.literal("tag" -> "ok", "val" -> js.typedarray.Uint8Array.from(bytes))
              )
          ended = true
          js.Promise.resolve(next)
        },
        "return" -> js.Any.fromFunction0 { () =>
          readerCancellations += 1
          ended = true
          js.Promise.resolve(done)
        }
      )
      val rawStream = js.Dynamic.literal()
      js.Dynamic.global.Reflect.set(rawStream, js.Symbol.asyncIterator, js.Any.fromFunction0(() => iterator))
      (js.Dynamic.literal().asInstanceOf[ToolHostApi.RawToolOutput], rawStream.asInstanceOf[ToolHostApi.RawByteStream])
    }
    val transport = new JsWireToolRpcTransport(
      rpc.asInstanceOf[ToolHostApi.RawToolRpc],
      _ => encoded.future,
      () => output()
    )
    val started = transport
      .start(Nil, null.asInstanceOf[WitTypedSchemaValue], None, stdout = true, stderr = true)
      .toOption
      .get

    StartedFixture(
      started,
      () => owner.success(JsInvocationResult(js.undefined)),
      () => observerCancellations,
      () => readerCancellations,
      () => observerDisposals,
      getting.future,
      disposed.future,
      () => { encoded.success(jsInput); () },
      value => { owner.success(value); () },
      () => { encoded.failure(new IllegalArgumentException("bad encoding")); () }
    )
  }

  override def spec = suite("GOL-40 terminal ownership acceptance")(
    test("explicit invocation cancellation does not cancel the output reader") {
      val fixture = startedFixture()
      for {
        _ <- ZIO.fromFuture(_ => fixture.getting)
        _  = fixture.started.cancel()
        _  = fixture.completeOwner()
        _ <- ZIO.fromFuture(_ => fixture.disposed)
      } yield assertTrue(fixture.observerCancellations() == 1, fixture.readerCancellations() == 0)
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
    test("detach during get stops delivery and defers disposal without cancelling outputs or invocation") {
      val fixture = startedFixture()
      for {
        _      <- ZIO.fromFuture(_ => fixture.getting)
        _       = fixture.started.detach()
        _       = fixture.started.detach()
        result <- ZIO.fromFuture(_ => fixture.started.result).either
        before  = fixture.observerDisposals()
        _       = fixture.started.cancel()
        _       = fixture.completeOwner()
        _      <- ZIO.fromFuture(_ => fixture.disposed)
        _       = fixture.started.cancel()
      } yield assertTrue(
        result.left.toOption.exists(_.getMessage == "tool result observer detached"),
        before == 0,
        fixture.observerDisposals() == 1,
        fixture.observerCancellations() == 1,
        fixture.readerCancellations() == 0
      )
    },
    test("detach before get releases the observer without starting observation") {
      val fixture = startedFixture(deferEncoding = true)
      fixture.started.detach()
      for {
        error <- ZIO.fromFuture(_ => fixture.started.result.failed)
        _      = fixture.admit()
        _     <- ZIO.fromFuture(_ => fixture.disposed)
        _      = fixture.started.cancel()
      } yield assertTrue(
        error.getMessage == "tool result observer detached",
        !fixture.getting.isCompleted,
        fixture.observerDisposals() == 1,
        fixture.observerCancellations() == 0,
        fixture.readerCancellations() == 0
      )
    },
    test("queued invocation cancellation survives detach in either order before admission") {
      ZIO
        .foreach(List(true, false)) { cancelFirst =>
          val fixture = startedFixture(deferEncoding = true)
          if (cancelFirst) fixture.started.cancel()
          fixture.started.detach()
          if (!cancelFirst) fixture.started.cancel()
          fixture.admit()
          ZIO.fromFuture(_ => fixture.disposed).map { _ =>
            fixture.started.cancel()
            assertTrue(
              !fixture.getting.isCompleted,
              fixture.observerCancellations() == 1,
              fixture.observerDisposals() == 1,
              fixture.readerCancellations() == 0
            )
          }
        }
        .map(_.reduce(_ && _))
    },
    test("encoding failure after detach cannot replace the selected detach outcome") {
      val fixture = startedFixture(deferEncoding = true)
      fixture.started.detach()
      for {
        error <- ZIO.fromFuture(_ => fixture.started.result.failed)
        _      = fixture.rejectEncoding()
        after <- ZIO.fromFuture(_ => fixture.started.result.failed)
      } yield assertTrue(error eq after, after.getMessage == "tool result observer detached")
    },
    test("permanent output close and result detach leave the sibling output making progress") {
      val fixture = startedFixture()
      for {
        reader  <- ZIO.fromFuture(_ => fixture.started.stdout.get.stream.startAsync.start.toFuture)
        partial <- ZIO.fromFuture(_ => reader.read[Any](new Object).toFuture)
        _       <- ZIO.fromFuture(_ => fixture.started.stdout.get.cancel())
        _        = fixture.started.detach()
        sibling <- ZIO.fromFuture(_ => fixture.started.stderr.get.stream.runCollectAsync.toFuture)
        _        = fixture.completeOwner()
        _       <- ZIO.fromFuture(_ => fixture.disposed)
        _       <- ZIO.fromFuture(_ => reader.close().toFuture)
      } yield assertTrue(
        partial == 7.toByte,
        sibling.toOption.get.toList == List[Byte](42, 43, 44),
        fixture.observerCancellations() == 0,
        fixture.readerCancellations() == 2
      )
    },
    test("wire observation and decoding failures return protocol errors while real cancellation stays failed") {
      val malformed = SchemaWireInterop.typedToJs(SchemaWire.typedSchemaValueToWit(IntoSchema[String].toTyped("hello")))
      malformed.value.valueNodes(0).asInstanceOf[js.Dynamic].updateDynamic("tag")("not-a-schema-value")
      ZIO
        .foreach(List("get", "decode", "cancel")) { mode =>
          val error: Throwable =
            if (mode == "cancel") new java.util.concurrent.CancellationException("real cancel")
            else new IllegalArgumentException("get failed")
          val fixture = startedFixture(getFailure = if (mode == "decode") None else Some(error))
          if (mode == "decode") fixture.completeWith(JsInvocationResult(malformed))
          for {
            result <- ZIO.fromFuture(_ => fixture.started.result).either
            _      <- ZIO.fromFuture(_ => fixture.disposed)
          } yield assertTrue(
            if (mode == "cancel") result == Left(error)
            else
              result.toOption
                .exists(_.left.toOption.exists(_.isInstanceOf[golem.tool.WireToolRpcFailure.ProtocolError])),
            fixture.observerDisposals() == 1,
            fixture.observerCancellations() == 0,
            fixture.readerCancellations() == 0
          )
        }
        .map(_.reduce(_ && _))
    },
    test("late result capabilities are released on detach, but delivered capabilities remain caller-owned") {
      ZIO
        .foreach(List(true, false)) { detach =>
          val fixture  = startedFixture()
          var releases = 0
          val released = Promise[Unit]()
          val source   = AgentStream.fromPull[String](
            () => Future.successful(None),
            () => { releases += 1; released.trySuccess(()); Future.successful(()) }
          )
          val typed = SchemaWire.typedSchemaValueToWit(IntoSchema[AgentStream[String]].toTyped(source))
          for {
            encoded <- ZIO.fromFuture(_ => SchemaWireInterop.typedToJsAsync(typed))
            _       <- ZIO.fromFuture(_ => fixture.getting)
            _        = if (detach) fixture.started.detach()
            _        = fixture.completeWith(JsInvocationResult(encoded))
            _       <- ZIO.fromFuture(_ => fixture.disposed)
            result  <- ZIO.fromFuture(_ => fixture.started.result).either
            before   = releases
            _       <- if (detach) ZIO.unit
                 else
                   ZIO.fromFuture { _ =>
                     val value = result.toOption.get.toOption.get.result.get
                     SchemaWire.releaseOwned(SchemaWire.typedSchemaValueFromWit(value).value)
                     Future.successful(())
                   }
            _ <- ZIO.fromFuture(_ => released.future)
          } yield assertTrue(
            before == (if (detach) 1 else 0),
            releases == 1,
            fixture.observerDisposals() == 1,
            fixture.observerCancellations() == 0,
            fixture.readerCancellations() == 0
          )
        }
        .map(_.reduce(_ && _))
    }
  )
}
