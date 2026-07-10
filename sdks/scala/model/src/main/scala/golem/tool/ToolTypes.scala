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

// Leaf types of the `golem:tool/common@0.1.0` data model that carry no
// type-node indices, shared verbatim between the SDK-side extended model
// ([[ExtendedToolType]]) and the flat wire carriers ([[golem.tool.wire]]).

/** Documentation attached to commands, arguments, results and errors. */
final case class Doc(
  summary: String,
  description: String,
  examples: List[Example] = Nil
)

object Doc {
  val empty: Doc = Doc("", "")
}

/** A worked example (title + body) attached to a [[Doc]]. */
final case class Example(title: String, body: String)

/**
 * Behavioral hints surfaced to MCP and other LLM-facing surfaces. All four
 * follow the MCP convention; when absent the surface treats them as untrusted
 * defaults (`destructive: true`, `openWorld: true`, `readOnly: false`,
 * `idempotent: false`).
 */
final case class CommandAnnotations(
  readOnly: Boolean,
  destructive: Boolean,
  idempotent: Boolean,
  openWorld: Boolean
)

/** Resolution policy for a repeated key in a `repeatable-map` option. */
sealed trait DuplicateKeyPolicy extends Product with Serializable
object DuplicateKeyPolicy {

  /** A repeated key is a usage error. */
  case object Reject extends DuplicateKeyPolicy

  /** A repeated key takes the last supplied value. */
  case object LastWins extends DuplicateKeyPolicy
}

/** How a repeatable option accepts multiple occurrences. */
sealed trait Repetition extends Product with Serializable
object Repetition {

  /** `--inc a --inc b` */
  case object Repeated extends Repetition

  /** `--inc=a,b` */
  final case class Delimited(delimiter: Char) extends Repetition

  /** Both surface forms accepted. */
  final case class Either(delimiter: Char) extends Repetition
}

/** The shape of a flag: boolean presence or a counted flag. */
sealed trait FlagShape extends Product with Serializable
object FlagShape {
  final case class BoolFlag(shape: BoolFlagShape) extends FlagShape

  /** Counted flag (-vvv); optional max count. */
  final case class CountFlag(max: Option[Int]) extends FlagShape
}

final case class BoolFlagShape(
  default: Boolean,
  /** If true, `--no-<name>` is auto-synthesized. */
  negatable: Boolean
)

/** A flag declaration; flags carry no author-supplied value type. */
final case class FlagSpec(
  long: String,
  short: Option[Char],
  aliases: List[String],
  doc: Doc,
  shape: FlagShape,
  envVar: Option[String]
)

/** stdin/stdout stream declaration on a command body. */
final case class StreamSpec(
  doc: Doc,
  mime: List[String],
  required: Boolean
)

/** A named output formatter declared by a result spec. */
final case class Formatter(name: String, doc: Doc)

/** Whether an error case is a usage error or a runtime error. */
sealed trait ErrorKind extends Product with Serializable
object ErrorKind {
  case object UsageError   extends ErrorKind
  case object RuntimeError extends ErrorKind
}

/** Quantifier applied to one side of an `implies`/`forbids` constraint. */
sealed trait Quantifier extends Product with Serializable
object Quantifier {
  case object All extends Quantifier
  case object Any extends Quantifier
}
