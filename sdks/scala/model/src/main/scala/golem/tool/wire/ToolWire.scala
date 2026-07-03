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

package golem.tool.wire

import golem.schema.wire.{WitSchemaGraph, WitSchemaValueTree, WitTypedSchemaValue}
import golem.tool._

// Flat carrier ADT mirroring `golem:tool/common@0.1.0` exactly. A tool owns a
// single `schema-graph` type-node pool (the [[WitTool.schema]] field); command
// bodies reference entries in it by type-node index (`Int`), the same way the
// agent model references its per-agent graph. Metadata-time values
// (option/positional defaults, the literal side of `value-is` refs) are
// [[WitSchemaValueTree]]s interpreted against the referenced type node.
//
// This is host-agnostic plain Scala (cross JVM+JS); the JS layer maps this ADT
// to/from the wasm-rquickjs facades mechanically. Types with no indices
// ([[Doc]], [[FlagSpec]], [[StreamSpec]], ...) are shared with the extended
// model and used here unchanged.

final case class WitTool(
  version: String,
  commands: WitCommandTree,
  schema: WitSchemaGraph
)

/** Flattened command hierarchy; always non-empty, root at index 0. */
final case class WitCommandTree(nodes: Vector[WitCommandNode])

final case class WitCommandNode(
  name: String,
  aliases: List[String],
  doc: Doc,
  globals: WitGlobals,
  subcommands: List[Int],
  body: Option[WitCommandBody]
)

final case class WitGlobals(
  options: List[WitOptionSpec],
  flags: List[FlagSpec]
)

final case class WitCommandBody(
  positionals: WitPositionals,
  options: List[WitOptionSpec],
  flags: List[FlagSpec],
  constraints: List[WitConstraint],
  stdin: Option[StreamSpec],
  stdout: Option[StreamSpec],
  result: Option[WitResultSpec],
  errors: List[WitErrorCase],
  annotations: Option[CommandAnnotations]
)

final case class WitPositionals(
  fixed: List[WitPositional],
  tail: Option[WitTailPositional]
)

final case class WitPositional(
  name: String,
  doc: Doc,
  valueName: Option[String],
  tpe: Int,
  default: Option[WitSchemaValueTree],
  required: Boolean,
  acceptsStdio: Boolean
)

final case class WitTailPositional(
  name: String,
  doc: Doc,
  valueName: Option[String],
  itemType: Int,
  min: Int,
  max: Option[Int],
  separator: Option[String],
  verbatim: Boolean,
  acceptsStdio: Boolean
)

final case class WitOptionSpec(
  long: String,
  short: Option[Char],
  aliases: List[String],
  doc: Doc,
  valueName: Option[String],
  shape: WitOptionShape,
  default: Option[WitSchemaValueTree],
  required: Boolean,
  envVar: Option[String]
)

sealed trait WitOptionShape extends Product with Serializable
object WitOptionShape {

  /** Required value: `--opt VALUE` or `--opt=VALUE`. */
  final case class Scalar(tpe: Int) extends WitOptionShape

  /** Bare presence collapses to `default`; with value parses normally. */
  final case class OptionalScalar(tpe: Int) extends WitOptionShape

  /**
   * Repeatable scalar option; the collected value is a `list` of the item type.
   */
  final case class RepeatableList(shape: WitRepeatableListShape) extends WitOptionShape

  /** Repeatable key-value option; the collected value is a `map` node. */
  final case class RepeatableMap(shape: WitRepeatableMapShape) extends WitOptionShape
}

final case class WitRepeatableListShape(
  repetition: Repetition,
  itemType: Int
)

final case class WitRepeatableMapShape(
  repetition: Repetition,
  mapType: Int,
  duplicateKeyPolicy: DuplicateKeyPolicy
)

sealed trait WitRef extends Product with Serializable
object WitRef {
  final case class Present(name: String)           extends WitRef
  final case class ValueIs(valueIs: WitValueIsRef) extends WitRef
}

final case class WitValueIsRef(
  name: String,
  value: WitSchemaValueTree
)

sealed trait WitConstraint extends Product with Serializable
object WitConstraint {
  final case class RequiresAll(refs: List[WitRef])        extends WitConstraint
  final case class AllOrNone(refs: List[WitRef])          extends WitConstraint
  final case class RequiresAny(refs: List[WitRef])        extends WitConstraint
  final case class MutexGroups(groups: List[WitRefGroup]) extends WitConstraint
  final case class Implies(impliesC: WitImpliesC)         extends WitConstraint
  final case class Forbids(forbidsC: WitForbidsC)         extends WitConstraint
}

final case class WitRefGroup(refs: List[WitRef])

final case class WitImpliesC(
  lhsQuant: Quantifier,
  lhs: List[WitRef],
  rhsQuant: Quantifier,
  rhs: List[WitRef]
)

final case class WitForbidsC(
  lhsQuant: Quantifier,
  lhs: List[WitRef],
  rhs: List[WitRef]
)

final case class WitResultSpec(
  tpe: Int,
  doc: Doc,
  formatters: List[Formatter],
  defaultFormatter: String
)

final case class WitErrorCase(
  name: String,
  doc: Doc,
  kind: ErrorKind,
  exitCode: Int,
  payload: Option[Int]
)

/**
 * The `tool-error` side of the invocation contract shared between guest and
 * host. [[WitToolError.CustomError]] mirrors `agent-error::custom-error`: the
 * payload is a self-contained `typed-schema-value` carrying the error value,
 * shaped so its root type matches one of the body's declared `error-case`
 * payload types.
 */
sealed trait WitToolError extends Product with Serializable
object WitToolError {
  final case class InvalidToolName(name: String)             extends WitToolError
  final case class InvalidCommandPath(path: List[String])    extends WitToolError
  final case class InvalidInput(message: String)             extends WitToolError
  final case class ConstraintViolation(message: String)      extends WitToolError
  final case class InvalidResult(message: String)            extends WitToolError
  final case class CustomError(payload: WitTypedSchemaValue) extends WitToolError
}
