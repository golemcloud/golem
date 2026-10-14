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

import golem.schema.{
  PathDirection,
  PathKind,
  PathSpec,
  SchemaGraph,
  SchemaType,
  SchemaTypeBody,
  SchemaValue,
  TextRestrictions,
  t
}
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
              None,
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

  /**
   * The Scala SDK representation of
   * `test-data/gol-40/rich-tool-conformance-v1.json`.
   */
  def gol40RichTool: ExtendedToolType = {
    val defs = ListMap(
      "ArtifactRequest" -> golem.schema.SchemaTypeDef(
        t.record(List(t.field("source", t.string), t.field("labels", t.map(t.string, t.string))))
      ),
      "ArtifactReport" -> golem.schema.SchemaTypeDef(
        t.record(
          List(
            t.field("artifactId", t.u64),
            t.field(
              "digest",
              SchemaType(
                SchemaTypeBody.TextType(
                  TextRestrictions(regex = Some("^[a-f0-9]+$"), minLength = Some(8), maxLength = Some(64))
                )
              )
            ),
            t.field("labels", t.map(t.string, t.string)),
            t.field("warnings", t.list(t.string))
          )
        )
      ),
      "ValidationFailure" -> golem.schema.SchemaTypeDef(
        t.record(
          List(
            t.field("field", t.string),
            t.field("reason", t.string),
            t.field("retryable", t.bool)
          )
        )
      )
    )
    def graph(root: SchemaType): SchemaGraph                                             = SchemaGraph(defs, root)
    def enumGraph(cases: String*): SchemaGraph                                           = graph(t.`enum`(cases.toList))
    def ref(id: String): SchemaGraph                                                     = graph(t.ref(id))
    def d(summary: String, description: String = "", examples: List[Example] = Nil): Doc =
      Doc(summary, description, examples)

    val rootGlobals = ExtendedGlobals(
      options = List(
        ExtendedOptionSpec(
          "region",
          Some('r'),
          List("location"),
          d("Execution region", "Inherited by every executable descendant."),
          Some("REGION"),
          ExtendedOptionShape.Scalar(enumGraph("eu-west-1", "us-east-1")),
          Some(SchemaValue.EnumValue(0)),
          required = false,
          Some("ARTIFACT_REGION")
        )
      ),
      flags = List(
        FlagSpec(
          "trace",
          Some('t'),
          List("diagnostics"),
          d("Emit trace details"),
          FlagShape.BoolFlag(BoolFlagShape(default = false, negatable = true)),
          None
        )
      )
    )

    val renderGlobals = ExtendedGlobals(
      options = List(
        ExtendedOptionSpec(
          "profile",
          Some('p'),
          Nil,
          d("Build profile", "Inherited by render descendants."),
          Some("PROFILE"),
          ExtendedOptionShape.Scalar(enumGraph("debug", "release")),
          Some(SchemaValue.EnumValue(1)),
          required = false,
          None
        )
      )
    )

    val renderBody = ExtendedCommandBody(
      positionals = ExtendedPositionals(
        fixed = List(
          ExtendedPositional(
            "request",
            d("Artifact request"),
            Some("REQUEST"),
            ref("ArtifactRequest"),
            None,
            true,
            false
          )
        ),
        tail = Some(
          ExtendedTailPositional(
            "inputs",
            d("Input modules", "One to three source modules."),
            Some("INPUT"),
            graph(
              SchemaType(
                SchemaTypeBody.PathType(
                  PathSpec(PathDirection.Input, PathKind.File, allowedExtensions = Some(List("wasm", "wat")))
                )
              )
            ),
            min = 1,
            max = Some(3),
            separator = Some("--"),
            verbatim = true,
            acceptsStdio = false
          )
        )
      ),
      options = List(
        ExtendedOptionSpec(
          "format",
          Some('f'),
          List("output-format"),
          d("Report format"),
          Some("FORMAT"),
          ExtendedOptionShape.Scalar(enumGraph("json", "text")),
          Some(SchemaValue.EnumValue(0)),
          required = false,
          None
        ),
        ExtendedOptionSpec(
          "tag",
          None,
          List("label"),
          d("Tags", "May be repeated or comma-delimited."),
          Some("TAG"),
          ExtendedOptionShape.RepeatableList(ExtendedRepeatableListShape(Repetition.Either(','), graph(t.string))),
          Some(SchemaValue.ListValue(Nil)),
          required = false,
          None
        ),
        ExtendedOptionSpec(
          "define",
          Some('D'),
          Nil,
          d("Numeric definitions", "Duplicate keys are rejected."),
          Some("KEY=VALUE"),
          ExtendedOptionShape.RepeatableMap(
            ExtendedRepeatableMapShape(Repetition.Repeated, graph(t.map(t.string, t.s64)), DuplicateKeyPolicy.Reject)
          ),
          Some(SchemaValue.MapValue(Nil)),
          required = false,
          None
        ),
        ExtendedOptionSpec(
          "color",
          None,
          Nil,
          d("Color mode", "Bare presence resolves to the default."),
          Some("WHEN"),
          ExtendedOptionShape.OptionalScalar(enumGraph("auto", "always", "never")),
          Some(SchemaValue.EnumValue(0)),
          required = false,
          None
        )
      ),
      flags = List(
        FlagSpec(
          "checksum",
          Some('c'),
          List("digest"),
          d("Include digest"),
          FlagShape.BoolFlag(BoolFlagShape(default = false, negatable = true)),
          None
        ),
        FlagSpec("verbose", Some('v'), Nil, d("Verbosity"), FlagShape.CountFlag(Some(3)), None)
      ),
      constraints = List(
        ExtendedConstraint.RequiresAll(List(ExtendedRef.Present("checksum"), ExtendedRef.Present("format"))),
        ExtendedConstraint.Implies(
          ExtendedImpliesC(
            Quantifier.All,
            List(
              ExtendedRef.ValueIs(
                ExtendedValueIsRef("profile", ExtendedValueIsLiteral.Resolved(SchemaValue.EnumValue(1)))
              )
            ),
            Quantifier.Any,
            List(ExtendedRef.Present("tag"))
          )
        ),
        ExtendedConstraint.Forbids(
          ExtendedForbidsC(
            Quantifier.Any,
            List(
              ExtendedRef.ValueIs(
                ExtendedValueIsRef("format", ExtendedValueIsLiteral.Resolved(SchemaValue.EnumValue(1)))
              )
            ),
            List(ExtendedRef.Present("define"))
          )
        )
      ),
      stdin = Some(StreamSpec(d("Optional module bytes"), List("application/wasm"), required = false)),
      stdout = Some(StreamSpec(d("Progress output"), List("text/plain; charset=utf-8"), required = true)),
      stderr = Some(StreamSpec(d("Diagnostic bytes"), List("application/octet-stream"), required = false)),
      result = Some(
        ExtendedResultSpec(
          ref("ArtifactReport"),
          d("Artifact report"),
          List(Formatter("json", d("JSON report")), Formatter("table", d("Tabular report"))),
          "json"
        )
      ),
      errors = List(
        ExtendedErrorCase(
          "invalid-request",
          d("Request validation failed"),
          ErrorKind.UsageError,
          2,
          Some(ref("ValidationFailure"))
        ),
        ExtendedErrorCase(
          "render-failed",
          d("Renderer failed"),
          ErrorKind.RuntimeError,
          70,
          Some(graph(t.record(List(t.field("stage", t.string), t.field("code", t.u32)))))
        )
      ),
      annotations =
        Some(CommandAnnotations(readOnly = false, destructive = false, idempotent = true, openWorld = false))
    )

    val statusBody = ExtendedCommandBody(
      positionals = ExtendedPositionals(
        fixed =
          List(ExtendedPositional("artifact-id", d("Artifact identifier"), Some("ID"), graph(t.u64), None, true, false))
      ),
      options = Nil,
      flags = Nil,
      constraints = Nil,
      stdin = None,
      stdout = None,
      stderr = None,
      result = Some(
        ExtendedResultSpec(
          enumGraph("queued", "ready", "failed"),
          d("Current status"),
          List(Formatter("json", d("JSON status"))),
          "json"
        )
      ),
      errors = Nil,
      annotations = Some(CommandAnnotations(readOnly = true, destructive = false, idempotent = true, openWorld = false))
    )

    ExtendedToolType(
      "1.0.0",
      Vector(
        ExtendedCommandNode(
          "artifact",
          List("art"),
          d(
            "Build and inspect artifacts",
            "A deliberately asymmetric conformance tool.",
            List(Example("Render", "artifact --region eu-west-1 render src/main.wasm --format json"))
          ),
          rootGlobals,
          List(1),
          None
        ),
        ExtendedCommandNode(
          "render",
          List("build"),
          d(
            "Render one artifact",
            "Build an artifact and return a structured report.",
            List(
              Example(
                "Release build",
                "artifact render src/main.wasm --format json --tag release --define opt=3 --checksum"
              )
            )
          ),
          renderGlobals,
          List(2),
          Some(renderBody)
        ),
        ExtendedCommandNode(
          "status",
          List("show"),
          d("Inspect render status"),
          ExtendedGlobals.empty,
          Nil,
          Some(statusBody)
        )
      ),
      requiresFilesystem = false
    )
  }
}
