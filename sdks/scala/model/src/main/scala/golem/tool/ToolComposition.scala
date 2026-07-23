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

import golem.schema.SchemaGraph
import golem.tool.ToolGraphs._
import golem.tool.ToolValidation.NameScope

/**
 * Composition of tool descriptors: grafting subtree children under a parent
 * command, de-projecting re-declared parameters onto inherited globals,
 * resolving deferred `value-is` literals once their comparand scope is known,
 * and re-inferring the tail positional against the parameters that actually
 * survived de-projection.
 */
object ToolComposition {

  def reconcileSubtreeParentGlobals(
    parentGlobals: ExtendedGlobals,
    strictAncestorGlobals: List[EffectiveCommandField],
    commandName: String
  ): Either[ToolBuildError, ExtendedGlobals] =
    try Right(reconcileGlobals(parentGlobals, strictAncestorGlobals, commandName))
    catch { case ToolBuildException(error) => Left(error) }

  def reconcileCommandInheritedGlobals(
    node: ExtendedCommandNode,
    strictAncestorGlobals: List[EffectiveCommandField],
    commandName: String
  ): Either[ToolBuildError, ExtendedCommandNode] =
    try {
      val globals = reconcileGlobals(node.globals, strictAncestorGlobals, commandName)
      val body    = node.body.map(reconcileBody(_, strictAncestorGlobals, commandName))
      Right(node.copy(globals = globals, body = body))
    } catch { case ToolBuildException(error) => Left(error) }

  /**
   * Turn a child subtree's command list into the graft-local nodes to splice
   * beneath a parent. The child root (index 0) becomes the parent's subtree
   * command: its globals and subcommands are preserved (so recursive globals
   * still apply and the graft-local child indices stay valid), and its
   * `name`/`doc`/`aliases` may be overridden by the parent's command
   * annotation.
   *
   * The child root may carry a body — the child trait's implicit-body method
   * (e.g. `git stash` runs the `stash` body while `git stash pop` walks to the
   * `pop` child). The subtree method's propagating params (`parentGlobals`) are
   * first reconciled against `strictAncestorGlobals`, then the child root's own
   * globals and body are reconciled against the full inherited set. A
   * compatible same-name re-declaration is de-projected onto the inherited
   * global; an incompatible one is rejected as
   * [[ToolBuildError.InheritedGlobalConflict]]. Surviving `parentGlobals` are
   * then prepended onto the grafted root's globals so they propagate to every
   * descendant subcommand.
   *
   * `expectedName` is the parent subtree method's command name; unless an
   * explicit `overrideName` is given, the child root name must equal it
   * ([[ToolBuildError.SubtreeRootNameMismatch]]). Command-annotations are not
   * supported on a subtree command (the model places them on a command body),
   * so a non-`None` `overrideAnnotations` is rejected rather than silently
   * dropped.
   *
   * Command indices are returned unchanged (graft-local); the final offset into
   * the parent's command tree is applied by [[appendGraftedSubtree]].
   */
  def graftSubtree(
    child: ExtendedToolType,
    expectedName: String,
    parentGlobals: ExtendedGlobals,
    strictAncestorGlobals: List[EffectiveCommandField],
    overrideName: Option[String],
    overrideDoc: Option[Doc],
    overrideAliases: Option[List[String]],
    overrideAnnotations: Option[CommandAnnotations]
  ): Either[ToolBuildError, Vector[ExtendedCommandNode]] =
    try {
      val nodes = child.commands
      val root0 = nodes.headOption.getOrElse(fail(ToolBuildError.EmptyCommandTree))
      if (overrideAnnotations.isDefined)
        fail(ToolBuildError.SubtreeAnnotationsUnsupported(overrideName.getOrElse(root0.name)))
      if (overrideName.isEmpty && root0.name != expectedName)
        fail(ToolBuildError.SubtreeRootNameMismatch(expectedName, root0.name))

      // Apply overrides first so any reconciliation error reports the final
      // grafted command name rather than the standalone child root name.
      var root = root0
      overrideName.foreach(name => root = root.copy(name = name))
      overrideDoc.foreach(doc => root = root.copy(doc = doc))
      overrideAliases.foreach(aliases => root = root.copy(aliases = aliases))

      val commandName = root.name

      // The subtree method's params become propagating globals on the grafted
      // root. They are inherited from the parent command, so first reconcile
      // them against strict ancestors above the graft point, then reconcile
      // the grafted root's own globals/body against the full inherited set.
      // Doing both here preserves the normal inherited-global contract even
      // though the parent globals will be stored as same-node globals on the
      // grafted root.
      val reconciledParentGlobals =
        reconcileGlobals(parentGlobals, strictAncestorGlobals, commandName)

      val inherited = strictAncestorGlobals ++
        reconciledParentGlobals.options.map(EffectiveCommandField.OptionField(_): EffectiveCommandField) ++
        reconciledParentGlobals.flags.map(EffectiveCommandField.FlagField(_))

      val rootGlobals = reconcileGlobals(root.globals, inherited, commandName)
      val rootBody    = root.body.map(reconcileBody(_, inherited, commandName))

      // Prepend the parent globals so they propagate to every descendant
      // subcommand of the grafted root. Globals and subcommands keep their
      // graft-local indices; appendGraftedSubtree shifts them on append.
      root = root.copy(
        globals = ExtendedGlobals(
          options = reconciledParentGlobals.options ++ rootGlobals.options,
          flags = reconciledParentGlobals.flags ++ rootGlobals.flags
        ),
        body = rootBody
      )

      Right(root +: nodes.tail)
    } catch { case ToolBuildException(error) => Left(error) }

  /**
   * Normalize a whole tool's command tree against inherited globals.
   *
   * A subtree child trait is synthesized independently, so a child command
   * whose signature repeats a parameter an ancestor declares as a global
   * projects it as a body option/flag/positional (or as its own global) in the
   * standalone descriptor. Likewise a leaf command in the same trait as the
   * root may repeat a root global. Once composed under the ancestor that
   * supplies that name as a global, the local re-declaration must be
   * reconciled, otherwise the canonical shape carries a body-local (or
   * nested-global) name colliding with an effective inherited global.
   *
   * This is the single source of truth for that reconciliation. It traverses
   * the tree root→leaf, carrying the *strict ancestor* globals in scope. For
   * every node it reconciles the node's own globals and its body arguments
   * against the strict-ancestor globals:
   *
   *   - a same-name re-declaration whose canonical input shape is *compatible*
   *     with the inherited global is removed — the ancestor global is the
   *     single source of truth for docs, defaults, requiredness, aliases, and
   *     parse behavior;
   *   - a same-name re-declaration whose shape is *incompatible* is an
   *     [[ToolBuildError.InheritedGlobalConflict]]: the composition is invalid
   *     and is rejected rather than silently dropping or replacing the local
   *     parameter.
   *
   * Body arguments are reconciled only against *strict ancestors*, never the
   * node's own globals; a body argument colliding with a global declared on the
   * same command is an ordinary authoring error left for
   * [[ToolValidation.validateTool]].
   *
   * The traversal guards against malformed (cyclic / out-of-bounds) trees so it
   * is safe to run before validation proves the tree well-formed.
   */
  def normalizeInheritedGlobals(
    tool: ExtendedToolType
  ): Either[ToolBuildError, ExtendedToolType] =
    if (tool.commands.isEmpty) Right(tool)
    else
      try {
        val commands = tool.commands.toArray
        val visited  = Array.fill(commands.length)(false)
        normalizeCommand(commands, 0, Nil, visited)
        Right(tool.copy(commands = commands.toVector))
      } catch { case ToolBuildException(error) => Left(error) }

  private def normalizeCommand(
    commands: Array[ExtendedCommandNode],
    index: Int,
    ancestorGlobals: List[EffectiveCommandField],
    visited: Array[Boolean]
  ): Unit = {
    if (index >= commands.length || visited(index)) return
    visited(index) = true

    val commandName = commands(index).name

    // Reconcile this node's own globals and body args against strict
    // ancestors.
    locally {
      val node    = commands(index)
      val globals = reconcileGlobals(node.globals, ancestorGlobals, commandName)
      val body    = node.body.map(reconcileBody(_, ancestorGlobals, commandName))
      commands(index) = node.copy(globals = globals, body = body)
    }

    // Resolve any deferred `value-is` literals now that the constraint scope —
    // strict-ancestor globals plus this node's own (surviving) globals and
    // body arguments — is known. A subtree child trait names a constraint
    // against a global supplied by an ancestor subtree method; the standalone
    // child could not type the literal, so it was deferred until this
    // composition step.
    locally {
      val node = commands(index)
      node.body.foreach { body =>
        val scope = valueIsScope(ancestorGlobals, node.globals, body)
        commands(index) = node.copy(body = Some(resolveDeferredValueIs(body, scope)))
      }
    }

    // Children inherit the strict-ancestor globals plus this node's surviving
    // globals (the ones not removed as compatible re-declarations).
    val node         = commands(index)
    val childGlobals = ancestorGlobals ++
      node.globals.options.map(EffectiveCommandField.OptionField(_): EffectiveCommandField) ++
      node.globals.flags.map(EffectiveCommandField.FlagField(_))

    node.subcommands.foreach { sub =>
      if (sub >= 0) normalizeCommand(commands, sub, childGlobals, visited)
    }
  }

  /**
   * Builds the `value-is` resolution scope for a command body: strict-ancestor
   * globals, the node's own globals, and the body's own arguments. This mirrors
   * the constraint scope assembled by validation so deferred-literal resolution
   * and validation agree on which names are value-carrying and on each name's
   * comparand graph.
   */
  private def valueIsScope(
    ancestors: List[EffectiveCommandField],
    nodeGlobals: ExtendedGlobals,
    body: ExtendedCommandBody
  ): NameScope = {
    val scope = new NameScope
    ancestors.foreach {
      case EffectiveCommandField.OptionField(opt) => ToolValidation.registerOptionScope(scope, opt)
      case EffectiveCommandField.FlagField(flag)  => ToolValidation.registerFlagScope(scope, flag)
    }
    nodeGlobals.options.foreach(ToolValidation.registerOptionScope(scope, _))
    nodeGlobals.flags.foreach(ToolValidation.registerFlagScope(scope, _))
    body.options.foreach(ToolValidation.registerOptionScope(scope, _))
    body.flags.foreach(ToolValidation.registerFlagScope(scope, _))
    body.positionals.fixed.foreach(ToolValidation.registerFixedPositionalScope(scope, _))
    body.positionals.tail.foreach(ToolValidation.registerTailScope(scope, _))
    scope
  }

  /**
   * Resolves every [[ExtendedValueIsLiteral.Deferred]] literal in `body`'s
   * constraints against `scope`, the effective constraint scope assembled by
   * [[valueIsScope]] (and mirrored by validation). This is the single source of
   * truth for typing a `value-is` literal: the descriptor macro carries the raw
   * literal and never re-derives a comparand graph, so resolution always agrees
   * with the validation performed on constraint refs.
   *
   * For a name with a value-carrying comparand graph the literal is interpreted
   * into a [[golem.schema.SchemaValue]] and then checked against the graph, so
   * a literal whose *value* is incompatible (a wrong type or one that violates
   * the option's refinements — e.g. a regex/numeric bound) is rejected here
   * rather than slipping through to a later stage. A name in scope but without
   * a comparand (a flag) is a [[ToolBuildError.ValueIsTypeMismatch]]. A name
   * not in scope is left deferred — it is reported as an unresolved constraint
   * reference by validation (the standalone subtree-child case where the
   * ancestor global is not present).
   */
  private def resolveDeferredValueIs(
    body: ExtendedCommandBody,
    scope: NameScope
  ): ExtendedCommandBody = {
    def resolveRef(r: ExtendedRef): ExtendedRef =
      r match {
        case ExtendedRef.ValueIs(v) =>
          v.value match {
            case ExtendedValueIsLiteral.Deferred(lit) =>
              scope.typed.get(v.name) match {
                case Some(ValueComparand.Typed(comparand)) =>
                  // The structural validator owns schema soundness. If the
                  // comparand graph is unsound (a dangling or
                  // pure-alias-cycle ref) leave the literal deferred so
                  // validation reports the real schema error instead of a
                  // cascading value-is mismatch against a graph that cannot
                  // be resolved.
                  if (comparandGraphIsSound(comparand.graph)) {
                    val value =
                      ToolLiterals.valueIsLiteralToSchemaValue(comparand.graph, lit) match {
                        case Right(value) => value
                        case Left(_)      => fail(ToolBuildError.ValueIsTypeMismatch(v.name))
                      }
                    if (!valueIsCompatible(comparand, value))
                      fail(ToolBuildError.ValueIsTypeMismatch(v.name))
                    ExtendedRef.ValueIs(v.copy(value = ExtendedValueIsLiteral.Resolved(value)))
                  } else r
                // A repeatable-map whose type is not a map: validation reports
                // the malformed type. Leave the literal deferred and suppress
                // the cascading value-is mismatch.
                case Some(ValueComparand.BlockedByTypeError) => r
                // A flag (in scope, no value type) cannot carry a value-is. A
                // name not in scope is left deferred — validation reports it
                // as an unresolved constraint reference.
                case None =>
                  if (scope.names.contains(v.name))
                    fail(ToolBuildError.ValueIsTypeMismatch(v.name))
                  else r
              }
            case _: ExtendedValueIsLiteral.Resolved => r
          }
        case _: ExtendedRef.Present => r
      }

    def resolveConstraint(c: ExtendedConstraint): ExtendedConstraint =
      c match {
        case ExtendedConstraint.RequiresAll(v) => ExtendedConstraint.RequiresAll(v.map(resolveRef))
        case ExtendedConstraint.AllOrNone(v)   => ExtendedConstraint.AllOrNone(v.map(resolveRef))
        case ExtendedConstraint.RequiresAny(v) => ExtendedConstraint.RequiresAny(v.map(resolveRef))
        case ExtendedConstraint.MutexGroups(g) =>
          ExtendedConstraint.MutexGroups(g.map(group => ExtendedRefGroup(group.refs.map(resolveRef))))
        case ExtendedConstraint.Implies(i) =>
          ExtendedConstraint.Implies(
            i.copy(lhs = i.lhs.map(resolveRef), rhs = i.rhs.map(resolveRef))
          )
        case ExtendedConstraint.Forbids(f) =>
          ExtendedConstraint.Forbids(
            f.copy(lhs = f.lhs.map(resolveRef), rhs = f.rhs.map(resolveRef))
          )
      }

    body.copy(constraints = body.constraints.map(resolveConstraint))
  }

  private def reconcileGlobals(
    globals: ExtendedGlobals,
    ancestors: List[EffectiveCommandField],
    command: String
  ): ExtendedGlobals =
    if (ancestors.isEmpty) globals
    else {
      val keptOptions = globals.options.filterNot { opt =>
        val shape = FieldShape.Value(optionCollectedGraph(opt.shape))
        reconcileLocal(optionSurfaceNames(opt), shape, ancestors, command)
      }
      val keptFlags = globals.flags.filterNot { flag =>
        reconcileLocal(flagSurfaceNames(flag), flagFieldShape(flag), ancestors, command)
      }
      ExtendedGlobals(keptOptions, keptFlags)
    }

  private def reconcileBody(
    body: ExtendedCommandBody,
    ancestors: List[EffectiveCommandField],
    command: String
  ): ExtendedCommandBody =
    if (ancestors.isEmpty) body
    else {
      val keptOptions = body.options.filterNot { opt =>
        val shape = FieldShape.Value(optionCollectedGraph(opt.shape))
        reconcileLocal(optionSurfaceNames(opt), shape, ancestors, command)
      }
      val keptFlags = body.flags.filterNot { flag =>
        reconcileLocal(flagSurfaceNames(flag), flagFieldShape(flag), ancestors, command)
      }
      val keptFixed = body.positionals.fixed.filterNot { positional =>
        reconcileLocal(List(positional.name), FieldShape.Value(positional.tpe), ancestors, command)
      }
      val keptTail = body.positionals.tail.filterNot { tail =>
        reconcileLocal(
          List(tail.name),
          FieldShape.Value(listWrapperGraph(tail.itemType)),
          ancestors,
          command
        )
      }
      val reconciled = body.copy(
        positionals = ExtendedPositionals(keptFixed, keptTail),
        options = keptOptions,
        flags = keptFlags
      )
      // De-projection may have removed the parameter that was the tail (or a
      // later positional that kept an earlier `Seq[T]` out of tail position),
      // so re-infer the tail against the parameters that actually survived.
      reinferBodyTail(reconciled)
    }

  /**
   * Finalize a command body's tail positional after [[reconcileBody]] removed
   * the inherited re-declarations that did not survive in scope.
   *
   * The macro emits only each `Seq[T]` candidate's *selected* surface into the
   * body (tail positional or repeatable-list option) and records the candidate,
   * in declaration order, in [[ExtendedCommandBody.positionalPlan]] (the *last*
   * positional `Seq[T]` is the tail, an earlier one is a repeatable-list
   * option). Because the selection assumed the macro-known inherited
   * re-declarations would de-project, this pass repairs it against the
   * parameters that actually survived. It:
   *
   *   - **demotes** an installed tail into a repeatable-list option
   *     (reconstructed by copying the tail's value graph) when another
   *     positional survived after it — rejecting an explicitly authored tail,
   *     or an inferred tail carrying occurrence bounds / tail-only attributes a
   *     repeatable-list option cannot represent; and
   *   - **promotes** the last surviving `Seq[T]` candidate's repeatable-list
   *     option into the tail (reconstructed likewise) when its natural tail was
   *     de-projected — rejecting one carrying option-only attributes a tail
   *     cannot represent.
   *
   * It is a no-op for hand-built bodies (empty plan) and whenever the natural
   * tail is already the last surviving positional.
   */
  private def reinferBodyTail(body0: ExtendedCommandBody): ExtendedCommandBody = {
    if (body0.positionalPlan.isEmpty) return body0
    var body = body0

    // The last surviving positional-eligible candidate, in declaration order.
    var lastIdx = -1
    body.positionalPlan.zipWithIndex.foreach { case (candidate, idx) =>
      if (bodyContainsPositional(body, candidate.name)) lastIdx = idx
    }
    if (lastIdx < 0) return body
    val lastName = body.positionalPlan(lastIdx).name

    // An explicitly-authored tail that survives de-projection must be the last
    // positional-eligible candidate. If a positional survives after it, its
    // authored order is violated — whether it still holds the tail slot or was
    // lowered to an inherited-global surrogate option (a form invisible to the
    // demote path below, which only sees an installed tail). An explicit tail
    // that was de-projected entirely is gone and no longer constrains: a
    // genuine later `Seq[T]` tail may legitimately take the slot.
    val explicitTailBeforeLast = body.positionalPlan.take(lastIdx).exists {
      case c: PositionalCandidate.VecCandidate =>
        c.explicitTail && bodyContainsPositional(body, c.name)
      case _ => false
    }
    if (explicitTailBeforeLast) fail(ToolBuildError.FixedPositionalAfterTail(lastName))

    // Demote first: an installed inferred tail whose declaration precedes a
    // surviving positional is no longer last. Doing this before promotion
    // means that if the last candidate is itself promoted (and its option
    // removed) the demoted option is still inserted relative to the remaining
    // later options.
    body.positionals.tail.map(_.name).foreach { tailName =>
      val tailIdx = body.positionalPlan.indexWhere(_.name == tailName)
      if (tailIdx >= 0 && tailIdx < lastIdx)
        body = demoteTailToOption(body, tailIdx)
    }

    // Promote: the last surviving candidate must hold the tail slot. When it
    // is a `Seq[T]` currently projected as a repeatable-list option (its
    // natural tail was de-projected), move it into the tail slot.
    body.positionalPlan(lastIdx) match {
      case _: PositionalCandidate.VecCandidate => body = promoteOptionToTail(body, lastIdx)
      case _                                   => ()
    }
    body
  }

  /**
   * Demote the body's installed inferred tail (the candidate at `tailIdx`) into
   * a repeatable-list option, because a positional survives after it. The
   * option is reconstructed from the tail's value graph and inserted in
   * declaration order among the body options. Rejects an inferred tail whose
   * authored occurrence bounds (`min`/`max`) or tail-only attributes a
   * repeatable-list option cannot represent. (An explicit tail before a
   * survivor is rejected earlier, in [[reinferBodyTail]].)
   */
  private def demoteTailToOption(
    body: ExtendedCommandBody,
    tailIdx: Int
  ): ExtendedCommandBody =
    body.positionalPlan(tailIdx) match {
      case _: PositionalCandidate.Plain        => body
      case c: PositionalCandidate.VecCandidate =>
        // Only act on the candidate that actually holds the tail slot.
        if (!body.positionals.tail.exists(_.name == c.name)) return body

        // `min`/`max` are overloaded between surfaces — occurrence bounds on a
        // tail, item numeric bounds on a repeatable-list option — so a
        // candidate that authored either cannot switch surface without
        // changing their meaning. This consults the authored fact rather than
        // the materialized tail shape because an authored `min = 0` coincides
        // with the tail default and so leaves no trace in the tail's
        // `min`/`max` fields.
        if (c.hasMinOrMaxAttr)
          fail(
            ToolBuildError.VecSurfaceConflict(
              c.name,
              "it authored a `min`/`max` bound, which means occurrence count " +
                "on a tail positional but item count on a repeatable-list " +
                "option; a parameter now follows it so it must become a " +
                "repeatable-list option, which would change that meaning"
            )
          )

        val tail = body.positionals.tail.get
        // A repeatable-list option has no separator/verbatim/stdio handling,
        // so a tail using any of those cannot be represented as one.
        if (tail.separator.isDefined || tail.verbatim || tail.acceptsStdio)
          fail(
            ToolBuildError.VecSurfaceConflict(
              c.name,
              "it has a tail-only attribute (`separator`/`verbatim`/`acceptsStdio`) " +
                "that a repeatable-list option cannot express, but a parameter now " +
                "follows it so it must become a repeatable-list option"
            )
          )

        val option = ExtendedOptionSpec(
          long = tail.name,
          short = None,
          aliases = Nil,
          doc = tail.doc,
          valueName = tail.valueName,
          shape = ExtendedOptionShape.RepeatableList(
            ExtendedRepeatableListShape(Repetition.Repeated, tail.itemType)
          ),
          default = None,
          required = false,
          envVar = None
        )
        // Body options are in declaration order. Insert before the first
        // option declared after this `Seq[T]` that survived de-projection;
        // otherwise append.
        val insertAt = c.laterOptionNames.iterator
          .map(later => body.options.indexWhere(_.long == later))
          .find(_ >= 0)
        val options = insertAt match {
          case Some(pos) => body.options.take(pos) ::: option :: body.options.drop(pos)
          case None      => body.options :+ option
        }
        body.copy(
          positionals = body.positionals.copy(tail = None),
          options = options
        )
    }

  /**
   * Promote the last surviving `Seq[T]` candidate (at `lastIdx`) from a
   * repeatable-list option into the tail positional, because its natural tail
   * was de-projected. An explicit tail lowered to an inherited-global surrogate
   * option installs its full authored spec
   * ([[PositionalCandidate.VecCandidate.authoredTailSurrogate]]) verbatim, so
   * its tail-only attributes are preserved; an inferred candidate is
   * reconstructed from the option's value graph instead. A no-op if it already
   * holds the tail slot or never projected to an option; rejects an inferred
   * candidate carrying option-only attributes (or an `Option[Seq[T]]`, or
   * authored `min`/`max`) that a tail cannot represent.
   */
  private def promoteOptionToTail(
    body: ExtendedCommandBody,
    lastIdx: Int
  ): ExtendedCommandBody =
    body.positionalPlan(lastIdx) match {
      case _: PositionalCandidate.Plain        => body
      case c: PositionalCandidate.VecCandidate =>
        // Already the tail (its natural tail survived): nothing to do.
        if (body.positionals.tail.exists(_.name == c.name)) return body
        val pos = body.options.indexWhere(_.long == c.name)
        if (pos < 0) return body

        // An explicit tail lowered to an inherited-global surrogate option:
        // install its full authored spec verbatim. The surrogate option
        // carries none of the tail-only fields, so reconstructing from it
        // would silently drop them; the authored spec keeps them (and any
        // resulting invalid state, e.g. `verbatim` without `separator`, is
        // caught later by validation). `min`/`max` are already occurrence
        // bounds here, so the inferred-only hasMinOrMaxAttr rejection is
        // skipped.
        c.authoredTailSurrogate match {
          case Some(authoredTail) =>
            // Defensive against hand-built bodies: confirm the option really
            // is the droppable repeatable-list surrogate before replacing it
            // with the tail.
            verifyPromotableSurrogate(body.options(pos), c.name)
            body.copy(
              positionals = body.positionals.copy(tail = Some(authoredTail)),
              options = body.options.take(pos) ++ body.options.drop(pos + 1)
            )
          case None =>
            // A tail is variadic with no absent state and interprets
            // `min`/`max` as occurrence bounds, so these authored shapes
            // cannot move onto a tail.
            if (c.optionalVec)
              fail(
                ToolBuildError.VecSurfaceConflict(
                  c.name,
                  "it is an `Option[Seq[T]]`, which has no tail-positional " +
                    "representation, but de-projection made it the last positional " +
                    "so it must become the tail"
                )
              )
            if (c.hasMinOrMaxAttr)
              fail(
                ToolBuildError.VecSurfaceConflict(
                  c.name,
                  "it has a `min`/`max` bound applied to its items as a " +
                    "repeatable-list option, but de-projection made it the last " +
                    "positional so it must become the tail (where `min`/`max` would " +
                    "instead bound the occurrence count)"
                )
              )
            verifyPromotableSurrogate(body.options(pos), c.name)

            val option   = body.options(pos)
            val itemType = option.shape match {
              case ExtendedOptionShape.RepeatableList(list) => list.itemType
              case _                                        => throw new IllegalStateException("shape checked as RepeatableList above")
            }
            body.copy(
              positionals = body.positionals.copy(tail =
                Some(
                  ExtendedTailPositional(
                    name = option.long,
                    doc = option.doc,
                    valueName = option.valueName,
                    itemType = itemType,
                    min = 0,
                    max = None,
                    separator = None,
                    verbatim = false,
                    acceptsStdio = false
                  )
                )
              ),
              options = body.options.take(pos) ++ body.options.drop(pos + 1)
            )
        }
    }

  /**
   * Verify that `option` is a droppable repeatable-list surrogate that can take
   * the tail slot: it must carry no option-only surface a tail positional
   * cannot express (`short`/`aliases`/`default`/`required`/`env`), and must be
   * a repeatable-list with `Repeated` (non-delimited) repetition.
   */
  private def verifyPromotableSurrogate(option: ExtendedOptionSpec, name: String): Unit = {
    if (
      option.short.isDefined || option.aliases.nonEmpty || option.default.isDefined ||
      option.required || option.envVar.isDefined
    )
      fail(
        ToolBuildError.VecSurfaceConflict(
          name,
          "it has an option-only attribute (`short`/`aliases`/`default`/" +
            "`required`/`env`) that a tail positional cannot express, but " +
            "de-projection made it the last positional so it must become the tail"
        )
      )
    option.shape match {
      case ExtendedOptionShape.RepeatableList(list) =>
        if (list.repetition != Repetition.Repeated)
          fail(
            ToolBuildError.VecSurfaceConflict(
              name,
              "it uses a delimited repetition that a tail positional cannot " +
                "express, but de-projection made it the last positional so it " +
                "must become the tail"
            )
          )
      case _ =>
        fail(
          ToolBuildError.VecSurfaceConflict(
            name,
            "it does not project to a repeatable list, so it has no " +
              "tail-positional representation"
          )
        )
    }
  }

  /**
   * Whether the body still carries a positional-eligible parameter with the
   * given surface name (as a fixed positional, the tail, or a repeatable-list
   * option), i.e. it survived de-projection.
   */
  private def bodyContainsPositional(body: ExtendedCommandBody, name: String): Boolean =
    body.positionals.fixed.exists(_.name == name) ||
      body.positionals.tail.exists(_.name == name) ||
      body.options.exists(_.long == name)

  /**
   * Decide the fate of one local declaration against the inherited globals in
   * scope. Returns `true` when the local is a compatible re-declaration that
   * should be removed (the inherited global covers it), `false` when no
   * inherited global shares a surface name (keep the local), and fails with
   * [[ToolBuildError.InheritedGlobalConflict]] when a surface name matches but
   * the input shapes are incompatible.
   */
  private def reconcileLocal(
    localNames: List[String],
    localShape: FieldShape,
    ancestors: List[EffectiveCommandField],
    command: String
  ): Boolean = {
    // A local may share a surface name with more than one inherited global
    // (its long name with one, an alias with another). All ancestors are
    // scanned so that an incompatible collision — even one found after a
    // compatible one — is reported immediately as a conflict. The local can be
    // de-projected onto an inherited global only when it collides with exactly
    // one *distinct* inherited global (matching the same global through both
    // its long name and an alias is a single ancestor entry, hence still one
    // global). Colliding compatibly with two or more distinct inherited
    // globals is ambiguous — there is no single global to inherit from, and
    // silently dropping the local would leave the parameter with no canonical
    // body field — so that is also a conflict.
    var compatible: List[String] = Nil
    ancestors.foreach { inherited =>
      val inheritedNames = effectiveFieldSurfaceNames(inherited)
      localNames.find(l => inheritedNames.contains(l)).foreach { colliding =>
        if (!fieldShapesCompatible(effectiveFieldShape(inherited), localShape))
          fail(
            ToolBuildError.InheritedGlobalConflict(
              colliding,
              effectiveFieldPrimaryName(inherited),
              command
            )
          )
        val primary = effectiveFieldPrimaryName(inherited)
        if (!compatible.contains(primary)) compatible = compatible :+ primary
      }
    }
    if (compatible.length > 1)
      fail(
        ToolBuildError.InheritedGlobalConflict(
          localNames.headOption.getOrElse(""),
          compatible.mkString(", "),
          command
        )
      )
    compatible.nonEmpty
  }

  /**
   * The primary (long) surface name of an inherited effective global, used to
   * name the colliding global in [[ToolBuildError.InheritedGlobalConflict]].
   */
  private def effectiveFieldPrimaryName(g: EffectiveCommandField): String =
    g match {
      case EffectiveCommandField.OptionField(o) => o.long
      case EffectiveCommandField.FlagField(f)   => f.long
    }

  /**
   * The canonical input "surface family" of a command field, used to decide
   * whether a local re-declaration is compatible with an inherited global.
   * Flags are distinguished by their flag family (a bool flag and a count flag
   * carry different values, and neither is interchangeable with a value-bearing
   * option or positional of the same name); every value-bearing form is
   * compared by its canonical input value graph.
   */
  private sealed trait FieldShape
  private object FieldShape {
    case object BoolFlag                       extends FieldShape
    case object CountFlag                      extends FieldShape
    final case class Value(graph: SchemaGraph) extends FieldShape
  }

  private def flagFieldShape(f: FlagSpec): FieldShape =
    f.shape match {
      case _: FlagShape.BoolFlag  => FieldShape.BoolFlag
      case _: FlagShape.CountFlag => FieldShape.CountFlag
    }

  private def effectiveFieldShape(g: EffectiveCommandField): FieldShape =
    g match {
      case EffectiveCommandField.OptionField(o) => FieldShape.Value(optionCollectedGraph(o.shape))
      case EffectiveCommandField.FlagField(f)   => flagFieldShape(f)
    }

  private def fieldShapesCompatible(a: FieldShape, b: FieldShape): Boolean =
    (a, b) match {
      case (FieldShape.BoolFlag, FieldShape.BoolFlag)   => true
      case (FieldShape.CountFlag, FieldShape.CountFlag) => true
      case (FieldShape.Value(ga), FieldShape.Value(gb)) => schemaShapesMatch(ga, gb)
      case _                                            => false
    }

  private def optionSurfaceNames(o: ExtendedOptionSpec): List[String] = o.long :: o.aliases

  private def flagSurfaceNames(f: FlagSpec): List[String] = f.long :: f.aliases

  private def effectiveFieldSurfaceNames(g: EffectiveCommandField): List[String] =
    g match {
      case EffectiveCommandField.OptionField(o) => optionSurfaceNames(o)
      case EffectiveCommandField.FlagField(f)   => flagSurfaceNames(f)
    }

  /**
   * Append a graft (graft-local command nodes whose index 0 is the dispatcher
   * placeholder) to `parent`, offsetting every internal subcommand index, and
   * return the extended parent list plus the parent index of the placeholder.
   * The caller links this index as a subcommand of the hosting command.
   */
  def appendGraftedSubtree(
    parent: Vector[ExtendedCommandNode],
    graft: Vector[ExtendedCommandNode]
  ): (Vector[ExtendedCommandNode], Int) = {
    val offset  = parent.length
    val shifted = graft.map(node => node.copy(subcommands = node.subcommands.map(_ + offset)))
    (parent ++ shifted, offset)
  }

  private def fail(error: ToolBuildError): Nothing = throw ToolBuildException(error)
}
