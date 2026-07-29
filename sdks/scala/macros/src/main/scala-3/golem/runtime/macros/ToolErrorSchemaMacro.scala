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

import golem.schema.{FromSchema, IntoSchema, SchemaGraph, TypedSchemaValue}
import golem.tool.*

import scala.quoted.*

/**
 * Derives a [[ToolErrorSchema]] for a tool error enum (or sealed trait) whose
 * cases carry `@error(kind, exitCode)` annotations — the Scala port of the Rust
 * SDK's `derive(ToolError)`:
 *
 *   - `errorCases` lists the declared error cases in declaration order, with
 *     the single case field (if any) as the payload schema;
 *   - `toErrorPayloadValue` encodes the payload of the matched case (the unit
 *     tuple for no-payload cases);
 *   - `fromErrorPayloadValue` decodes by payload compatibility: cases are tried
 *     in declaration order and the first whose payload type decodes wins.
 */
object ToolErrorSchemaDerivation {
  inline def derive[E]: ToolErrorSchema[E] = ${ deriveImpl[E] }

  private def deriveImpl[E: Type](using Quotes): Expr[ToolErrorSchema[E]] = {
    val core = new ToolMacroCore
    new ToolErrorSchemaAssembler(core).deriveExpr[E]
  }
}

/**
 * Assembles the derived [[ToolErrorSchema]] expression from the error-case IR
 * parsed by [[ToolMacroCore]].
 */
private[macros] class ToolErrorSchemaAssembler(val core: ToolMacroCore) {
  import core.q
  import q.reflect.*
  import ToolMacroExprs.given

  def deriveExpr[E](using Type[E]): Expr[ToolErrorSchema[E]] = {
    val errType = TypeRepr.of[E]
    val pos     = Position.ofMacroExpansion

    // `Either[Unit, T]` declares no error cases; the unit error is carried as
    // the unit payload.
    if (errType.dealias =:= TypeRepr.of[Unit])
      return '{
        new ToolErrorSchema[E] {
          def errorCases: Either[ToolBuildError, List[ExtendedErrorCase]] = Right(Nil)

          def toErrorPayloadValue(error: E): Either[String, TypedSchemaValue] =
            ToolErrorSupport.encodeUnitPayload

          def fromErrorPayloadValue(value: TypedSchemaValue): Either[String, E] =
            if (ToolErrorSupport.isUnitPayload(value)) Right(().asInstanceOf[E])
            else Left(ToolErrorSupport.unmatchedPayload)
        }
      }

    val cases = core.errorCasesOf(errType, pos)

    val casesExpr   = errorCasesExpr(cases, pos)
    val toPayload   = toPayloadExpr[E](cases, pos)
    val fromPayload = fromPayloadExpr[E](cases, pos)

    '{
      new ToolErrorSchema[E] {
        def errorCases: Either[ToolBuildError, List[ExtendedErrorCase]] = Right($casesExpr)

        def toErrorPayloadValue(error: E): Either[String, TypedSchemaValue] =
          $toPayload(error)

        def fromErrorPayloadValue(value: TypedSchemaValue): Either[String, E] =
          $fromPayload(value)
      }
    }
  }

  def errorCasesExpr(
    cases: List[core.ErrorCaseIR],
    pos: Position
  ): Expr[List[ExtendedErrorCase]] =
    Expr.ofList(cases.map { ec =>
      val payloadExpr: Expr[Option[SchemaGraph]] = ec.payload match {
        case Some(p) => '{ Some(${ payloadGraph(p, pos) }) }
        case None    => '{ None }
      }
      '{
        ExtendedErrorCase(
          ${ Expr(ec.name) },
          ${ Expr(ec.doc) },
          ${ Expr(ec.kind) },
          ${ Expr(ec.exitCode) },
          $payloadExpr
        )
      }
    })

  private def payloadGraph(tpe: TypeRepr, pos: Position): Expr[SchemaGraph] =
    tpe.asType match {
      case '[t] =>
        Expr.summon[IntoSchema[t]] match {
          case Some(into) => '{ $into.graph }
          case None       =>
            report.errorAndAbort(
              s"No implicit IntoSchema available for tool error payload type ${Type.show[t]}",
              pos
            )
        }
    }

  private def caseType(child: Symbol): Type[?] = TypeIdent(child).tpe.asType

  private def caseMatches[E: Type](e: Expr[E], child: Symbol): Expr[Boolean] =
    if (child.isTerm) '{ $e == ${ Ref(child).asExprOf[Any] } }
    else if (child.flags.is(Flags.Module)) '{ $e == ${ Ref(child.companionModule).asExprOf[Any] } }
    else
      caseType(child) match {
        case '[t] => '{ $e.isInstanceOf[t] }
      }

  private def constructCase(child: Symbol, payload: Option[Term]): Term =
    if (child.isTerm) Ref(child)
    else if (child.flags.is(Flags.Module)) Ref(child.companionModule)
    else
      Apply(Select.unique(Ref(child.companionModule), "apply"), payload.toList)

  def toPayloadExpr[E: Type](
    cases: List[core.ErrorCaseIR],
    pos: Position
  ): Expr[E => Either[String, TypedSchemaValue]] = '{ (e: E) =>
    ${
      cases.foldRight[Expr[Either[String, TypedSchemaValue]]](
        '{ Left("tool error value did not match any declared error case") }
      ) { (ec, elseExpr) =>
        val child = ec.caseSym
        ec.payload match {
          case None =>
            '{ if (${ caseMatches[E]('e, child) }) ToolErrorSupport.encodeUnitPayload else $elseExpr }
          case Some(ptype) =>
            ptype.asType match {
              case '[p] =>
                val into = Expr
                  .summon[IntoSchema[p]]
                  .getOrElse(
                    report.errorAndAbort(
                      s"No implicit IntoSchema available for tool error payload type ${Type.show[p]}",
                      pos
                    )
                  )
                val payloadTerm = caseType(child) match {
                  case '[t] =>
                    Select('{ e.asInstanceOf[t] }.asTerm, child.caseFields.head)
                }
                '{
                  if (${ caseMatches[E]('e, child) })
                    ToolErrorSupport.encodePayload[p](${ payloadTerm.asExprOf[p] }, $into)
                  else $elseExpr
                }
            }
        }
      }
    }
  }

  def fromPayloadExpr[E: Type](
    cases: List[core.ErrorCaseIR],
    pos: Position
  ): Expr[TypedSchemaValue => Either[String, E]] = '{ (value: TypedSchemaValue) =>
    ${
      cases.foldRight[Expr[Either[String, E]]](
        '{ Left(ToolErrorSupport.unmatchedPayload) }
      ) { (ec, elseExpr) =>
        val child = ec.caseSym
        ec.payload match {
          case None =>
            '{
              if (ToolErrorSupport.isUnitPayload(value))
                Right(${ constructCase(child, None).asExprOf[E] })
              else $elseExpr
            }
          case Some(ptype) =>
            ptype.asType match {
              case '[p] =>
                val from = Expr
                  .summon[FromSchema[p]]
                  .getOrElse(
                    report.errorAndAbort(
                      s"No implicit FromSchema available for tool error payload type ${Type.show[p]}",
                      pos
                    )
                  )
                '{
                  ToolErrorSupport.decodePayload[p](value, $from) match {
                    case Right(payload) =>
                      Right(${ constructCase(child, Some('{ payload }.asTerm)).asExprOf[E] })
                    case Left(_) => $elseExpr
                  }
                }
            }
        }
      }
    }
  }
}
