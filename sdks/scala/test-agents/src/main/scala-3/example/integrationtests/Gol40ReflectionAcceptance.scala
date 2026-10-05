/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 */

package example.integrationtests

import golem.{BaseAgent, Principal}
import golem.reflection.{Reflection, ToolClientDefinition}
import golem.runtime.annotations.*
import golem.tool.{ToolError, ToolOutputStream}
import zio.blocks.schema.Schema
import zio.blocks.schema.json.Json

import scala.concurrent.{ExecutionContext, Future}

final case class Gol40ReflectionInput(label: String, left: Long, right: Long)
object Gol40ReflectionInput {
  implicit val schema: Schema[Gol40ReflectionInput] = Schema.derived
}

final case class Gol40ReflectionOutput(path: String, label: String, weighted: Long, principal: String)
object Gol40ReflectionOutput {
  implicit val schema: Schema[Gol40ReflectionOutput] = Schema.derived
}

final case class Gol40ReflectionErrorPayload(field: String, reason: String)
object Gol40ReflectionErrorPayload {
  implicit val schema: Schema[Gol40ReflectionErrorPayload] = Schema.derived
}

enum Gol40ReflectionError {
  @error(kind = "usage-error", exitCode = 2)
  case Rejected(payload: Gol40ReflectionErrorPayload)
}

@toolDefinition(name = "nested", version = "1.0.0")
trait Gol40ReflectionNested {
  def inspect(input: Gol40ReflectionInput, principal: Principal): Gol40ReflectionOutput
}

@toolDefinition(name = "scala-gol40-reflection-acceptance", version = "1.0.0")
trait Gol40ReflectionTool {
  def nested(prefix: String): Gol40ReflectionNested
  def typedFailure(message: String): Either[Gol40ReflectionError, String]
  def principal(principal: Principal): String
  def streaming(label: String, stdout: ToolOutputStream): Future[String]
}

@toolImplementation()
final class Gol40ReflectionToolImpl extends Gol40ReflectionTool {
  override def nested(prefix: String): Gol40ReflectionNested = new Gol40ReflectionNested {
    override def inspect(input: Gol40ReflectionInput, principal: Principal): Gol40ReflectionOutput =
      Gol40ReflectionOutput(
        path = "nested/inspect",
        label = s"$prefix:${input.label}",
        weighted = input.left * 7L + input.right * 3L,
        principal = principalLabel(principal)
      )
  }

  override def typedFailure(message: String): Either[Gol40ReflectionError, String] =
    Left(Gol40ReflectionError.Rejected(Gol40ReflectionErrorPayload("message", message)))

  override def principal(principal: Principal): String = principalLabel(principal)

  override def streaming(label: String, stdout: ToolOutputStream): Future[String] =
    stdout
      .write(Array[Byte](0, 2, 5, 9, -1))
      .flatMap {
        case Right(_)    => Future.successful(s"streamed:$label")
        case Left(error) => Future.failed(new IllegalStateException(error.toString))
      }(ExecutionContext.parasitic)

  private def principalLabel(principal: Principal): String = principal match {
    case Principal.Anonymous                            => "anonymous"
    case Principal.Agent(_, name)                       => s"agent:$name"
    case Principal.GolemUser(_)                         => "golem-user"
    case Principal.Oidc(sub, _, _, _, _, _, _, _, _, _) => s"oidc:$sub"
  }
}

final case class Gol40ReflectionObservation(
  namedClientBound: Boolean,
  nestedMatches: Boolean,
  generatedNested: String,
  reflectedNested: String,
  typedErrorMatches: Boolean,
  principalMatches: Boolean,
  generatedPrincipal: String,
  reflectedPrincipal: String,
  streamMatches: Boolean,
  generatedStreamBytes: List[Int],
  reflectedStreamBytes: List[Int],
  generatedStreamResult: String,
  reflectedStreamResult: String
)
object Gol40ReflectionObservation {
  implicit val schema: Schema[Gol40ReflectionObservation] = Schema.derived
}

@agentDefinition()
trait ScalaGol40ReflectionAcceptance extends BaseAgent {
  class Id(val name: String)
  def observe(): Future[Gol40ReflectionObservation]
}

@agentImplementation()
final class ScalaGol40ReflectionAcceptanceImpl(name: String) extends ScalaGol40ReflectionAcceptance {
  private implicit val ec: ExecutionContext = ExecutionContext.global

  override def observe(): Future[Gol40ReflectionObservation] = {
    val input = Gol40ReflectionInput("asymmetric", left = 11L, right = 4L)
    val named = ToolClientDefinition
      .named("scala-gol40-reflection-acceptance")(Gol40ReflectionToolClient.apply)
      .client
      .fold(error => throw new IllegalStateException(error.toString), identity)
    val reflectedTool = Reflection
      .getToolType("scala-gol40-reflection-acceptance")
      .fold(error => throw new IllegalStateException(error.toString), identity)

    val generated = for {
      nested    <- expectRight(named.nested("shared-prefix").inspect(input))
      failure   <- named.typedFailure("shared-error")
      principal <- expectRight(named.principal())
      stream    <- named.streaming("shared-stream") match {
                  case Left(error)       => Future.failed(new IllegalStateException(error.toString))
                  case Right(invocation) => invocation.collect()
                }
    } yield (nested, failure, principal, stream)

    val reflected = for {
      nested <- invokeJson(
                  reflectedTool,
                  List("nested", "inspect"),
                  Json.Object(
                    "prefix" -> Json.String("shared-prefix"),
                    "input"  -> Json.Object(
                      "label" -> Json.String("asymmetric"),
                      "left"  -> Json.String("11"),
                      "right" -> Json.String("4")
                    )
                  )
                )
      failure <- reflectedTool
                   .command(List("typed-failure"))
                   .fold(
                     error => Future.failed(new IllegalStateException(error.toString)),
                     _.invokeJson(Json.Object("message" -> Json.String("shared-error")))
                   )
      principal    <- invokeJson(reflectedTool, List("principal"), Json.Object())
      streamCommand = reflectedTool
                        .command(List("streaming"))
                        .fold(error => throw new IllegalStateException(error.toString), identity)
      streamInvocation = streamCommand
                           .startJson(Json.Object("label" -> Json.String("shared-stream")))
                           .fold(error => throw new IllegalStateException(error.toString), identity)
      stream <- streamInvocation.collect()
    } yield (nested, failure, principal, stream)

    (for {
      generatedResult <- generated
      reflectedResult <- reflected
    } yield {
      val (generatedNested, generatedFailure, generatedPrincipal, generatedStream) = generatedResult
      val (reflectedNested, reflectedFailure, reflectedPrincipal, reflectedStream) = reflectedResult
      val generatedErrorMatches                                                    = generatedFailure match {
        case Left(ToolError.Tool(Gol40ReflectionError.Rejected(payload))) =>
          payload == Gol40ReflectionErrorPayload("message", "shared-error")
        case _ => false
      }
      val reflectedErrorMatches = reflectedFailure match {
        case Left(ToolError.Tool(error)) =>
          error.name == "rejected" && error.payload.value.toString.contains("shared-error")
        case _ => false
      }
      val generatedBytes = generatedStream.stdout
        .fold(error => throw new IllegalStateException(error.toString), identity)
        .toList
        .flatMap(_.toList.map(_.toInt))
      val reflectedBytes = reflectedStream.stdout
        .fold(error => throw new IllegalStateException(error.toString), identity)
        .toList
        .flatMap(_.toList.map(_.toInt))
      val generatedStreamResult = generatedStream.result
        .fold(error => throw new IllegalStateException(error.toString), identity)
      val reflectedStreamResult = reflectedStream.result
        .fold(error => throw new IllegalStateException(error.toString), _.map(_.toString).getOrElse(""))
      val expectedNested = Gol40ReflectionOutput(
        path = "nested/inspect",
        label = "shared-prefix:asymmetric",
        weighted = 89L,
        principal = generatedPrincipal
      )

      Gol40ReflectionObservation(
        namedClientBound = true,
        nestedMatches = generatedNested == expectedNested &&
          reflectedNested.contains("nested/inspect") &&
          reflectedNested.contains("shared-prefix:asymmetric") &&
          reflectedNested.contains("89") &&
          reflectedNested.contains(generatedPrincipal),
        generatedNested = generatedNested.toString,
        reflectedNested = reflectedNested,
        typedErrorMatches = generatedErrorMatches && reflectedErrorMatches,
        principalMatches = reflectedPrincipal.contains(generatedPrincipal),
        generatedPrincipal = generatedPrincipal,
        reflectedPrincipal = reflectedPrincipal,
        streamMatches = generatedBytes == List(0, 2, 5, 9, -1) &&
          reflectedBytes == generatedBytes &&
          generatedStreamResult == "streamed:shared-stream" &&
          reflectedStreamResult.contains("streamed:shared-stream"),
        generatedStreamBytes = generatedBytes,
        reflectedStreamBytes = reflectedBytes,
        generatedStreamResult = generatedStreamResult,
        reflectedStreamResult = reflectedStreamResult
      )
    })
  }

  private def invokeJson(
    tool: golem.reflection.ToolType,
    path: List[String],
    input: Json
  ): Future[String] = {
    val invoked = tool
      .command(path)
      .fold(error => Future.failed(new IllegalStateException(error.toString)), _.invokeJson(input))

    expectRight(invoked)
      .map(_.map(_.toString).getOrElse(""))
  }

  private def expectRight[E, A](future: Future[Either[E, A]]): Future[A] =
    future.flatMap {
      case Right(value) => Future.successful(value)
      case Left(error)  => Future.failed(new IllegalStateException(error.toString))
    }
}
