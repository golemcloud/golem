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

import golem.schema.{FromSchema, IntoSchema}
import golem.tool.*

import scala.concurrent.Future
import scala.quoted.*

/**
 * Generates the [[ToolImplementationHandle]] for a tool implementation class:
 * the tool descriptor plus the invocation surface (per-method bindings that
 * decode canonical input fields, inject Principal/stdin/stdout, call the
 * implementation, and encode the outcome; and subtree forwarding links).
 */
object ToolImplementationMacro {

  inline def handle[Trait, Impl <: Trait]: ToolImplementationHandle =
    ${ handleImpl[Trait, Impl] }

  private def handleImpl[Trait: Type, Impl: Type](using
    Quotes
  ): Expr[ToolImplementationHandle] = {
    val core = new ToolMacroCore
    new ToolImplementationAssembler(core).handleExpr[Trait, Impl]
  }
}

private[macros] class ToolImplementationAssembler(val core: ToolMacroCore) {
  import core.q
  import q.reflect.*
  import ToolMacroExprs.given

  def handleExpr[Trait: Type, Impl: Type]: Expr[ToolImplementationHandle] = {
    val traitRepr = TypeRepr.of[Trait]
    val implRepr  = TypeRepr.of[Impl]
    val implSym   = implRepr.typeSymbol

    if (implSym.flags.is(Flags.Abstract) || implSym.flags.is(Flags.Trait))
      report.errorAndAbort(s"tool implementation type must be a concrete class, found: ${implSym.fullName}")
    if (implSym.flags.is(Flags.Module))
      report.errorAndAbort(s"tool implementation type must be a concrete class, found object: ${implSym.fullName}")
    if (implSym.typeMembers.exists(_.isTypeParam) || implRepr.typeArgs.nonEmpty)
      report.errorAndAbort(s"tool implementation type must not have type parameters: ${implSym.fullName}")

    val ctor = implSym.primaryConstructor
    if (ctor == Symbol.noSymbol)
      report.errorAndAbort(s"tool implementation type ${implSym.fullName} has no accessible primary constructor")
    val ctorParams = ctor.paramSymss.filter(_.forall(_.isTerm)).flatten
    if (ctorParams.nonEmpty)
      report.errorAndAbort(
        s"a tool implementation class must have an empty primary constructor: tools are stateless " +
          s"and are instantiated by the runtime, found parameters in ${implSym.fullName}"
      )

    val ir          = core.parseTool(traitRepr)
    val rootGlobals = core.rootGlobalSurfacesOf(ir)
    val descriptor  = new ToolDefinitionAssembler(core).descriptorExprOf[Trait]

    val leafMethods =
      (ir.rootMethod.toList ++ ir.childMethods).filter(_.subtreeTrait.isEmpty)

    val subtreeForwards: List[ToolSubtreeForward] =
      ir.childMethods.filter(_.subtreeTrait.isDefined).flatMap { m =>
        val childName = core.parseTool(m.subtreeTrait.get).toolName
        val prefixes  = m.commandName :: m.aliases
        prefixes.map(p => ToolSubtreeForward(List(p), childName))
      }

    val instance: Expr[Trait] =
      Apply(Select(New(TypeTree.of[Impl]), ctor), Nil).asExprOf[Trait]

    def bindingsExpr(implE: Expr[Trait]): Expr[List[ToolMethodBinding]] =
      Expr.ofList(leafMethods.map { m =>
        val classified = core.classifyCommand(ir, m, rootGlobals)
        bindingExpr[Trait](ir, m, classified, implE)
      })

    '{
      val impl: Trait = $instance
      ToolImplementationHandle(
        $descriptor,
        ${ bindingsExpr('impl) },
        ${ Expr(subtreeForwards) }
      )
    }
  }

  private def bindingExpr[Trait: Type](
    ir: core.ToolIR,
    m: core.MethodIR,
    classified: core.ClassifiedCommand,
    implE: Expr[Trait]
  ): Expr[ToolMethodBinding] = {
    val pos  = m.sym.pos.getOrElse(Position.ofMacroExpansion)
    val path = if (m.isRoot) Nil else List(m.commandName)

    val decoders: Expr[List[ToolParamDecoder]] = Expr.ofList(classified.bindings.map {
      case core.ParamBindingIR.Field(name, _, true) =>
        '{ ToolParamDecoder.Field(${ Expr(name) }, ToolInvokerRuntime.countFlagDecoder) }
      case core.ParamBindingIR.Field(name, tpe, _) =>
        tpe.asType match {
          case '[t] =>
            val from = Expr
              .summon[FromSchema[t]]
              .getOrElse(
                report.errorAndAbort(
                  s"No implicit FromSchema available for tool parameter type ${Type.show[t]}",
                  pos
                )
              )
            '{ ToolParamDecoder.Field(${ Expr(name) }, ToolInvokerRuntime.fieldDecoder[t]($from)) }
        }
      case core.ParamBindingIR.PrincipalB => '{ ToolParamDecoder.PrincipalParam }
      case core.ParamBindingIR.StdinB     => '{ ToolParamDecoder.StdinParam }
      case core.ParamBindingIR.StdoutB    => '{ ToolParamDecoder.StdoutParam }
    })

    '{
      ToolMethodBinding(
        ${ Expr(m.methodName) },
        ${ Expr(path) },
        (ctx: ToolInvocationContext) =>
          ToolInvokerRuntime.decodeArgs(ctx, $decoders) match {
            case Left(error)              => Future.successful(Left(error))
            case Right((args, stdoutOpt)) =>
              ${ callAndEncode[Trait](m, implE, 'args, 'stdoutOpt) }
          }
      )
    }
  }

  /** Builds the method call term, casting each decoded argument. */
  private def callTerm[Trait: Type](
    m: core.MethodIR,
    implE: Expr[Trait],
    argsE: Expr[Vector[Any]]
  ): Term = {
    val sel = Select(implE.asTerm, m.sym)
    if (m.sym.paramSymss.isEmpty) sel
    else {
      val argTerms = m.params.zipWithIndex.map { case (p, i) =>
        p.tpe.asType match {
          case '[t] => '{ $argsE(${ Expr(i) }).asInstanceOf[t] }.asTerm
        }
      }
      Apply(sel, argTerms)
    }
  }

  private def callAndEncode[Trait: Type](
    m: core.MethodIR,
    implE: Expr[Trait],
    argsE: Expr[Vector[Any]],
    stdoutE: Expr[Option[ToolOutputStream]]
  ): Expr[Future[Either[ToolInvokeError, ToolInvokeResult]]] = {
    val pos  = m.sym.pos.getOrElse(Position.ofMacroExpansion)
    val call = callTerm[Trait](m, implE, argsE)

    m.shape.kind match {
      case core.ReturnKind.UnitK =>
        if (m.shape.async)
          '{
            ${ call.asExprOf[Future[Unit]] }
              .map(_ => ToolInvokerRuntime.encodeUnit($stdoutE))(ToolInvokerRuntime.executionContext)
          }
        else
          '{
            ${ call.asExprOf[Unit] }
            Future.successful(ToolInvokerRuntime.encodeUnit($stdoutE))
          }

      case core.ReturnKind.Value(tpe) =>
        tpe.asType match {
          case '[t] =>
            val into = Expr
              .summon[IntoSchema[t]]
              .getOrElse(
                report.errorAndAbort(
                  s"No implicit IntoSchema available for tool result type ${Type.show[t]}",
                  pos
                )
              )
            if (m.shape.async)
              '{
                ${ call.asExprOf[Future[t]] }
                  .map(v => ToolInvokerRuntime.encodeSuccess[t](v, $into, $stdoutE))(
                    ToolInvokerRuntime.executionContext
                  )
              }
            else
              '{
                Future.successful(
                  ToolInvokerRuntime.encodeSuccess[t](${ call.asExprOf[t] }, $into, $stdoutE)
                )
              }
        }

      case core.ReturnKind.EitherK(errTpe, okTpe) =>
        errTpe.asType match {
          case '[e] =>
            val tes = new ToolErrorSchemaAssembler(core).deriveExpr[e]
            okTpe match {
              case Some(ok) =>
                ok.asType match {
                  case '[t] =>
                    val into = Expr
                      .summon[IntoSchema[t]]
                      .getOrElse(
                        report.errorAndAbort(
                          s"No implicit IntoSchema available for tool result type ${Type.show[t]}",
                          pos
                        )
                      )
                    if (m.shape.async)
                      '{
                        val schema = $tes
                        ${ call.asExprOf[Future[Either[e, t]]] }.map {
                          case Left(error) =>
                            Left(ToolInvokerRuntime.customError[e](error, schema))
                          case Right(value) =>
                            ToolInvokerRuntime.encodeSuccess[t](value, $into, $stdoutE)
                        }(ToolInvokerRuntime.executionContext)
                      }
                    else
                      '{
                        val schema = $tes
                        ${ call.asExprOf[Either[e, t]] } match {
                          case Left(error) =>
                            Future.successful(Left(ToolInvokerRuntime.customError[e](error, schema)))
                          case Right(value) =>
                            Future.successful(
                              ToolInvokerRuntime.encodeSuccess[t](value, $into, $stdoutE)
                            )
                        }
                      }
                }
              case None =>
                if (m.shape.async)
                  '{
                    val schema = $tes
                    ${ call.asExprOf[Future[Either[e, Unit]]] }.map {
                      case Left(error) => Left(ToolInvokerRuntime.customError[e](error, schema))
                      case Right(_)    => ToolInvokerRuntime.encodeUnit($stdoutE)
                    }(ToolInvokerRuntime.executionContext)
                  }
                else
                  '{
                    val schema = $tes
                    ${ call.asExprOf[Either[e, Unit]] } match {
                      case Left(error) =>
                        Future.successful(Left(ToolInvokerRuntime.customError[e](error, schema)))
                      case Right(_) =>
                        Future.successful(ToolInvokerRuntime.encodeUnit($stdoutE))
                    }
                  }
            }
        }
    }
  }
}
