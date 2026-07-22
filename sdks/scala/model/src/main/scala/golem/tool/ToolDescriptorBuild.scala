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

import golem.schema.{NumericBound, PathDirection, PathKind, SchemaGraph, SchemaType, SchemaTypeBody}
import golem.schema.validation.RefResolution

/**
 * The build-time descriptor specification emitted by the tool-definition macro.
 * The macro performs all *classification* (which parameter becomes an
 * option/flag/positional/tail/global, shapes, docs, plans) at compile time and
 * records the result here; [[ToolDescriptorBuilder]] resolves it at runtime
 * into an [[ExtendedToolType]] by applying `@arg` refinements onto the derived
 * schema graphs, interpreting default literals, and composing subtrees through
 * [[ToolComposition]] — exactly mirroring the code the Rust SDK's
 * `#[tool_definition]` macro generates inline.
 */
final case class ToolArgRefinements(
  regex: Option[String] = None,
  minLength: Option[Int] = None,
  maxLength: Option[Int] = None,
  pathKind: Option[PathKind] = None,
  direction: Option[PathDirection] = None,
  mime: Option[List[String]] = None,
  schemes: Option[List[String]] = None,
  min: Option[ToolLiteral] = None,
  max: Option[ToolLiteral] = None,
  unit: Option[String] = None
) {
  def hasText: Boolean    = regex.isDefined || minLength.isDefined || maxLength.isDefined
  def hasPath: Boolean    = pathKind.isDefined || direction.isDefined || mime.isDefined
  def hasUrl: Boolean     = schemes.isDefined
  def hasNumeric: Boolean = min.isDefined || max.isDefined || unit.isDefined
}

object ToolArgRefinements {
  val empty: ToolArgRefinements = ToolArgRefinements()
}

/** The macro-classified shape of an option's value. */
sealed trait OptionShapeBuild extends Product with Serializable
object OptionShapeBuild {
  final case class Scalar(base: SchemaGraph, optionalScalar: Boolean)          extends OptionShapeBuild
  final case class RepeatableList(repetition: Repetition, item: SchemaGraph)   extends OptionShapeBuild
  final case class RepeatableMap(repetition: Repetition, mapType: SchemaGraph) extends OptionShapeBuild
}

final case class OptionBuild(
  long: String,
  short: Option[Char],
  aliases: List[String],
  doc: Doc,
  valueName: Option[String],
  shape: OptionShapeBuild,
  refinements: ToolArgRefinements,
  default: Option[ToolLiteral],
  required: Boolean,
  envVar: Option[String]
)

final case class PositionalBuild(
  name: String,
  doc: Doc,
  valueName: Option[String],
  base: SchemaGraph,
  refinements: ToolArgRefinements,
  default: Option[ToolLiteral],
  required: Boolean,
  acceptsStdio: Boolean
)

final case class TailBuild(
  name: String,
  doc: Doc,
  valueName: Option[String],
  item: SchemaGraph,
  refinements: ToolArgRefinements,
  min: Int,
  max: Option[Int],
  separator: Option[String],
  verbatim: Boolean,
  acceptsStdio: Boolean
)

final case class GlobalsBuild(
  options: List[OptionBuild] = Nil,
  flags: List[FlagSpec] = Nil
)

object GlobalsBuild {
  val empty: GlobalsBuild = GlobalsBuild()
}

final case class ResultBuild(
  graph: SchemaGraph,
  formatters: List[Formatter],
  defaultFormatter: String
)

final case class ErrorCaseBuild(
  name: String,
  doc: Doc,
  kind: ErrorKind,
  exitCode: Int,
  payload: Option[SchemaGraph]
)

/**
 * The macro-recorded positional plan, mirroring [[PositionalCandidate]] but
 * with the explicit-tail surrogate still in build form (its item graph must be
 * refined before it can become an [[ExtendedTailPositional]]).
 */
sealed trait PositionalPlanBuild extends Product with Serializable
object PositionalPlanBuild {
  final case class Plain(name: String) extends PositionalPlanBuild
  final case class VecCandidate(
    name: String,
    explicitTail: Boolean,
    optionalVec: Boolean,
    hasMinOrMaxAttr: Boolean,
    authoredTailSurrogate: Option[TailBuild],
    laterOptionNames: List[String]
  ) extends PositionalPlanBuild
}

final case class BodyBuild(
  fixed: List[PositionalBuild] = Nil,
  tail: Option[TailBuild] = None,
  options: List[OptionBuild] = Nil,
  flags: List[FlagSpec] = Nil,
  constraints: List[ExtendedConstraint] = Nil,
  stdin: Option[StreamSpec] = None,
  stdout: Option[StreamSpec] = None,
  result: Option[ResultBuild] = None,
  errors: List[ErrorCaseBuild] = Nil,
  annotations: Option[CommandAnnotations] = None,
  positionalPlan: List[PositionalPlanBuild] = Nil
)

final case class CommandBuild(
  name: String,
  aliases: List[String],
  doc: Doc,
  globals: GlobalsBuild,
  body: Option[BodyBuild]
)

/** One child of the root command: a leaf command or a grafted subtree. */
sealed trait ChildBuild extends Product with Serializable
object ChildBuild {
  final case class Leaf(command: CommandBuild) extends ChildBuild
  final case class Subtree(
    expectedName: String,
    overrideName: Option[String],
    overrideDoc: Option[Doc],
    overrideAliases: Option[List[String]],
    parentGlobals: GlobalsBuild,
    descriptor: ToolBuildCtx => Either[ToolBuildError, ExtendedToolType]
  ) extends ChildBuild
}

/**
 * Resolves a macro-emitted descriptor build spec into an [[ExtendedToolType]].
 * This is the single runtime interpreter of the tool-definition macro output;
 * its control flow mirrors the descriptor function the Rust macro generates:
 * root node first (with pending-graft-root application and inherited-global
 * reconciliation), then leaf/subtree children in declaration order, then
 * inherited-global normalization at the outermost descriptor only.
 */
object ToolDescriptorBuilder {

  def build(
    identity: String,
    version: String,
    root: CommandBuild,
    children: List[ChildBuild]
  )(ctx: ToolBuildCtx): Either[ToolBuildError, ExtendedToolType] =
    ctx.withDescriptor(identity) { ctx =>
      try {
        val rootNode = {
          val resolved   = resolveCommand(root)
          val withGraft  = get(ctx.applyPendingGraftRoot(resolved))
          val rootName   = withGraft.name
          val reconciled =
            get(ToolComposition.reconcileCommandInheritedGlobals(withGraft, ctx.inheritedGlobals, rootName))
          reconciled
        }

        var commands = Vector(rootNode)
        var rootSubs = List.empty[Int]

        children.foreach {
          case ChildBuild.Leaf(c) =>
            commands = commands :+ resolveCommand(c)
            rootSubs = rootSubs :+ (commands.length - 1)

          case s: ChildBuild.Subtree =>
            val strictAncestors = ctx.inheritedGlobals ++ effectiveFields(rootNode.globals)
            val parentGlobals   = resolveGlobals(s.parentGlobals)
            val graftedName     = s.overrideName.getOrElse(s.expectedName)

            // If reconciling the subtree method's own params fails, still probe
            // the child descriptor so a root-name mismatch (the more precise
            // authoring error) is preferred over the reconciliation failure.
            val surviving = ToolComposition.reconcileSubtreeParentGlobals(
              parentGlobals,
              strictAncestors,
              graftedName
            ) match {
              case Right(g)  => g
              case Left(err) =>
                val probe = ctx.withGraftRoot(s.expectedName, s.overrideName) { c2 =>
                  c2.withInheritedGlobals(strictAncestors)(s.descriptor)
                }
                probe match {
                  case Left(m: ToolBuildError.SubtreeRootNameMismatch) => fail(m)
                  case _                                               => fail(err)
                }
            }

            val childInherited = strictAncestors ++ effectiveFields(surviving)
            val child          = get(ctx.withGraftRoot(s.expectedName, s.overrideName) { c2 =>
              c2.withInheritedGlobals(childInherited)(s.descriptor)
            })
            val graft = get(
              ToolComposition.graftSubtree(
                child,
                s.expectedName,
                parentGlobals,
                strictAncestors,
                s.overrideName,
                s.overrideDoc,
                s.overrideAliases,
                None
              )
            )
            val (appended, off) = ToolComposition.appendGraftedSubtree(commands, graft)
            commands = appended
            rootSubs = rootSubs :+ off
        }

        commands = commands.updated(0, commands(0).copy(subcommands = rootSubs))

        val tool = ExtendedToolType(version, commands)
        if (ctx.isOutermostDescriptor) ToolComposition.normalizeInheritedGlobals(tool)
        else Right(tool)
      } catch { case ToolBuildException(error) => Left(error) }
    }

  /**
   * Descriptor stub emitted when the macro detects a subtree cycle at expansion
   * time. It is only ever invoked while an ancestor descriptor with the same
   * identity is already on the recursion stack, so
   * [[ToolBuildCtx.withDescriptor]] reports the [[ToolBuildError.SubtreeCycle]]
   * before the (empty) body could run.
   */
  def cycleStub(
    identity: String,
    version: String
  ): ToolBuildCtx => Either[ToolBuildError, ExtendedToolType] =
    ctx => ctx.withDescriptor(identity)(_ => Right(ExtendedToolType(version, Vector.empty)))

  private def get[T](e: Either[ToolBuildError, T]): T =
    e match {
      case Right(v)  => v
      case Left(err) => fail(err)
    }

  private def fail(error: ToolBuildError): Nothing = throw ToolBuildException(error)

  private def effectiveFields(globals: ExtendedGlobals): List[EffectiveCommandField] =
    globals.options.map(o => EffectiveCommandField.OptionField(o): EffectiveCommandField) ++
      globals.flags.map(f => EffectiveCommandField.FlagField(f))

  private def resolveCommand(build: CommandBuild): ExtendedCommandNode =
    ExtendedCommandNode(
      name = build.name,
      aliases = build.aliases,
      doc = build.doc,
      globals = resolveGlobals(build.globals),
      subcommands = Nil,
      body = build.body.map(resolveBody)
    )

  private def resolveGlobals(build: GlobalsBuild): ExtendedGlobals =
    ExtendedGlobals(
      options = build.options.map(resolveOption),
      flags = build.flags
    )

  private def resolveBody(build: BodyBuild): ExtendedCommandBody =
    ExtendedCommandBody(
      positionals = ExtendedPositionals(
        fixed = build.fixed.map(resolvePositional),
        tail = build.tail.map(resolveTail)
      ),
      options = build.options.map(resolveOption),
      flags = build.flags,
      constraints = build.constraints,
      stdin = build.stdin,
      stdout = build.stdout,
      result = build.result.map(r => ExtendedResultSpec(r.graph, Doc.empty, r.formatters, r.defaultFormatter)),
      errors = build.errors.map(e => ExtendedErrorCase(e.name, e.doc, e.kind, e.exitCode, e.payload)),
      annotations = build.annotations,
      positionalPlan = build.positionalPlan.map(resolvePlan)
    )

  private def resolvePlan(build: PositionalPlanBuild): PositionalCandidate =
    build match {
      case PositionalPlanBuild.Plain(name)     => PositionalCandidate.Plain(name)
      case c: PositionalPlanBuild.VecCandidate =>
        PositionalCandidate.VecCandidate(
          name = c.name,
          explicitTail = c.explicitTail,
          optionalVec = c.optionalVec,
          hasMinOrMaxAttr = c.hasMinOrMaxAttr,
          authoredTailSurrogate = c.authoredTailSurrogate.map(resolveTail),
          laterOptionNames = c.laterOptionNames
        )
    }

  private def resolveOption(build: OptionBuild): ExtendedOptionSpec = {
    val shape = build.shape match {
      case OptionShapeBuild.Scalar(base, optionalScalar) =>
        val refined = refineGraph(base, build.refinements)
        if (optionalScalar) ExtendedOptionShape.OptionalScalar(refined)
        else ExtendedOptionShape.Scalar(refined)
      case OptionShapeBuild.RepeatableList(repetition, item) =>
        ExtendedOptionShape.RepeatableList(
          ExtendedRepeatableListShape(repetition, refineGraph(item, build.refinements))
        )
      case OptionShapeBuild.RepeatableMap(repetition, mapType) =>
        ExtendedOptionShape.RepeatableMap(
          ExtendedRepeatableMapShape(
            repetition,
            refineGraph(mapType, build.refinements),
            DuplicateKeyPolicy.Reject
          )
        )
    }
    val default = build.default.map { lit =>
      get(ToolLiterals.literalToSchemaValue(ToolGraphs.optionCollectedGraph(shape), lit))
    }
    ExtendedOptionSpec(
      long = build.long,
      short = build.short,
      aliases = build.aliases,
      doc = build.doc,
      valueName = build.valueName,
      shape = shape,
      default = default,
      required = build.required,
      envVar = build.envVar
    )
  }

  private def resolvePositional(build: PositionalBuild): ExtendedPositional = {
    val graph   = refineGraph(build.base, build.refinements)
    val default = build.default.map(lit => get(ToolLiterals.literalToSchemaValue(graph, lit)))
    ExtendedPositional(
      name = build.name,
      doc = build.doc,
      valueName = build.valueName,
      tpe = graph,
      default = default,
      required = build.required,
      acceptsStdio = build.acceptsStdio
    )
  }

  private def resolveTail(build: TailBuild): ExtendedTailPositional =
    ExtendedTailPositional(
      name = build.name,
      doc = build.doc,
      valueName = build.valueName,
      itemType = refineGraph(build.item, build.refinements),
      min = build.min,
      max = build.max,
      separator = build.separator,
      verbatim = build.verbatim,
      acceptsStdio = build.acceptsStdio
    )

  /**
   * Applies the authored `@arg` refinements onto the root of the derived value
   * graph, in the same family order as the Rust descriptor: text, path, url,
   * numeric. A refinement family authored against a schema kind that cannot
   * carry it is a [[ToolBuildError.RefinementTypeMismatch]].
   */
  private def refineGraph(graph: SchemaGraph, r: ToolArgRefinements): SchemaGraph = {
    var root = graph.root
    if (r.hasText)
      root = get(ToolRefinement.refineText(root, r.regex, r.minLength, r.maxLength))
    if (r.hasPath)
      root = get(ToolRefinement.refinePath(root, r.direction, r.pathKind, r.mime))
    if (r.hasUrl)
      root = get(ToolRefinement.refineUrl(root, r.schemes))
    if (r.hasNumeric) {
      val minBound = r.min.map(lit => numericBound(graph, root, lit))
      val maxBound = r.max.map(lit => numericBound(graph, root, lit))
      root = get(ToolRefinement.refineNumeric(root, minBound, maxBound, r.unit))
    }
    graph.copy(root = root)
  }

  /**
   * Converts a `min`/`max`/`bounds` literal into the canonical [[NumericBound]]
   * representation for the numeric kind the (possibly `Ref`-indirected) target
   * type resolves to.
   */
  private def numericBound(graph: SchemaGraph, root: SchemaType, lit: ToolLiteral): NumericBound = {
    val resolved = RefResolution.resolveRef(graph, root) match {
      case Right(t) => t
      case Left(e)  => fail(ToolBuildError.InvalidNumericBound(e.message))
    }
    import SchemaTypeBody._
    def signed(i: BigInt): NumericBound =
      if (i < BigInt(Long.MinValue) || i > BigInt(Long.MaxValue))
        fail(ToolBuildError.InvalidNumericBound(s"signed bound $i does not fit in 64 bits"))
      else NumericBound.Signed(i.longValue)
    def unsigned(i: BigInt): NumericBound =
      if (i < 0 || i > (BigInt(1) << 64) - 1)
        fail(ToolBuildError.InvalidNumericBound(s"unsigned bound $i is out of the u64 range"))
      else NumericBound.Unsigned(i.longValue)
    def float(d: Double): NumericBound = get(ToolRefinement.floatBound(d))

    (resolved.body, lit) match {
      case (_: S8Type | _: S16Type | _: S32Type | _: S64Type, ToolLiteral.IntLiteral(i)) => signed(i)
      case (_: U8Type | _: U16Type | _: U32Type | _: U64Type, ToolLiteral.IntLiteral(i)) => unsigned(i)
      case (_: F32Type | _: F64Type, ToolLiteral.IntLiteral(i))                          => float(i.toDouble)
      case (_: F32Type | _: F64Type, ToolLiteral.FloatLiteral(d))                        => float(d)
      case (
            _: S8Type | _: S16Type | _: S32Type | _: S64Type | _: U8Type | _: U16Type | _: U32Type | _: U64Type,
            other
          ) =>
        fail(ToolBuildError.InvalidNumericBound(s"integer bound expected, found $other"))
      case (otherType, _) =>
        fail(
          ToolBuildError.RefinementTypeMismatch("numeric", ToolRefinement.schemaKindName(otherType))
        )
    }
  }
}
