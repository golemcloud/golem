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

package golem.runtime.annotations

import scala.annotation.StaticAnnotation

/**
 * Marks a trait as a Golem tool definition. The tool (root command) name
 * defaults to the kebab-cased trait name; `version` is the wire `tool.version`.
 */
final class toolDefinition(
  val name: String = "",
  val version: String = "0.0.0"
) extends StaticAnnotation

/**
 * Overrides a tool method's command name and declares command aliases. On a
 * subtree method the `name` also renames the grafted child root.
 */
final class command(
  val name: String = "",
  val aliases: Array[String] = Array()
) extends StaticAnnotation

/**
 * MCP-style behavioral hints for a command body. Defaults follow the MCP
 * untrusted-default convention.
 */
final class annotations(
  val readOnly: Boolean = false,
  val destructive: Boolean = true,
  val idempotent: Boolean = false,
  val openWorld: Boolean = true
) extends StaticAnnotation

/**
 * Refines how one method parameter is exposed as a command argument. The first
 * argument is the parameter's kebab-cased surface name (e.g. `"git-dir"` for a
 * `gitDir` parameter).
 *
 * Keys mirror the Rust SDK's `#[arg(...)]` attribute:
 *   - `scope`: `"global"`, `"positional"`, `"option"`, `"flag"`, `"tail"`
 *   - `kind`: `"flag"` / `"count-flag"` (arg kind) or `"file"` / `"dir"` /
 *     `"any"` (path kind)
 *   - `repeatable`: `"repeated"`, `"delimited"`, `"either"`
 *   - `direction`: `"input"`, `"output"`, `"inout"`
 *   - `default`, `min`, `max` and `bounds` accept literal values (string,
 *     number, bool, char, or `Array`/tuple of literals; large unsigned bounds
 *     may be given as decimal strings)
 */
final class arg(
  val name: String,
  val scope: String = "",
  val kind: String = "",
  val short: Char = '\u0000',
  val aliases: Array[String] = Array(),
  val env: String = "",
  val required: Boolean = false,
  val negatable: Boolean = false,
  val optionalScalar: Boolean = false,
  val repeatable: String = "",
  val delim: Char = '\u0000',
  val default: Any = null,
  val separator: String = "",
  val verbatim: Boolean = false,
  val acceptsStdio: Boolean = false,
  val regex: String = "",
  val minLength: Int = -1,
  val maxLength: Int = -1,
  val direction: String = "",
  val mime: Array[String] = null,
  val schemes: Array[String] = null,
  val min: Any = null,
  val max: Any = null,
  val bounds: Any = null,
  val unit: String = "",
  val doc: String = "",
  val valueName: String = ""
) extends StaticAnnotation

/**
 * Declares one cross-argument constraint on a command body. Exactly one of the
 * keyword parameters must be given per `@constraint` occurrence; repeat the
 * annotation for multiple constraints.
 *
 * Refs are argument surface-name strings or [[ValueIs]] values.
 */
final class constraint(
  val requiresAll: Array[Any] = null,
  val requiresAny: Array[Any] = null,
  val allOrNone: Array[Any] = null,
  val mutexGroups: Array[Array[Any]] = null,
  val implies: Implies = null,
  val forbids: Forbids = null
) extends StaticAnnotation

/**
 * A `value-is` constraint reference: satisfied when the named argument's value
 * equals the literal.
 */
final case class ValueIs(name: String, value: Any)

/**
 * An `implies` constraint: when the lhs holds (under `lhsQuant`), the rhs must
 * hold (under `rhsQuant`). `lhs`/`rhs` are a single ref or an `Array` of refs;
 * quantifiers are `"all"` or `"any"`.
 */
final case class Implies(
  lhs: Any,
  rhs: Any,
  lhsQuant: String = "all",
  rhsQuant: String = "all"
)

/**
 * A `forbids` constraint: when the lhs holds (under `lhsQuant`), none of the
 * rhs refs may hold.
 */
final case class Forbids(
  lhs: Any,
  rhs: Any,
  lhsQuant: String = "all"
)

/**
 * Declares the result formatters of a command. Without formatters a single
 * `"default"` formatter is synthesized; without an explicit `default` the first
 * formatter is the default.
 */
final class result(
  val formatters: Array[String] = Array(),
  val default: String = ""
) extends StaticAnnotation

/**
 * Declares the error kind and exit code of one case of a tool error enum.
 * `kind` is `"usage-error"` (or `"usage"`) or `"runtime-error"` (or
 * `"runtime"`).
 */
final class error(
  val kind: String,
  val exitCode: Int
) extends StaticAnnotation

/**
 * Attaches a worked example to a tool trait, a tool method, or a tool error
 * case. Repeatable.
 */
final class example(
  val body: String,
  val title: String = ""
) extends StaticAnnotation
