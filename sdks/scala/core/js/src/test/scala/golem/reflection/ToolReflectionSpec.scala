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

import golem.Uuid
import golem.schema._
import golem.schema.SchemaTypeBody.StringType
import golem.schema.SchemaValue._
import golem.schema.wire.SchemaWire
import golem.tool._
import golem.tool.wire._
import zio.ZIO
import zio.blocks.schema.json.Json
import zio.test._

import scala.collection.immutable.ListMap
import scala.concurrent.{ExecutionContext, Future}

object ToolReflectionSpec extends ZIOSpecDefault {
  private def sample(): ToolType = {
    val doc    = Doc("", "", Nil)
    val schema = SchemaWire.schemaGraphToWit(SchemaGraph(ListMap.empty, SchemaType(StringType)))
    val body   = WitCommandBody(
      WitPositionals(
        List(WitPositional("message", doc, None, schema.root, None, required = true, acceptsStdio = false)),
        None
      ),
      Nil,
      Nil,
      Nil,
      None,
      None,
      Some(WitResultSpec(schema.root, doc, Nil, "")),
      Nil,
      None
    )
    new ToolType(
      "sample",
      WitTool(
        "1",
        WitCommandTree(
          Vector(
            WitCommandNode("sample", Nil, doc, WitGlobals(Nil, Nil), List(1), None),
            WitCommandNode("run", List("r"), doc, WitGlobals(Nil, Nil), Nil, Some(body))
          )
        ),
        schema
      ),
      ComponentId(Uuid(0, 0))
    )
  }

  def spec = suite("ToolReflectionSpec")(
    test("alias resolves to the canonical command path") {
      val command = sample().command(List("r")).toOption.get
      assertTrue(command.path == List("run"), command.arguments.map(_.name) == List("message"))
    },
    test("optional fields use the canonical option carrier") {
      val original = sample()
      val nodes    = original.definition.commands.nodes
      val current  = nodes(1)
      val body     = current.body.get
      val option   = WitOptionSpec(
        "maybe",
        None,
        Nil,
        Doc("", "", Nil),
        None,
        WitOptionShape.Scalar(original.definition.schema.root),
        None,
        required = false,
        None
      )
      val updatedBody = body.copy(
        positionals = body.positionals.copy(fixed = body.positionals.fixed.map(_.copy(required = false))),
        options = List(option)
      )
      val updated = new ToolType(
        original.lookupName,
        original.definition.copy(commands =
          original.definition.commands.copy(nodes = nodes.updated(1, current.copy(body = Some(updatedBody))))
        ),
        original.implementedBy
      )
      val command  = updated.command(List("run")).toOption.get
      val omitted  = RecordValue(List(OptionValue(None), OptionValue(None)))
      val supplied = RecordValue(
        List(
          OptionValue(Some(StringValue("message"))),
          OptionValue(Some(StringValue("value")))
        )
      )
      assertTrue(
        command.inputSchema.validateValue(omitted).isRight,
        command.inputSchema.validateValue(supplied).isRight,
        command.packJson(Json.Object("message" -> Json.Null, "maybe" -> Json.Null)) == Right(omitted),
        command.packJson(Json.Object("message" -> Json.String("message"), "maybe" -> Json.String("value"))) == Right(
          supplied
        )
      )
    },
    test("schema-native input fails locally before opening RPC") {
      val command = sample().command(List("run")).toOption.get
      val invalid = command.startValue(RecordValue(List(S32Value(1))))
      assertTrue(invalid.left.toOption.exists(_.isInstanceOf[ToolError.InvalidInput]))
    },
    test("missing and mismatched remote values are malformed output") {
      val command = sample().command(List("run")).toOption.get
      val wrong   = TypedSchemaValue(SchemaGraph(ListMap.empty, SchemaType(SchemaTypeBody.S32Type(None))), S32Value(1))
      assertTrue(
        command
          .decodeResult(ToolInvokeResult(None))
          .left
          .toOption
          .exists(_.isInstanceOf[ToolError.MalformedRemoteOutput]),
        command
          .decodeResult(ToolInvokeResult(Some(wrong)))
          .left
          .toOption
          .exists(_.isInstanceOf[ToolError.MalformedRemoteOutput])
      )
    },
    test("stream failures remain recoverable for reflected calls") {
      val broken = new ToolInputStream {
        override def read(): Future[Either[ByteStreamFailure, Option[Array[Byte]]]] =
          Future.successful(Left(ByteStreamFailure.Failed("broken")))
      }
      val terminal: Future[Either[ToolError[NamedToolError], Option[SchemaValue]]] = Future.successful(Right(None))
      val invocation                                                               = ReflectedToolInvocation(Some(broken), terminal, () => ())
      ZIO.fromFuture(_ => invocation.collect()(using ExecutionContext.global)).map { result =>
        assertTrue(result.left.toOption.exists(_.isInstanceOf[ToolError.Rpc]))
      }
    }
  )
}
