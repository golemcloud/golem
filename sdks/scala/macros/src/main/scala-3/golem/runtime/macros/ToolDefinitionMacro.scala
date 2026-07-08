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

import golem.schema.{IntoSchema, SchemaGraph}
import golem.tool.*

import scala.quoted.*

/**
 * Generates the tool descriptor for a `@toolDefinition` trait: an
 * [[ExtendedToolType]] builder that resolves the macro-classified command
 * surfaces through [[ToolDescriptorBuilder]]. Subtree methods (methods whose
 * return type names another `@toolDefinition` trait) recursively inline their
 * child descriptors.
 */
object ToolDefinitionMacro {

  /** The tool descriptor entry point (composable, for subtree grafting). */
  inline def descriptor[T]: ToolBuildCtx => Either[ToolBuildError, ExtendedToolType] =
    ${ descriptorImpl[T] }

  /** Builds the tool's descriptor with a fresh context, throwing on failure. */
  inline def metadata[T]: ExtendedToolType =
    ${ metadataImpl[T] }

  /** Builds the tool's descriptor with a fresh context. */
  inline def tryMetadata[T]: Either[ToolBuildError, ExtendedToolType] =
    ${ tryMetadataImpl[T] }

  private def descriptorImpl[T: Type](using
    Quotes
  ): Expr[ToolBuildCtx => Either[ToolBuildError, ExtendedToolType]] = {
    val core = new ToolMacroCore
    new ToolDefinitionAssembler(core).descriptorExprOf[T]
  }

  private def metadataImpl[T: Type](using Quotes): Expr[ExtendedToolType] = {
    val d = descriptorImpl[T]
    '{
      $d(new ToolBuildCtx) match {
        case Right(tool) => tool
        case Left(error) =>
          throw new IllegalArgumentException(s"tool descriptor build failed: ${error.message}")
      }
    }
  }

  private def tryMetadataImpl[T: Type](using
    Quotes
  ): Expr[Either[ToolBuildError, ExtendedToolType]] = {
    val d = descriptorImpl[T]
    '{ $d(new ToolBuildCtx) }
  }
}

/**
 * Assembles descriptor build-spec expressions from the compile-time
 * classification produced by [[ToolMacroCore]].
 */
private[macros] class ToolDefinitionAssembler(val core: ToolMacroCore) {
  import core.q
  import q.reflect.*
  import ToolMacroExprs.given

  private val schemaHint: String =
    "\nHint: IntoSchema is derived from zio.blocks.schema.Schema.\n" +
      "Define or import an implicit Schema[T] for your type.\n" +
      "`final case class T(...) derives zio.blocks.schema.Schema` (or `given Schema[T] = Schema.derived`).\n"

  def graphExpr(tpe: TypeRepr, pos: Position): Expr[SchemaGraph] =
    tpe.asType match {
      case '[t] =>
        Expr.summon[IntoSchema[t]] match {
          case Some(into) => '{ $into.graph }
          case None       =>
            report.errorAndAbort(
              s"No implicit IntoSchema available for type ${Type.show[t]}.$schemaHint",
              pos
            )
        }
    }

  /** Generates the descriptor function for the tool trait `T`. */
  def descriptorExprOf[T: Type]: Expr[ToolBuildCtx => Either[ToolBuildError, ExtendedToolType]] =
    descriptorExpr(TypeRepr.of[T], Set.empty)

  /**
   * Generates the descriptor function for a tool trait. `visiting` carries the
   * trait identities on the current macro-expansion path so a subtree cycle is
   * cut at expansion time (surfacing as the runtime `SubtreeCycle` error).
   */
  def descriptorExpr(
    traitRepr: TypeRepr,
    visiting: Set[String]
  ): Expr[ToolBuildCtx => Either[ToolBuildError, ExtendedToolType]] = {
    val ir = core.parseTool(traitRepr)

    if (visiting.contains(ir.identity))
      return '{ ToolDescriptorBuilder.cycleStub(${ Expr(ir.identity) }, ${ Expr(ir.version) }) }

    val nextVisiting = visiting + ir.identity
    val rootGlobals  = core.rootGlobalSurfacesOf(ir)

    val rootBuild: Expr[CommandBuild] = ir.rootMethod match {
      case Some(rootMethod) =>
        val classified = core.classifyCommand(ir, rootMethod, Nil)
        commandBuildExpr(ir, rootMethod, classified)
      case None =>
        '{
          CommandBuild(
            ${ Expr(ir.toolName) },
            Nil,
            ${ Expr(ir.traitDoc) },
            GlobalsBuild.empty,
            None
          )
        }
    }

    val children: List[Expr[ChildBuild]] = ir.childMethods.map { m =>
      m.subtreeTrait match {
        case Some(_) =>
          val subtree       = core.classifySubtree(ir, m)
          val childDesc     = descriptorExpr(subtree.childTrait, nextVisiting)
          val parentGlobals = '{
            GlobalsBuild(
              ${ Expr.ofList(subtree.parentOptions.map(optionBuildExpr(_, m))) },
              ${ Expr(subtree.parentFlags) }
            )
          }
          val overrideDoc =
            if (m.doc.summary.nonEmpty || m.doc.description.nonEmpty || m.doc.examples.nonEmpty)
              Some(m.doc)
            else None
          val overrideAliases = if (m.aliases.nonEmpty) Some(m.aliases) else None
          '{
            ChildBuild.Subtree(
              ${ Expr(m.commandName) },
              ${ Expr(m.nameOverride) },
              ${ Expr(overrideDoc) },
              ${ Expr(overrideAliases) },
              $parentGlobals,
              $childDesc
            )
          }
        case None =>
          val classified = core.classifyCommand(ir, m, rootGlobals)
          '{ ChildBuild.Leaf(${ commandBuildExpr(ir, m, classified) }) }
      }
    }

    '{
      val root      = $rootBuild
      val childList = ${ Expr.ofList(children) }
      (ctx: ToolBuildCtx) =>
        ToolDescriptorBuilder.build(${ Expr(ir.identity) }, ${ Expr(ir.version) }, root, childList)(
          ctx
        )
    }
  }

  private def commandBuildExpr(
    ir: core.ToolIR,
    m: core.MethodIR,
    c: core.ClassifiedCommand
  ): Expr[CommandBuild] = {
    val pos = m.sym.pos.getOrElse(Position.ofMacroExpansion)

    val resultExpr: Expr[Option[ResultBuild]] = core.resultOf(m) match {
      case None    => '{ None }
      case Some(r) =>
        val graph = graphExpr(r.okType, pos)
        '{
          Some(
            ResultBuild($graph, ${ Expr(r.formatters) }, ${ Expr(r.defaultFormatter) })
          )
        }
    }

    val errorsExpr: Expr[List[ErrorCaseBuild]] = m.shape.kind match {
      case core.ReturnKind.EitherK(err, _) =>
        val cases = core.errorCasesOf(err, pos)
        Expr.ofList(cases.map { ec =>
          val payloadExpr: Expr[Option[SchemaGraph]] = ec.payload match {
            case Some(p) => '{ Some(${ graphExpr(p, pos) }) }
            case None    => '{ None }
          }
          '{
            ErrorCaseBuild(
              ${ Expr(ec.name) },
              ${ Expr(ec.doc) },
              ${ Expr(ec.kind) },
              ${ Expr(ec.exitCode) },
              $payloadExpr
            )
          }
        })
      case _ => '{ Nil }
    }

    val bodyExpr: Expr[Option[BodyBuild]] = '{
      Some(
        BodyBuild(
          fixed = ${ Expr.ofList(c.fixed.map(positionalBuildExpr(_, m))) },
          tail = ${ optionalTailExpr(c.tail, m) },
          options = ${ Expr.ofList(c.bodyOptions.map(optionBuildExpr(_, m))) },
          flags = ${ Expr(c.bodyFlags) },
          constraints = ${ Expr(m.constraints) },
          stdin = ${ Expr(c.stdin) },
          stdout = ${ Expr(c.stdout) },
          result = $resultExpr,
          errors = $errorsExpr,
          annotations = ${ Expr(m.annotations) },
          positionalPlan = ${ Expr.ofList(c.plan.map(planExpr(_, m))) }
        )
      )
    }

    '{
      CommandBuild(
        ${ Expr(m.commandName) },
        ${ Expr(m.aliases) },
        ${ Expr(m.doc) },
        GlobalsBuild(
          ${ Expr.ofList(c.globalOptions.map(optionBuildExpr(_, m))) },
          ${ Expr(c.globalFlags) }
        ),
        $bodyExpr
      )
    }
  }

  private def planExpr(p: core.PlanIR, m: core.MethodIR): Expr[PositionalPlanBuild] =
    p match {
      case core.PlanIR.Plain(name) => '{ PositionalPlanBuild.Plain(${ Expr(name) }) }
      case v: core.PlanIR.Vec      =>
        '{
          PositionalPlanBuild.VecCandidate(
            ${ Expr(v.name) },
            ${ Expr(v.explicitTail) },
            ${ Expr(v.optionalVec) },
            ${ Expr(v.hasMinOrMaxAttr) },
            ${ optionalTailExpr(v.authoredTailSurrogate, m) },
            ${ Expr(v.laterOptionNames) }
          )
        }
    }

  private def optionalTailExpr(
    tail: Option[core.TailIR],
    m: core.MethodIR
  ): Expr[Option[TailBuild]] =
    tail match {
      case None    => '{ None }
      case Some(t) => '{ Some(${ tailBuildExpr(t, m) }) }
    }

  private def tailBuildExpr(t: core.TailIR, m: core.MethodIR): Expr[TailBuild] = {
    val pos = m.sym.pos.getOrElse(Position.ofMacroExpansion)
    '{
      TailBuild(
        name = ${ Expr(t.name) },
        doc = ${ Expr(t.doc) },
        valueName = ${ Expr(t.valueName) },
        item = ${ graphExpr(t.item, pos) },
        refinements = ${ Expr(t.refinements) },
        min = ${ Expr(t.min) },
        max = ${ Expr(t.max) },
        separator = ${ Expr(t.separator) },
        verbatim = ${ Expr(t.verbatim) },
        acceptsStdio = ${ Expr(t.acceptsStdio) }
      )
    }
  }

  private def positionalBuildExpr(p: core.PositionalIR, m: core.MethodIR): Expr[PositionalBuild] = {
    val pos = m.sym.pos.getOrElse(Position.ofMacroExpansion)
    '{
      PositionalBuild(
        name = ${ Expr(p.name) },
        doc = ${ Expr(p.doc) },
        valueName = ${ Expr(p.valueName) },
        base = ${ graphExpr(p.tpe, pos) },
        refinements = ${ Expr(p.refinements) },
        default = ${ Expr(p.default) },
        required = ${ Expr(p.required) },
        acceptsStdio = ${ Expr(p.acceptsStdio) }
      )
    }
  }

  private def optionBuildExpr(o: core.OptionIR, m: core.MethodIR): Expr[OptionBuild] = {
    val pos                               = m.sym.pos.getOrElse(Position.ofMacroExpansion)
    val shapeExpr: Expr[OptionShapeBuild] = o.shape match {
      case core.ShapeIR.Scalar(tpe, optionalScalar) =>
        '{ OptionShapeBuild.Scalar(${ graphExpr(tpe, pos) }, ${ Expr(optionalScalar) }) }
      case core.ShapeIR.RList(item, repetition) =>
        '{ OptionShapeBuild.RepeatableList(${ Expr(repetition) }, ${ graphExpr(item, pos) }) }
      case core.ShapeIR.RMap(mapTpe, repetition) =>
        '{ OptionShapeBuild.RepeatableMap(${ Expr(repetition) }, ${ graphExpr(mapTpe, pos) }) }
    }
    '{
      OptionBuild(
        long = ${ Expr(o.long) },
        short = ${ Expr(o.short) },
        aliases = ${ Expr(o.aliases) },
        doc = ${ Expr(o.doc) },
        valueName = ${ Expr(o.valueName) },
        shape = $shapeExpr,
        refinements = ${ Expr(o.refinements) },
        default = ${ Expr(o.default) },
        required = ${ Expr(o.required) },
        envVar = ${ Expr(o.env) }
      )
    }
  }
}
