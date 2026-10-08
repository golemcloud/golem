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

import golem.{BaseAgent, Principal, ULong}
import golem.reflection.{AgentClientDefinition, DynamicAgentClient, DynamicToolClient, GolemReflectError, Reflection}
import golem.runtime.annotations.*
import golem.runtime.{InputRecordCodec, OutputCodec}
import golem.schema.{Quantity, QuantityUnit, SchemaValue, TypedSchemaValue}
import golem.tool.{ByteStreamFailure, ToolOutputStream}
import zio.blocks.schema.json.Json
import zio.blocks.schema.Schema
import zio.blocks.typeid.TypeId

import java.time.{Duration => JDuration}
import scala.concurrent.{ExecutionContext, Future}

sealed trait ReflectionMeters

object ReflectionMeters {
  implicit val unit: QuantityUnit[ReflectionMeters] = new QuantityUnit[ReflectionMeters] {
    override val baseUnit: String                 = "m"
    override val allowedSuffixes: List[String]    = Nil
    override val typeId: TypeId[ReflectionMeters] = TypeId.of[ReflectionMeters]
  }
}

final case class ScalaConformanceInput(primary: String, nested: Map[String, Seq[Long]])
object ScalaConformanceInput {
  implicit val schema: Schema[ScalaConformanceInput] = Schema.derived
}

final case class ScalaConformanceOutput(summary: String, counts: Seq[Long])
object ScalaConformanceOutput {
  implicit val schema: Schema[ScalaConformanceOutput] = Schema.derived
}

final case class ScalaConformanceFailurePayload(field: String, reason: String)
object ScalaConformanceFailurePayload {
  implicit val schema: Schema[ScalaConformanceFailurePayload] = Schema.derived
}

enum ScalaConformanceFailure {
  @error(kind = "usage-error", exitCode = 2)
  case Invalid(payload: ScalaConformanceFailurePayload)
}

@toolDefinition(name = "nested")
trait ScalaReflectionNestedTool {
  def inspect(input: ScalaConformanceInput): ScalaConformanceOutput
}

final class ScalaReflectionNestedToolImpl(prefix: String) extends ScalaReflectionNestedTool {
  override def inspect(input: ScalaConformanceInput): ScalaConformanceOutput =
    ScalaConformanceOutput(
      s"$prefix:${input.primary}:${input.nested.keys.toSeq.sorted.mkString(",")}",
      input.nested.toSeq.sortBy(_._1).map(_._2.sum)
    )
}

@toolDefinition(name = "scala-reflection-test")
trait ScalaReflectionTestTool {
  def echo(label: String): String
  @arg("maybe", scope = "option")
  def optional(maybe: Option[String]): String
  @arg("maybe", scope = "option")
  def canonicalValues(
    signed: Long,
    unsigned: ULong,
    duration: JDuration,
    quantity: Quantity[ReflectionMeters],
    maybe: Option[String]
  ): String
  def asymmetric(input: ScalaConformanceInput): ScalaConformanceOutput
  def nested(prefix: String): ScalaReflectionNestedTool
  def typedFailure(message: String): Either[ScalaConformanceFailure, String]
  def principal(label: String, principal: Principal): String
  def streaming(label: String, stdout: ToolOutputStream): Future[String]
}

@toolImplementation()
final class ScalaReflectionTestToolImpl extends ScalaReflectionTestTool {
  override def echo(label: String): String             = s"scala-tool:$label"
  override def optional(maybe: Option[String]): String = maybe.getOrElse("omitted")
  override def canonicalValues(
    signed: Long,
    unsigned: ULong,
    duration: JDuration,
    quantity: Quantity[ReflectionMeters],
    maybe: Option[String]
  ): String =
    if (
      signed == Long.MinValue &&
      unsigned.value == ((BigInt(1) << 64) - 1) &&
      duration.toNanos == Long.MaxValue &&
      quantity == Quantity[ReflectionMeters](Long.MinValue, -9, "m") &&
      maybe.isEmpty
    ) "scala-canonical-ok"
    else "scala-canonical-mismatch"

  override def asymmetric(input: ScalaConformanceInput): ScalaConformanceOutput =
    ScalaConformanceOutput(
      s"${input.primary}:${input.nested.keys.toSeq.sorted.mkString(",")}",
      input.nested.toSeq.sortBy(_._1).map(_._2.sum)
    )

  override def nested(prefix: String): ScalaReflectionNestedTool =
    new ScalaReflectionNestedToolImpl(prefix)

  override def typedFailure(message: String): Either[ScalaConformanceFailure, String] =
    Left(ScalaConformanceFailure.Invalid(ScalaConformanceFailurePayload("message", message)))

  override def principal(label: String, principal: Principal): String =
    s"$label:${principalLabel(principal)}"

  override def streaming(label: String, stdout: ToolOutputStream): Future[String] =
    stdout
      .write(s"scala:$label".getBytes("UTF-8"))
      .flatMap {
        case Right(_)    => Future.successful(s"streamed:$label")
        case Left(error) => Future.failed(new IllegalStateException(s"stream write failed: $error"))
      }(ExecutionContext.global)

  private def principalLabel(principal: Principal): String = principal match {
    case Principal.Anonymous                            => "anonymous"
    case Principal.Agent(_, name)                       => s"agent:$name"
    case Principal.GolemUser(_)                         => "golem-user"
    case Principal.Oidc(sub, _, _, _, _, _, _, _, _, _) => s"oidc:$sub"
  }
}

@agentDefinition()
trait ScalaPrincipalIdentity extends BaseAgent {
  class Id(val name: String)
  def who(): Future[String]
}

@agentImplementation()
final class ScalaPrincipalIdentityImpl(name: String, principal: Principal) extends ScalaPrincipalIdentity {
  override def who(): Future[String] = Future.successful(name)
}

@agentDefinition()
trait ScalaToolReflectionCaller extends BaseAgent {
  class Id(val name: String)
  def roundTrip(): Future[String]
  def optionalRoundTrip(): Future[String]
  def canonicalRoundTrip(): Future[String]
  def principalRoundTrip(): Future[String]
  def agentRoundTrip(): Future[String]
  def toolConformanceRoundTrip(principal: Principal): Future[String]
}

@agentImplementation()
final class ScalaToolReflectionCallerImpl(name: String) extends ScalaToolReflectionCaller {
  private implicit val ec: ExecutionContext = ExecutionContext.global

  override def optionalRoundTrip(): Future[String] = {
    val prepared = for {
      tool    <- Reflection.getToolType("scala-reflection-test").left.map(_.toString)
      command <- tool.command(List("optional")).left.map(_.toString)
    } yield command

    prepared match {
      case Left(error)    => Future.successful(s"error:$error")
      case Right(command) =>
        for {
          omittedJson    <- command.invokeJson(Json.Object())
          nullJson       <- command.invokeJson(Json.Object("maybe" -> Json.Null))
          suppliedJson   <- command.invokeJson(Json.Object("maybe" -> Json.String("supplied")))
          omittedNative  <- command.invokeValue(SchemaValue.RecordValue(List(SchemaValue.OptionValue(None))))
          suppliedNative <-
            command.invokeValue(
              SchemaValue.RecordValue(List(SchemaValue.OptionValue(Some(SchemaValue.StringValue("supplied")))))
            )
        } yield s"$omittedJson|$nullJson|$suppliedJson|$omittedNative|$suppliedNative"
    }
  }

  override def canonicalRoundTrip(): Future[String] = {
    val prepared = for {
      tool    <- Reflection.getToolType("scala-reflection-test").left.map(_.toString)
      command <- tool.command(List("canonical-values")).left.map(_.toString)
    } yield command

    prepared match {
      case Left(error)    => Future.successful(s"error:$error")
      case Right(command) =>
        command
          .invokeJson(
            Json.Object(
              "signed"   -> Json.String(Long.MinValue.toString),
              "unsigned" -> Json.String("18446744073709551615"),
              "duration" -> Json.Object("nanoseconds" -> Json.String(Long.MaxValue.toString)),
              "quantity" -> Json.Object(
                "mantissa" -> Json.String(Long.MinValue.toString),
                "scale"    -> Json.Number(BigDecimal(-9)),
                "unit"     -> Json.String("m")
              )
            )
          )
          .map(_.toString)
    }
  }

  override def principalRoundTrip(): Future[String] = {
    val identityName = s"principal-${this.name}"
    val full         = AgentClientDefinition.full[String]("ScalaPrincipalIdentity", InputRecordCodec.single[String]("name"))
    val methodOnly   = AgentClientDefinition.methodOnly
    val who          = full.method[Unit, String]("who", InputRecordCodec.unit, OutputCodec.single[String])
    val prepared     = for {
      agent <- Reflection
                 .getAgentType("ScalaPrincipalIdentity")
                 .flatMap(_.toRight(GolemReflectError.Discovery("ScalaPrincipalIdentity was not found")))
      reflected       <- agent.client.get(Json.Object("name" -> Json.String(identityName)))
      reflectedMethod <- reflected.method("who")
      id              <- full.agentId(identityName)
      parts           <- id.parts
      _               <- Either.cond(
             parts.constructorValue == SchemaValue.RecordValue(List(SchemaValue.StringValue(identityName))),
             (),
             GolemReflectError.Identity("principal appeared in the agent ID")
           )
      fullyBound  <- full.bind(id)
      methodBound <- methodOnly.bind(id)
    } yield (reflectedMethod, fullyBound, methodBound)

    prepared match {
      case Left(error)                                       => Future.successful(s"error:$error")
      case Right((reflectedMethod, fullyBound, methodBound)) =>
        for {
          reflectedCall  <- reflectedMethod.invokeJson(Json.Object())
          fullCall       <- fullyBound.method(who).invoke(())
          methodOnlyCall <- methodBound.method(who).invoke(())
          generatedCall  <- ScalaPrincipalIdentityClient.get(identityName).who()
        } yield s"$reflectedCall|$fullCall|$methodOnlyCall|$generatedCall"
    }
  }

  override def roundTrip(): Future[String] = {
    val prepared = for {
      tool    <- Reflection.getToolType("scala-reflection-test").left.map(_.toString)
      command <- tool.command(List("echo")).left.map(_.toString)
      json    <- Json.parse("""{"label":"scala"}""").left.map(_.toString)
      input   <- command.packJson(json).left.map(_.toString)
    } yield (command, json, input)

    prepared match {
      case Left(error)                   => Future.successful(s"error:$error")
      case Right((command, json, input)) =>
        val invalid      = Json.parse("""{"label":42}""").toOption.exists(command.packJson(_).isLeft)
        val dynamicInput = TypedSchemaValue(command.inputSchema.graph, input)
        for {
          native  <- command.invokeValue(input)
          encoded <- command.invokeJson(json)
          dynamic <- new DynamicToolClient("scala-reflection-test").invoke(command.path, dynamicInput)
        } yield {
          val decodedDynamic = dynamic.left.map(_.toString).flatMap { raw =>
            (command.result, raw.result) match {
              case (Some(schema), Some(output)) =>
                schema
                  .validateValue(output.value)
                  .left
                  .map(_.map(_.message).mkString("; "))
                  .flatMap(_ => schema.unpackJson(output.value).left.map(_.message))
              case _ => Left("missing or unexpected tool result")
            }
          }
          s"$native|$encoded|$invalid|$decodedDynamic"
        }
    }
  }

  override def toolConformanceRoundTrip(principal: Principal): Future[String] = {
    val input    = ScalaConformanceInput("left", Map("z" -> Seq(8L, 1L), "a" -> Seq(3L)))
    val direct   = new ScalaReflectionTestToolImpl
    val captured = new CapturingOutput
    val definition = for {
      streamed <- direct.streaming("definition", captured)
      _        <- captured.finish()
    } yield (
      direct.asymmetric(input),
      direct.nested("definition-nested").inspect(input),
      direct.typedFailure("definition-error"),
      direct.principal("definition", principal),
      streamed,
      captured.text,
      captured.terminal
    )

    val generated = for {
      asymmetric <- ScalaReflectionTestToolClient().asymmetric(input)
      nested     <- ScalaReflectionTestToolClient().nested("generated-nested").inspect(input)
      failure    <- ScalaReflectionTestToolClient().typedFailure("generated-error")
      identity   <- ScalaReflectionTestToolClient().principal("generated")
      stream     <- ScalaReflectionTestToolClient().streaming("generated") match {
                  case Left(error)       => Future.failed(new IllegalStateException(error.toString))
                  case Right(invocation) => invocation.collect()
                }
    } yield (asymmetric, nested, failure, identity, stream)

    val reflected = for {
      tool <- Future.fromTry(
                scala.util.Try(
                  Reflection
                    .getToolType("scala-reflection-test")
                    .fold(error => throw new IllegalStateException(error.toString), identity)
                )
              )
      asymmetric <- tool
                      .command(List("asymmetric"))
                      .fold(
                        error => Future.failed(new IllegalStateException(error.toString)),
                        command =>
                          command.invokeJson(
                            Json.Object(
                              "input" -> Json.Object(
                                "primary" -> Json.String("left"),
                                "nested"  -> Json.Object(
                                  "z" -> Json.Array(Json.Number(BigDecimal(8)), Json.Number(BigDecimal(1))),
                                  "a" -> Json.Array(Json.Number(BigDecimal(3)))
                                )
                              )
                            )
                          )
                      )
      nested <- tool
                  .command(List("nested", "inspect"))
                  .fold(
                    error => Future.failed(new IllegalStateException(error.toString)),
                    command =>
                      command.invokeJson(
                        Json.Object(
                          "prefix" -> Json.String("reflected-nested"),
                          "input"  -> Json.Object(
                            "primary" -> Json.String("left"),
                            "nested"  -> Json.Object(
                              "z" -> Json.Array(Json.Number(BigDecimal(8)), Json.Number(BigDecimal(1))),
                              "a" -> Json.Array(Json.Number(BigDecimal(3)))
                            )
                          )
                        )
                      )
                  )
      failure <- tool
                   .command(List("typed-failure"))
                   .fold(
                     error => Future.failed(new IllegalStateException(error.toString)),
                     command => command.invokeJson(Json.Object("message" -> Json.String("reflected-error")))
                   )
      identity <- tool
                    .command(List("principal"))
                    .fold(
                      error => Future.failed(new IllegalStateException(error.toString)),
                      command => command.invokeJson(Json.Object("label" -> Json.String("reflected")))
                    )
      stream <- tool
                  .command(List("streaming"))
                  .fold(
                    error => Future.failed(new IllegalStateException(error.toString)),
                    command =>
                      command.startJson(Json.Object("label" -> Json.String("reflected"))) match {
                        case Left(error)       => Future.failed(new IllegalStateException(error.toString))
                        case Right(invocation) => invocation.collect()
                      }
                  )
    } yield (asymmetric, nested, failure, identity, stream)

    for {
      definitionResult <- definition
      generatedResult <- generated
      reflectedResult <- reflected
    } yield s"definition=$definitionResult|generated=$generatedResult|reflected=$reflectedResult"
  }

  private final class CapturingOutput extends ToolOutputStream {
    private var bytes                            = Vector.empty[Byte]
    private var terminalState                    = "open"
    def text: String                             = new String(bytes.toArray, "UTF-8")
    def terminal: String                         = terminalState
    override def write(chunk: Array[Byte])       = { bytes ++= chunk; Future.successful(Right(())) }
    override def finish()                        = { terminalState = "ended"; Future.successful(Right(())) }
    override def fail(reason: ByteStreamFailure) = { terminalState = s"failed:$reason"; Future.successful(Right(())) }
  }

  override def agentRoundTrip(): Future[String] = {
    val prepared = for {
      agent <- Reflection
                 .getAgentType("StatefulCounter")
                 .flatMap(_.toRight(GolemReflectError.Discovery("StatefulCounter was not found")))
      constructor <- Json
                       .parse("""{"initialCount":0}""")
                       .left
                       .map(error => GolemReflectError.Validation(error.toString))
      client    <- agent.client.get(constructor)
      current   <- client.method("current")
      increment <- client.method("increment")
    } yield (current, increment)

    prepared match {
      case Left(error)                 => Future.successful(s"error:$error")
      case Right((current, increment)) =>
        val empty = SchemaValue.RecordValue(Nil)
        for {
          first     <- current.invokeJson(Json.Object())
          native    <- current.invokeValue(empty)
          increased <- increment.invokeJson(Json.Object())
          second    <- current.invokeJson(Json.Object())
          dynamic   <- first match {
                       case Left(error)       => Future.successful(Left(error))
                       case Right(invocation) =>
                         DynamicAgentClient.fromAgentId(invocation.metadata.agentId) match {
                           case Left(error)   => Future.successful(Left(error))
                           case Right(client) => client.method("current").invokeValue(empty)
                         }
                     }
        } yield {
          val result = for {
            firstCall     <- first
            nativeCall    <- native
            increasedCall <- increased
            secondCall    <- second
            dynamicCall   <- dynamic
            parts         <- firstCall.metadata.agentId.parts
            discovered    <- Reflection
                            .getAgentTypeFor(firstCall.metadata.agentId)
                            .flatMap(_.toRight(GolemReflectError.Discovery("StatefulCounter ID was not found")))
          } yield s"${parts.typeName}|${discovered.name}|${firstCall.value}|${nativeCall.value}|${increasedCall.value}|${secondCall.value}|${dynamicCall.value}"
          result.fold(error => s"error:$error", identity)
        }
    }
  }
}
