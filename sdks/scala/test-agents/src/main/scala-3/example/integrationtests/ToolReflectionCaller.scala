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

import golem.BaseAgent
import golem.reflection.{DynamicToolClient, Reflection}
import golem.runtime.annotations.*
import golem.schema.{SchemaValue, TypedSchemaValue}
import zio.blocks.schema.json.Json

import scala.concurrent.{ExecutionContext, Future}

@toolDefinition(name = "scala-reflection-test")
trait ScalaReflectionTestTool {
  def echo(label: String): String
}

@toolImplementation()
final class ScalaReflectionTestToolImpl extends ScalaReflectionTestTool {
  override def echo(label: String): String = s"scala-tool:$label"
}

@agentDefinition()
trait ScalaToolReflectionCaller extends BaseAgent {
  class Id(val name: String)
  def roundTrip(): Future[String]
}

@agentImplementation()
final class ScalaToolReflectionCallerImpl(name: String) extends ScalaToolReflectionCaller {
  private implicit val ec: ExecutionContext = ExecutionContext.global

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
          dynamic <- new DynamicToolClient("scala-reflection-test").invoke(List("echo"), dynamicInput)
        } yield s"$native|$encoded|$invalid|${dynamic.map(_.result.map(_.value))}"
    }
  }
}
