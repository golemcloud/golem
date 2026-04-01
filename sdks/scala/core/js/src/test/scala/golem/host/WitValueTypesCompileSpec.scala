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

import zio.test._

object WitValueTypesCompileSpec extends ZIOSpecDefault {
  import WitValueTypes._

  private val allWitNodes: List[WitNode] = List(
    WitNode.RecordValue(List(0, 1, 2)),
    WitNode.VariantValue(0, Some(1)),
    WitNode.VariantValue(1, None),
    WitNode.EnumValue(3),
    WitNode.FlagsValue(List(true, false, true)),
    WitNode.TupleValue(List(0, 1)),
    WitNode.ListValue(List(0, 1, 2)),
    WitNode.OptionValue(Some(0)),
    WitNode.OptionValue(None),
    WitNode.ResultValue(Some(0), None),
    WitNode.ResultValue(None, Some(1)),
    WitNode.ResultValue(None, None),
    WitNode.PrimU8(42: Short),
    WitNode.PrimU16(1000),
    WitNode.PrimU32(100000L),
    WitNode.PrimU64(BigInt("18446744073709551615")),
    WitNode.PrimS8((-1).toByte),
    WitNode.PrimS16((-100).toShort),
    WitNode.PrimS32(-1000),
    WitNode.PrimS64(-100000L),
    WitNode.PrimFloat32(3.14f),
    WitNode.PrimFloat64(2.718281828),
    WitNode.PrimChar('A'),
    WitNode.PrimBool(true),
    WitNode.PrimString("hello"),
    WitNode.Handle("urn:example:resource", BigInt(42))
  )

  private def describeWitNode(n: WitNode): String = n match {
    case WitNode.RecordValue(fields)  => s"record(${fields.size})"
    case WitNode.VariantValue(ci, v)  => s"variant($ci,$v)"
    case WitNode.EnumValue(ci)        => s"enum($ci)"
    case WitNode.FlagsValue(flags)    => s"flags(${flags.size})"
    case WitNode.TupleValue(elems)    => s"tuple(${elems.size})"
    case WitNode.ListValue(elems)     => s"list(${elems.size})"
    case WitNode.OptionValue(v)       => s"option($v)"
    case WitNode.ResultValue(ok, err) => s"result($ok,$err)"
    case WitNode.PrimU8(v)            => s"u8($v)"
    case WitNode.PrimU16(v)           => s"u16($v)"
    case WitNode.PrimU32(v)           => s"u32($v)"
    case WitNode.PrimU64(v)           => s"u64($v)"
    case WitNode.PrimS8(v)            => s"s8($v)"
    case WitNode.PrimS16(v)           => s"s16($v)"
    case WitNode.PrimS32(v)           => s"s32($v)"
    case WitNode.PrimS64(v)           => s"s64($v)"
    case WitNode.PrimFloat32(v)       => s"f32($v)"
    case WitNode.PrimFloat64(v)       => s"f64($v)"
    case WitNode.PrimChar(v)          => s"char($v)"
    case WitNode.PrimBool(v)          => s"bool($v)"
    case WitNode.PrimString(v)        => s"string($v)"
    case WitNode.Handle(uri, rid)     => s"handle($uri,$rid)"
  }

  private val allWitTypeNodes: List[WitTypeNode] = List(
    WitTypeNode.RecordType(List(("name", 0), ("age", 1))),
    WitTypeNode.VariantType(List(("ok", Some(0)), ("err", Some(1)), ("none", None))),
    WitTypeNode.EnumType(List("red", "green", "blue")),
    WitTypeNode.FlagsType(List("read", "write", "execute")),
    WitTypeNode.TupleType(List(0, 1)),
    WitTypeNode.ListType(0),
    WitTypeNode.OptionType(0),
    WitTypeNode.ResultType(Some(0), Some(1)),
    WitTypeNode.ResultType(None, None),
    WitTypeNode.PrimU8Type,
    WitTypeNode.PrimU16Type,
    WitTypeNode.PrimU32Type,
    WitTypeNode.PrimU64Type,
    WitTypeNode.PrimS8Type,
    WitTypeNode.PrimS16Type,
    WitTypeNode.PrimS32Type,
    WitTypeNode.PrimS64Type,
    WitTypeNode.PrimF32Type,
    WitTypeNode.PrimF64Type,
    WitTypeNode.PrimCharType,
    WitTypeNode.PrimBoolType,
    WitTypeNode.PrimStringType,
    WitTypeNode.HandleType(BigInt(1), ResourceMode.Owned),
    WitTypeNode.HandleType(BigInt(2), ResourceMode.Borrowed)
  )

  private def describeWitTypeNode(n: WitTypeNode): String = n match {
    case WitTypeNode.RecordType(fields)    => s"record(${fields.size})"
    case WitTypeNode.VariantType(cases)    => s"variant(${cases.size})"
    case WitTypeNode.EnumType(cases)       => s"enum(${cases.size})"
    case WitTypeNode.FlagsType(flags)      => s"flags(${flags.size})"
    case WitTypeNode.TupleType(elems)      => s"tuple(${elems.size})"
    case WitTypeNode.ListType(elem)        => s"list($elem)"
    case WitTypeNode.OptionType(elem)      => s"option($elem)"
    case WitTypeNode.ResultType(ok, err)   => s"result($ok,$err)"
    case WitTypeNode.PrimU8Type            => "u8"
    case WitTypeNode.PrimU16Type           => "u16"
    case WitTypeNode.PrimU32Type           => "u32"
    case WitTypeNode.PrimU64Type           => "u64"
    case WitTypeNode.PrimS8Type            => "s8"
    case WitTypeNode.PrimS16Type           => "s16"
    case WitTypeNode.PrimS32Type           => "s32"
    case WitTypeNode.PrimS64Type           => "s64"
    case WitTypeNode.PrimF32Type           => "f32"
    case WitTypeNode.PrimF64Type           => "f64"
    case WitTypeNode.PrimCharType          => "char"
    case WitTypeNode.PrimBoolType          => "bool"
    case WitTypeNode.PrimStringType        => "string"
    case WitTypeNode.HandleType(rid, mode) => s"handle($rid,$mode)"
  }

  private val resourceModes: List[ResourceMode] = List(ResourceMode.Owned, ResourceMode.Borrowed)

  private val namedNodes: List[NamedWitTypeNode] = List(
    NamedWitTypeNode(Some("field"), Some("owner"), WitTypeNode.PrimStringType),
    NamedWitTypeNode(None, None, WitTypeNode.PrimS32Type),
    NamedWitTypeNode(Some("x"), None, WitTypeNode.ListType(0))
  )

  private val witValue: WitValue = WitValue(allWitNodes)
  private val witType: WitType   = WitType(namedNodes)
  private val vat: ValueAndType  = ValueAndType(witValue, witType)

  private val _nodeIdx: NodeIndex = 0

  def spec = suite("WitValueTypesCompileSpec")(
    test("all 22 WitNode variants constructed") {
      assertTrue(allWitNodes.map(describeWitNode).distinct.size >= 22)
    },

    test("exhaustive WitNode match compiles") {
      allWitNodes.foreach(n => assertTrue(describeWitNode(n).nonEmpty))
      assertCompletes
    },

    test("all 21 WitTypeNode variants constructed") {
      assertTrue(allWitTypeNodes.map(describeWitTypeNode).distinct.size >= 21)
    },

    test("exhaustive WitTypeNode match compiles") {
      allWitTypeNodes.foreach(n => assertTrue(describeWitTypeNode(n).nonEmpty))
      assertCompletes
    },

    test("ResourceMode exhaustive") {
      resourceModes.foreach {
        case ResourceMode.Owned    => ()
        case ResourceMode.Borrowed => ()
      }
      assertCompletes
    },

    test("NamedWitTypeNode construction") {
      assertTrue(
        namedNodes.size == 3,
        namedNodes.head.name.contains("field"),
        namedNodes.head.owner.contains("owner"),
        namedNodes(1).name.isEmpty
      )
    },

    test("WitValue construction") {
      assertTrue(witValue.nodes.nonEmpty)
    },

    test("WitType construction") {
      assertTrue(witType.nodes.nonEmpty)
    },

    test("ValueAndType construction") {
      assertTrue(
        vat.value.nodes.nonEmpty,
        vat.typ.nodes.nonEmpty
      )
    },

    test("NodeIndex type alias") {
      val idx: NodeIndex = 42
      assertTrue(idx == 42)
    }
  )
}
