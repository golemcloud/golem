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

import golem.FutureInterop
import golem.host.ToolWireInterop
import golem.host.js.tool.JsInvocationResult
import golem.schema._
import golem.schema.SchemaTypeBody.{BoolType, ListType, OptionType, RecordType, RefType, S32Type, StringType, U32Type}
import golem.schema.SchemaValue._
import golem.schema.wire.{GraphEncoder, SchemaWire}
import golem.tool._
import golem.tool.wire._
import zio.ZIO
import zio.blocks.async.*
import zio.blocks.schema.json.Json
import zio.blocks.streams.{JvmType, Stream}
import zio.test._

import java.util.concurrent.CancellationException
import scala.collection.immutable.ListMap
import scala.concurrent.{ExecutionContext, Future, Promise}
import scala.scalajs.js

object ToolReflectionSpec extends ZIOSpecDefault {
  private val stringGraph = SchemaGraph(ListMap.empty, SchemaType(StringType))

  private def bytes(values: Byte*): ToolInputStream = new ToolInputStream {
    override val stream: Stream[ByteStreamFailure, Byte] = Stream.fromArray(values.toArray)
    override def cancel(): Future[Unit]                  = Future.successful(())
  }

  private def failedRead(failure: Throwable): ToolInputStream = new ToolInputStream {
    override val stream: Stream[ByteStreamFailure, Byte] = Stream.unfoldAsync(()) { _ =>
      Async.fromFuture[Option[(Byte, Unit)]](Future.failed(failure))
    }(using JvmType.Infer.byte)
    override def cancel(): Future[Unit] = Future.successful(())
  }

  private def throwingStream(failure: Throwable): ToolInputStream = new ToolInputStream {
    override def stream: Stream[ByteStreamFailure, Byte] = throw failure
    override def cancel(): Future[Unit]                  = Future.successful(())
  }

  private def sample(valueGraph: SchemaGraph = stringGraph, errorName: Option[String] = None): ToolType = {
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
      None,
      Some(WitResultSpec(schema.root, doc, Nil, "")),
      errorName.toList.map(name => WitErrorCase(name, doc, ErrorKind.RuntimeError, 1, Some(schema.root))),
      None
    )
    new ToolType(
      "sample",
      WitTool(
        "1",
        requiresFilesystem = true,
        WitCommandTree(
          Vector(
            WitCommandNode("sample", Nil, doc, WitGlobals(Nil, Nil), List(1), None),
            WitCommandNode("run", List("r"), doc, WitGlobals(Nil, Nil), Nil, Some(body))
          )
        ),
        schema
      ),
      ComponentId(new java.util.UUID(0L, 0L))
    )
  }

  /**
   * A bash-shaped command: a required `script` positional, `cwd` and `timeout`
   * options with declared defaults, a `shell` option whose type is already an
   * option and declares a default, an optional `env-file` option without a
   * default, a `login` bool flag and a `verbose` count flag. With
   * `collections`, it also declares a required `target` positional with a
   * default, an `args` tail and an `include` repeatable option with a default.
   */
  private def bash(collections: Boolean = false): ToolType = {
    val doc                                                                                       = Doc("", "", Nil)
    val encoder                                                                                   = new GraphEncoder(ListMap.empty)
    val string                                                                                    = encoder.encodeType(SchemaType(StringType))
    val u32                                                                                       = encoder.encodeType(SchemaType(U32Type(None)))
    val optional                                                                                  = encoder.encodeType(SchemaType(OptionType(SchemaType(StringType))))
    val schema                                                                                    = encoder.finish()
    def option(name: String, index: Int, default: Option[SchemaValue], required: Boolean = false) =
      WitOptionSpec(
        name,
        None,
        Nil,
        doc,
        None,
        WitOptionShape.Scalar(index),
        default.map(SchemaWire.schemaValueToWit),
        required,
        None
      )
    def positional(name: String, default: Option[SchemaValue]) =
      WitPositional(
        name,
        doc,
        None,
        string,
        default.map(SchemaWire.schemaValueToWit),
        required = true,
        acceptsStdio = false
      )
    val include = WitOptionSpec(
      "include",
      None,
      Nil,
      doc,
      None,
      WitOptionShape.RepeatableList(WitRepeatableListShape(Repetition.Repeated, string)),
      Some(SchemaWire.schemaValueToWit(ListValue(List(StringValue("default.sh"))))),
      required = false,
      None
    )
    val body = WitCommandBody(
      WitPositionals(
        positional("script", None) :: (if (collections) List(positional("target", Some(StringValue("all")))) else Nil),
        if (collections) Some(WitTailPositional("args", doc, None, string, 0, None, None, verbatim = false, false))
        else None
      ),
      List(
        option("cwd", string, Some(StringValue(""))),
        option("timeout", u32, Some(U32Value(600))),
        option("shell", optional, Some(OptionValue(Some(StringValue("sh"))))),
        option("env-file", string, None)
      ) ++ (if (collections) List(include) else Nil),
      List(
        FlagSpec("login", None, Nil, doc, FlagShape.BoolFlag(BoolFlagShape(default = false, negatable = true)), None),
        FlagSpec("verbose", None, Nil, doc, FlagShape.CountFlag(None), None)
      ),
      Nil,
      None,
      None,
      None,
      None,
      Nil,
      None
    )
    new ToolType(
      "bash",
      WitTool(
        "1",
        requiresFilesystem = false,
        WitCommandTree(Vector(WitCommandNode("bash", Nil, doc, WitGlobals(Nil, Nil), Nil, Some(body)))),
        schema
      ),
      ComponentId(new java.util.UUID(0L, 0L))
    )
  }

  private def bashValue(
    timeout: Long = 600,
    cwd: String = "",
    shell: Option[String] = Some("sh"),
    envFile: Option[String] = None,
    login: Boolean = false,
    verbose: Long = 0
  ): SchemaValue =
    RecordValue(
      List(
        StringValue("echo ok"),
        StringValue(cwd),
        U32Value(timeout),
        OptionValue(shell.map(StringValue(_))),
        OptionValue(envFile.map(StringValue(_))),
        BoolValue(login),
        U32Value(verbose)
      )
    )

  private def jsonFields(json: Json): ListMap[String, Json] = json match {
    case Json.Object(values) => ListMap(values.toList: _*)
    case other               => throw new AssertionError(s"expected a JSON object, found $other")
  }

  def spec = suite("ToolReflectionSpec")(
    test("JSON packing fills omitted declared and flag defaults") {
      val command = bash().command(Nil).toOption.get
      val input   = Json.Object("script" -> Json.String("echo ok"))
      assertTrue(
        command.packJson(input) == Right(bashValue()),
        command.inputSchema.validateJson(input) == Right(bashValue()),
        command.inputSchema.packJson(input) == Right(bashValue())
      )
    },
    test("JSON packing keeps explicit values, including null for option-typed arguments") {
      val command  = bash().command(Nil).toOption.get
      val explicit = Json.Object(
        "script"   -> Json.String("echo ok"),
        "cwd"      -> Json.String("/tmp"),
        "timeout"  -> Json.Number(BigDecimal(30)),
        "shell"    -> Json.Null,
        "env-file" -> Json.String(".env"),
        "login"    -> Json.Boolean(true),
        "verbose"  -> Json.Number(BigDecimal(2))
      )
      val large = Json.Object("script" -> Json.String("echo ok"), "timeout" -> Json.Number(BigDecimal(120000)))
      assertTrue(
        command.packJson(explicit) ==
          Right(bashValue(timeout = 30, cwd = "/tmp", shell = None, envFile = Some(".env"), login = true, verbose = 2)),
        command.packJson(large) == Right(bashValue(timeout = 120000)),
        command.packJson(Json.Object("script" -> Json.String("echo ok"), "env-file" -> Json.Null)) == Right(bashValue())
      )
    },
    test("JSON packing names a missing required argument and rejects unknown arguments") {
      val command = bash().command(Nil).toOption.get
      val missing = Json.Object("timeout" -> Json.Number(BigDecimal(30)))
      val unknown = Json.Object("script" -> Json.String("echo ok"), "bogus" -> Json.Boolean(true))
      assertTrue(
        command.packJson(missing) == Left(ToolError.InvalidInput("missing field 'script'")),
        command.inputSchema.validateJson(missing) == Left(List(SchemaIssue("missing field 'script'", List("script")))),
        command.packJson(unknown) == Left(ToolError.InvalidInput("unknown field 'bogus'")),
        command.inputSchema.validateJson(unknown) == Left(List(SchemaIssue("unknown field 'bogus'", List("bogus")))),
        command.inputSchema
          .validateJson(Json.Object("script" -> Json.String("echo ok"), "timeout" -> Json.Number(BigDecimal(-1))))
          .left
          .toOption
          .exists(_.map(_.path) == List(List("timeout")))
      )
    },
    test("input JSON Schema omits defaulted arguments from required and declares their defaults") {
      val command    = bash().command(Nil).toOption.get
      val rendered   = jsonFields(command.inputSchema.toJsonSchema())
      val plain      = jsonFields(SchemaRef(command.inputSchema.graph, command.inputSchema.root).toJsonSchema())
      val properties = jsonFields(rendered("properties"))
      val original   = jsonFields(plain("properties"))
      val defaults   = ListMap(
        "cwd"     -> Json.String(""),
        "timeout" -> Json.Number(BigDecimal(600)),
        "shell"   -> Json.String("sh"),
        "login"   -> Json.Boolean(false),
        "verbose" -> Json.Number(BigDecimal(0))
      )
      assertTrue(
        rendered("required") == Json.Array(Json.String("script")),
        rendered.keys.toList == plain.keys.toList,
        rendered.removed("required").removed("properties") == plain.removed("required").removed("properties"),
        properties.keys.toList == List("script", "cwd", "timeout", "shell", "env-file", "login", "verbose"),
        properties.forall { case (name, property) =>
          defaults.get(name) match {
            case Some(default) => jsonFields(property) == jsonFields(original(name)).updated("default", default)
            case None          => property == original(name)
          }
        }
      )
    },
    test("JSON defaults leave the canonical wire record and the value surface unchanged") {
      val command = bash().command(Nil).toOption.get
      val root    = SchemaType(
        RecordType(
          List(
            NamedFieldType("script", SchemaType(StringType)),
            NamedFieldType("cwd", SchemaType(StringType)),
            NamedFieldType("timeout", SchemaType(U32Type(None))),
            NamedFieldType("shell", SchemaType(OptionType(SchemaType(StringType)))),
            NamedFieldType("env-file", SchemaType(OptionType(SchemaType(StringType)))),
            NamedFieldType("login", SchemaType(BoolType)),
            NamedFieldType("verbose", SchemaType(U32Type(None)))
          )
        )
      )
      assertTrue(
        command.inputSchema.root == root,
        command.inputSchema.graph == SchemaGraph(ListMap.empty, root),
        command.inputSchema.validateValue(bashValue()).isRight,
        command.inputSchema.validateValue(RecordValue(List(StringValue("echo ok")))).isLeft,
        command.startValue(RecordValue(List(StringValue("echo ok")))).left.toOption.exists {
          case ToolError.InvalidInput(_) => true
          case _                         => false
        }
      )
    },
    test("tails and repeatable options are not filled, while a required positional default is") {
      val command = bash(collections = true).command(Nil).toOption.get
      val schema  = jsonFields(command.inputSchema.toJsonSchema())
      val include = jsonFields(jsonFields(schema("properties"))("include"))
      val target  = jsonFields(jsonFields(schema("properties"))("target"))
      assertTrue(
        command.packJson(Json.Object("script" -> Json.String("echo ok"))) ==
          Left(ToolError.InvalidInput("missing field 'args'")),
        command.packJson(Json.Object("script" -> Json.String("echo ok"), "args" -> Json.Array())) ==
          Left(ToolError.InvalidInput("missing field 'include'")),
        command
          .packJson(Json.Object("script" -> Json.String("echo ok"), "args" -> Json.Array(), "include" -> Json.Array()))
          .toOption
          .exists {
            case RecordValue(fields) =>
              fields.take(3) == List(StringValue("echo ok"), StringValue("all"), ListValue(Nil)) &&
              fields(7) == ListValue(Nil)
            case _ => false
          },
        schema("required") == Json.Array(Json.String("script"), Json.String("args"), Json.String("include")),
        !include.contains("default"),
        target.get("default").contains(Json.String("all"))
      )
    },
    test("reflected value, JSON, and dynamic clients preserve explicit detach failure") {
      ZIO.fromFuture { implicit ec =>
        val global        = js.Dynamic.global.globalThis
        val previous      = global.selectDynamic("__golemScalaTestHost")
        val pending       = Promise[JsInvocationResult]()
        var drops         = 0
        var cancellations = 0
        val observer      = js.Dynamic.literal(
          "get"    -> js.Any.fromFunction0(() => FutureInterop.toPromise(pending.future)),
          "cancel" -> js.Any.fromFunction0(() => cancellations += 1)
        )
        js.Dynamic.global.Reflect
          .set(observer, js.Dynamic.global.Symbol.selectDynamic("dispose"), js.Any.fromFunction0(() => drops += 1))
        val rpc = js.Dynamic.literal("asyncInvokeAndAwait" -> js.Any.fromFunction5 {
          (_: js.Any, _: js.Any, _: js.Any, _: js.Any, _: js.Any) => observer
        })
        global.updateDynamic("__golemScalaTestHost")(
          js.Dynamic.literal(
            "createToolRpc" -> js.Any.fromFunction1((name: String) =>
              if (name == "sample") rpc else throw new IllegalArgumentException(s"unknown tool: $name")
            )
          )
        )
        val command = sample().command(List("run")).toOption.get
        val json    = Json.Object("message" -> Json.String("hello"))
        val value   = command.startValue(command.packJson(json).toOption.get).toOption.get
        val packed  = command.startJson(json).toOption.get
        val dynamic =
          new DynamicToolClient("sample").start(List("run"), IntoSchema[String].toTyped("hello")).toOption.get
        value.detach()
        packed.detach()
        dynamic.detach()
        Future.sequence(List(value.result.failed, packed.result.failed, dynamic.result.failed)).transform { outcome =>
          pending.trySuccess(JsInvocationResult(js.undefined))
          global.updateDynamic("__golemScalaTestHost")(previous)
          outcome.map(errors =>
            assertTrue(
              errors.forall(_.isInstanceOf[IllegalStateException]),
              errors.forall(_.getMessage == "tool result observer detached"),
              cancellations == 0
            )
          )
        }
      }
    },
    test("filesystem requirement survives reflection and JS wire conversion") {
      val wire         = sample().definition
      val js           = ToolWireInterop.toolToJs(wire)
      val roundtripped = ToolWireInterop.toolFromJs(js)
      assertTrue(
        wire.requiresFilesystem,
        ToolReflection.fromWire(wire).requiresFilesystem,
        js.requiresFilesystem,
        roundtripped.requiresFilesystem
      )
    },
    test("alias resolves to the canonical command path") {
      val command = sample().command(List("r")).toOption.get
      assertTrue(command.path == List("run"), command.arguments.map(_.name) == List("message"))
    },
    test("namespace-only commands remain inspectable and reject invocation locally") {
      val namespace = sample().command(Nil).toOption.get
      assertTrue(
        namespace.path.isEmpty,
        namespace.body.isEmpty,
        namespace.subcommands == List("run"),
        namespace.startValue(RecordValue(Nil)).left.toOption.exists(_.isInstanceOf[ToolError.InvalidInput])
      )
    },
    test("nested namespace metadata exposes canonical child names") {
      val original = sample()
      val nodes    = original.definition.commands.nodes
      val nested   = new ToolType(
        original.lookupName,
        original.definition.copy(commands =
          original.definition.commands.copy(nodes =
            Vector(
              nodes(0).copy(subcommands = List(1)),
              nodes(0).copy(name = "group", aliases = List("g"), subcommands = List(2)),
              nodes(1)
            )
          )
        ),
        original.implementedBy
      )
      val namespace = nested.command(List("g")).toOption.get
      val command   = nested.command(List("g", "r")).toOption.get
      assertTrue(
        namespace.path == List("group"),
        namespace.body.isEmpty,
        namespace.subcommands == List("run"),
        command.path == List("group", "run")
      )
    },
    test("caller-owned tool definitions bind named and partial clients without discovery") {
      val named   = ToolClientDefinition.named("stable-tool")(identity[String])
      val partial = ToolClientDefinition.unnamed(identity[String])
      assertTrue(
        named.client == Right("stable-tool"),
        named.client("ignored") == Right("stable-tool"),
        partial.client.isLeft,
        partial.client("deployed-tool") == Right("deployed-tool")
      )
    },
    test("generated tool transport creation reports an invalid target fallibly") {
      val definition = ToolClientDefinition.named("missing-tool")(
        golem.runtime.tool.client.ToolRpcClient.transport
      )
      assertTrue(definition.client.left.toOption.exists(_.isInstanceOf[GolemReflectError.ToolRpc]))
    },
    test("caller-owned tool construction retains the structured RPC failure") {
      val failure    = ToolRpcFailure.Denied("not authorized")
      val definition = ToolClientDefinition.unnamed[String](_ =>
        throw new golem.runtime.tool.client.ToolRpcConstructionException(failure)
      )
      assertTrue(definition.client("target") == Left(GolemReflectError.ToolRpc(failure)))
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
    test("deep result and declared error graphs remain structurally comparable") {
      def nested(depth: Int, leaf: SchemaType): SchemaType =
        (0 until depth).foldLeft(leaf)((current, _) => SchemaType(ListType(current)))
      def value(depth: Int): SchemaValue =
        (0 until depth).foldLeft(StringValue("hello"): SchemaValue)((current, _) => ListValue(List(current)))
      def graph(depth: Int, id: String, leaf: SchemaType): SchemaGraph =
        SchemaGraph(ListMap(id -> SchemaTypeDef(nested(depth, leaf))), SchemaType(RefType(id)))

      assertTrue(List(31, 32, 33).forall { depth =>
        val expected     = graph(depth, "Expected", SchemaType(StringType))
        val matching     = TypedSchemaValue(graph(depth, "Equivalent", SchemaType(StringType)), value(depth))
        val wrong        = TypedSchemaValue(graph(depth, "Expected", SchemaType(S32Type(None))), value(depth))
        val command      = sample(expected, Some("declared")).command(List("run")).toOption.get
        val matchedError = command.mapFailure(
          ToolRpcFailure.RemoteToolError(ToolInvokeError.UnknownToolError("declared", matching))
        )
        val wrongError = command.mapFailure(
          ToolRpcFailure.RemoteToolError(ToolInvokeError.UnknownToolError("declared", wrong))
        )
        command.decodeResult(ToolInvokeResult(Some(matching))).isRight &&
        command.decodeResult(ToolInvokeResult(Some(wrong))).isLeft &&
        matchedError == ToolError.Tool(NamedToolError("declared", matching)) &&
        wrongError.isInstanceOf[ToolError.MalformedRemoteOutput]
      })
    },
    test("stream failures remain recoverable for reflected calls") {
      val broken = new ToolInputStream {
        override val stream   = zio.blocks.streams.Stream.fail(ByteStreamFailure.Failed("broken"))
        override def cancel() = Future.successful(())
      }
      val terminal: Future[Either[ToolError[NamedToolError], Option[SchemaValue]]] = Future.successful(Right(None))
      val invocation                                                               = ReflectedToolInvocation(Some(broken), None, terminal, () => ())
      ZIO.fromFuture(_ => invocation.collect()(using ExecutionContext.global)).map { result =>
        assertTrue(
          result.result == Right(None),
          result.stdout == Left(ByteStreamFailure.Failed("broken")),
          result.stderr == Right(None)
        )
      }
    },
    test("declared tool and stdout failures are both retained after all channels settle") {
      val broken = new ToolInputStream {
        override val stream   = zio.blocks.streams.Stream.fail(ByteStreamFailure.Failed("broken"))
        override def cancel() = Future.successful(())
      }
      val terminal   = Promise[Either[ToolError[NamedToolError], Option[SchemaValue]]]()
      val invocation = ReflectedToolInvocation(Some(broken), None, terminal.future, () => ())
      val collected  = invocation.collect()(using ExecutionContext.global)
      val payload    = TypedSchemaValue(stringGraph, StringValue("details"))
      terminal.success(Left(ToolError.Tool(NamedToolError("declared", payload))))
      ZIO.fromFuture(_ => collected).map { result =>
        assertTrue(
          result.result == Left(ToolError.Tool(NamedToolError("declared", payload))),
          result.stdout == Left(ByteStreamFailure.Failed("broken")),
          result.stderr == Right(None)
        )
      }
    },
    test("failed reflected result and read futures remain isolated from sibling output") {
      val resultFailure = new RuntimeException("reflected result failed")
      val readFailure   = new RuntimeException("reflected stdout failed")
      val invocation    = ReflectedToolInvocation(
        Some(failedRead(readFailure)),
        Some(bytes(5, 6)),
        Future.failed(resultFailure),
        () => ()
      )
      ZIO.fromFuture(ec => invocation.collect()(ec)).map { result =>
        assertTrue(
          result.result == Left(ToolError.Rpc(RpcError.Protocol("reflected result failed"))),
          result.stdout == Left(ByteStreamFailure.Failed("reflected stdout failed")),
          result.stderr.exists(_.exists(_.sameElements(Array[Byte](5, 6))))
        )
      }
    },
    test("synchronous reflected stream failures remain isolated from sibling output") {
      val failure    = new RuntimeException("reflected stderr failed")
      val invocation = ReflectedToolInvocation(
        Some(bytes(1, 2)),
        Some(throwingStream(failure)),
        Future.successful(Right(None)),
        () => ()
      )
      ZIO.fromFuture(ec => invocation.collect()(ec)).map { result =>
        assertTrue(
          result.result == Right(None),
          result.stdout.exists(_.exists(_.sameElements(Array[Byte](1, 2)))),
          result.stderr == Left(ByteStreamFailure.Failed("reflected stderr failed"))
        )
      }
    },
    test("reflected collection does not convert cancellation into a result") {
      val cancellation = new CancellationException("cancelled")
      val invocation   = ReflectedToolInvocation(None, None, Future.failed(cancellation), () => ())
      ZIO.fromFuture(ec => invocation.collect()(ec).failed).map(error => assertTrue(error eq cancellation))
    },
    test("reflected collection does not convert a boxed fatal failure") {
      val fatal      = new LinkageError("fatal reflected result")
      val invocation = ReflectedToolInvocation(None, None, Future.failed(fatal), () => ())
      ZIO.fromFuture(ec => invocation.collect()(ec).failed).map(error => assertTrue(error.getCause eq fatal))
    }
  )
}
