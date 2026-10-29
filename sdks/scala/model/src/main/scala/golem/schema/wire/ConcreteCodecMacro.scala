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

    // Matched by class symbol: quoted type patterns cannot bind a union-typed
    // allow-list argument, and must not match the `Url` / `Inline` subclasses.
    val unstructuredText   = Symbol.requiredClass("golem.schema.UnstructuredText")
    val unstructuredBinary = Symbol.requiredClass("golem.schema.UnstructuredBinary")
    val quantity           = Symbol.requiredClass("golem.schema.Quantity")

    // SDK types whose wire form is a hand-written codec; the static metadata describes the same form.
    val handWrittenTypes                    = Set("golem.host.PermissionsApi.PersistentCard")
    def handWritten(tpe: TypeRepr): Boolean =
      handWrittenTypes(tpe.typeSymbol.fullName.replace("$.", ".")) || handWrittenTypes(tpe.show)

    def build[T: Type](registry: Expr[ConcreteCodecs], active: Set[String]): Expr[ConcreteCodec[T]] = {
      val tpe = TypeRepr.of[T].dealias
      val key = golem.schema.StableTypeId.of(TypeRepr.of[T])
      if (active(key)) return '{ $registry.ref[T](${ Expr(key) }) }
      val next = active + key

      def child(tpe: TypeRepr): Expr[ConcreteCodec[Any]] = tpe.asType match {
        case '[t] => '{ ${ build[t](registry, next) }.asInstanceOf[ConcreteCodec[Any]] }
      }

      // A pattern such as `'[Map[k, v]]` also matches subtypes, whose values the carrier codec cannot build.
      def exact[C](what: String)(using Type[C]): Unit =
        if (tpe.typeSymbol != TypeRepr.of[C].typeSymbol)
          report.errorAndAbort(s"Unsupported wire type ${tpe.show}: use $what instead")

      val body: Expr[ConcreteCodec[?]] = Type.of[T] match {
        case _ if handWritten(tpe) =>
          Expr.summon[ConcreteCodec[T]].getOrElse {
            report.errorAndAbort(s"Missing the concrete wire codec of ${tpe.show}")
          }
        case '[Unit]                                 => '{ ConcreteCodec.unit }
        case '[Boolean]                              => '{ ConcreteCodec.boolean }
        case '[Byte]                                 => '{ ConcreteCodec.byte }
        case '[Short]                                => '{ ConcreteCodec.short }
        case '[Int]                                  => '{ ConcreteCodec.int }
        case '[Long]                                 => '{ ConcreteCodec.long }
        case '[Float]                                => '{ ConcreteCodec.float }
        case '[Double]                               => '{ ConcreteCodec.double }
        case '[Char]                                 => '{ ConcreteCodec.char }
        case '[String]                               => '{ ConcreteCodec.string }
        case '[BigInt]                               => '{ ConcreteCodec.bigInt }
        case '[BigDecimal]                           => '{ ConcreteCodec.bigDecimal }
        case '[golem.schema.GolemPath]               => '{ ConcreteCodec.path }
        case '[java.net.URI]                         => '{ ConcreteCodec.url }
        case '[golem.schema.Text]                    => '{ ConcreteCodec.text }
        case _ if tpe.typeSymbol == unstructuredText =>
          tpe.typeArgs.head.asType match {
            case '[l] =>
              val allowed = Expr.summon[golem.schema.AllowedLanguages[l]].getOrElse {
                report.errorAndAbort(s"Invalid UnstructuredText language type ${Type.show[l]}")
              }
              '{ golem.schema.UnstructuredText.concreteCodec[l](using $allowed) }
          }
        case _ if tpe.typeSymbol == unstructuredBinary =>
          tpe.typeArgs.head.asType match {
            case '[m] =>
              val allowed = Expr.summon[golem.schema.AllowedMimeTypes[m]].getOrElse {
                report.errorAndAbort(s"Invalid UnstructuredBinary MIME type ${Type.show[m]}")
              }
              '{ golem.schema.UnstructuredBinary.concreteCodec[m](using $allowed) }
          }
        case _ if tpe.typeSymbol == quantity =>
          tpe.typeArgs.head.asType match {
            case '[u] =>
              golem.schema.QuantityUnitMacro.spec(TypeRepr.of[u]) match {
                case Left(error) => report.errorAndAbort(error)
                case Right(_)    => '{ golem.schema.Quantity.concreteCodec[u](using golem.schema.QuantityUnit.derived[u]) }
              }
          }
        case '[golem.UByte]        => '{ ConcreteCodec.ubyte }
        case '[golem.UShort]       => '{ ConcreteCodec.ushort }
        case '[golem.UInt]         => '{ ConcreteCodec.uint }
        case '[golem.ULong]        => '{ ConcreteCodec.ulong }
        case '[java.util.UUID]     => '{ ConcreteCodec.uuid }
        case '[java.time.Instant]  => '{ ConcreteCodec.instant }
        case '[java.time.Duration] => '{ ConcreteCodec.duration }
        case '[Option[t]]          =>
          exact[Option[Any]]("Option[A]")
          '{ ConcreteCodec.option(${ build[t](registry, next) }) }
        case '[List[t]] =>
          exact[List[Any]]("List[A]")
          '{ ConcreteCodec.list(${ build[t](registry, next) }) }
        case '[Vector[t]] =>
          '{ ConcreteCodec.list(${ build[t](registry, next) }).xmap[Vector[t]](_.toVector, _.toList) }
        case '[Set[t]] =>
          exact[Set[Any]]("Set[A]")
          '{ ConcreteCodec.list(${ build[t](registry, next) }).xmap[Set[t]](_.toSet, _.toList) }
        case '[Seq[t]] =>
          exact[Seq[Any]]("Seq[A], List[A] or Vector[A]")
          '{ ConcreteCodec.list(${ build[t](registry, next) }).xmap[Seq[t]](identity, _.toList) }
        case '[Array[Byte]] => '{ ConcreteCodec.bytes }
        case '[Array[t]]    =>
          val tag =
            Expr.summon[scala.reflect.ClassTag[t]].getOrElse(report.errorAndAbort(s"No ClassTag for ${Type.show[t]}"))
          '{ ConcreteCodec.list(${ build[t](registry, next) }).xmap[Array[t]](_.toArray(using $tag), _.toList) }
        case '[Map[k, v]] =>
          exact[Map[Any, Any]]("Map[K, V]")
          '{ ConcreteCodec.map(${ build[k](registry, next) }, ${ build[v](registry, next) }) }
        case '[golem.schema.AgentStream[t]] =>
          '{ golem.schema.AgentStream.concreteCodec(${ build[t](registry, next) }) }
        case '[Either[e, a]] =>
          exact[Either[Any, Any]]("Either[E, A]")
          '{ ConcreteCodec.result(${ build[e](registry, next) }, ${ build[a](registry, next) }) }
        case _ =>
          Expr.summon[Mirror.ProductOf[T]] match {
            case Some(mirror) =>
              val tuple  = tpe <:< TypeRepr.of[Tuple]
              val fields = tpe.typeSymbol.caseFields
              val names  = Expr.ofList(fields.map(f => Expr(f.name)))
              val codecs =
                if (tuple) Expr.ofList(golem.schema.TupleElements.of(tpe).map(child))
                else Expr.ofList(fields.map(f => child(tpe.memberType(f))))
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
              val sumMirror = Expr.summon[Mirror.SumOf[T]]
              if (sumMirror.isEmpty)
                return Expr.summon[ConcreteCodec[T]].getOrElse {
                  report.errorAndAbort(s"Cannot structurally derive a concrete wire codec for ${tpe.show}")
                }
              val mirror = sumMirror.get
              val cases  = tpe.typeSymbol.children.map { c =>
                val caseTpe            = if (c.isTerm) c.termRef else c.typeRef
                val fields             = c.caseFields
                val caseName           = c.name.stripSuffix("$")
                def singleton: Expr[T] =
                  (if (c.isTerm) Ref(c) else Ref(c.companionModule)).asExprOf[T]
                if (c.isTerm || c.flags.is(Flags.Module))
                  '{ ConcreteCodec.Case[T](${ Expr(caseName) }, None, _ => (), _ => $singleton) }
                else if (c.flags.is(Flags.Sealed) && !c.flags.is(Flags.Case)) {
                  val codec = child(caseTpe)
                  '{ ConcreteCodec.Case[T](${ Expr(caseName) }, Some($codec), identity, _.asInstanceOf[T]) }
                } else
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
