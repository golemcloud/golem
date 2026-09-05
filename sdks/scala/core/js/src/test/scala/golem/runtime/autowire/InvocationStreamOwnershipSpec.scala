/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 */
package golem.runtime.autowire

import golem.{FutureInterop, Principal}
import golem.runtime.{InputRecordCodec, MethodMetadata, OutputCodec}
import golem.schema.{AgentStream, FromSchema, FromSchemaError, IntoSchema, SchemaGraph, SchemaValue}
import zio.ZIO
import zio.test._

import scala.concurrent.Future
import scala.scalajs.js
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue

object InvocationStreamOwnershipSpec extends ZIOSpecDefault {
  private val booleanInput = InputRecordCodec.single[Boolean]("stream")

  private def ownedStreamInput(
    onFinalize: () => Unit,
    failAfterCreation: Boolean = false
  ): InputRecordCodec[AgentStream[String]] =
    new InputRecordCodec[AgentStream[String]] {
      override val userParams         = booleanInput.userParams
      override val graph: SchemaGraph = booleanInput.graph

      override def toValue(value: AgentStream[String]): SchemaValue = booleanInput.toValue(true)

      override def fromValue(value: SchemaValue): Either[FromSchemaError, AgentStream[String]] =
        booleanInput.fromValue(value).flatMap { _ =>
          var item   = Option("value")
          val stream = AgentStream.fromPull(
            () => {
              val result = item
              item = None
              Future.successful(result)
            },
            () => {
              onFinalize()
              Future.successful(())
            }
          )
          if (failAfterCreation) Left(FromSchemaError("rejected after stream creation"))
          else Right(stream)
        }
    }

  private def method[Out](
    input: InputRecordCodec[AgentStream[String]],
    output: OutputCodec[Out]
  )(
    handler: AgentStream[String] => Future[Out]
  ): MethodBinding[Unit] = {
    val metadata = MethodMetadata(
      "stream-method",
      None,
      None,
      None,
      input.inputMetadata,
      output.metadata
    )
    MethodBinding.async[Unit, AgentStream[String], Out](metadata, input, output)((_, stream, _) => handler(stream))
  }

  private def invoke[Out](binding: MethodBinding[Unit]) =
    FutureInterop.fromPromise(binding.invoke((), SchemaPayload.encode(true)(using booleanInput), Principal.Anonymous))

  override def spec: Spec[TestEnvironment, Any] =
    suite("InvocationStreamOwnershipSpec")(
      test("closes an unread input stream after a successful method") {
        var finalizations = 0
        val binding       =
          method(ownedStreamInput(() => finalizations += 1), OutputCodec.unit[Unit])(_ => Future.successful(()))

        ZIO.fromFuture(_ => invoke(binding)).map(_ => assertTrue(finalizations == 1))
      },
      test("closes a stream created before input decoding fails") {
        var finalizations = 0
        val binding       = method(
          ownedStreamInput(() => finalizations += 1, failAfterCreation = true),
          OutputCodec.unit[Unit]
        )(_ => Future.successful(()))

        ZIO.fromFuture(_ => invoke(binding).failed).map(_ => assertTrue(finalizations == 1))
      },
      test("closes an input stream when the handler fails") {
        var finalizations = 0
        val failure       = new RuntimeException("handler failed")
        val binding       =
          method(ownedStreamInput(() => finalizations += 1), OutputCodec.unit[Unit])(_ => Future.failed(failure))

        ZIO.fromFuture(_ => invoke(binding).failed).map(error => assertTrue(error eq failure, finalizations == 1))
      },
      test("closes an input stream when output encoding fails") {
        var finalizations = 0
        val standardInto  = IntoSchema[String]
        val failingInto   = new IntoSchema[String] {
          override val graph: SchemaGraph                  = standardInto.graph
          override def toValue(value: String): SchemaValue = throw new RuntimeException("encoding failed")
        }
        val output  = OutputCodec.single[String](using failingInto, FromSchema[String])
        val binding = method(ownedStreamInput(() => finalizations += 1), output)(_ => Future.successful("result"))

        ZIO.fromFuture(_ => invoke(binding).failed).map(_ => assertTrue(finalizations == 1))
      },
      test("closes a mapped input stream when the mapped stream is abandoned") {
        var finalizations = 0
        val binding       = method(
          ownedStreamInput(() => finalizations += 1),
          OutputCodec.unit[Unit]
        ) { stream =>
          val _ = stream.map(identity)
          Future.successful(())
        }

        ZIO.fromFuture(_ => invoke(binding)).map(_ => assertTrue(finalizations == 1))
      },
      test("rolls back a transferred input stream when a later output field encoder throws") {
        var finalizations = 0
        val streamInto    = IntoSchema[AgentStream[String]]
        val failingInto   = new IntoSchema[AgentStream[String]] {
          override val graph: SchemaGraph                               = streamInto.graph
          override def toValue(value: AgentStream[String]): SchemaValue = {
            streamInto.toValue(value)
            throw new RuntimeException("later field encoding failed")
          }
        }
        val output  = OutputCodec.single[AgentStream[String]](using failingInto, FromSchema[AgentStream[String]])
        val binding = method(
          ownedStreamInput(() => finalizations += 1),
          output
        )(Future.successful)

        ZIO.fromFuture(_ => invoke(binding).failed).map(_ => assertTrue(finalizations == 1))
      },
      test("rolls back a transferred input stream when output wire preflight fails") {
        var finalizations = 0
        val streamInto    = IntoSchema[AgentStream[String]]
        val failingInto   = new IntoSchema[AgentStream[String]] {
          override val graph: SchemaGraph                               = streamInto.graph
          override def toValue(value: AgentStream[String]): SchemaValue =
            SchemaValue.TupleValue(
              List(
                streamInto.toValue(value),
                SchemaValue.DatetimeValue(golem.schema.Datetime(0L, -1))
              )
            )
        }
        val output  = OutputCodec.single[AgentStream[String]](using failingInto, FromSchema[AgentStream[String]])
        val binding = method(
          ownedStreamInput(() => finalizations += 1),
          output
        )(Future.successful)

        ZIO.fromFuture(_ => invoke(binding).failed).map(_ => assertTrue(finalizations == 1))
      },
      test("releases an input stream handle even when input decoding never consumes it") {
        val rejectedInput = new InputRecordCodec[Unit] {
          override val userParams                                                   = booleanInput.userParams
          override val graph: SchemaGraph                                           = booleanInput.graph
          override def toValue(value: Unit): SchemaValue                            = SchemaValue.RecordValue(Nil)
          override def fromValue(value: SchemaValue): Either[FromSchemaError, Unit] =
            Left(FromSchemaError("first parameter was invalid"))
        }
        val binding = MethodBinding.async[Unit, Unit, Unit](
          MethodMetadata(
            "reject-stream",
            None,
            None,
            None,
            rejectedInput.inputMetadata,
            OutputCodec.unit[Unit].metadata
          ),
          rejectedInput,
          OutputCodec.unit[Unit]
        )((_, _, _) => Future.successful(()))

        val streamInto = IntoSchema[AgentStream[String]]
        val inputInto  = new IntoSchema[AgentStream[String]] {
          override val graph: SchemaGraph                               = streamInto.graph
          override def toValue(value: AgentStream[String]): SchemaValue =
            SchemaValue.RecordValue(List(SchemaValue.StringValue("invalid"), streamInto.toValue(value)))
        }
        var finalizations = 0
        val source        = AgentStream.fromPull[String](
          () => Future.successful(None),
          () => {
            finalizations += 1
            Future.successful(())
          }
        )

        ZIO.fromFuture { _ =>
          SchemaPayload.encodeAsync(source)(using inputInto).flatMap { input =>
            streamMock.reset()
            FutureInterop
              .fromPromise(binding.invoke((), input, Principal.Anonymous))
              .failed
              .map(_ => assertTrue(streamMock.state.unwraps.asInstanceOf[Int] == 1, finalizations == 1))
          }
        }
      },
      test("does not close an input stream transferred to the output") {
        var finalizations = 0
        val binding       = method(
          ownedStreamInput(() => finalizations += 1),
          OutputCodec.single[AgentStream[String]]
        )(Future.successful)

        ZIO.fromFuture { implicit ec =>
          invoke(binding).flatMap { output =>
            val stream = SchemaPayload.decode[AgentStream[String]](output.get).fold(throw _, identity)
            for {
              before <- Future.successful(finalizations)
              item   <- stream.pull()
              end    <- stream.pull()
            } yield assertTrue(before == 0, item.contains("value"), end.isEmpty, finalizations == 1)
          }
        }
      }
    ) @@ TestAspect.sequential

  private def streamMock: js.Dynamic =
    js.Dynamic.global.selectDynamic("__golemSchemaValueStreamMock")
}
