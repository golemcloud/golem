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

import scala.util.control.NoStackTrace

/**
 * Everything that can go wrong while validating an [[ExtendedToolType]] or
 * encoding it into its wire form. [[ToolBuildError.message]] carries the
 * human-readable diagnostic.
 */
sealed trait ToolBuildError extends Product with Serializable {
  def message: String
}

object ToolBuildError {
  private def q(s: String): String = "\"" + s + "\""

  case object EmptyCommandTree extends ToolBuildError {
    def message: String = "the command tree is empty"
  }
  final case class CommandIndexOutOfBounds(index: Int, len: Int) extends ToolBuildError {
    def message: String = s"command index $index is out of bounds (tree has $len nodes)"
  }
  final case class UnreachableCommandNode(index: Int) extends ToolBuildError {
    def message: String = s"command node $index is not reachable from the root"
  }
  final case class CommandTreeCycle(index: Int) extends ToolBuildError {
    def message: String = s"the command tree contains a cycle at node $index"
  }
  final case class DuplicateCommandParent(index: Int) extends ToolBuildError {
    def message: String = s"command node $index has more than one parent"
  }
  final case class InvalidIdentifier(kind: String, value: String) extends ToolBuildError {
    def message: String = s"invalid $kind: ${q(value)}"
  }
  final case class SubtreeCycle(path: String) extends ToolBuildError {
    def message: String = s"subtree cycle detected: $path"
  }
  final case class SubtreeRootNameMismatch(expected: String, actual: String) extends ToolBuildError {
    def message: String =
      s"subtree root name ${q(actual)} does not match the parent command name ${q(expected)}"
  }
  final case class SubtreeAnnotationsUnsupported(name: String) extends ToolBuildError {
    def message: String =
      s"annotations are not supported on a subtree command method ${q(name)} (the model places command-annotations on a command body)"
  }
  final case class DuplicateName(name: String) extends ToolBuildError {
    def message: String = s"duplicate tool metadata name: $name"
  }
  final case class DuplicateShort(short: Char) extends ToolBuildError {
    def message: String = s"duplicate short form: '$short'"
  }
  final case class InheritedGlobalConflict(name: String, inherited: String, command: String) extends ToolBuildError {
    def message: String =
      s"parameter surface name ${q(name)} on command ${q(command)} conflicts with inherited " +
        s"global ${q(inherited)}: it either has an incompatible shape or collides with more " +
        "than one distinct inherited global; rename the parameter or align it with a " +
        "single compatible inherited global"
  }
  final case class UnresolvedTypeRef(position: String, id: String) extends ToolBuildError {
    def message: String =
      s"type reference ${q(id)} at $position does not resolve within its schema graph"
  }
  final case class IllFormedSchema(position: String, detail: String) extends ToolBuildError {
    def message: String = s"schema at $position is not well-formed: $detail"
  }
  final case class EncodeError(detail: String) extends ToolBuildError {
    def message: String = s"tool metadata encode error: $detail"
  }
  final case class DefaultTypeMismatch(detail: String) extends ToolBuildError {
    def message: String = s"default value does not match schema: $detail"
  }
  final case class ValueIsTypeMismatch(name: String) extends ToolBuildError {
    def message: String = s"value-is literal does not match the argument type: $name"
  }
  final case class RepeatableMapTypeNotMap(name: String) extends ToolBuildError {
    def message: String = s"repeatable-map option does not collect into a map: $name"
  }
  final case class UnresolvedDefaultFormatter(name: String) extends ToolBuildError {
    def message: String = s"default-formatter is not declared: $name"
  }
  final case class VerbatimWithoutSeparator(name: String) extends ToolBuildError {
    def message: String = s"verbatim tail positional has no separator: $name"
  }
  final case class VariantInInputPosition(name: String) extends ToolBuildError {
    def message: String = s"a variant type is reachable from input position: $name"
  }
  final case class CommandNotFound(name: String) extends ToolBuildError {
    def message: String = s"command not found: $name"
  }
  final case class UnresolvedConstraintRef(name: String) extends ToolBuildError {
    def message: String = s"constraint references an unknown argument: $name"
  }
  final case class AutoInjectedToolParameter(position: String) extends ToolBuildError {
    def message: String =
      s"auto-injected types are not valid tool value parameters or results: $position"
  }
  final case class InvalidNumericBound(detail: String) extends ToolBuildError {
    def message: String = s"invalid numeric bound: $detail"
  }
  final case class RefinementTypeMismatch(refinement: String, actual: String) extends ToolBuildError {
    def message: String =
      s"$refinement refinement cannot apply to a $actual schema; the parameter's type " +
        s"resolves to a schema kind that has no $refinement restrictions to set"
  }
  final case class UnresolvedValueIsLiteral(name: String) extends ToolBuildError {
    def message: String =
      s"value-is literal for argument ${q(name)} was not resolved against its comparand type " +
        "during composition"
  }
  final case class InvalidTailOccurrenceBounds(name: String, min: Int, max: Int) extends ToolBuildError {
    def message: String =
      s"tail positional ${q(name)} has an impossible occurrence range: min $min is greater " +
        s"than max $max"
  }
  final case class RequiredPositionalAfterOptional(name: String) extends ToolBuildError {
    def message: String =
      s"required positional ${q(name)} cannot appear after an optional positional; optional " +
        "positionals must be trailing"
  }
  final case class FixedPositionalAfterTail(name: String) extends ToolBuildError {
    def message: String =
      s"fixed positional ${q(name)} cannot appear after a tail positional; the tail " +
        "positional must be the last positional"
  }
  final case class VecSurfaceConflict(name: String, reason: String) extends ToolBuildError {
    def message: String =
      s"the variadic `Seq[_]` parameter ${q(name)} cannot be re-projected after inherited-global " +
        s"de-projection: $reason"
  }
}

/**
 * Internal control-flow carrier for [[ToolBuildError]]s; thrown by the private
 * builders/validators and caught at the public `Either`-returning entry points.
 * Never escapes the `golem.tool` package.
 */
private[tool] final case class ToolBuildException(error: ToolBuildError)
    extends RuntimeException(error.message)
    with NoStackTrace
