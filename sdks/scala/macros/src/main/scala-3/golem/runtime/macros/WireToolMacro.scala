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

import golem.schema.*
import golem.schema.wire.*
import golem.tool.*
import golem.tool.wire.*

import scala.concurrent.Future
import scala.quoted.*

object WireToolMacro {
  inline def handle[Trait, Impl <: Trait]: WireToolImplementation = ${ implementation[Trait, Impl] }

  inline def inputGraph[Trait](inline path: List[String]): WitSchemaGraph = ${ inputGraphImpl[Trait]('path) }

  private def inputGraphImpl[Trait: Type](path: Expr[List[String]])(using Quotes): Expr[WitSchemaGraph] = {
    val core = new ToolMacroCore
    import core.q.reflect.*
    val metadata = new CompiledWireMetadata[core.type](core)
    val tool     = metadata.tool(TypeRepr.of[Trait])
    val index    = tool
      .commandIndexByPath(path.valueOrAbort)
      .getOrElse(report.errorAndAbort("Unknown compiled tool command path"))
    val graph = tool
      .canonicalInputRecordSchema(index)
      .fold(error => report.errorAndAbort(error.toString), identity)
    metadata.literal(SchemaWire.schemaGraphToWit(graph))
  }

  private def implementation[Trait: Type, Impl: Type](using Quotes): Expr[WireToolImplementation] = {
    val core = new ToolMacroCore
    new WireToolAssembler(core).implementation[Trait, Impl]
  }
}

private[macros] final class WireToolAssembler(val core: ToolMacroCore) {
  import core.q
  import q.reflect.*

  private val metadata = new CompiledWireMetadata[core.type](core)

  def implementation[Trait: Type, Impl: Type]: Expr[WireToolImplementation] = {
    val impl        = TypeRepr.of[Impl]
    val constructor = impl.typeSymbol.primaryConstructor
    if (
      impl.typeSymbol.flags.is(Flags.Abstract) || impl.typeSymbol.flags.is(Flags.Trait) ||
      impl.typeSymbol.flags.is(Flags.Module) || constructor == Symbol.noSymbol ||
      constructor.paramSymss.flatten.nonEmpty
    )
      report.errorAndAbort("a tool implementation must be a concrete class with an empty primary constructor")

    val tool         = metadata.tool(TypeRepr.of[Trait])
    val wire         = metadata.literal(tool.tryToTool.toOption.get)
    val instanceExpr = Apply(Select(New(TypeTree.of[Impl]), constructor), Nil).asExprOf[Trait]

    def bindings(receiver: Term): Expr[List[WireToolBinding]] = {
      def descend(tpe: TypeRepr, prefix: List[String], parents: List[core.MethodIR]): List[Expr[WireToolBinding]] = {
        val ir = core.parseTool(tpe)
        (ir.rootMethod.toList ++ ir.childMethods).flatMap { m =>
          val path = if (m.isRoot) prefix else prefix :+ m.commandName
          m.subtreeTrait match {
            case Some(child) => descend(child, path, parents :+ m)
            case None        =>
              val index = tool
                .commandIndexByPath(path)
                .getOrElse(report.errorAndAbort(s"missing compiled command ${path.mkString(" ")}"))
              val fields                                                                  = tool.canonicalInputFields(index)
              def paths(node: Int, target: Int, prefix: List[String]): List[List[String]] =
                if (node == target) List(prefix)
                else
                  tool.commands(node).subcommands.flatMap { child =>
                    (tool.commands(child).name :: tool.commands(child).aliases)
                      .flatMap(name => paths(child, target, prefix :+ name))
                  }
              val accepted = metadata.literal(paths(0, index, Nil))
              val methods  = parents :+ m
              val params   = methods.flatMap(_.params)

              def args(
                input: Expr[WireToolInput],
                reader: Expr[WireValuesReader],
                indices: Expr[Vector[Int]]
              ): Expr[Vector[Any]] = {
                val values = params.map { p =>
                  if (core.isPrincipal(p.tpe)) '{ $input.principal }
                  else if (core.isStdin(p.tpe)) '{
                    $input.stdin.getOrElse(throw SchemaDecodeError("missing tool stdin"))
                  }
                  else if (core.isStdout(p.tpe)) '{
                    $input.stdout.getOrElse(throw SchemaDecodeError("missing tool stdout"))
                  }
                  else {
                    val names    = p.kebab :: p.arg.map(_.aliases).getOrElse(Nil)
                    val position = fields.indexWhere(f => names.exists(n => n == f.name || f.aliases.contains(n)))
                    if (position < 0) report.errorAndAbort(s"missing compiled input field ${p.kebab}")
                    val field      = fields(position)
                    val valueIndex = '{ $indices(${ Expr(position) }) }
                    val root       = field.schema.root.body
                    if (core.isInt(p.tpe) && root.isInstanceOf[SchemaTypeBody.U32Type])
                      '{
                        $reader.at($valueIndex) {
                          case WitSchemaValueNode.U32Value(value) if value <= Int.MaxValue && value >= 0 => value.toInt
                        }
                      }
                    else if (p.tpe =:= TypeRepr.of[String] && root.isInstanceOf[SchemaTypeBody.TextType])
                      '{ $reader.at($valueIndex) { case WitSchemaValueNode.TextValue(value) => value.text } }
                    else
                      p.tpe.asType match {
                        case '[t] => '{ ConcreteCodec.derived[t].read($reader, $valueIndex) }
                      }
                  }
                }
                val all = Expr.ofList(values.map(_.asExprOf[Any]))
                '{ $all.toVector }
              }

              def call(arguments: Expr[Vector[Any]]): Term = {
                var current = receiver
                var offset  = 0
                methods.foreach { method =>
                  val supplied = method.params.zipWithIndex.map { case (p, i) =>
                    p.tpe.asType match {
                      case '[t] => '{ $arguments(${ Expr(offset + i) }).asInstanceOf[t] }.asTerm
                    }
                  }
                  val selected = Select(current, method.sym)
                  current = if (method.sym.paramSymss.isEmpty) selected else Apply(selected, supplied)
                  offset += supplied.size
                }
                current
              }

              List('{
                WireToolBinding(
                  $accepted,
                  ${ metadata.literal(tool.commands(index).body.get.stdout) },
                  input => {
                    val arguments =
                      WireToolImplementation.arguments(input, ${ Expr(fields.length) }) { (reader, indices) =>
                        ${ args('input, 'reader, 'indices) }
                      }
                    ${ encodeCall(m, call('arguments)) }
                  }
                )
              })
          }
        }
      }
      Expr.ofList(descend(TypeRepr.of[Trait], Nil, Nil))
    }
    '{
      val instance: Trait = $instanceExpr
      WireToolImplementation($wire, ${ bindings('instance.asTerm) })
    }
  }

  private def encodeCall(
    m: core.MethodIR,
    call: Term
  ): Expr[Future[Either[WitToolError, Option[WitTypedSchemaValue]]]] = {
    def graph(tpe: TypeRepr): Expr[WitSchemaGraph]                                    = metadata.literal(SchemaWire.schemaGraphToWit(metadata.graph(tpe)))
    def outcome(value: Term): Expr[Either[WitToolError, Option[WitTypedSchemaValue]]] = m.shape.kind match {
      case core.ReturnKind.UnitK      => '{ ${ value.asExprOf[Unit] }; Right(None) }
      case core.ReturnKind.Value(tpe) =>
        tpe.asType match {
          case '[t] =>
            '{ WireToolImplementation.success(${ value.asExprOf[t] }, ConcreteCodec.derived[t], ${ graph(tpe) }) }
        }
      case core.ReturnKind.EitherK(err, ok) =>
        err.asType match {
          case '[e] =>
            val cases                                                                          = core.errorCasesOf(err, m.sym.pos.getOrElse(Position.ofMacroExpansion))
            def error(value: Expr[e]): Expr[Either[WitToolError, Option[WitTypedSchemaValue]]] =
              cases.foldRight[Expr[Either[WitToolError, Option[WitTypedSchemaValue]]]](
                '{ Left(WitToolError.InvalidResult("unrecognized tool error case")) }
              ) { (c, rest) =>
                val sym     = c.caseSym
                val matches =
                  if (sym.isTerm) '{ $value == ${ Ref(sym).asExprOf[Any] } }
                  else if (sym.flags.is(Flags.Module)) '{ $value == ${ Ref(sym.companionModule).asExprOf[Any] } }
                  else sym.typeRef.asType match { case '[s] => '{ $value.isInstanceOf[s] } }
                val encoded = c.payload match {
                  case None =>
                    '{
                      WireToolImplementation.custom(
                        ${ Expr(c.name) },
                        (),
                        ConcreteCodec.unit,
                        ${ graph(TypeRepr.of[Unit]) }
                      )
                    }
                  case Some(tpe) =>
                    tpe.asType match {
                      case '[t] =>
                        '{
                          WireToolImplementation.custom(
                            ${ Expr(c.name) },
                            $value.asInstanceOf[Product].productElement(0).asInstanceOf[t],
                            ConcreteCodec.derived[t],
                            ${ graph(tpe) }
                          )
                        }
                    }
                }
                '{ if ($matches) $encoded else $rest }
              }
            ok match {
              case None =>
                '{
                  ${ value.asExprOf[Either[e, Unit]] } match {
                    case Left(e)  => ${ error('e) }
                    case Right(_) => Right(None)
                  }
                }
              case Some(tpe) =>
                tpe.asType match {
                  case '[t] =>
                    '{
                      ${ value.asExprOf[Either[e, t]] } match {
                        case Left(e)      => ${ error('e) }
                        case Right(value) =>
                          WireToolImplementation.success(value, ConcreteCodec.derived[t], ${ graph(tpe) })
                      }
                    }
                }
            }
        }
    }
    if (!m.shape.async) '{ Future.successful(${ outcome(call) }) }
    else
      m.shape.raw.asType match {
        case '[t] =>
          '{
            ${ call.asExprOf[Future[t]] }
              .map(value => ${ outcome('value.asTerm) })(using WireToolImplementation.executionContext)
          }
      }
  }
}
