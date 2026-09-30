/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 */

package golem.reflection

import golem.reflection.{DynamicToolClient, Reflection}
import golem.schema.TypedSchemaValue
import zio.blocks.schema.json.Json
import zio.test._

import scala.concurrent.{ExecutionContext, Future}

object MixedApproachRecipeSpec extends ZIOSpecDefault {
  def invokeDiscoveredToolDynamically(
    toolName: String,
    path: List[String],
    json: Json
  )(using ExecutionContext): Future[Either[String, Option[Json]]] = {
    val prepared = for {
      tool    <- Reflection.getToolType(toolName).left.map(_.toString)
      command <- tool.command(path).left.map(_.toString)
      input   <- command.packJson(json).left.map(_.toString)
      _       <- command.inputSchema
             .validateValue(input)
             .left
             .map(_.map(_.message).mkString("; "))
    } yield (tool, command, TypedSchemaValue(command.inputSchema.graph, input))

    prepared match {
      case Left(error)                   => Future.successful(Left(error))
      case Right((tool, command, input)) =>
        new DynamicToolClient(tool.lookupName).invoke(command.path, input).map {
          case Left(error) => Left(error.toString)
          case Right(raw)  =>
            (command.result, raw.result) match {
              case (None, None)                 => Right(None)
              case (Some(schema), Some(output)) =>
                schema
                  .validateValue(output.value)
                  .left
                  .map(_.map(_.message).mkString("; "))
                  .flatMap(_ => schema.unpackJson(output.value).left.map(_.message))
                  .map(Some(_))
              case _ => Left("missing or unexpected tool result")
            }
        }
    }
  }

  override def spec = suite("discovery-to-dynamic guide")(
    test("recipe typechecks without invoking the host")(assertTrue(true))
  )
}
