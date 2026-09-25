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
import golem.tool.*

import scala.quoted.*

/**
 * Builds and validates metadata in the compiler, never in the generated guest.
 */
private[macros] final class CompiledWireMetadata[C <: ToolMacroCore](val core: C) {
  import core.q
  import q.reflect.*

  def graph(tpe: TypeRepr): SchemaGraph = {
    val builder                 = new SchemaBuilder
    def id(t: TypeRepr): String = t.dealias.typeSymbol.fullName.replace("$.", ".").stripSuffix("$") +
      (if (t.typeArgs.isEmpty) "" else t.typeArgs.map(id).mkString("<", ",", ">"))

    def body(tpe: TypeRepr): SchemaType = {
      val ty = tpe.dealias
      ty.asType match {
        case '[Unit]                               => t.tuple(Nil)
        case '[Boolean]                            => t.bool
        case '[Byte]                               => t.s8
        case '[Short]                              => t.s16
        case '[Int]                                => t.s32
        case '[Long]                               => t.s64
        case '[Float]                              => t.f32
        case '[Double]                             => t.f64
        case '[Char]                               => t.char
        case '[String] | '[BigInt] | '[BigDecimal] => t.string
        case '[golem.UByte]                        => t.u8
        case '[golem.UShort]                       => t.u16
        case '[golem.UInt]                         => t.u32
        case '[golem.ULong]                        => t.u64
        case '[GolemPath]                          => SchemaType(SchemaTypeBody.PathType(GolemPath.defaultSpec))
        case '[Url]                                => SchemaType(SchemaTypeBody.UrlType(Url.defaultRestrictions))
        case '[java.time.Instant]                  => SchemaType(SchemaTypeBody.DatetimeType)
        case '[java.time.Duration]                 => SchemaType(SchemaTypeBody.DurationType)
        case '[golem.Uuid]                         =>
          builder.register(
            "uuid.Uuid",
            () => t.record(List(t.field("high-bits", t.u64), t.field("low-bits", t.u64))),
            Some("uuid")
          )
        case '[Option[a]]      => t.option(body(TypeRepr.of[a]))
        case '[Either[e, a]]   => t.result(Some(body(TypeRepr.of[a])), Some(body(TypeRepr.of[e])))
        case '[Map[k, v]]      => t.map(body(TypeRepr.of[k]), body(TypeRepr.of[v]))
        case '[List[a]]        => t.list(body(TypeRepr.of[a]))
        case '[Vector[a]]      => t.list(body(TypeRepr.of[a]))
        case '[Seq[a]]         => t.list(body(TypeRepr.of[a]))
        case '[Array[a]]       => t.list(body(TypeRepr.of[a]))
        case '[AgentStream[a]] => SchemaType(SchemaTypeBody.StreamType(Some(body(TypeRepr.of[a]))))
        case _                 =>
          if (ty <:< TypeRepr.of[Tuple]) t.tuple(ty.typeArgs.map(body))
          else {
            val sym = ty.typeSymbol
            builder.register(
              id(ty),
              () => {
                if (sym.flags.is(Flags.Case) || sym.flags.is(Flags.Module))
                  t.record(sym.caseFields.map(f => t.field(f.name, body(ty.memberType(f)))))
                else if (sym.flags.is(Flags.Sealed) || sym.flags.is(Flags.Enum)) {
                  val cases = sym.children.map { c =>
                    val child   = if (c.isTerm) c.termRef else c.typeRef
                    val fields  = c.caseFields
                    val payload =
                      if (c.isTerm || c.flags.is(Flags.Module) || fields.isEmpty) None
                      else if (fields.size == 1 && fields.head.name == "value")
                        Some(body(child.memberType(fields.head)))
                      else Some(body(child))
                    VariantCaseType(c.name.stripSuffix("$"), payload)
                  }
                  if (cases.forall(_.payload.isEmpty)) t.`enum`(cases.map(_.name)) else t.variant(cases)
                } else report.errorAndAbort(s"Cannot derive static wire metadata for ${ty.show}")
              },
              Some(sym.name.stripSuffix("$"))
            )
          }
      }
    }
    builder.buildGraph(body(tpe))
  }

  def tool(traitRepr: TypeRepr): ExtendedToolType = {
    def descriptor(tpe: TypeRepr, active: Set[String]): ToolBuildCtx => Either[ToolBuildError, ExtendedToolType] = {
      val ir = core.parseTool(tpe)
      if (active(ir.identity)) return ToolDescriptorBuilder.cycleStub(ir.identity, ir.version)
      val next = active + ir.identity

      def option(o: core.OptionIR): OptionBuild = {
        val shape = o.shape match {
          case core.ShapeIR.Scalar(tpe, optional)   => OptionShapeBuild.Scalar(graph(tpe), optional)
          case core.ShapeIR.RList(item, repetition) => OptionShapeBuild.RepeatableList(repetition, graph(item))
          case core.ShapeIR.RMap(tpe, repetition)   => OptionShapeBuild.RepeatableMap(repetition, graph(tpe))
        }
        OptionBuild(o.long, o.short, o.aliases, o.doc, o.valueName, shape, o.refinements, o.default, o.required, o.env)
      }
      def tail(v: core.TailIR): TailBuild = TailBuild(
        v.name,
        v.doc,
        v.valueName,
        graph(v.item),
        v.refinements,
        v.min,
        v.max,
        v.separator,
        v.verbatim,
        v.acceptsStdio
      )
      def command(m: core.MethodIR, globals: List[(String, List[String])]): CommandBuild = {
        val c      = core.classifyCommand(ir, m, globals)
        val errors = m.shape.kind match {
          case core.ReturnKind.EitherK(err, _) =>
            core.errorCasesOf(err, m.sym.pos.getOrElse(Position.ofMacroExpansion)).map { e =>
              ErrorCaseBuild(e.name, e.doc, e.kind, e.exitCode, e.payload.map(graph))
            }
          case _ => Nil
        }
        CommandBuild(
          m.commandName,
          m.aliases,
          m.doc,
          GlobalsBuild(c.globalOptions.map(option), c.globalFlags),
          Some(
            BodyBuild(
              fixed = c.fixed.map(p =>
                PositionalBuild(
                  p.name,
                  p.doc,
                  p.valueName,
                  graph(p.tpe),
                  p.refinements,
                  p.default,
                  p.required,
                  p.acceptsStdio
                )
              ),
              tail = c.tail.map(tail),
              options = c.bodyOptions.map(option),
              flags = c.bodyFlags,
              constraints = m.constraints,
              stdin = c.stdin,
              stdout = c.stdout,
              result = core.resultOf(m).map(r => ResultBuild(graph(r.okType), r.formatters, r.defaultFormatter)),
              errors = errors,
              annotations = m.annotations,
              positionalPlan = c.plan.map {
                case core.PlanIR.Plain(name) => PositionalPlanBuild.Plain(name)
                case p: core.PlanIR.Vec      =>
                  PositionalPlanBuild.VecCandidate(
                    p.name,
                    p.explicitTail,
                    p.optionalVec,
                    p.hasMinOrMaxAttr,
                    p.authoredTailSurrogate.map(tail),
                    p.laterOptionNames
                  )
              }
            )
          )
        )
      }
      val root = ir.rootMethod
        .map(command(_, Nil))
        .getOrElse(CommandBuild(ir.toolName, Nil, ir.traitDoc, GlobalsBuild.empty, None))
      val children = ir.childMethods.map { m =>
        m.subtreeTrait match {
          case None    => ChildBuild.Leaf(command(m, core.rootGlobalSurfacesOf(ir)))
          case Some(_) =>
            val c = core.classifySubtree(ir, m)
            ChildBuild.Subtree(
              m.commandName,
              m.nameOverride,
              Option.when(m.doc.summary.nonEmpty || m.doc.description.nonEmpty || m.doc.examples.nonEmpty)(m.doc),
              Option.when(m.aliases.nonEmpty)(m.aliases),
              GlobalsBuild(c.parentOptions.map(option), c.parentFlags),
              descriptor(c.childTrait, next)
            )
        }
      }
      ToolDescriptorBuilder.build(ir.identity, ir.version, root, children)
    }
    descriptor(traitRepr, Set.empty)(new ToolBuildCtx) match {
      case Right(tool) =>
        tool.tryToTool.left.foreach(error => report.errorAndAbort(error.message))
        tool
      case Left(error) => report.errorAndAbort(error.message)
    }
  }

  /**
   * Lift immutable wire carriers as constructor expressions, not serialized
   * runtime data.
   */
  def literal[A: Type](value: A): Expr[A] = {
    def lift(value: Any, expected: TypeRepr): Term = value match {
      case v: String  => Expr(v).asTerm
      case v: Boolean => Expr(v).asTerm
      case v: Byte    => Expr(v).asTerm
      case v: Short   => Expr(v).asTerm
      case v: Int     => Expr(v).asTerm
      case v: Long    => Expr(v).asTerm
      case v: Float   => Expr(v).asTerm
      case v: Double  => Expr(v).asTerm
      case v: Char    => Expr(v).asTerm
      case None       => '{ None }.asTerm
      case Some(v)    =>
        expected.typeArgs.head.asType match {
          case '[a] => '{ Some(${ lift(v, TypeRepr.of[a]).asExprOf[a] }) }.asTerm
        }
      case v: List[?] =>
        expected.typeArgs.head.asType match {
          case '[a] => Expr.ofList(v.map(x => lift(x, TypeRepr.of[a]).asExprOf[a])).asTerm
        }
      case v: Vector[?] =>
        expected.typeArgs.head.asType match {
          case '[a] => '{ ${ Expr.ofList(v.toList.map(x => lift(x, TypeRepr.of[a]).asExprOf[a])) }.toVector }.asTerm
        }
      case v: Product =>
        val className = v.getClass.getName
        val name      = className.stripSuffix("$").replace('$', '.')
        val module    = Symbol.requiredModule(name)
        if (className.endsWith("$")) Ref(module)
        else {
          val apply    = module.methodMember("apply").find(_.paramSymss.flatten.size == v.productArity).get
          val selected = Select(Ref(module), apply)
          val params   = selected.tpe.widen.asInstanceOf[MethodType].paramTypes
          Apply(selected, v.productIterator.zip(params).map { case (field, tpe) => lift(field, tpe) }.toList)
        }
      case other => report.errorAndAbort(s"Cannot emit static wire metadata literal: $other")
    }
    lift(value, TypeRepr.of[A]).asExprOf[A]
  }
}
