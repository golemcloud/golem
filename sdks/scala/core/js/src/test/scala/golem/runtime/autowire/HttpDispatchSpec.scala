// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
package golem.runtime.autowire

import golem.{FutureInterop, Principal, UShort}
import golem.runtime.*
import golem.runtime.http.*
import golem.schema.*
import golem.schema.SchemaValue.*
import golem.schema.wire.SchemaWire
import scala.concurrent.{Future, Promise}
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
import zio.ZIO
import zio.test.*

object HttpDispatchSpec extends ZIOSpecDefault {
  private val input                  = InputRecordCodec.single[HttpRequest]("request")
  private val output                 = OutputCodec.single[HttpResponse]
  private def metadata(raw: Boolean) = MethodMetadata(
    "arbitrary",
    None,
    None,
    None,
    input.inputMetadata,
    output.metadata,
    httpEndpoints = if (raw) List(HttpEndpointDetails(HttpMethod.Any, Nil, Nil, Nil, None, None)) else Nil
  )
  private def request(method: String) = HttpRequest(
    method,
    "http",
    "localhost",
    "/",
    None,
    Nil,
    AgentStream.fromPull[Array[Byte]](() => Future.successful(None))
  )

  def spec = suite("HttpDispatchSpec")(
    suite("suppressed bodies")(
      List(("HEAD", 200), ("GET", 204), ("GET", 205), ("GET", 304)).map { case (verb, status) =>
        test(s"$verb/$status disposes without pulling and waits for cleanup") {
          var pulls     = 0
          var finalized = 0
          val started   = Promise[Unit]()
          val release   = Promise[Unit]()
          val binding   = MethodBinding.sync[Unit, HttpRequest, HttpResponse](metadata(true), input, output) { (_, _, _) =>
            HttpResponse(
              UShort(status),
              List(HttpHeader.ascii("content-length", "17")),
              AgentStream.fromPull(
                () => { pulls += 1; Future.failed(new IllegalStateException("must not pull")) },
                () => { finalized += 1; started.success(()); release.future }
              )
            )
          }
          ZIO.fromFuture { _ =>
            SchemaPayload.encodeAsync(request(verb))(input).flatMap { wire =>
              val invocation = FutureInterop.fromPromise(binding.invoke((), wire, Principal.Anonymous))
              for {
                _       <- started.future
                pending  = !invocation.isCompleted
                _        = release.success(())
                result  <- invocation
                response = SchemaPayload.decode[HttpResponse](result.get).toOption.get
                end     <- response.body.pull()
                _       <- response.body.close()
              } yield assertTrue(
                pending,
                pulls == 0,
                finalized == 1,
                end.isEmpty,
                response.status.value == status,
                response.headers.head.value.toList == "17".getBytes.toList
              )
            }
          }
        }
      }
    ),
    test("GET/200 and non-HTTP methods retain their producer") {
      ZIO
        .foreach(List((true, "GET"), (false, "HEAD"))) { case (raw, verb) =>
          var pulls   = 0
          val binding = MethodBinding.sync[Unit, HttpRequest, HttpResponse](metadata(raw), input, output) { (_, _, _) =>
            HttpResponse(
              UShort(200),
              Nil,
              AgentStream.fromPull { () =>
                pulls += 1; Future.successful(if (pulls == 1) Some(Array[Byte](128.toByte)) else None)
              }
            )
          }
          ZIO.fromFuture { _ =>
            for {
              wire    <- SchemaPayload.encodeAsync(request(verb))(input)
              result  <- FutureInterop.fromPromise(binding.invoke((), wire, Principal.Anonymous))
              response = SchemaPayload.decode[HttpResponse](result.get).toOption.get
              item    <- response.body.pull()
              _       <- response.body.close()
            } yield assertTrue(item.exists(_.toList == List(128.toByte)), pulls == 1)
          }
        }
        .map(_.reduce(_ && _))
    },
    test("structural codecs decode once; HEAD uses the original envelope and closes an echoed body") {
      var decoded         = 0
      var finalized       = 0
      val structuralInput = new InputRecordCodec[SchemaValue] {
        val userParams                    = input.userParams
        val graph                         = input.graph
        def toValue(value: SchemaValue)   = RecordValue(List(value))
        def fromValue(value: SchemaValue) = value match {
          case RecordValue(List(RecordValue(fields))) =>
            decoded += 1
            Right(RecordValue(StringValue("GET") :: fields.tail))
          case _ => Left(FromSchemaError("invalid request"))
        }
      }
      val into = new IntoSchema[SchemaValue] {
        val graph                       = HttpResponse.intoSchema.graph
        def toValue(value: SchemaValue) = value
      }
      val from    = new FromSchema[SchemaValue] { def fromValue(value: SchemaValue) = Right(value) }
      val binding = MethodBinding.sync[Unit, SchemaValue, SchemaValue](
        metadata(true),
        structuralInput,
        OutputCodec.single[SchemaValue](into, from)
      ) { (_, value, _) =>
        val body = value.asInstanceOf[RecordValue].fields.last
        RecordValue(List(U16Value(200), ListValue(Nil), body))
      }
      ZIO.fromFuture { _ =>
        val body = AgentStream.fromPull[Array[Byte]](
          () => Future.failed(new IllegalStateException("must not pull")),
          () => { finalized += 1; Future.successful(()) }
        )
        for {
          wire    <- SchemaPayload.encodeAsync(request("HEAD").copy(body = body))(input)
          result  <- FutureInterop.fromPromise(binding.invoke((), wire, Principal.Anonymous))
          response = SchemaPayload.decode[HttpResponse](result.get).toOption.get
          end     <- response.body.pull()
        } yield assertTrue(decoded == 1, finalized == 1, end.isEmpty)
      }
    },
    test("wire-native HEAD responses dispose their body before crossing the component bridge") {
      var pulls     = 0
      var finalized = 0
      val response  = HttpResponse(
        UShort(200),
        Nil,
        AgentStream.fromPull[Array[Byte]](
          () => { pulls += 1; Future.failed(new IllegalStateException("must not pull")) },
          () => { finalized += 1; Future.successful(()) }
        )
      )
      val method = new WireImplementationMethod[Unit] {
        val name = "arbitrary"
        def invoke(
          instance: Unit,
          input: golem.schema.wire.WitSchemaValueTree,
          principal: Principal
        ): Future[Option[golem.schema.wire.WitSchemaValueTree]] =
          Future.successful(Some(SchemaWire.schemaValueToWit(HttpResponse.intoSchema.toValue(response))))
      }
      val descriptor = WireAgentMetadata.fromModel(
        AgentMetadata(
          "router",
          AgentTypeKind.HttpRouter,
          None,
          None,
          List(metadata(true)),
          ConstructorMetadata(None, "router", None)
        )
      )
      val binding = MethodBinding.wire(descriptor, method)
      ZIO.fromFuture { _ =>
        for {
          wire    <- SchemaPayload.encodeAsync(request("HEAD"))(input)
          result  <- FutureInterop.fromPromise(binding.invoke((), wire, Principal.Anonymous))
          response = SchemaPayload.decode[HttpResponse](result.get).toOption.get
          end     <- response.body.pull()
        } yield assertTrue(pulls == 0, finalized == 1, end.isEmpty)
      }
    },
    test("suppressed producer cleanup failure fails the invocation") {
      var finalized = 0
      val failure   = new IllegalStateException("cleanup failed")
      val binding   = MethodBinding.sync[Unit, HttpRequest, HttpResponse](metadata(true), input, output) { (_, _, _) =>
        HttpResponse(
          UShort(204),
          Nil,
          AgentStream
            .fromPull[Array[Byte]](() => Future.successful(None), () => { finalized += 1; Future.failed(failure) })
        )
      }
      ZIO.fromFuture { _ =>
        for {
          wire  <- SchemaPayload.encodeAsync(request("GET"))(input)
          error <- FutureInterop.fromPromise(binding.invoke((), wire, Principal.Anonymous)).failed
        } yield assertTrue(error eq failure, finalized == 1)
      }
    }
  ) @@ TestAspect.sequential
}
