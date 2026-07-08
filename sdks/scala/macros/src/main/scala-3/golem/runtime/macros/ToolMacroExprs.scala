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

import golem.schema.{PathDirection, PathKind}
import golem.tool.*

import scala.quoted.*

/**
 * `ToExpr` instances for the pure-data parts of the tool model, letting the
 * tool macros lift compile-time classified metadata into the generated
 * descriptor build spec. Graph-carrying builds (options, positionals, tails)
 * are assembled manually by the macros because their schema graphs come from
 * spliced `IntoSchema` summons.
 */
private[macros] object ToolMacroExprs {

  given ToExpr[Example] with {
    def apply(e: Example)(using Quotes): Expr[Example] =
      '{ Example(${ Expr(e.title) }, ${ Expr(e.body) }) }
  }

  given ToExpr[Doc] with {
    def apply(d: Doc)(using Quotes): Expr[Doc] =
      '{ Doc(${ Expr(d.summary) }, ${ Expr(d.description) }, ${ Expr(d.examples) }) }
  }

  given ToExpr[ToolLiteral] with {
    def apply(lit: ToolLiteral)(using Quotes): Expr[ToolLiteral] =
      lit match {
        case ToolLiteral.BoolLiteral(v)     => '{ ToolLiteral.BoolLiteral(${ Expr(v) }) }
        case ToolLiteral.IntLiteral(v)      => '{ ToolLiteral.IntLiteral(BigInt(${ Expr(v.toString) })) }
        case ToolLiteral.FloatLiteral(v)    => '{ ToolLiteral.FloatLiteral(${ Expr(v) }) }
        case ToolLiteral.CharLiteral(v)     => '{ ToolLiteral.CharLiteral(${ Expr(v) }) }
        case ToolLiteral.StrLiteral(v)      => '{ ToolLiteral.StrLiteral(${ Expr(v) }) }
        case ToolLiteral.ListLiteral(items) =>
          '{ ToolLiteral.ListLiteral(${ Expr(items) }) }
        case ToolLiteral.MapLiteral(entries) =>
          '{ ToolLiteral.MapLiteral(${ Expr(entries) }) }
      }
  }

  given ToExpr[Quantifier] with {
    def apply(q: Quantifier)(using Quotes): Expr[Quantifier] =
      q match {
        case Quantifier.All => '{ Quantifier.All }
        case Quantifier.Any => '{ Quantifier.Any }
      }
  }

  given ToExpr[ErrorKind] with {
    def apply(k: ErrorKind)(using Quotes): Expr[ErrorKind] =
      k match {
        case ErrorKind.UsageError   => '{ ErrorKind.UsageError }
        case ErrorKind.RuntimeError => '{ ErrorKind.RuntimeError }
      }
  }

  given ToExpr[Repetition] with {
    def apply(r: Repetition)(using Quotes): Expr[Repetition] =
      r match {
        case Repetition.Repeated     => '{ Repetition.Repeated }
        case Repetition.Delimited(d) => '{ Repetition.Delimited(${ Expr(d) }) }
        case Repetition.Either(d)    => '{ Repetition.Either(${ Expr(d) }) }
      }
  }

  given ToExpr[BoolFlagShape] with {
    def apply(s: BoolFlagShape)(using Quotes): Expr[BoolFlagShape] =
      '{ BoolFlagShape(${ Expr(s.default) }, ${ Expr(s.negatable) }) }
  }

  given ToExpr[FlagShape] with {
    def apply(s: FlagShape)(using Quotes): Expr[FlagShape] =
      s match {
        case FlagShape.BoolFlag(shape) => '{ FlagShape.BoolFlag(${ Expr(shape) }) }
        case FlagShape.CountFlag(max)  => '{ FlagShape.CountFlag(${ Expr(max) }) }
      }
  }

  given ToExpr[FlagSpec] with {
    def apply(f: FlagSpec)(using Quotes): Expr[FlagSpec] =
      '{
        FlagSpec(
          ${ Expr(f.long) },
          ${ Expr(f.short) },
          ${ Expr(f.aliases) },
          ${ Expr(f.doc) },
          ${ Expr(f.shape) },
          ${ Expr(f.envVar) }
        )
      }
  }

  given ToExpr[StreamSpec] with {
    def apply(s: StreamSpec)(using Quotes): Expr[StreamSpec] =
      '{ StreamSpec(${ Expr(s.doc) }, ${ Expr(s.mime) }, ${ Expr(s.required) }) }
  }

  given ToExpr[Formatter] with {
    def apply(f: Formatter)(using Quotes): Expr[Formatter] =
      '{ Formatter(${ Expr(f.name) }, ${ Expr(f.doc) }) }
  }

  given ToExpr[CommandAnnotations] with {
    def apply(a: CommandAnnotations)(using Quotes): Expr[CommandAnnotations] =
      '{
        CommandAnnotations(
          ${ Expr(a.readOnly) },
          ${ Expr(a.destructive) },
          ${ Expr(a.idempotent) },
          ${ Expr(a.openWorld) }
        )
      }
  }

  given ToExpr[PathKind] with {
    def apply(k: PathKind)(using Quotes): Expr[PathKind] =
      k match {
        case PathKind.File      => '{ PathKind.File }
        case PathKind.Directory => '{ PathKind.Directory }
        case PathKind.Any       => '{ PathKind.Any }
      }
  }

  given ToExpr[PathDirection] with {
    def apply(d: PathDirection)(using Quotes): Expr[PathDirection] =
      d match {
        case PathDirection.Input  => '{ PathDirection.Input }
        case PathDirection.Output => '{ PathDirection.Output }
        case PathDirection.InOut  => '{ PathDirection.InOut }
      }
  }

  given ToExpr[ToolArgRefinements] with {
    def apply(r: ToolArgRefinements)(using Quotes): Expr[ToolArgRefinements] =
      '{
        ToolArgRefinements(
          regex = ${ Expr(r.regex) },
          minLength = ${ Expr(r.minLength) },
          maxLength = ${ Expr(r.maxLength) },
          pathKind = ${ Expr(r.pathKind) },
          direction = ${ Expr(r.direction) },
          mime = ${ Expr(r.mime) },
          schemes = ${ Expr(r.schemes) },
          min = ${ Expr(r.min) },
          max = ${ Expr(r.max) },
          unit = ${ Expr(r.unit) }
        )
      }
  }

  given ToExpr[ExtendedValueIsLiteral] with {
    def apply(l: ExtendedValueIsLiteral)(using Quotes): Expr[ExtendedValueIsLiteral] =
      l match {
        case ExtendedValueIsLiteral.Deferred(lit) =>
          '{ ExtendedValueIsLiteral.Deferred(${ Expr(lit) }) }
        case _: ExtendedValueIsLiteral.Resolved =>
          throw new IllegalStateException(
            "the tool macro only emits deferred value-is literals; resolution happens during composition"
          )
      }
  }

  given ToExpr[ExtendedValueIsRef] with {
    def apply(r: ExtendedValueIsRef)(using Quotes): Expr[ExtendedValueIsRef] =
      '{ ExtendedValueIsRef(${ Expr(r.name) }, ${ Expr(r.value) }) }
  }

  given ToExpr[ExtendedRef] with {
    def apply(r: ExtendedRef)(using Quotes): Expr[ExtendedRef] =
      r match {
        case ExtendedRef.Present(name) => '{ ExtendedRef.Present(${ Expr(name) }) }
        case ExtendedRef.ValueIs(v)    => '{ ExtendedRef.ValueIs(${ Expr(v) }) }
      }
  }

  given ToExpr[ExtendedRefGroup] with {
    def apply(g: ExtendedRefGroup)(using Quotes): Expr[ExtendedRefGroup] =
      '{ ExtendedRefGroup(${ Expr(g.refs) }) }
  }

  given ToExpr[ExtendedImpliesC] with {
    def apply(i: ExtendedImpliesC)(using Quotes): Expr[ExtendedImpliesC] =
      '{
        ExtendedImpliesC(
          ${ Expr(i.lhsQuant) },
          ${ Expr(i.lhs) },
          ${ Expr(i.rhsQuant) },
          ${ Expr(i.rhs) }
        )
      }
  }

  given ToExpr[ExtendedForbidsC] with {
    def apply(f: ExtendedForbidsC)(using Quotes): Expr[ExtendedForbidsC] =
      '{ ExtendedForbidsC(${ Expr(f.lhsQuant) }, ${ Expr(f.lhs) }, ${ Expr(f.rhs) }) }
  }

  given ToExpr[ExtendedConstraint] with {
    def apply(c: ExtendedConstraint)(using Quotes): Expr[ExtendedConstraint] =
      c match {
        case ExtendedConstraint.RequiresAll(refs) =>
          '{ ExtendedConstraint.RequiresAll(${ Expr(refs) }) }
        case ExtendedConstraint.AllOrNone(refs) =>
          '{ ExtendedConstraint.AllOrNone(${ Expr(refs) }) }
        case ExtendedConstraint.RequiresAny(refs) =>
          '{ ExtendedConstraint.RequiresAny(${ Expr(refs) }) }
        case ExtendedConstraint.MutexGroups(groups) =>
          '{ ExtendedConstraint.MutexGroups(${ Expr(groups) }) }
        case ExtendedConstraint.Implies(i) =>
          '{ ExtendedConstraint.Implies(${ Expr(i) }) }
        case ExtendedConstraint.Forbids(f) =>
          '{ ExtendedConstraint.Forbids(${ Expr(f) }) }
      }
  }

  given ToExpr[ToolSubtreeForward] with {
    def apply(f: ToolSubtreeForward)(using Quotes): Expr[ToolSubtreeForward] =
      '{ ToolSubtreeForward(${ Expr(f.pathPrefix) }, ${ Expr(f.childToolName) }) }
  }
}
