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

package golem

import golem.{UByte, UShort, UInt, ULong}
import golem.runtime.autowire.{AgentImplementation, MethodBinding}
import golem.runtime.{OutputMetadata, ParameterMetadata}
import golem.schema.{SchemaGraph, SchemaTypeBody}
import zio.test._
import zio.blocks.schema.Schema

import scala.concurrent.Future

/**
 * Verifies that the agent macros produce correct schema-native per-parameter
 * metadata (the `golem:agent@2.0.0` `input-schema` / `output-schema`) for a
 * broad range of Scala parameter / return types, asserting on the model
 * [[SchemaGraph]] (not the JS facade).
 */
object SchemaVerificationSpec extends ZIOSpecDefault {

  // ---------------------------------------------------------------------------
  // Fixture types
  // ---------------------------------------------------------------------------

  final case class PersonInfo(name: String, age: Int)
  object PersonInfo { implicit val schema: Schema[PersonInfo] = Schema.derived }

  sealed trait Color
  object Color {
    case object Red   extends Color
    case object Green extends Color
    case object Blue  extends Color
    implicit val schema: Schema[Color] = Schema.derived
  }

  // ---------------------------------------------------------------------------
  // Agent with many parameter/return type combinations
  // ---------------------------------------------------------------------------

  @agentDefinition("schema-verify-agent")
  trait SchemaVerifyAgent extends BaseAgent {
    class Id()
    def stringMethod(s: String): Future[String]
    def intMethod(i: Int): Future[Int]
    def boolMethod(b: Boolean): Future[Boolean]
    def byteMethod(b: Byte): Future[Byte]
    def shortMethod(s: Short): Future[Short]
    def longMethod(l: Long): Future[Long]
    def doubleMethod(d: Double): Future[Double]
    def floatMethod(f: Float): Future[Float]
    def ubyteMethod(u: UByte): Future[UByte]
    def ushortMethod(u: UShort): Future[UShort]
    def uintMethod(u: UInt): Future[UInt]
    def ulongMethod(u: ULong): Future[ULong]
    def optionMethod(o: Option[String]): Future[Option[String]]
    def listMethod(l: List[Int]): Future[List[Int]]
    def caseClassMethod(p: PersonInfo): Future[PersonInfo]
    def multiParamMethod(name: String, age: Int, active: Boolean): Future[String]
    def unitReturnMethod(s: String): Future[Unit]
    def pureEnumMethod(c: Color): Future[Color]
  }

  @agentImplementation()
  final class SchemaVerifyAgentImpl() extends SchemaVerifyAgent {
    override def stringMethod(s: String): Future[String]                                   = Future.successful(s)
    override def intMethod(i: Int): Future[Int]                                            = Future.successful(i)
    override def boolMethod(b: Boolean): Future[Boolean]                                   = Future.successful(b)
    override def byteMethod(b: Byte): Future[Byte]                                         = Future.successful(b)
    override def shortMethod(s: Short): Future[Short]                                      = Future.successful(s)
    override def longMethod(l: Long): Future[Long]                                         = Future.successful(l)
    override def doubleMethod(d: Double): Future[Double]                                   = Future.successful(d)
    override def floatMethod(f: Float): Future[Float]                                      = Future.successful(f)
    override def ubyteMethod(u: UByte): Future[UByte]                                      = Future.successful(u)
    override def ushortMethod(u: UShort): Future[UShort]                                   = Future.successful(u)
    override def uintMethod(u: UInt): Future[UInt]                                         = Future.successful(u)
    override def ulongMethod(u: ULong): Future[ULong]                                      = Future.successful(u)
    override def optionMethod(o: Option[String]): Future[Option[String]]                   = Future.successful(o)
    override def listMethod(l: List[Int]): Future[List[Int]]                               = Future.successful(l)
    override def caseClassMethod(p: PersonInfo): Future[PersonInfo]                        = Future.successful(p)
    override def multiParamMethod(name: String, age: Int, active: Boolean): Future[String] =
      Future.successful(s"$name-$age-$active")
    override def unitReturnMethod(s: String): Future[Unit] = Future.successful(())
    override def pureEnumMethod(c: Color): Future[Color]   = Future.successful(c)
  }

  private lazy val defn = AgentImplementation.registerClass[SchemaVerifyAgent, SchemaVerifyAgentImpl]

  private def findMethod(name: String): MethodBinding[SchemaVerifyAgent] =
    defn.methodMetadata.find(_.metadata.name == name).getOrElse(sys.error(s"method not found: $name"))

  private def params(methodName: String): List[ParameterMetadata] =
    findMethod(methodName).metadata.input.userSupplied

  private def inputElementCount(methodName: String): Int =
    params(methodName).size

  private def outputElementCount(methodName: String): Int =
    findMethod(methodName).metadata.output match {
      case OutputMetadata.Unit      => 0
      case OutputMetadata.Single(_) => 1
    }

  private def inputElementNames(methodName: String): List[String] =
    params(methodName).map(_.name)

  /**
   * The effective root body of a graph, dereferencing a top-level named ref.
   */
  private def rootBody(g: SchemaGraph): SchemaTypeBody = g.root.body match {
    case SchemaTypeBody.RefType(id) => g.defs(id).body.body
    case other                      => other
  }

  /** The schema body of the first (single) parameter of a method. */
  private def firstParamBody(methodName: String): SchemaTypeBody =
    rootBody(params(methodName).head.graph)

  def spec = suite("SchemaVerificationSpec")(
    test("single-param method has 1 input element") {
      assertTrue(
        inputElementCount("stringMethod") == 1,
        inputElementCount("intMethod") == 1,
        inputElementCount("boolMethod") == 1,
        inputElementCount("optionMethod") == 1,
        inputElementCount("listMethod") == 1,
        inputElementCount("caseClassMethod") == 1
      )
    },
    test("multi-param method has 3 input elements") {
      assertTrue(inputElementCount("multiParamMethod") == 3)
    },
    test("single-output method has 1 output element") {
      assertTrue(outputElementCount("stringMethod") == 1)
    },
    test("unit-returning method has 0 output elements") {
      assertTrue(outputElementCount("unitReturnMethod") == 0)
    },
    test("single-param method element name is the parameter name") {
      assertTrue(
        inputElementNames("stringMethod") == List("s"),
        inputElementNames("intMethod") == List("i"),
        inputElementNames("caseClassMethod") == List("p")
      )
    },
    test("multi-param method element names match parameter names") {
      assertTrue(inputElementNames("multiParamMethod") == List("name", "age", "active"))
    },
    test("byte method produces s8 schema body") {
      assertTrue(firstParamBody("byteMethod") == SchemaTypeBody.S8Type())
    },
    test("short method produces s16 schema body") {
      assertTrue(firstParamBody("shortMethod") == SchemaTypeBody.S16Type())
    },
    test("int method produces s32 schema body") {
      assertTrue(firstParamBody("intMethod") == SchemaTypeBody.S32Type())
    },
    test("long method produces s64 schema body") {
      assertTrue(firstParamBody("longMethod") == SchemaTypeBody.S64Type())
    },
    test("float method produces f32 schema body") {
      assertTrue(firstParamBody("floatMethod") == SchemaTypeBody.F32Type())
    },
    test("double method produces f64 schema body") {
      assertTrue(firstParamBody("doubleMethod") == SchemaTypeBody.F64Type())
    },
    test("ubyte method produces u8 schema body") {
      assertTrue(firstParamBody("ubyteMethod") == SchemaTypeBody.U8Type())
    },
    test("ushort method produces u16 schema body") {
      assertTrue(firstParamBody("ushortMethod") == SchemaTypeBody.U16Type())
    },
    test("uint method produces u32 schema body") {
      assertTrue(firstParamBody("uintMethod") == SchemaTypeBody.U32Type())
    },
    test("ulong method produces u64 schema body") {
      assertTrue(firstParamBody("ulongMethod") == SchemaTypeBody.U64Type())
    },
    test("string method produces string schema body") {
      assertTrue(firstParamBody("stringMethod") == SchemaTypeBody.StringType)
    },
    test("bool method produces bool schema body") {
      assertTrue(firstParamBody("boolMethod") == SchemaTypeBody.BoolType)
    },
    test("pure enum method produces enum schema body") {
      assertTrue(firstParamBody("pureEnumMethod") match {
        case _: SchemaTypeBody.EnumType => true
        case _                          => false
      })
    },
    test("schema-verify agent has 18 registered methods") {
      assertTrue(defn.methodMetadata.size == 18)
    }
  )
}
