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
import golem.schema.SchemaTypeBody.{RecordType, RefType, S32Type, StringType}
import golem.schema.SchemaValue._
import golem.schema.wire.SchemaWire
import golem.tool._
import golem.tool.wire._
import zio.ZIO
import zio.blocks.schema.json.Json
import zio.test._

import scala.collection.immutable.ListMap
import scala.concurrent.{ExecutionContext, Future, Promise}

object ToolReflectionSpec extends ZIOSpecDefault {
  private val stringGraph = SchemaGraph(ListMap.empty, SchemaType(StringType))

  private def sample(valueGraph: SchemaGraph = stringGraph): ToolType = {
    val doc    = Doc("", "", Nil)
    val schema = SchemaWire.schemaGraphToWit(valueGraph)
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
    test("constraints use the declared flag default and nested value-is") {
      val original = sample()
      val nodes    = original.definition.commands.nodes
      val current  = nodes(1)
      val body     = current.body.get
      val option   = WitOptionSpec(
        "mode",
        None,
        Nil,
        Doc("", "", Nil),
        None,
        WitOptionShape.Scalar(original.definition.schema.root),
        None,
        required = false,
        None
      )
      val flag = FlagSpec(
        "enabled",
        None,
        Nil,
        Doc("", "", Nil),
        FlagShape.BoolFlag(BoolFlagShape(default = true, negatable = true)),
        None
      )
      val constraint = WitConstraint.RequiresAll(
        List(
          WitRef.Present("enabled"),
          WitRef.ValueIs(WitValueIsRef("mode", SchemaWire.schemaValueToWit(StringValue("fast"))))
        )
      )
      val updatedBody = body.copy(options = List(option), flags = List(flag), constraints = List(constraint))
      val updated     = new ToolType(
        original.lookupName,
        original.definition.copy(commands =
          original.definition.commands.copy(
            nodes = nodes.updated(1, current.copy(body = Some(updatedBody)))
          )
        ),
        original.implementedBy
      )
      val command = updated.command(List("run")).toOption.get
      val valid   =
        Json.Object("message" -> Json.String("hello"), "mode" -> Json.String("fast"), "enabled" -> Json.Boolean(false))
      val defaultFlag =
        Json.Object("message" -> Json.String("hello"), "mode" -> Json.String("fast"), "enabled" -> Json.Boolean(true))
      val wrongValue =
        Json.Object("message" -> Json.String("hello"), "mode" -> Json.String("slow"), "enabled" -> Json.Boolean(false))
      assertTrue(
        command.arguments.last.default.contains(BoolValue(true)),
        command.packJson(valid).isRight,
        command.packJson(defaultFlag).isLeft,
        command.packJson(wrongValue).isLeft
      )
    },
    test("schema-native input fails locally before opening RPC") {
      val command = sample().command(List("run")).toOption.get
      val invalid = command.startValue(RecordValue(List(S32Value(1))))
      assertTrue(invalid.left.toOption.exists(_.isInstanceOf[ToolError.InvalidInput]))
    },
    test("command-level JSON packing enforces schema refinements") {
      val restricted = SchemaGraph(
        ListMap.empty,
        SchemaType(S32Type(Some(NumericRestrictions(max = Some(NumericBound.Signed(3))))))
      )
      val command = sample(restricted).command(List("run")).toOption.get
      assertTrue(
        command.packJson(Json.Object("message" -> Json.Number(BigDecimal(3)))).isRight,
        command.packJson(Json.Object("message" -> Json.Number(BigDecimal(4)))).isLeft
      )
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
    test("result graph comparison resolves references structurally") {
      def graph(id: String, field: String) = SchemaGraph(
        ListMap(id -> SchemaTypeDef(SchemaType(RecordType(List(NamedFieldType(field, SchemaType(StringType))))))),
        SchemaType(RefType(id))
      )
      val command     = sample(graph("Expected", "name")).command(List("run")).toOption.get
      val sameIdWrong = TypedSchemaValue(graph("Expected", "password"), RecordValue(List(StringValue("hello"))))
      val otherIdSame = TypedSchemaValue(graph("Equivalent", "name"), RecordValue(List(StringValue("hello"))))
      assertTrue(
        command.decodeResult(ToolInvokeResult(Some(sameIdWrong))).isLeft,
        command.decodeResult(ToolInvokeResult(Some(otherIdSame))).isRight
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
    },
    test("declared tool errors win after stdout failure while both channels settle") {
      val broken = new ToolInputStream {
        override def read(): Future[Either[ByteStreamFailure, Option[Array[Byte]]]] =
          Future.successful(Left(ByteStreamFailure.Failed("broken")))
      }
      val terminal   = Promise[Either[ToolError[NamedToolError], Option[SchemaValue]]]()
      val invocation = ReflectedToolInvocation(Some(broken), terminal.future, () => ())
      val collected  = invocation.collect()(using ExecutionContext.global)
      val payload    = TypedSchemaValue(stringGraph, StringValue("details"))
      terminal.success(Left(ToolError.Tool(NamedToolError("declared", payload))))
      ZIO.fromFuture(_ => collected).map { result =>
        assertTrue(result == Left(ToolError.Tool(NamedToolError("declared", payload))))
      }
    }
  )
}
