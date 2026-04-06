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

package golem.host

import golem.host.js._
import zio.test._

import scala.scalajs.js

object WitValueTypesRoundtripSpec extends ZIOSpecDefault {
  import WitValueTypes._

  private def roundtripNode(node: WitNode, expectedTag: String): Unit = {
    val jsNode = WitNode.toJs(node)
    Predef.assert(jsNode.asInstanceOf[js.Dynamic].tag.asInstanceOf[String] == expectedTag)
    val parsed = WitNode.fromJs(jsNode)
    (node, parsed) match {
      case (WitNode.Handle(u1, r1), WitNode.Handle(u2, r2)) =>
        Predef.assert(u1 == u2); Predef.assert(r1 == r2)
      case _ => Predef.assert(parsed == node)
    }
  }

  private def roundtripTypeNode(node: WitTypeNode, expectedTag: String): Unit = {
    val jsNode = WitTypeNode.toJs(node)
    Predef.assert(jsNode.asInstanceOf[js.Dynamic].tag.asInstanceOf[String] == expectedTag)
    val parsed = WitTypeNode.fromJs(jsNode)
    (node, parsed) match {
      case (WitTypeNode.HandleType(r1, m1), WitTypeNode.HandleType(r2, m2)) =>
        Predef.assert(r1 == r2); Predef.assert(m1 == m2)
      case _ => Predef.assert(parsed == node)
    }
  }

  def spec = suite("WitValueTypesRoundtripSpec")(
    test("RecordValue round-trip") {
      roundtripNode(WitNode.RecordValue(List(0, 1, 2)), "record-value")
      assertCompletes
    },

    test("VariantValue with Some round-trip") {
      roundtripNode(WitNode.VariantValue(0, Some(1)), "variant-value")
      assertCompletes
    },

    test("VariantValue with None round-trip") {
      roundtripNode(WitNode.VariantValue(1, None), "variant-value")
      assertCompletes
    },

    test("EnumValue round-trip") {
      roundtripNode(WitNode.EnumValue(3), "enum-value")
      assertCompletes
    },

    test("FlagsValue round-trip") {
      roundtripNode(WitNode.FlagsValue(List(true, false, true)), "flags-value")
      assertCompletes
    },

    test("TupleValue round-trip") {
      roundtripNode(WitNode.TupleValue(List(0, 1)), "tuple-value")
      assertCompletes
    },

    test("ListValue round-trip") {
      roundtripNode(WitNode.ListValue(List(0, 1, 2)), "list-value")
      assertCompletes
    },

    test("OptionValue Some round-trip") {
      roundtripNode(WitNode.OptionValue(Some(0)), "option-value")
      assertCompletes
    },

    test("OptionValue None round-trip") {
      roundtripNode(WitNode.OptionValue(None), "option-value")
      assertCompletes
    },

    test("ResultValue ok round-trip") {
      roundtripNode(WitNode.ResultValue(Some(0), None), "result-value")
      assertCompletes
    },

    test("ResultValue err round-trip") {
      roundtripNode(WitNode.ResultValue(None, Some(1)), "result-value")
      assertCompletes
    },

    test("ResultValue both-none round-trip") {
      roundtripNode(WitNode.ResultValue(None, None), "result-value")
      assertCompletes
    },

    test("PrimU8 round-trip") {
      roundtripNode(WitNode.PrimU8(42), "prim-u8")
      assertCompletes
    },

    test("PrimU16 round-trip") {
      roundtripNode(WitNode.PrimU16(1000), "prim-u16")
      assertCompletes
    },

    test("PrimU32 round-trip") {
      val node   = WitNode.PrimU32(100000L)
      val jsNode = WitNode.toJs(node)
      Predef.assert(jsNode.asInstanceOf[js.Dynamic].tag.asInstanceOf[String] == "prim-u32")
      val parsed = WitNode.fromJs(jsNode)
      assertTrue(
        parsed.isInstanceOf[WitNode.PrimU32],
        parsed.asInstanceOf[WitNode.PrimU32].value == 100000L
      )
    },

    test("PrimU64 round-trip") {
      val node   = WitNode.PrimU64(BigInt("18446744073709551615"))
      val jsNode = WitNode.toJs(node)
      Predef.assert(jsNode.asInstanceOf[js.Dynamic].tag.asInstanceOf[String] == "prim-u64")
      val parsed = WitNode.fromJs(jsNode)
      assertTrue(
        parsed.isInstanceOf[WitNode.PrimU64],
        parsed.asInstanceOf[WitNode.PrimU64].value == BigInt("18446744073709551615")
      )
    },

    test("PrimS8 round-trip") {
      roundtripNode(WitNode.PrimS8((-1).toByte), "prim-s8")
      assertCompletes
    },

    test("PrimS16 round-trip") {
      roundtripNode(WitNode.PrimS16((-100).toShort), "prim-s16")
      assertCompletes
    },

    test("PrimS32 round-trip") {
      roundtripNode(WitNode.PrimS32(-1000), "prim-s32")
      assertCompletes
    },

    test("PrimS64 round-trip") {
      val node   = WitNode.PrimS64(-100000L)
      val jsNode = WitNode.toJs(node)
      Predef.assert(jsNode.asInstanceOf[js.Dynamic].tag.asInstanceOf[String] == "prim-s64")
      val parsed = WitNode.fromJs(jsNode)
      assertTrue(
        parsed.isInstanceOf[WitNode.PrimS64],
        parsed.asInstanceOf[WitNode.PrimS64].value == -100000L
      )
    },

    test("PrimFloat32 round-trip") {
      val node   = WitNode.PrimFloat32(3.14f)
      val jsNode = WitNode.toJs(node)
      val parsed = WitNode.fromJs(jsNode)
      assertTrue(
        parsed.isInstanceOf[WitNode.PrimFloat32],
        scala.math.abs(parsed.asInstanceOf[WitNode.PrimFloat32].value - 3.14f) < 0.001f
      )
    },

    test("PrimFloat64 round-trip") {
      roundtripNode(WitNode.PrimFloat64(2.718281828), "prim-float64")
      assertCompletes
    },

    test("PrimChar round-trip") {
      roundtripNode(WitNode.PrimChar('A'), "prim-char")
      assertCompletes
    },

    test("PrimBool round-trip") {
      roundtripNode(WitNode.PrimBool(true), "prim-bool")
      roundtripNode(WitNode.PrimBool(false), "prim-bool")
      assertCompletes
    },

    test("PrimString round-trip") {
      roundtripNode(WitNode.PrimString("hello"), "prim-string")
      assertCompletes
    },

    test("Handle round-trip") {
      val node   = WitNode.Handle("urn:example:resource", BigInt(42))
      val jsNode = WitNode.toJs(node)
      Predef.assert(jsNode.asInstanceOf[js.Dynamic].tag.asInstanceOf[String] == "handle")
      val parsed = WitNode.fromJs(jsNode)
      assertTrue(
        parsed.isInstanceOf[WitNode.Handle],
        parsed.asInstanceOf[WitNode.Handle].uri == "urn:example:resource",
        parsed.asInstanceOf[WitNode.Handle].resourceId == BigInt(42)
      )
    },

    test("unknown WitNode tag throws") {
      val raw = js.Dynamic.literal(tag = "unknown-tag", `val` = 0)
      assertTrue(scala.util.Try(WitNode.fromJs(raw.asInstanceOf[JsWitNode])).isFailure)
    },

    // --- WitTypeNode round-trips ---

    test("RecordType round-trip") {
      roundtripTypeNode(WitTypeNode.RecordType(List(("name", 0), ("age", 1))), "record-type")
      assertCompletes
    },

    test("VariantType round-trip") {
      roundtripTypeNode(WitTypeNode.VariantType(List(("ok", Some(0)), ("none", None))), "variant-type")
      assertCompletes
    },

    test("EnumType round-trip") {
      roundtripTypeNode(WitTypeNode.EnumType(List("red", "green", "blue")), "enum-type")
      assertCompletes
    },

    test("FlagsType round-trip") {
      roundtripTypeNode(WitTypeNode.FlagsType(List("read", "write")), "flags-type")
      assertCompletes
    },

    test("TupleType round-trip") {
      roundtripTypeNode(WitTypeNode.TupleType(List(0, 1)), "tuple-type")
      assertCompletes
    },

    test("ListType round-trip") {
      roundtripTypeNode(WitTypeNode.ListType(0), "list-type")
      assertCompletes
    },

    test("OptionType round-trip") {
      roundtripTypeNode(WitTypeNode.OptionType(0), "option-type")
      assertCompletes
    },

    test("ResultType round-trip") {
      roundtripTypeNode(WitTypeNode.ResultType(Some(0), Some(1)), "result-type")
      roundtripTypeNode(WitTypeNode.ResultType(None, None), "result-type")
      assertCompletes
    },

    test("all primitive type nodes round-trip") {
      val prims = List(
        (WitTypeNode.PrimU8Type, "prim-u8-type"),
        (WitTypeNode.PrimU16Type, "prim-u16-type"),
        (WitTypeNode.PrimU32Type, "prim-u32-type"),
        (WitTypeNode.PrimU64Type, "prim-u64-type"),
        (WitTypeNode.PrimS8Type, "prim-s8-type"),
        (WitTypeNode.PrimS16Type, "prim-s16-type"),
        (WitTypeNode.PrimS32Type, "prim-s32-type"),
        (WitTypeNode.PrimS64Type, "prim-s64-type"),
        (WitTypeNode.PrimF32Type, "prim-f32-type"),
        (WitTypeNode.PrimF64Type, "prim-f64-type"),
        (WitTypeNode.PrimCharType, "prim-char-type"),
        (WitTypeNode.PrimBoolType, "prim-bool-type"),
        (WitTypeNode.PrimStringType, "prim-string-type")
      )
      prims.foreach { case (node, tag) => roundtripTypeNode(node, tag) }
      assertCompletes
    },

    test("HandleType round-trip") {
      roundtripTypeNode(WitTypeNode.HandleType(BigInt(1), ResourceMode.Owned), "handle-type")
      roundtripTypeNode(WitTypeNode.HandleType(BigInt(2), ResourceMode.Borrowed), "handle-type")
      assertCompletes
    },

    test("unknown WitTypeNode tag throws") {
      val raw = js.Dynamic.literal(tag = "unknown-type-tag", `val` = 0)
      assertTrue(scala.util.Try(WitTypeNode.fromJs(raw.asInstanceOf[JsWitTypeNode])).isFailure)
    },

    // --- ResourceMode ---

    test("ResourceMode.fromString") {
      assertTrue(
        ResourceMode.fromString("owned") == ResourceMode.Owned,
        ResourceMode.fromString("borrowed") == ResourceMode.Borrowed,
        ResourceMode.fromString("other") == ResourceMode.Owned
      )
    },

    // --- NamedWitTypeNode round-trip ---

    test("NamedWitTypeNode with name and owner round-trip") {
      val node   = NamedWitTypeNode(Some("field"), Some("owner"), WitTypeNode.PrimStringType)
      val jsNode = NamedWitTypeNode.toJs(node)
      val parsed = NamedWitTypeNode.fromJs(jsNode)
      assertTrue(
        parsed.name == Some("field"),
        parsed.owner == Some("owner"),
        parsed.typeNode == WitTypeNode.PrimStringType
      )
    },

    test("NamedWitTypeNode with None name and owner round-trip") {
      val node   = NamedWitTypeNode(None, None, WitTypeNode.PrimS32Type)
      val jsNode = NamedWitTypeNode.toJs(node)
      val parsed = NamedWitTypeNode.fromJs(jsNode)
      assertTrue(
        parsed.name == None,
        parsed.owner == None,
        parsed.typeNode == WitTypeNode.PrimS32Type
      )
    },

    // --- WitValue round-trip ---

    test("WitValue round-trip with multiple nodes") {
      val value = WitValue(
        List(
          WitNode.PrimString("hello"),
          WitNode.PrimS32(42),
          WitNode.PrimBool(true)
        )
      )
      val jsVal  = WitValue.toJs(value)
      val parsed = WitValue.fromJs(jsVal)
      assertTrue(
        parsed.nodes.size == 3,
        parsed.nodes(0) == WitNode.PrimString("hello"),
        parsed.nodes(1) == WitNode.PrimS32(42),
        parsed.nodes(2) == WitNode.PrimBool(true)
      )
    },

    // --- WitType round-trip ---

    test("WitType round-trip with multiple nodes") {
      val wt = WitType(
        List(
          NamedWitTypeNode(Some("name"), None, WitTypeNode.PrimStringType),
          NamedWitTypeNode(None, None, WitTypeNode.PrimS32Type)
        )
      )
      val jsWt   = WitType.toJs(wt)
      val parsed = WitType.fromJs(jsWt)
      assertTrue(
        parsed.nodes.size == 2,
        parsed.nodes(0).name == Some("name"),
        parsed.nodes(0).typeNode == WitTypeNode.PrimStringType,
        parsed.nodes(1).name == None,
        parsed.nodes(1).typeNode == WitTypeNode.PrimS32Type
      )
    },

    // --- ValueAndType round-trip ---

    test("ValueAndType round-trip") {
      val vat = ValueAndType(
        WitValue(List(WitNode.PrimString("test"), WitNode.PrimS32(99))),
        WitType(
          List(
            NamedWitTypeNode(Some("s"), None, WitTypeNode.PrimStringType),
            NamedWitTypeNode(Some("n"), None, WitTypeNode.PrimS32Type)
          )
        )
      )
      val jsVat  = ValueAndType.toJs(vat)
      val parsed = ValueAndType.fromJs(jsVat)
      assertTrue(
        parsed.value.nodes.size == 2,
        parsed.typ.nodes.size == 2,
        parsed.value.nodes(0) == WitNode.PrimString("test"),
        parsed.typ.nodes(0).name == Some("s")
      )
    }
  )
}
