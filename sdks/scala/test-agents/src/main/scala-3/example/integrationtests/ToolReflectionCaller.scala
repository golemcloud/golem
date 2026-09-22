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
import golem.reflection.{AgentClientDefinition, DynamicAgentClient, DynamicToolClient, GolemReflectError, Reflection}
import golem.runtime.annotations.*
import golem.runtime.{InputRecordCodec, OutputCodec}
import golem.schema.{SchemaValue, TypedSchemaValue}
import zio.blocks.schema.json.Json

import scala.concurrent.{ExecutionContext, Future}

@toolDefinition(name = "scala-reflection-test")
trait ScalaReflectionTestTool {
  def echo(label: String): String
  @arg("maybe", scope = "option")
  def optional(maybe: Option[String]): String
}

@toolImplementation()
final class ScalaReflectionTestToolImpl extends ScalaReflectionTestTool {
  override def echo(label: String): String             = s"scala-tool:$label"
  override def optional(maybe: Option[String]): String = maybe.getOrElse("omitted")
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
  def principalRoundTrip(): Future[String]
  def agentRoundTrip(): Future[String]
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
          omittedJson    <- command.invokeJson(Json.Object("maybe" -> Json.Null))
          suppliedJson   <- command.invokeJson(Json.Object("maybe" -> Json.String("supplied")))
          omittedNative  <- command.invokeValue(SchemaValue.RecordValue(List(SchemaValue.OptionValue(None))))
          suppliedNative <-
            command.invokeValue(
              SchemaValue.RecordValue(List(SchemaValue.OptionValue(Some(SchemaValue.StringValue("supplied")))))
            )
        } yield s"$omittedJson|$suppliedJson|$omittedNative|$suppliedNative"
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
