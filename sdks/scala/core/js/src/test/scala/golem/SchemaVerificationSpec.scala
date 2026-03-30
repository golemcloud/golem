/*
 * Copyright 2024-2026 John A. De Goes and the ZIO Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem

import golem.data.{UByte, UShort, UInt, ULong}
import golem.data.multimodal.Multimodal
import golem.data.unstructured.{AllowedLanguages, AllowedMimeTypes, BinarySegment, TextSegment}
import golem.runtime.autowire.{AgentImplementation, MethodBinding}
import zio.test._
import zio.blocks.schema.Schema

import scala.concurrent.Future
import scala.scalajs.js

object SchemaVerificationSpec extends ZIOSpecDefault {

  // ---------------------------------------------------------------------------
  // Fixture types
  // ---------------------------------------------------------------------------

  final case class PersonInfo(name: String, age: Int)
  object PersonInfo { implicit val schema: Schema[PersonInfo] = Schema.derived }

  final case class Address(street: String, city: String, zip: Int)
  object Address { implicit val schema: Schema[Address] = Schema.derived }

  sealed trait SvLang
  object SvLang {
    implicit val allowed: AllowedLanguages[SvLang] = new AllowedLanguages[SvLang] {
      override val codes: Option[List[String]] = Some(List("en", "de"))
    }
  }

  sealed trait SvMime
  object SvMime {
    implicit val allowed: AllowedMimeTypes[SvMime] = new AllowedMimeTypes[SvMime] {
      override val mimeTypes: Option[List[String]] = Some(List("application/json"))
    }
  }

  // ---------------------------------------------------------------------------
  // Agent with many parameter/return type combinations
  // ---------------------------------------------------------------------------

  sealed trait Color
  object Color {
    case object Red   extends Color
    case object Green extends Color
    case object Blue  extends Color
    implicit val schema: Schema[Color] = Schema.derived
  }

  implicit val eitherStringIntSchema: Schema[Either[String, Int]] = Schema.derived

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
    def textSegmentMethod(t: TextSegment[SvLang]): Future[TextSegment[SvLang]]
    def binarySegmentMethod(b: BinarySegment[SvMime]): Future[BinarySegment[SvMime]]
    def multimodalMethod(m: Multimodal[PersonInfo]): Future[Multimodal[PersonInfo]]
    def pureEnumMethod(c: Color): Future[Color]
    def eitherMethod(e: Either[String, Int]): Future[Either[String, Int]]
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
    override def unitReturnMethod(s: String): Future[Unit]                                    = Future.successful(())
    override def textSegmentMethod(t: TextSegment[SvLang]): Future[TextSegment[SvLang]]       = Future.successful(t)
    override def binarySegmentMethod(b: BinarySegment[SvMime]): Future[BinarySegment[SvMime]] = Future.successful(b)
    override def multimodalMethod(m: Multimodal[PersonInfo]): Future[Multimodal[PersonInfo]]  = Future.successful(m)
    override def pureEnumMethod(c: Color): Future[Color]                                      = Future.successful(c)
    override def eitherMethod(e: Either[String, Int]): Future[Either[String, Int]]            = Future.successful(e)
  }

  private lazy val defn = AgentImplementation.registerClass[SchemaVerifyAgent, SchemaVerifyAgentImpl]

  private def findMethod(name: String): MethodBinding[SchemaVerifyAgent] =
    defn.methodMetadata.find(_.metadata.name == name).getOrElse(sys.error(s"method not found: $name"))

  private def schemaTag(schema: golem.host.js.JsDataSchema): String =
    schema.tag

  private val multimodalMethods = Set("multimodalMethod")

  private def inputElementCount(methodName: String): Int = {
    val m   = findMethod(methodName)
    val arr = m.inputSchema.asInstanceOf[js.Dynamic].selectDynamic("val").asInstanceOf[js.Array[js.Any]]
    arr.length
  }

  private def outputElementCount(methodName: String): Int = {
    val m   = findMethod(methodName)
    val arr = m.outputSchema.asInstanceOf[js.Dynamic].selectDynamic("val").asInstanceOf[js.Array[js.Any]]
    arr.length
  }

  private def inputElementNames(methodName: String): List[String] = {
    val m   = findMethod(methodName)
    val arr = m.inputSchema.asInstanceOf[js.Dynamic].selectDynamic("val").asInstanceOf[js.Array[js.Array[js.Any]]]
    (0 until arr.length).map(i => arr(i)(0).asInstanceOf[String]).toList
  }

  private def firstInputElementTag(methodName: String): String = {
    val m    = findMethod(methodName)
    val arr  = m.inputSchema.asInstanceOf[js.Dynamic].selectDynamic("val").asInstanceOf[js.Array[js.Array[js.Any]]]
    val elem = arr(0)(1).asInstanceOf[js.Dynamic]
    elem.selectDynamic("tag").asInstanceOf[String]
  }

  /**
   * For a single-param component-model method, returns the WIT type node tag of
   * its root type node.
   */
  private def firstInputWitTypeTag(methodName: String): String = {
    val m    = findMethod(methodName)
    val arr  = m.inputSchema.asInstanceOf[js.Dynamic].selectDynamic("val").asInstanceOf[js.Array[js.Array[js.Any]]]
    val elem = arr(0)(1).asInstanceOf[js.Dynamic]
    // elem is { tag: "component-model", val: { nodes: [...] } }
    val witType = elem.selectDynamic("val").asInstanceOf[js.Dynamic]
    val nodes   = witType.selectDynamic("nodes").asInstanceOf[js.Array[js.Dynamic]]
    // Root node is first element; get its "type" field's tag
    val rootNode = nodes(0)
    rootNode.selectDynamic("type").selectDynamic("tag").asInstanceOf[String]
  }

  def spec = suite("SchemaVerificationSpec")(
    test("non-multimodal methods have tuple inputSchema tag") {
      defn.methodMetadata
        .filterNot(m => multimodalMethods(m.metadata.name))
        .foreach { m =>
          assert(schemaTag(m.inputSchema) == "tuple")(Assertion.isTrue)
        }
      assertCompletes
    },
    test("non-multimodal methods have tuple outputSchema tag") {
      defn.methodMetadata
        .filterNot(m => multimodalMethods(m.metadata.name))
        .foreach { m =>
          assert(schemaTag(m.outputSchema) == "tuple")(Assertion.isTrue)
        }
      assertCompletes
    },
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
    test("string-returning method has 1 output element") {
      assertTrue(outputElementCount("stringMethod") == 1)
    },
    test("unit-returning method has 0 output elements") {
      assertTrue(outputElementCount("unitReturnMethod") == 0)
    },
    test("single-param method element name is 'value' (from GolemSchema.single)") {
      assertTrue(
        inputElementNames("stringMethod") == List("value"),
        inputElementNames("intMethod") == List("value"),
        inputElementNames("caseClassMethod") == List("value")
      )
    },
    test("multi-param method element names match parameter names") {
      assertTrue(inputElementNames("multiParamMethod") == List("name", "age", "active"))
    },
    test("TextSegment parameter produces unstructured-text schema tag") {
      assertTrue(firstInputElementTag("textSegmentMethod") == "unstructured-text")
    },
    test("BinarySegment parameter produces unstructured-binary schema tag") {
      assertTrue(firstInputElementTag("binarySegmentMethod") == "unstructured-binary")
    },
    test("Multimodal parameter produces multimodal input schema tag") {
      val m = findMethod("multimodalMethod")
      assertTrue(schemaTag(m.inputSchema) == "multimodal")
    },
    test("byte method produces prim-s8-type WIT node") {
      assertTrue(firstInputWitTypeTag("byteMethod") == "prim-s8-type")
    },
    test("short method produces prim-s16-type WIT node") {
      assertTrue(firstInputWitTypeTag("shortMethod") == "prim-s16-type")
    },
    test("int method produces prim-s32-type WIT node") {
      assertTrue(firstInputWitTypeTag("intMethod") == "prim-s32-type")
    },
    test("float method produces prim-f32-type WIT node") {
      assertTrue(firstInputWitTypeTag("floatMethod") == "prim-f32-type")
    },
    test("double method produces prim-f64-type WIT node") {
      assertTrue(firstInputWitTypeTag("doubleMethod") == "prim-f64-type")
    },
    test("ubyte method produces prim-u8-type WIT node") {
      assertTrue(firstInputWitTypeTag("ubyteMethod") == "prim-u8-type")
    },
    test("ushort method produces prim-u16-type WIT node") {
      assertTrue(firstInputWitTypeTag("ushortMethod") == "prim-u16-type")
    },
    test("uint method produces prim-u32-type WIT node") {
      assertTrue(firstInputWitTypeTag("uintMethod") == "prim-u32-type")
    },
    test("ulong method produces prim-u64-type WIT node") {
      assertTrue(firstInputWitTypeTag("ulongMethod") == "prim-u64-type")
    },
    test("schema-verify agent has 22 registered methods") {
      assertTrue(defn.methodMetadata.size == 22)
    },
    test("pure enum method produces enum-type WIT node") {
      assertTrue(firstInputWitTypeTag("pureEnumMethod") == "enum-type")
    },
    test("either method produces result-type WIT node") {
      assertTrue(firstInputWitTypeTag("eitherMethod") == "result-type")
    }
  )
}
