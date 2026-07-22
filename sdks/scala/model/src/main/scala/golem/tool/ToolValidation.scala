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

import golem.tool.ToolGraphs._

import scala.collection.mutable

/**
 * Producer-side validation of an [[ExtendedToolType]] against the construction
 * invariants documented in `golem:tool/common`. Mirrors the canonical host-side
 * validator, adapted to the SDK's per-argument [[golem.schema.SchemaGraph]]
 * representation: type/value well-formedness of embedded schemas is delegated
 * to the schema validators; each embedded graph is validated for structural
 * well-formedness (including dangling refs and ill-formed inline restrictions)
 * by [[ToolGraphs.checkGraphClosed]].
 */
object ToolValidation {

  /**
   * Validate the whole tool. Runs the structural command-tree check first so
   * the subsequent recursive traversal can index the tree without bounds/cycle
   * hazards.
   */
  def validateTool(tool: ExtendedToolType): Either[ToolBuildError, Unit] =
    try {
      checkCommandTreeStructureThrowing(tool)
      visitCommand(tool, 0, Nil)
      Right(())
    } catch {
      case ToolBuildException(error) => Left(error)
    }

  /**
   * Structural integrity of the command tree: non-empty, every subcommand index
   * in bounds, acyclic, single-rooted tree (no shared subcommands), all nodes
   * reachable from the root.
   */
  def checkCommandTreeStructure(tool: ExtendedToolType): Either[ToolBuildError, Unit] =
    try {
      checkCommandTreeStructureThrowing(tool)
      Right(())
    } catch {
      case ToolBuildException(error) => Left(error)
    }

  private def checkCommandTreeStructureThrowing(tool: ExtendedToolType): Unit = {
    val len = tool.commands.length
    if (len == 0) fail(ToolBuildError.EmptyCommandTree)
    tool.commands.foreach { node =>
      node.subcommands.foreach { sub =>
        if (sub < 0 || sub >= len) fail(ToolBuildError.CommandIndexOutOfBounds(sub, len))
      }
    }
    val visited = Array.fill(len)(false)
    val onStack = Array.fill(len)(false)

    def dfs(idx: Int): Unit = {
      if (onStack(idx)) fail(ToolBuildError.CommandTreeCycle(idx))
      if (visited(idx)) fail(ToolBuildError.DuplicateCommandParent(idx))
      visited(idx) = true
      onStack(idx) = true
      // Bounds were validated above.
      tool.commands(idx).subcommands.foreach(dfs)
      onStack(idx) = false
    }

    dfs(0)
    var i = 0
    while (i < len) {
      if (!visited(i)) fail(ToolBuildError.UnreachableCommandNode(i))
      i += 1
    }
  }

  /**
   * Recursive, scope-aware traversal mirroring the canonical validator.
   * `ancestorGlobals` are the globals of strict ancestors; the current node's
   * own globals are appended to form the in-scope set for its body and
   * children.
   */
  private def visitCommand(
    tool: ExtendedToolType,
    index: Int,
    ancestorGlobals: List[ExtendedGlobals]
  ): Unit = {
    val node = tool.commands(index)
    checkIdentifier("command name", node.name)
    node.aliases.foreach(checkIdentifier("command alias", _))
    checkGlobalsDecls(node.globals)
    checkGlobalScopeUniqueness(ancestorGlobals, node.globals)

    val inScope = ancestorGlobals :+ node.globals

    node.body.foreach(checkBody(_, inScope))
    checkSubcommandUniqueness(tool, node)

    node.subcommands.foreach(sub => visitCommand(tool, sub, inScope))
  }

  /**
   * Identifier, repeatable-map, default, and variant-in-input checks for the
   * declarations within one command's globals (uniqueness is handled by
   * [[checkGlobalScopeUniqueness]]).
   */
  private def checkGlobalsDecls(globals: ExtendedGlobals): Unit = {
    globals.options.foreach(checkOptionDecl)
    globals.flags.foreach(checkFlagIdentifiers)
  }

  private def checkOptionDecl(opt: ExtendedOptionSpec): Unit = {
    checkIdentifier("option long name", opt.long)
    opt.aliases.foreach(checkIdentifier("option alias", _))
    orFail(checkGraphClosed(optionAuthoredGraph(opt.shape), s"option --${opt.long}"))
    opt.shape match {
      case ExtendedOptionShape.RepeatableMap(shape) if !resolvesToMap(shape.mapType) =>
        fail(ToolBuildError.RepeatableMapTypeNotMap(opt.long))
      case _ => ()
    }
    opt.default.foreach(default => orFail(validateDefault(default, optionCollectedGraph(opt.shape))))
    if (graphReachesVariant(optionInputGraph(opt.shape)))
      fail(ToolBuildError.VariantInInputPosition(opt.long))
  }

  private def checkFlagIdentifiers(flag: FlagSpec): Unit = {
    checkIdentifier("flag long name", flag.long)
    flag.aliases.foreach(checkIdentifier("flag alias", _))
  }

  /**
   * The current command's own globals must be unique among themselves and
   * against every ancestor global (long names, aliases, and short forms).
   */
  private def checkGlobalScopeUniqueness(
    ancestors: List[ExtendedGlobals],
    own: ExtendedGlobals
  ): Unit = {
    val names  = mutable.Set.empty[String]
    val shorts = mutable.Set.empty[Char]
    ancestors.foreach(seedGlobalTokens(_, names, shorts))
    own.options.foreach { opt =>
      insertUnique(names, opt.long)
      opt.aliases.foreach(insertUnique(names, _))
      opt.short.foreach(insertUniqueShort(shorts, _))
    }
    own.flags.foreach { flag =>
      insertUnique(names, flag.long)
      flag.aliases.foreach(insertUnique(names, _))
      flag.short.foreach(insertUniqueShort(shorts, _))
    }
  }

  private def seedGlobalTokens(
    globals: ExtendedGlobals,
    names: mutable.Set[String],
    shorts: mutable.Set[Char]
  ): Unit = {
    globals.options.foreach { opt =>
      names += opt.long
      names ++= opt.aliases
      opt.short.foreach(shorts += _)
    }
    globals.flags.foreach { flag =>
      names += flag.long
      names ++= flag.aliases
      flag.short.foreach(shorts += _)
    }
  }

  /**
   * Per-name `value-is` comparand scope for constraint resolution (only
   * value-typed names; a name in `names` but absent from `typed` is a flag).
   */
  private[tool] final class NameScope {
    val names: mutable.Set[String]                 = mutable.Set.empty
    val typed: mutable.Map[String, ValueComparand] = mutable.Map.empty
  }

  /**
   * Registers a flag's referenceable names. A flag carries no value type, so it
   * is never added to `typed`; a `value-is` against it is rejected.
   */
  private[tool] def registerFlagScope(scope: NameScope, flag: FlagSpec): Unit = {
    scope.names += flag.long
    scope.names ++= flag.aliases
  }

  /**
   * Registers a fixed positional's name and its whole-or-one-peel comparand (a
   * fixed positional is a non-collecting value surface: its declared type is
   * the comparand).
   */
  private[tool] def registerFixedPositionalScope(
    scope: NameScope,
    positional: ExtendedPositional
  ): Unit = {
    scope.names += positional.name
    scope.typed.update(
      positional.name,
      ValueComparand.Typed(ValueIsComparand(positional.tpe, ValueIsMode.WholeOrOnePeel))
    )
  }

  /**
   * Registers a tail positional's name and its per-occurrence comparand. A tail
   * collects occurrences into a `list<item>`, so a `value-is` literal matches
   * one item exactly (the tail's item type), never the whole collected list.
   */
  private[tool] def registerTailScope(scope: NameScope, tail: ExtendedTailPositional): Unit = {
    scope.names += tail.name
    scope.typed.update(
      tail.name,
      ValueComparand.Typed(ValueIsComparand(tail.itemType, ValueIsMode.Exact))
    )
  }

  private[tool] def registerOptionScope(scope: NameScope, opt: ExtendedOptionSpec): Unit = {
    scope.names += opt.long
    scope.names ++= opt.aliases
    val comparand = optionValueIsComparand(opt.shape)
    scope.typed.update(opt.long, comparand)
    opt.aliases.foreach(alias => scope.typed.update(alias, comparand))
  }

  private def checkBody(body: ExtendedCommandBody, inScope: List[ExtendedGlobals]): Unit = {
    // In-scope global tokens (for uniqueness) and resolution scope (for refs).
    val names  = mutable.Set.empty[String]
    val shorts = mutable.Set.empty[Char]
    val scope  = new NameScope
    inScope.foreach { globals =>
      seedGlobalTokens(globals, names, shorts)
      globals.options.foreach(registerOptionScope(scope, _))
      globals.flags.foreach(registerFlagScope(scope, _))
    }

    body.options.foreach { opt =>
      checkOptionDecl(opt)
      insertUnique(names, opt.long)
      opt.aliases.foreach(insertUnique(names, _))
      opt.short.foreach(insertUniqueShort(shorts, _))
      registerOptionScope(scope, opt)
    }

    body.flags.foreach { flag =>
      checkFlagIdentifiers(flag)
      insertUnique(names, flag.long)
      flag.aliases.foreach(insertUnique(names, _))
      flag.short.foreach(insertUniqueShort(shorts, _))
      registerFlagScope(scope, flag)
    }

    // Optional fixed positionals must be trailing: once an optional one
    // appears, no required one may follow, or the boundary between them is
    // ambiguous. The macro enforces this for locally declared positionals, but
    // inherited-global de-projection can leave a re-declared optional
    // positional local at runtime, so the surviving order is re-checked here.
    var sawOptionalPositional = false
    body.positionals.fixed.foreach { positional =>
      checkIdentifier("positional name", positional.name)
      orFail(checkGraphClosed(positional.tpe, s"positional ${positional.name}"))
      insertUnique(names, positional.name)
      registerFixedPositionalScope(scope, positional)
      positional.default.foreach(default => orFail(validateDefault(default, positional.tpe)))
      if (graphReachesVariant(positional.tpe))
        fail(ToolBuildError.VariantInInputPosition(positional.name))
      if (positional.required) {
        if (sawOptionalPositional)
          fail(ToolBuildError.RequiredPositionalAfterOptional(positional.name))
      } else {
        sawOptionalPositional = true
      }
    }

    body.positionals.tail.foreach { tail =>
      checkIdentifier("positional name", tail.name)
      orFail(checkGraphClosed(tail.itemType, s"tail ${tail.name}"))
      insertUnique(names, tail.name)
      registerTailScope(scope, tail)
      if (graphReachesVariant(tail.itemType))
        fail(ToolBuildError.VariantInInputPosition(tail.name))
      if (tail.verbatim && tail.separator.isEmpty)
        fail(ToolBuildError.VerbatimWithoutSeparator(tail.name))
      tail.max.foreach { max =>
        if (tail.min > max)
          fail(ToolBuildError.InvalidTailOccurrenceBounds(tail.name, tail.min, max))
      }
    }

    body.constraints.foreach(checkConstraint(_, scope))

    body.result.foreach { result =>
      orFail(checkGraphClosed(result.tpe, "result"))
      result.formatters.foreach(f => checkIdentifier("formatter name", f.name))
      if (!result.formatters.exists(_.name == result.defaultFormatter))
        fail(ToolBuildError.UnresolvedDefaultFormatter(result.defaultFormatter))
    }

    body.errors.foreach { errorCase =>
      checkIdentifier("error-case name", errorCase.name)
      errorCase.payload.foreach(payload => orFail(checkGraphClosed(payload, s"error ${errorCase.name}")))
    }
  }

  private def checkSubcommandUniqueness(tool: ExtendedToolType, node: ExtendedCommandNode): Unit = {
    val seen = mutable.Set.empty[String]
    node.subcommands.foreach { sub =>
      val child = tool.commands(sub)
      insertUnique(seen, child.name)
      child.aliases.foreach(insertUnique(seen, _))
    }
  }

  private def checkConstraint(c: ExtendedConstraint, scope: NameScope): Unit =
    c match {
      case ExtendedConstraint.RequiresAll(v) => checkRefs(v, scope)
      case ExtendedConstraint.AllOrNone(v)   => checkRefs(v, scope)
      case ExtendedConstraint.RequiresAny(v) => checkRefs(v, scope)
      case ExtendedConstraint.MutexGroups(g) => g.foreach(group => checkRefs(group.refs, scope))
      case ExtendedConstraint.Implies(i)     => checkRefs(i.lhs, scope); checkRefs(i.rhs, scope)
      case ExtendedConstraint.Forbids(f)     => checkRefs(f.lhs, scope); checkRefs(f.rhs, scope)
    }

  private def checkRefs(refs: List[ExtendedRef], scope: NameScope): Unit =
    refs.foreach {
      case ExtendedRef.Present(name) =>
        if (!scope.names.contains(name)) fail(ToolBuildError.UnresolvedConstraintRef(name))
      case ExtendedRef.ValueIs(v) =>
        if (!scope.names.contains(v.name)) fail(ToolBuildError.UnresolvedConstraintRef(v.name))
        scope.typed.get(v.name) match {
          // A name with no value type (a flag) cannot carry a value-is.
          case None => fail(ToolBuildError.ValueIsTypeMismatch(v.name))
          // A repeatable-map whose type is not a map: the malformed type is
          // reported by the structural checks; suppress the cascading value-is
          // mismatch.
          case Some(ValueComparand.BlockedByTypeError) => ()
          case Some(ValueComparand.Typed(comparand))   =>
            v.value match {
              case ExtendedValueIsLiteral.Resolved(value) =>
                if (!valueIsCompatible(comparand, value))
                  fail(ToolBuildError.ValueIsTypeMismatch(v.name))
              // The name is in scope, so composition (which carries the
              // comparand type) should have resolved this literal. A surviving
              // deferred literal is a resolution gap, not a silently
              // acceptable ref.
              case _: ExtendedValueIsLiteral.Deferred =>
                fail(ToolBuildError.UnresolvedValueIsLiteral(v.name))
            }
        }
    }

  /**
   * Resolve a command path (names or aliases) to a command-tree index using
   * checked lookups, so a malformed tree cannot fail unexpectedly.
   */
  private[tool] def resolveCommandPath(
    tool: ExtendedToolType,
    commandPath: List[String]
  ): Either[ToolBuildError, Int] = {
    if (tool.commands.isEmpty) return Left(ToolBuildError.EmptyCommandTree)
    var idx = 0
    val it  = commandPath.iterator
    while (it.hasNext) {
      val part = it.next()
      tool.commands.lift(idx) match {
        case None       => return Left(ToolBuildError.CommandNotFound(part))
        case Some(node) =>
          node.subcommands.find { childIdx =>
            tool.commands
              .lift(childIdx)
              .exists(child => child.name == part || child.aliases.contains(part))
          } match {
            case Some(next) => idx = next
            case None       => return Left(ToolBuildError.CommandNotFound(part))
          }
      }
    }
    Right(idx)
  }

  /**
   * `^[a-z][a-z0-9]*(-[a-z0-9]+)*$`: lowercase kebab-case starting with a
   * letter, no leading/trailing or doubled dashes.
   */
  private[tool] def isValidIdentifier(s: String): Boolean = {
    if (s.isEmpty || s.endsWith("-")) return false
    var prevDash = false
    var i        = 0
    while (i < s.length) {
      val c  = s.charAt(i)
      val ok =
        if (i == 0) c >= 'a' && c <= 'z'
        else (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '-'
      val dashOk = !(c == '-' && prevDash)
      prevDash = c == '-'
      if (!ok || !dashOk) return false
      i += 1
    }
    true
  }

  private[tool] def checkIdentifier(kind: String, value: String): Unit =
    if (!isValidIdentifier(value)) fail(ToolBuildError.InvalidIdentifier(kind, value))

  private def insertUnique(set: mutable.Set[String], name: String): Unit =
    if (!set.add(name)) fail(ToolBuildError.DuplicateName(name))

  private def insertUniqueShort(set: mutable.Set[Char], short: Char): Unit =
    if (!set.add(short)) fail(ToolBuildError.DuplicateShort(short))

  private def fail(error: ToolBuildError): Nothing = throw ToolBuildException(error)

  private[tool] def orFail(result: Either[ToolBuildError, Unit]): Unit =
    result match {
      case Left(error) => throw ToolBuildException(error)
      case Right(_)    => ()
    }
}
