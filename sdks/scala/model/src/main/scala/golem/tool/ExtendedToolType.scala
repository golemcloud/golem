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

import golem.schema.{SchemaGraph, SchemaValue, TypedSchemaValue}
import golem.tool.wire.WitTool

import scala.collection.immutable.SortedSet

/**
 * The SDK-side extended tool model: structurally the wire `tool` shape, but
 * every type position carries its own self-contained [[SchemaGraph]] instead of
 * an index into a merged pool, and `value-is` literals may still be deferred.
 * [[ExtendedToolType.tryToTool]] validates the model and encodes it into the
 * flat [[WitTool]] wire carrier.
 */
final case class ExtendedToolType(
  version: String,
  commands: Vector[ExtendedCommandNode]
) {

  /** The tool's identity: its root command name. */
  def toolName: String = commands.headOption.map(_.name).getOrElse("")

  /** Like [[tryToTool]] but throws on failure. */
  def toTool: WitTool =
    tryToTool.fold(e => throw ToolBuildException(e), identity)

  /**
   * Validate the model against the producer-side construction invariants and
   * encode it into the wire form (merging the per-argument graphs into the
   * single tool-level schema pool).
   */
  def tryToTool: Either[ToolBuildError, WitTool] =
    ToolEncoding.tryToTool(this)

  /**
   * The globals in scope for the command at `commandIndex`: every ancestor's
   * (and its own) globals in root-to-leaf declaration order.
   */
  def effectiveGlobals(commandIndex: Int): List[EffectiveCommandField] = {
    val path = pathTo(commandIndex).getOrElse(Nil)
    path.flatMap { idx =>
      commands.lift(idx) match {
        case None       => Nil
        case Some(node) =>
          node.globals.options.map(EffectiveCommandField.OptionField(_): EffectiveCommandField) ++
            node.globals.flags.map(EffectiveCommandField.FlagField(_))
      }
    }
  }

  /**
   * Resolve a command path (names or aliases, root excluded) to a command
   * index, returning `None` when the path does not resolve or the resolved
   * command has no body.
   */
  def commandIndexByPath(commandPath: List[String]): Option[Int] = {
    if (commands.isEmpty) return None
    var current = 0
    val it      = commandPath.iterator
    while (it.hasNext) {
      val segment = it.next()
      val next    = commands(current).subcommands.find { idx =>
        commands.lift(idx).exists(node => node.name == segment || node.aliases.contains(segment))
      }
      next match {
        case Some(idx) => current = idx
        case None      => return None
      }
    }
    commands(current).body.map(_ => current)
  }

  /**
   * The canonical input fields of the command at `commandIndex`, in canonical
   * order: effective globals (not shadowed by a body-local surface name), then
   * fixed positionals, the tail, body options, and body flags.
   */
  def canonicalInputFields(commandIndex: Int): List[CanonicalInputField] = {
    // Collect the body field names first so an inherited global can be
    // shadowed by a body-local declaration of the same surface name. A
    // well-formed (normalized) descriptor never has such a collision, but this
    // method may be called on a not-yet-validated or hand-built descriptor,
    // and surfacing the body-local field is the least misleading fallback (it
    // reflects the parameter the author actually wrote).
    val body = commands.lift(commandIndex).flatMap(_.body)

    // Body surface names include option/flag aliases, so an inherited global
    // is shadowed when *any* of its surface names (long or alias) collides
    // with any body-local surface name.
    val bodyNames: SortedSet[String] = body match {
      case None    => SortedSet.empty
      case Some(b) =>
        SortedSet.empty[String] ++
          b.positionals.fixed.map(_.name) ++
          b.positionals.tail.map(_.name) ++
          b.options.flatMap(o => o.long :: o.aliases) ++
          b.flags.flatMap(f => f.long :: f.aliases)
    }

    val globalFields = effectiveGlobals(commandIndex).flatMap {
      case EffectiveCommandField.OptionField(o) =>
        if (bodyNames.contains(o.long) || o.aliases.exists(bodyNames.contains)) None
        else Some(CanonicalInputField(o.long, o.aliases, ToolGraphs.optionCollectedGraph(o.shape)))
      case EffectiveCommandField.FlagField(f) =>
        if (bodyNames.contains(f.long) || f.aliases.exists(bodyNames.contains)) None
        else Some(CanonicalInputField(f.long, f.aliases, ToolGraphs.flagGraph(f)))
    }

    val bodyFields = body match {
      case None    => Nil
      case Some(b) =>
        b.positionals.fixed.map(p => CanonicalInputField(p.name, Nil, p.tpe)) ++
          b.positionals.tail.map(t => CanonicalInputField(t.name, Nil, ToolGraphs.listWrapperGraph(t.itemType))) ++
          b.options.map(o => CanonicalInputField(o.long, o.aliases, ToolGraphs.optionCollectedGraph(o.shape))) ++
          b.flags.map(f => CanonicalInputField(f.long, f.aliases, ToolGraphs.flagGraph(f)))
    }

    globalFields ++ bodyFields
  }

  def canonicalInputModel(commandIndex: Int): Either[ToolBuildError, CanonicalInputModel] =
    checkCanonicalInputCommandIndex(commandIndex).flatMap(_ =>
      CanonicalInputModel.fromFields(canonicalInputFields(commandIndex))
    )

  def canonicalInputRecordSchema(commandIndex: Int): Either[ToolBuildError, SchemaGraph] =
    checkCanonicalInputCommandIndex(commandIndex).flatMap(_ =>
      CanonicalInputModel.recordSchema(canonicalInputFields(commandIndex))
    )

  def decodeCanonicalInputRecord(
    commandIndex: Int,
    value: SchemaValue
  ): Either[CanonicalInputDecodeError, List[CanonicalInputValue]] =
    canonicalInputModel(commandIndex) match {
      case Left(error)  => Left(CanonicalInputDecodeError.Model(error))
      case Right(model) => model.decodeRecord(value)
    }

  private def checkCanonicalInputCommandIndex(commandIndex: Int): Either[ToolBuildError, Unit] =
    ToolValidation.checkCommandTreeStructure(this).flatMap { _ =>
      if (commandIndex < 0 || commandIndex >= commands.length)
        Left(ToolBuildError.CommandIndexOutOfBounds(commandIndex, commands.length))
      else if (pathTo(commandIndex).isEmpty)
        Left(ToolBuildError.UnreachableCommandNode(commandIndex))
      else Right(())
    }

  /**
   * The root-to-target index path for a command, or `None` when the command is
   * not reachable. Guards against malformed (cyclic) command trees so it is
   * safe to call before validation proves the tree acyclic.
   */
  private[tool] def pathTo(commandIndex: Int): Option[List[Int]] = {
    val onStack = scala.collection.mutable.Set.empty[Int]
    val path    = scala.collection.mutable.ListBuffer.empty[Int]

    def visit(cur: Int): Boolean = {
      if (!onStack.add(cur)) return false
      path += cur
      if (cur == commandIndex) return true
      val found = commands(cur).subcommands.exists(child => child >= 0 && child < commands.length && visit(child))
      if (!found) {
        path.remove(path.length - 1)
        onStack.remove(cur)
      }
      found
    }

    if (commands.nonEmpty && visit(0)) Some(path.toList) else None
  }
}

final case class ExtendedCommandNode(
  name: String,
  aliases: List[String],
  doc: Doc,
  globals: ExtendedGlobals,
  subcommands: List[Int],
  body: Option[ExtendedCommandBody]
)

final case class ExtendedGlobals(
  options: List[ExtendedOptionSpec] = Nil,
  flags: List[FlagSpec] = Nil
)

object ExtendedGlobals {
  val empty: ExtendedGlobals = ExtendedGlobals()
}

final case class ExtendedCommandBody(
  positionals: ExtendedPositionals,
  options: List[ExtendedOptionSpec],
  flags: List[FlagSpec],
  constraints: List[ExtendedConstraint],
  stdin: Option[StreamSpec],
  stdout: Option[StreamSpec],
  result: Option[ExtendedResultSpec],
  errors: List[ExtendedErrorCase],
  annotations: Option[CommandAnnotations],
  /**
   * The body's positional-eligible parameters, in declaration order, used to
   * finalize the tail positional after inherited-global de-projection (see
   * [[ToolComposition]]). The final surface of a `Seq[T]` candidate (tail
   * positional vs repeatable-list option) depends on which following
   * re-declarations survive composition, so the macro records the authored
   * facts needed to finalize it; an explicit tail additionally carries its full
   * authored spec so promotion is lossless. Empty for hand-built bodies and
   * ignored by canonical conversion.
   */
  positionalPlan: List[PositionalCandidate] = Nil
)

/**
 * One positional-eligible parameter of a command body, recorded by the macro in
 * declaration order so the runtime can finalize the tail positional after
 * inherited-global de-projection.
 */
sealed trait PositionalCandidate extends Product with Serializable {
  def name: String
}

object PositionalCandidate {

  /**
   * A parameter that can never be the tail (a fixed scalar positional, or an
   * explicitly scoped positional). Recorded only so the declaration order of
   * surviving candidates is known.
   */
  final case class Plain(name: String) extends PositionalCandidate

  /**
   * A `Seq[T]` whose final surface — the tail positional or a repeatable-list
   * option — depends on which following re-declarations survive de-projection
   * (the *last* positional `Seq[T]` is the tail; an earlier one is a
   * repeatable-list option).
   *
   * @param explicitTail
   *   whether the tail was explicitly authored; an explicit tail is never
   *   silently demoted to a repeatable-list option.
   * @param optionalVec
   *   whether the parameter was `Option[Seq[T]]`; it can never become the tail
   *   (a tail is already variadic and has no representable absent state).
   * @param hasMinOrMaxAttr
   *   whether `min`/`max` were authored. They are overloaded — occurrence
   *   bounds on a tail, item numeric bounds on a repeatable-list option — so an
   *   *inferred* candidate carrying them cannot switch surface without changing
   *   their meaning.
   * @param authoredTailSurrogate
   *   for an explicit tail the macro lowered to an inherited-global option
   *   surrogate: the full authored tail spec, installed verbatim when the
   *   surrogate survives as the last positional. `None` for inferred
   *   candidates.
   * @param laterOptionNames
   *   long names of the body options declared after this `Seq[T]`, in
   *   declaration order; a demoted candidate is inserted before the first of
   *   these that survived de-projection.
   */
  final case class VecCandidate(
    name: String,
    explicitTail: Boolean,
    optionalVec: Boolean,
    hasMinOrMaxAttr: Boolean,
    authoredTailSurrogate: Option[ExtendedTailPositional],
    laterOptionNames: List[String]
  ) extends PositionalCandidate
}

final case class ExtendedPositionals(
  fixed: List[ExtendedPositional] = Nil,
  tail: Option[ExtendedTailPositional] = None
)

object ExtendedPositionals {
  val empty: ExtendedPositionals = ExtendedPositionals()
}

final case class ExtendedPositional(
  name: String,
  doc: Doc,
  valueName: Option[String],
  tpe: SchemaGraph,
  default: Option[SchemaValue],
  required: Boolean,
  acceptsStdio: Boolean
)

final case class ExtendedTailPositional(
  name: String,
  doc: Doc,
  valueName: Option[String],
  itemType: SchemaGraph,
  min: Int,
  max: Option[Int],
  separator: Option[String],
  verbatim: Boolean,
  acceptsStdio: Boolean
)

final case class ExtendedOptionSpec(
  long: String,
  short: Option[Char],
  aliases: List[String],
  doc: Doc,
  valueName: Option[String],
  shape: ExtendedOptionShape,
  default: Option[SchemaValue],
  required: Boolean,
  envVar: Option[String]
)

sealed trait ExtendedOptionShape extends Product with Serializable
object ExtendedOptionShape {
  final case class Scalar(tpe: SchemaGraph)                           extends ExtendedOptionShape
  final case class OptionalScalar(tpe: SchemaGraph)                   extends ExtendedOptionShape
  final case class RepeatableList(shape: ExtendedRepeatableListShape) extends ExtendedOptionShape
  final case class RepeatableMap(shape: ExtendedRepeatableMapShape)   extends ExtendedOptionShape
}

final case class ExtendedRepeatableListShape(
  repetition: Repetition,
  itemType: SchemaGraph
)

final case class ExtendedRepeatableMapShape(
  repetition: Repetition,
  mapType: SchemaGraph,
  duplicateKeyPolicy: DuplicateKeyPolicy
)

final case class ExtendedResultSpec(
  tpe: SchemaGraph,
  doc: Doc,
  formatters: List[Formatter],
  defaultFormatter: String
)

final case class ExtendedErrorCase(
  name: String,
  doc: Doc,
  kind: ErrorKind,
  exitCode: Int,
  payload: Option[SchemaGraph]
)

/**
 * Provided by the `ToolError` derivation for error enums. A tool method
 * returning `Either[E, T]` reads its declared error cases from
 * [[ToolErrorSchema.errorCases]].
 */
trait ToolErrorSchema[E] {
  def errorCases: Either[ToolBuildError, List[ExtendedErrorCase]]

  def toErrorPayloadValue(error: E): Either[String, TypedSchemaValue]

  def fromErrorPayloadValue(value: TypedSchemaValue): Either[String, E]
}

object ToolErrorSchema {
  def apply[E](implicit schema: ToolErrorSchema[E]): ToolErrorSchema[E] = schema
}

sealed trait ExtendedRef extends Product with Serializable
object ExtendedRef {
  final case class Present(name: String)                extends ExtendedRef
  final case class ValueIs(valueIs: ExtendedValueIsRef) extends ExtendedRef
}

final case class ExtendedValueIsRef(
  name: String,
  value: ExtendedValueIsLiteral
)

/**
 * The literal a `value-is` constraint compares against.
 *
 * The descriptor macro always emits the raw, un-typed
 * [[ExtendedValueIsLiteral.Deferred]] literal; it never re-derives a comparand
 * graph. Every deferred literal is resolved against the effective constraint
 * scope by [[ToolComposition.normalizeInheritedGlobals]] and is type-checked
 * there, becoming [[ExtendedValueIsLiteral.Resolved]]. A deferred literal that
 * survives composition (the standalone subtree-child case) is reported as an
 * unresolved constraint reference by validation rather than silently accepted.
 */
sealed trait ExtendedValueIsLiteral extends Product with Serializable
object ExtendedValueIsLiteral {
  final case class Resolved(value: SchemaValue)   extends ExtendedValueIsLiteral
  final case class Deferred(literal: ToolLiteral) extends ExtendedValueIsLiteral
}

sealed trait ExtendedConstraint extends Product with Serializable
object ExtendedConstraint {
  final case class RequiresAll(refs: List[ExtendedRef])        extends ExtendedConstraint
  final case class AllOrNone(refs: List[ExtendedRef])          extends ExtendedConstraint
  final case class RequiresAny(refs: List[ExtendedRef])        extends ExtendedConstraint
  final case class MutexGroups(groups: List[ExtendedRefGroup]) extends ExtendedConstraint
  final case class Implies(impliesC: ExtendedImpliesC)         extends ExtendedConstraint
  final case class Forbids(forbidsC: ExtendedForbidsC)         extends ExtendedConstraint
}

final case class ExtendedRefGroup(refs: List[ExtendedRef])

final case class ExtendedImpliesC(
  lhsQuant: Quantifier,
  lhs: List[ExtendedRef],
  rhsQuant: Quantifier,
  rhs: List[ExtendedRef]
)

final case class ExtendedForbidsC(
  lhsQuant: Quantifier,
  lhs: List[ExtendedRef],
  rhs: List[ExtendedRef]
)

/** An effective (inherited or same-node) global as seen by a command body. */
sealed trait EffectiveCommandField extends Product with Serializable
object EffectiveCommandField {
  final case class OptionField(option: ExtendedOptionSpec) extends EffectiveCommandField
  final case class FlagField(flag: FlagSpec)               extends EffectiveCommandField
}

final case class EffectiveCommandBody(
  globals: List[EffectiveCommandField],
  body: Option[ExtendedCommandBody]
)
