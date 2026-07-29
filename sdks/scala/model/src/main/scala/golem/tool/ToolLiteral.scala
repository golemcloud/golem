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

package golem.tool

import golem.schema.validation.RefResolution
import golem.schema.{SchemaGraph, SchemaMapEntry, SchemaType, SchemaTypeBody, SchemaValue}

/**
 * A literal written in a tool authoring attribute (a default or a `value-is`
 * comparand), captured before it is interpreted against the referenced type
 * node. The macro builds one of these from the Scala annotation expression; the
 * target type itself determines how it is interpreted (e.g. a string against an
 * `enum` type selects a case).
 */
sealed trait ToolLiteral extends Product with Serializable

object ToolLiteral {
  final case class BoolLiteral(value: Boolean) extends ToolLiteral

  /**
   * Integer literal, widened to [[BigInt]] so it can carry any signed or
   * unsigned target down to the concrete numeric type (including `u64` max).
   */
  final case class IntLiteral(value: BigInt)                             extends ToolLiteral
  final case class FloatLiteral(value: Double)                           extends ToolLiteral
  final case class CharLiteral(codePoint: Int)                           extends ToolLiteral
  final case class StrLiteral(value: String)                             extends ToolLiteral
  final case class ListLiteral(items: List[ToolLiteral])                 extends ToolLiteral
  final case class MapLiteral(entries: List[(ToolLiteral, ToolLiteral)]) extends ToolLiteral
}

/**
 * Interpretation of metadata-time literals against their target type node,
 * producing the [[SchemaValue]] the tool model stores for option/positional
 * defaults and `value-is` constraint references.
 */
object ToolLiterals {
  import ToolLiteral._

  /**
   * Interprets `lit` against the root type of `graph` (resolving any leading
   * `Ref` indirections), returning the [[SchemaValue]] to store as a default or
   * `value-is` literal.
   */
  def literalToSchemaValue(
    graph: SchemaGraph,
    lit: ToolLiteral
  ): Either[ToolBuildError, SchemaValue] =
    interpret(graph, graph.root, lit)

  /**
   * Interprets `lit` as a `value-is` comparand literal against `graph`,
   * honoring the WIT "any occurrence / element equals this literal" rule: when
   * the literal is not a whole-value match and the comparand resolves (through
   * `Option` wrappers) to a list, the literal is interpreted as a single
   * element value; to a map, as a single map-value.
   *
   * `graph` is the comparand graph the runtime registered for the referenced
   * argument. Because this peels exactly one element/value level from the
   * *whole declared type*, `valueIs("xs", item)` is accepted as an item literal
   * whether `xs` is a `Seq[T]`, a map, or an ancestor-supplied global, and
   * stays consistent with the compatibility check applied next.
   */
  def valueIsLiteralToSchemaValue(
    graph: SchemaGraph,
    lit: ToolLiteral
  ): Either[ToolBuildError, SchemaValue] =
    literalToSchemaValue(graph, lit) match {
      case Right(value) => Right(value)
      case Left(direct) =>
        // The literal is not a whole-value match; for a list-shaped comparand,
        // interpret it as one element.
        resolvePeelingOptions(graph, graph.root) match {
          case None     => Left(direct)
          case Some(ty) =>
            ty.body match {
              case SchemaTypeBody.ListType(element) =>
                interpret(graph, element, lit).left.map(_ => direct)
              case SchemaTypeBody.FixedListType(element, _) =>
                interpret(graph, element, lit).left.map(_ => direct)
              case SchemaTypeBody.MapType(_, value) =>
                interpret(graph, value, lit).left.map(_ => direct)
              case _ => Left(direct)
            }
        }
    }

  /**
   * Resolve refs, then peel any number of `option` wrappers (resolving refs at
   * each step).
   */
  private def resolvePeelingOptions(graph: SchemaGraph, tpe: SchemaType): Option[SchemaType] =
    RefResolution.resolveRef(graph, tpe).toOption.flatMap { resolved =>
      resolved.body match {
        case SchemaTypeBody.OptionType(inner) => resolvePeelingOptions(graph, inner)
        case _                                => Some(resolved)
      }
    }

  private def mismatch(ty: SchemaType, lit: ToolLiteral): ToolBuildError =
    ToolBuildError.DefaultTypeMismatch(s"literal $lit is not valid for type $ty")

  private def interpret(
    graph: SchemaGraph,
    ty: SchemaType,
    lit: ToolLiteral
  ): Either[ToolBuildError, SchemaValue] = {
    // Resolve through any number of `Ref` indirections first.
    val resolved = RefResolution.resolveRef(graph, ty) match {
      case Right(t)  => t
      case Left(err) => return Left(ToolBuildError.DefaultTypeMismatch(err.message))
    }

    def intValue(min: BigInt, max: BigInt)(build: BigInt => SchemaValue): Either[ToolBuildError, SchemaValue] =
      lit match {
        case IntLiteral(i) =>
          if (i < min || i > max)
            Left(
              ToolBuildError.DefaultTypeMismatch(
                s"integer literal $i is out of range for $resolved"
              )
            )
          else Right(build(i))
        case _ => Left(mismatch(resolved, lit))
      }

    resolved.body match {
      case SchemaTypeBody.BoolType =>
        lit match {
          case BoolLiteral(b) => Right(SchemaValue.BoolValue(b))
          case _              => Left(mismatch(resolved, lit))
        }
      case _: SchemaTypeBody.S8Type =>
        intValue(BigInt(Byte.MinValue.toInt), BigInt(Byte.MaxValue.toInt))(v => SchemaValue.S8Value(v.toByte))
      case _: SchemaTypeBody.S16Type =>
        intValue(BigInt(Short.MinValue.toInt), BigInt(Short.MaxValue.toInt))(v => SchemaValue.S16Value(v.toShort))
      case _: SchemaTypeBody.S32Type =>
        intValue(BigInt(Int.MinValue), BigInt(Int.MaxValue))(v => SchemaValue.S32Value(v.toInt))
      case _: SchemaTypeBody.S64Type =>
        intValue(BigInt(Long.MinValue), BigInt(Long.MaxValue))(v => SchemaValue.S64Value(v.toLong))
      case _: SchemaTypeBody.U8Type =>
        intValue(BigInt(0), BigInt(255))(v => SchemaValue.U8Value(v.toInt))
      case _: SchemaTypeBody.U16Type =>
        intValue(BigInt(0), BigInt(65535))(v => SchemaValue.U16Value(v.toInt))
      case _: SchemaTypeBody.U32Type =>
        intValue(BigInt(0), BigInt(4294967295L))(v => SchemaValue.U32Value(v.toLong))
      case _: SchemaTypeBody.U64Type =>
        intValue(BigInt(0), (BigInt(1) << 64) - 1)(v => SchemaValue.U64Value(v.longValue))
      case _: SchemaTypeBody.F32Type =>
        lit match {
          case FloatLiteral(f) => Right(SchemaValue.F32Value(f.toFloat))
          case IntLiteral(i)   => Right(SchemaValue.F32Value(i.toFloat))
          case _               => Left(mismatch(resolved, lit))
        }
      case _: SchemaTypeBody.F64Type =>
        lit match {
          case FloatLiteral(f) => Right(SchemaValue.F64Value(f))
          case IntLiteral(i)   => Right(SchemaValue.F64Value(i.toDouble))
          case _               => Left(mismatch(resolved, lit))
        }
      case SchemaTypeBody.CharType =>
        lit match {
          case CharLiteral(c) => Right(SchemaValue.CharValue(c))
          case _              => Left(mismatch(resolved, lit))
        }
      case SchemaTypeBody.StringType =>
        lit match {
          case StrLiteral(s) => Right(SchemaValue.StringValue(s))
          case _             => Left(mismatch(resolved, lit))
        }
      case _: SchemaTypeBody.TextType =>
        lit match {
          case StrLiteral(s) => Right(SchemaValue.TextValue(s, None))
          case _             => Left(mismatch(resolved, lit))
        }
      case _: SchemaTypeBody.PathType =>
        lit match {
          case StrLiteral(s) => Right(SchemaValue.PathValue(s))
          case _             => Left(mismatch(resolved, lit))
        }
      case _: SchemaTypeBody.UrlType =>
        lit match {
          case StrLiteral(s) => Right(SchemaValue.UrlValue(s))
          case _             => Left(mismatch(resolved, lit))
        }
      case SchemaTypeBody.EnumType(cases) =>
        lit match {
          case StrLiteral(s) =>
            cases.indexOf(s) match {
              case -1 =>
                Left(
                  ToolBuildError.DefaultTypeMismatch(
                    s"enum case ${"\"" + s + "\""} is not one of $cases"
                  )
                )
              case idx => Right(SchemaValue.EnumValue(idx))
            }
          case _ => Left(mismatch(resolved, lit))
        }
      case SchemaTypeBody.OptionType(inner) =>
        interpret(graph, inner, lit).map(v => SchemaValue.OptionValue(Some(v)))
      case SchemaTypeBody.ListType(element) =>
        lit match {
          case ListLiteral(items) =>
            traverse(items)(item => interpret(graph, element, item))
              .map(SchemaValue.ListValue(_))
          case _ => Left(mismatch(resolved, lit))
        }
      case SchemaTypeBody.FixedListType(element, length) =>
        lit match {
          case ListLiteral(items) if items.length == length =>
            traverse(items)(item => interpret(graph, element, item))
              .map(SchemaValue.FixedListValue(_))
          case _ => Left(mismatch(resolved, lit))
        }
      case SchemaTypeBody.MapType(key, value) =>
        lit match {
          case MapLiteral(entries) =>
            traverse(entries) { case (k, v) =>
              for {
                kv <- interpret(graph, key, k)
                vv <- interpret(graph, value, v)
              } yield SchemaMapEntry(kv, vv)
            }.map(SchemaValue.MapValue(_))
          // An empty array literal `[]` is the natural way to author an empty
          // map default; it parses as a (List) literal but carries no entries,
          // so it interprets as an empty map.
          case ListLiteral(Nil) => Right(SchemaValue.MapValue(Nil))
          case _                => Left(mismatch(resolved, lit))
        }
      case _ =>
        Left(
          ToolBuildError.DefaultTypeMismatch(
            s"literals are not supported for type $resolved"
          )
        )
    }
  }

  private def traverse[A, B](items: List[A])(f: A => Either[ToolBuildError, B]): Either[ToolBuildError, List[B]] = {
    val out = List.newBuilder[B]
    val it  = items.iterator
    while (it.hasNext) {
      f(it.next()) match {
        case Right(b)  => out += b
        case Left(err) => return Left(err)
      }
    }
    Right(out.result())
  }
}
