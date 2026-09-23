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

package golem.schema.wire

import scala.deriving.Mirror
import scala.quoted.*

private[wire] object ConcreteCodecMacro {
  def derive[A: Type](using Quotes): Expr[ConcreteCodec[A]] = {
    import quotes.reflect.*

    def id(tpe: TypeRepr): String = {
      val t = tpe.dealias
      t.typeSymbol.fullName.replace("$.", ".").stripSuffix("$") +
        (if (t.typeArgs.isEmpty) "" else t.typeArgs.map(id).mkString("<", ",", ">"))
    }

    def build[T: Type](registry: Expr[ConcreteCodecs], active: Set[String]): Expr[ConcreteCodec[T]] = {
      val tpe = TypeRepr.of[T].dealias
      val key = id(tpe)
      Expr.summon[ConcreteCodec[T]] match {
        case Some(codec) => return codec
        case None        => ()
      }
      if (active(key)) return '{ $registry.ref[T](${ Expr(key) }) }
      val next = active + key

      def child(tpe: TypeRepr): Expr[ConcreteCodec[Any]] = tpe.asType match {
        case '[t] => '{ ${ build[t](registry, next) }.asInstanceOf[ConcreteCodec[Any]] }
      }

      val body: Expr[ConcreteCodec[?]] = Type.of[T] match {
        case '[Unit]                   => '{ ConcreteCodec.unit }
        case '[Boolean]                => '{ ConcreteCodec.boolean }
        case '[Byte]                   => '{ ConcreteCodec.byte }
        case '[Short]                  => '{ ConcreteCodec.short }
        case '[Int]                    => '{ ConcreteCodec.int }
        case '[Long]                   => '{ ConcreteCodec.long }
        case '[Float]                  => '{ ConcreteCodec.float }
        case '[Double]                 => '{ ConcreteCodec.double }
        case '[Char]                   => '{ ConcreteCodec.char }
        case '[String]                 => '{ ConcreteCodec.string }
        case '[BigInt]                 => '{ ConcreteCodec.string.xmap[BigInt](BigInt(_), _.toString) }
        case '[BigDecimal]             => '{ ConcreteCodec.string.xmap[BigDecimal](BigDecimal(_), _.toString) }
        case '[golem.schema.GolemPath] => '{ ConcreteCodec.path }
        case '[golem.schema.Url]       => '{ ConcreteCodec.url }
        case '[golem.UByte]            => '{ ConcreteCodec.ubyte }
        case '[golem.UShort]           => '{ ConcreteCodec.ushort }
        case '[golem.UInt]             => '{ ConcreteCodec.uint }
        case '[golem.ULong]            => '{ ConcreteCodec.ulong }
        case '[golem.Uuid]             => '{ ConcreteCodec.uuid }
        case '[java.time.Instant]      => '{ ConcreteCodec.instant }
        case '[java.time.Duration]     => '{ ConcreteCodec.duration }
        case '[Option[t]]              => '{ ConcreteCodec.option(${ build[t](registry, next) }) }
        case '[List[t]]                => '{ ConcreteCodec.list(${ build[t](registry, next) }) }
        case '[Vector[t]]              =>
          '{ ConcreteCodec.list(${ build[t](registry, next) }).xmap[Vector[t]](_.toVector, _.toList) }
        case '[Seq[t]]   => '{ ConcreteCodec.list(${ build[t](registry, next) }).xmap[Seq[t]](identity, _.toList) }
        case '[Array[t]] =>
          val tag =
            Expr.summon[scala.reflect.ClassTag[t]].getOrElse(report.errorAndAbort(s"No ClassTag for ${Type.show[t]}"))
          '{ ConcreteCodec.list(${ build[t](registry, next) }).xmap[Array[t]](_.toArray(using $tag), _.toList) }
        case '[Map[k, v]]                   => '{ ConcreteCodec.map(${ build[k](registry, next) }, ${ build[v](registry, next) }) }
        case '[golem.schema.AgentStream[t]] =>
          '{ golem.schema.AgentStream.concreteCodec(${ build[t](registry, next) }) }
        case '[Either[e, a]] => '{ ConcreteCodec.result(${ build[e](registry, next) }, ${ build[a](registry, next) }) }
        case _               =>
          Expr.summon[Mirror.ProductOf[T]] match {
            case Some(mirror) =>
              val fields = tpe.typeSymbol.caseFields
              val names  = Expr.ofList(fields.map(f => Expr(f.name)))
              val codecs = Expr.ofList(fields.map(f => child(tpe.memberType(f))))
              val tuple  = tpe <:< TypeRepr.of[Tuple]
              '{
                ConcreteCodec.product[T](
                  ${ Expr(key) },
                  ${ Expr(tpe.typeSymbol.name.stripSuffix("$")) },
                  $names.toVector,
                  $codecs.toVector,
                  ${ Expr(tuple) }
                ) { values =>
                  $mirror.fromProduct(Tuple.fromArray(values.toArray))
                }
              }
            case None =>
              val mirror = Expr.summon[Mirror.SumOf[T]].getOrElse {
                report.errorAndAbort(s"No concrete wire codec for ${tpe.show}; define a ConcreteCodec for this type")
              }
              val cases = tpe.typeSymbol.children.map { c =>
                val caseTpe            = if (c.isTerm) c.termRef else c.typeRef
                val fields             = c.caseFields
                val caseName           = c.name.stripSuffix("$")
                def singleton: Expr[T] =
                  (if (c.isTerm) Ref(c) else Ref(c.companionModule)).asExprOf[T]
                if (c.isTerm || c.flags.is(Flags.Module))
                  '{ ConcreteCodec.Case[T](${ Expr(caseName) }, None, _ => (), _ => $singleton) }
                else
                  caseTpe.asType match {
                    case '[s] =>
                      val product = Expr.summon[Mirror.ProductOf[s]].getOrElse {
                        report.errorAndAbort(s"Variant case ${caseTpe.show} is not a concrete product")
                      }
                      if (fields.isEmpty)
                        '{
                          ConcreteCodec.Case[T](
                            ${ Expr(caseName) },
                            None,
                            _ => (),
                            _ => $product.fromProduct(EmptyTuple).asInstanceOf[T]
                          )
                        }
                      else if (fields.size == 1 && fields.head.name == "value") {
                        val codec = child(caseTpe.memberType(fields.head))
                        '{
                          ConcreteCodec.Case[T](
                            ${ Expr(caseName) },
                            Some($codec),
                            value => value.asInstanceOf[Product].productElement(0),
                            value => $product.fromProduct(Tuple1(value)).asInstanceOf[T]
                          )
                        }
                      } else {
                        val codec = child(caseTpe)
                        '{ ConcreteCodec.Case[T](${ Expr(caseName) }, Some($codec), identity, _.asInstanceOf[T]) }
                      }
                  }
              }
              val all = Expr.ofList(cases)
              '{
                ConcreteCodec.variant[T](${ Expr(key) }, ${ Expr(tpe.typeSymbol.name) }, $all.toVector, $mirror.ordinal)
              }
          }
      }
      '{ $registry.define[T](${ Expr(key) })($body.asInstanceOf[ConcreteCodec[T]]) }
    }

    '{
      val registry = new ConcreteCodecs
      ${ build[A]('registry, Set.empty) }
    }
  }
}
