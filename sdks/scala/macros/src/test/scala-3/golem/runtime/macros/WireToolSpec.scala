/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.runtime.macros

import golem.Principal
import golem.schema.*
import golem.schema.SchemaTypeBody.{ListType, S8Type, U8Type}
import golem.schema.validation.RefResolution
import golem.schema.wire.*
import golem.tool.*
import golem.tool.wire.*
import zio.test.*

object WireToolSpec extends ZIOSpecDefault {
  private val echo = WireToolMacro.handle[ToolInvokerSpec.Echo, ToolInvokerSpec.EchoImpl]
  private val git  = WireToolMacro.handle[ToolInvokerSpec.Git, ToolInvokerSpec.GitImpl]

  @golem.runtime.annotations.toolDefinition(version = "1.0.0")
  trait UuidTool {
    def echo(value: golem.Uuid): golem.Uuid
  }

  final class UuidToolImpl extends UuidTool {
    def echo(value: golem.Uuid): golem.Uuid = value
  }

  @golem.runtime.annotations.toolDefinition(version = "1.0.0")
  trait ByteArrayTool {
    def echo(value: Array[Byte], scalar: Byte): Array[Byte]
  }

  final class ByteArrayToolImpl extends ByteArrayTool {
    def echo(value: Array[Byte], scalar: Byte): Array[Byte] = value
  }

  private val uuidTool      = WireToolMacro.handle[UuidTool, UuidToolImpl]
  private val byteArrayTool = WireToolMacro.handle[ByteArrayTool, ByteArrayToolImpl]

  private def rootBody(graph: SchemaGraph): SchemaTypeBody =
    RefResolution.resolveRef(graph, graph.root).toOption.get.body

  private def input(nodes: WitSchemaValueNode*): WireToolInput =
    WireToolInput(
      WitSchemaValueTree(nodes.toVector :+ WitSchemaValueNode.RecordValue(nodes.indices.toVector), nodes.length),
      None,
      None,
      None,
      Principal.Anonymous
    )

  private def invoke(tool: WireToolImplementation, path: String, input: WireToolInput) =
    tool.invoke(if (path.isEmpty) Nil else path.split('/').toList, input).value.get.get

  private def text(result: Either[WitToolError, Option[WitTypedSchemaValue]]) =
    ConcreteCodec.string.decode(result.fold(error => throw new AssertionError(error), _.get).value)

  override def spec = suite("WireToolSpec")(
    test("compiled wire descriptor exactly matches the dynamic reflection descriptor") {
      assertTrue(
        echo.descriptor == ToolDefinitionMacro.metadata[ToolInvokerSpec.Echo].tryToTool.toOption.get,
        git.descriptor == ToolDefinitionMacro.metadata[ToolInvokerSpec.Git].tryToTool.toOption.get
      )
    },
    test("generated methods decode direct canonical positions and encode results") {
      val plain = invoke(echo, "", input(WitSchemaValueNode.StringValue("hello")))
      val count = invoke(echo, "repeat", input(WitSchemaValueNode.StringValue("x"), WitSchemaValueNode.U32Value(3)))
      val async = invoke(echo, "async-echo", input(WitSchemaValueNode.StringValue("later")))
      assertTrue(text(plain) == "echo: hello", text(count) == "xxx", text(async) == "async: later")
    },
    test("custom errors encode the concrete payload and case name") {
      val failed = invoke(echo, "fail", input(WitSchemaValueNode.StringValue("wrong")))
      assertTrue(failed match {
        case Left(WitToolError.CustomError(error)) =>
          error.name == "bad-input" && ConcreteCodec.string.decode(error.payload.value) == "wrong"
        case _ => false
      })
    },
    test("static subtree calls preserve parent and leaf input ordering") {
      val result = invoke(
        git,
        "remote/add",
        input(
          WitSchemaValueNode.StringValue(".git"),
          WitSchemaValueNode.StringValue("origin"),
          WitSchemaValueNode.StringValue("https://example.test")
        )
      )
      assertTrue(text(result) == "origin=https://example.test")
    },
    test("wrong kinds, arity, paths and signed count carriers reject") {
      val kind   = invoke(echo, "", input(WitSchemaValueNode.S32Value(9)))
      val arity  = invoke(echo, "", input())
      val path   = invoke(echo, "absent", input())
      val signed = invoke(echo, "repeat", input(WitSchemaValueNode.StringValue("x"), WitSchemaValueNode.S32Value(3)))
      assertTrue(
        kind.left.toOption.exists(_.isInstanceOf[WitToolError.InvalidInput]),
        arity.left.toOption.exists(_.isInstanceOf[WitToolError.InvalidInput]),
        path == Left(WitToolError.InvalidCommandPath(List("absent"))),
        signed.left.toOption.exists(_.isInstanceOf[WitToolError.InvalidInput])
      )
    },
    test("Uuid invocation uses the u64 record representation declared by its descriptor") {
      val result = invoke(
        uuidTool,
        "echo",
        WireToolInput(
          WitSchemaValueTree(
            Vector(
              WitSchemaValueNode.U64Value(Long.MinValue),
              WitSchemaValueNode.U64Value(-1L),
              WitSchemaValueNode.RecordValue(Vector(0, 1)),
              WitSchemaValueNode.RecordValue(Vector(2))
            ),
            3
          ),
          None,
          None,
          None,
          Principal.Anonymous
        )
      )
      assertTrue(
        result.exists(
          _.exists(value =>
            value.value.valueNodes == Vector(
              WitSchemaValueNode.U64Value(Long.MinValue),
              WitSchemaValueNode.U64Value(-1L),
              WitSchemaValueNode.RecordValue(Vector(0, 1))
            ) && value.value.root == 2 && value.graph == ConcreteCodec.uuid.graph
          )
        )
      )
    },
    test("compiled byte arrays use list<u8> while scalar bytes remain s8") {
      val reflected = ToolReflection.fromWire(byteArrayTool.descriptor)
      val body      = reflected.commands(reflected.commandIndexByPath(List("echo")).get).body.get
      assertTrue(
        rootBody(body.positionals.fixed.head.tpe) == ListType(SchemaType(U8Type(None))),
        rootBody(body.positionals.fixed(1).tpe) == S8Type(None),
        rootBody(body.result.get.tpe) == ListType(SchemaType(U8Type(None)))
      )
    },
    test("compiled byte array codecs preserve high-bit values") {
      val result = invoke(
        byteArrayTool,
        "echo",
        WireToolInput(
          WitSchemaValueTree(
            Vector(
              WitSchemaValueNode.U8Value(255),
              WitSchemaValueNode.U8Value(128),
              WitSchemaValueNode.ListValue(Vector(0, 1)),
              WitSchemaValueNode.S8Value(-1),
              WitSchemaValueNode.RecordValue(Vector(2, 3))
            ),
            4
          ),
          None,
          None,
          None,
          Principal.Anonymous
        )
      )
      assertTrue(
        result.exists(_.exists(value => ConcreteCodec.bytes.decode(value.value).sameElements(Array[Byte](-1, -128))))
      )
    }
  )
}
