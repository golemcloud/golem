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

package golem.runtime.tool

import golem.schema.{t, SchemaGraph, SchemaValue}
import golem.tool._

import scala.collection.immutable.ListMap

/**
 * Shared `ExtendedToolType` builders for the tool registry / guest / interop
 * specs.
 */
object ToolTestFixtures {

  def doc(summary: String): Doc = Doc(summary, "")

  def strGraph: SchemaGraph = SchemaGraph(ListMap.empty, t.string)
  def u32Graph: SchemaGraph = SchemaGraph(ListMap.empty, t.u32)

  /**
   * Minimal valid tool: a root command with no globals, no subcommands, no
   * body.
   */
  def leafTool(name: String): ExtendedToolType =
    ExtendedToolType(
      "0.1.0",
      Vector(ExtendedCommandNode(name, Nil, doc(""), ExtendedGlobals.empty, Nil, None))
    )

  /**
   * A tool whose root body takes a single string positional (used for invoke
   * roundtrips).
   */
  def echoTool(name: String): ExtendedToolType =
    ExtendedToolType(
      "0.1.0",
      Vector(
        ExtendedCommandNode(
          name,
          Nil,
          doc("echoes its input"),
          ExtendedGlobals.empty,
          Nil,
          Some(
            ExtendedCommandBody(
              ExtendedPositionals(
                List(ExtendedPositional("input", doc("the input"), None, strGraph, None, true, false)),
                None
              ),
              Nil,
              Nil,
              Nil,
              None,
              None,
              None,
              Nil,
              None
            )
          )
        )
      )
    )

  /**
   * A tool exercising every wire-carrier shape: globals (scalar option with
   * short/alias/env/default, negatable bool flag, capped count flag), a
   * subcommand body with fixed + tail positionals, all four option shapes, all
   * six constraint kinds (`present` and `value-is` refs), stdin/stdout stream
   * specs, a result spec with formatters, error cases with and without payload,
   * and command annotations.
   */
  def richTool(name: String): ExtendedToolType =
    ExtendedToolType(
      "0.2.0",
      Vector(
        ExtendedCommandNode(
          name,
          List("rt"),
          Doc("rich root", "root description", List(Example("basic", s"$name run input"))),
          ExtendedGlobals(
            List(
              ExtendedOptionSpec(
                "level",
                Some('l'),
                List("lvl"),
                doc("global level"),
                Some("N"),
                ExtendedOptionShape.Scalar(u32Graph),
                Some(SchemaValue.U32Value(1)),
                false,
                Some("RICH_LEVEL")
              )
            ),
            List(
              FlagSpec(
                "quiet",
                Some('q'),
                Nil,
                doc("quiet"),
                FlagShape.BoolFlag(BoolFlagShape(default = false, negatable = true)),
                None
              ),
              FlagSpec(
                "verbose",
                Some('v'),
                Nil,
                doc("verbose"),
                FlagShape.CountFlag(Some(3)),
                Some("RICH_VERBOSE")
              )
            )
          ),
          List(1),
          None
        ),
        ExtendedCommandNode(
          "run",
          List("r"),
          doc("run it"),
          ExtendedGlobals.empty,
          Nil,
          Some(
            ExtendedCommandBody(
              ExtendedPositionals(
                List(
                  ExtendedPositional("input", doc("input"), Some("INPUT"), strGraph, None, true, false),
                  ExtendedPositional(
                    "mode",
                    doc("mode"),
                    None,
                    strGraph,
                    Some(SchemaValue.StringValue("fast")),
                    false,
                    false
                  )
                ),
                Some(
                  ExtendedTailPositional(
                    "files",
                    doc("files"),
                    Some("FILE"),
                    strGraph,
                    0,
                    Some(10),
                    Some("--"),
                    verbatim = true,
                    acceptsStdio = true
                  )
                )
              ),
              List(
                ExtendedOptionSpec(
                  "config",
                  Some('c'),
                  Nil,
                  doc("config"),
                  None,
                  ExtendedOptionShape.RepeatableMap(
                    ExtendedRepeatableMapShape(
                      Repetition.Delimited(','),
                      SchemaGraph(ListMap.empty, t.map(t.string, t.string)),
                      DuplicateKeyPolicy.LastWins
                    )
                  ),
                  None,
                  false,
                  None
                ),
                ExtendedOptionSpec(
                  "exclude",
                  Some('x'),
                  Nil,
                  doc("exclude"),
                  None,
                  ExtendedOptionShape.RepeatableList(
                    ExtendedRepeatableListShape(Repetition.Either(','), strGraph)
                  ),
                  None,
                  false,
                  None
                ),
                ExtendedOptionSpec(
                  "output",
                  None,
                  List("out"),
                  doc("output"),
                  Some("PATH"),
                  ExtendedOptionShape.Scalar(strGraph),
                  Some(SchemaValue.StringValue("out")),
                  false,
                  None
                ),
                ExtendedOptionSpec(
                  "opt-level",
                  None,
                  Nil,
                  doc("optimization level"),
                  None,
                  ExtendedOptionShape.OptionalScalar(u32Graph),
                  None,
                  false,
                  None
                )
              ),
              List(
                FlagSpec(
                  "force",
                  Some('f'),
                  Nil,
                  doc("force"),
                  FlagShape.BoolFlag(BoolFlagShape(default = false, negatable = false)),
                  None
                )
              ),
              List(
                ExtendedConstraint.RequiresAll(List(ExtendedRef.Present("input"))),
                ExtendedConstraint.AllOrNone(List(ExtendedRef.Present("force"), ExtendedRef.Present("output"))),
                ExtendedConstraint.RequiresAny(List(ExtendedRef.Present("input"), ExtendedRef.Present("files"))),
                ExtendedConstraint.MutexGroups(
                  List(
                    ExtendedRefGroup(List(ExtendedRef.Present("force"))),
                    ExtendedRefGroup(List(ExtendedRef.Present("exclude")))
                  )
                ),
                ExtendedConstraint.Implies(
                  ExtendedImpliesC(
                    Quantifier.All,
                    List(ExtendedRef.Present("force")),
                    Quantifier.Any,
                    List(
                      ExtendedRef.ValueIs(
                        ExtendedValueIsRef(
                          "output",
                          ExtendedValueIsLiteral.Resolved(SchemaValue.StringValue("out"))
                        )
                      )
                    )
                  )
                ),
                ExtendedConstraint.Forbids(
                  ExtendedForbidsC(
                    Quantifier.Any,
                    List(ExtendedRef.Present("quiet")),
                    List(ExtendedRef.Present("verbose"))
                  )
                )
              ),
              Some(StreamSpec(doc("stdin"), List("text/plain"), required = false)),
              Some(StreamSpec(doc("stdout"), List("application/json"), required = true)),
              Some(
                ExtendedResultSpec(
                  strGraph,
                  doc("result"),
                  List(Formatter("json", doc("json output")), Formatter("plain", doc("plain output"))),
                  "json"
                )
              ),
              List(
                ExtendedErrorCase("not-found", doc("missing"), ErrorKind.RuntimeError, 2, Some(strGraph)),
                ExtendedErrorCase("bad-usage", doc("bad usage"), ErrorKind.UsageError, 64, None)
              ),
              Some(CommandAnnotations(readOnly = false, destructive = true, idempotent = false, openWorld = true))
            )
          )
        )
      )
    )
}
