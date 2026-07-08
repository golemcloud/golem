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

package golem.host.js.tool

import golem.host.js.JsShape
import golem.host.js.schema.{JsSchemaGraph, JsSchemaValueTree, JsTypedSchemaValue}

import scala.scalajs.js
import scala.scalajs.js.annotation.JSName

// ---------------------------------------------------------------------------
// `golem:tool/common@0.1.0` JS facades (wasm-rquickjs shape).
//
// Mirrors `golem_tool_0_1_0_common.d.ts`: `{ tag, val }` tagged unions, plain
// string enums for `duplicate-key-policy` / `quantifier` / `error-kind`,
// `undefined` (`js.UndefOr`) for absent options, camelCase record fields, and
// the trailing-underscore `default_` field names emitted by wasm-rquickjs for
// the reserved word `default`. Schema positions reuse the
// `golem:core/types@2.0.0` facades from [[golem.host.js.schema]]; the
// `wasi:io/streams` handles carried by the invocation contract are opaque.
// ---------------------------------------------------------------------------

/** Opaque `wasi:io/streams@0.2.3` `input-stream` resource handle. */
@js.native
sealed trait JsWasiInputStream extends js.Object

/** Opaque `wasi:io/streams@0.2.3` `output-stream` resource handle. */
@js.native
sealed trait JsWasiOutputStream extends js.Object

// === Documentation ===

@js.native
sealed trait JsExample extends js.Object {
  def title: String = js.native
  def body: String  = js.native
}
object JsExample {
  def apply(title: String, body: String): JsExample =
    js.Dynamic.literal("title" -> title, "body" -> body).asInstanceOf[JsExample]
}

@js.native
sealed trait JsDoc extends js.Object {
  def summary: String               = js.native
  def description: String           = js.native
  def examples: js.Array[JsExample] = js.native
}
object JsDoc {
  def apply(summary: String, description: String, examples: js.Array[JsExample]): JsDoc =
    js.Dynamic
      .literal("summary" -> summary, "description" -> description, "examples" -> examples)
      .asInstanceOf[JsDoc]
}

// === Behavioral annotations ===

@js.native
sealed trait JsCommandAnnotations extends js.Object {
  def readOnly: Boolean    = js.native
  def destructive: Boolean = js.native
  def idempotent: Boolean  = js.native
  def openWorld: Boolean   = js.native
}
object JsCommandAnnotations {
  def apply(readOnly: Boolean, destructive: Boolean, idempotent: Boolean, openWorld: Boolean): JsCommandAnnotations =
    js.Dynamic
      .literal(
        "readOnly"    -> readOnly,
        "destructive" -> destructive,
        "idempotent"  -> idempotent,
        "openWorld"   -> openWorld
      )
      .asInstanceOf[JsCommandAnnotations]
}

// === Option shapes ===

/** `duplicate-key-policy` is a plain string enum. */
object JsDuplicateKeyPolicy {
  val reject: String   = "reject"
  val lastWins: String = "last-wins"
}

@js.native
sealed trait JsRepetition extends js.Object {
  def tag: String = js.native
}
object JsRepetition {
  def repeated: JsRepetition                     = JsShape.tagOnly[JsRepetition]("repeated")
  def delimited(delimiter: String): JsRepetition = JsShape.tagged[JsRepetition]("delimited", delimiter)
  def either(delimiter: String): JsRepetition    = JsShape.tagged[JsRepetition]("either", delimiter)
}

@js.native
sealed trait JsRepeatableListShape extends js.Object {
  def repetition: JsRepetition = js.native
  def itemType: Int            = js.native
}
object JsRepeatableListShape {
  def apply(repetition: JsRepetition, itemType: Int): JsRepeatableListShape =
    js.Dynamic.literal("repetition" -> repetition, "itemType" -> itemType).asInstanceOf[JsRepeatableListShape]
}

@js.native
sealed trait JsRepeatableMapShape extends js.Object {
  def repetition: JsRepetition   = js.native
  def mapType: Int               = js.native
  def duplicateKeyPolicy: String = js.native
}
object JsRepeatableMapShape {
  def apply(repetition: JsRepetition, mapType: Int, duplicateKeyPolicy: String): JsRepeatableMapShape =
    js.Dynamic
      .literal("repetition" -> repetition, "mapType" -> mapType, "duplicateKeyPolicy" -> duplicateKeyPolicy)
      .asInstanceOf[JsRepeatableMapShape]
}

@js.native
sealed trait JsOptionShape extends js.Object {
  def tag: String = js.native
}
object JsOptionShape {
  def scalar(tpe: Int): JsOptionShape                             = JsShape.tagged[JsOptionShape]("scalar", tpe)
  def optionalScalar(tpe: Int): JsOptionShape                     = JsShape.tagged[JsOptionShape]("optional-scalar", tpe)
  def repeatableList(shape: JsRepeatableListShape): JsOptionShape =
    JsShape.tagged[JsOptionShape]("repeatable-list", shape)
  def repeatableMap(shape: JsRepeatableMapShape): JsOptionShape =
    JsShape.tagged[JsOptionShape]("repeatable-map", shape)
}

// === Flag shapes ===

@js.native
sealed trait JsBoolFlagShape extends js.Object {
  @JSName("default_")
  def default: Boolean   = js.native
  def negatable: Boolean = js.native
}
object JsBoolFlagShape {
  def apply(default: Boolean, negatable: Boolean): JsBoolFlagShape =
    js.Dynamic.literal("default_" -> default, "negatable" -> negatable).asInstanceOf[JsBoolFlagShape]
}

@js.native
sealed trait JsFlagShape extends js.Object {
  def tag: String = js.native
}
object JsFlagShape {
  def boolFlag(shape: JsBoolFlagShape): JsFlagShape = JsShape.tagged[JsFlagShape]("bool-flag", shape)
  def countFlag(max: js.UndefOr[Int]): JsFlagShape  =
    JsShape.taggedOptional[JsFlagShape]("count-flag", max.map(v => v: js.Any))
}

// === Constraints ===

@js.native
sealed trait JsValueIsRef extends js.Object {
  def name: String             = js.native
  def value: JsSchemaValueTree = js.native
}
object JsValueIsRef {
  def apply(name: String, value: JsSchemaValueTree): JsValueIsRef =
    js.Dynamic.literal("name" -> name, "value" -> value).asInstanceOf[JsValueIsRef]
}

@js.native
sealed trait JsRef extends js.Object {
  def tag: String = js.native
}
object JsRef {
  def present(name: String): JsRef          = JsShape.tagged[JsRef]("present", name)
  def valueIs(valueIs: JsValueIsRef): JsRef = JsShape.tagged[JsRef]("value-is", valueIs)
}

@js.native
sealed trait JsRefGroup extends js.Object {
  def refs: js.Array[JsRef] = js.native
}
object JsRefGroup {
  def apply(refs: js.Array[JsRef]): JsRefGroup =
    js.Dynamic.literal("refs" -> refs).asInstanceOf[JsRefGroup]
}

/** `quantifier` is a plain string enum. */
object JsQuantifier {
  val all: String = "all"
  val any: String = "any"
}

@js.native
sealed trait JsImpliesC extends js.Object {
  def lhsQuant: String     = js.native
  def lhs: js.Array[JsRef] = js.native
  def rhsQuant: String     = js.native
  def rhs: js.Array[JsRef] = js.native
}
object JsImpliesC {
  def apply(lhsQuant: String, lhs: js.Array[JsRef], rhsQuant: String, rhs: js.Array[JsRef]): JsImpliesC =
    js.Dynamic
      .literal("lhsQuant" -> lhsQuant, "lhs" -> lhs, "rhsQuant" -> rhsQuant, "rhs" -> rhs)
      .asInstanceOf[JsImpliesC]
}

@js.native
sealed trait JsForbidsC extends js.Object {
  def lhsQuant: String     = js.native
  def lhs: js.Array[JsRef] = js.native
  def rhs: js.Array[JsRef] = js.native
}
object JsForbidsC {
  def apply(lhsQuant: String, lhs: js.Array[JsRef], rhs: js.Array[JsRef]): JsForbidsC =
    js.Dynamic.literal("lhsQuant" -> lhsQuant, "lhs" -> lhs, "rhs" -> rhs).asInstanceOf[JsForbidsC]
}

@js.native
sealed trait JsConstraint extends js.Object {
  def tag: String = js.native
}
object JsConstraint {
  def requiresAll(refs: js.Array[JsRef]): JsConstraint        = JsShape.tagged[JsConstraint]("requires-all", refs)
  def allOrNone(refs: js.Array[JsRef]): JsConstraint          = JsShape.tagged[JsConstraint]("all-or-none", refs)
  def requiresAny(refs: js.Array[JsRef]): JsConstraint        = JsShape.tagged[JsConstraint]("requires-any", refs)
  def mutexGroups(groups: js.Array[JsRefGroup]): JsConstraint =
    JsShape.tagged[JsConstraint]("mutex-groups", groups)
  def implies(impliesC: JsImpliesC): JsConstraint = JsShape.tagged[JsConstraint]("implies", impliesC)
  def forbids(forbidsC: JsForbidsC): JsConstraint = JsShape.tagged[JsConstraint]("forbids", forbidsC)
}

// === Positionals ===

@js.native
sealed trait JsPositional extends js.Object {
  def name: String                  = js.native
  def doc: JsDoc                    = js.native
  def valueName: js.UndefOr[String] = js.native
  @JSName("type")
  def tpe: Int = js.native
  @JSName("default_")
  def default: js.UndefOr[JsSchemaValueTree] = js.native
  def required: Boolean                      = js.native
  def acceptsStdio: Boolean                  = js.native
}
object JsPositional {
  def apply(
    name: String,
    doc: JsDoc,
    valueName: js.UndefOr[String],
    tpe: Int,
    default: js.UndefOr[JsSchemaValueTree],
    required: Boolean,
    acceptsStdio: Boolean
  ): JsPositional = {
    val o = js.Dynamic.literal(
      "name"         -> name,
      "doc"          -> doc,
      "type"         -> tpe,
      "required"     -> required,
      "acceptsStdio" -> acceptsStdio
    )
    valueName.foreach(v => o.updateDynamic("valueName")(v))
    default.foreach(v => o.updateDynamic("default_")(v))
    o.asInstanceOf[JsPositional]
  }
}

@js.native
sealed trait JsTailPositional extends js.Object {
  def name: String                  = js.native
  def doc: JsDoc                    = js.native
  def valueName: js.UndefOr[String] = js.native
  def itemType: Int                 = js.native
  def min: Int                      = js.native
  def max: js.UndefOr[Int]          = js.native
  def separator: js.UndefOr[String] = js.native
  def verbatim: Boolean             = js.native
  def acceptsStdio: Boolean         = js.native
}
object JsTailPositional {
  def apply(
    name: String,
    doc: JsDoc,
    valueName: js.UndefOr[String],
    itemType: Int,
    min: Int,
    max: js.UndefOr[Int],
    separator: js.UndefOr[String],
    verbatim: Boolean,
    acceptsStdio: Boolean
  ): JsTailPositional = {
    val o = js.Dynamic.literal(
      "name"         -> name,
      "doc"          -> doc,
      "itemType"     -> itemType,
      "min"          -> min,
      "verbatim"     -> verbatim,
      "acceptsStdio" -> acceptsStdio
    )
    valueName.foreach(v => o.updateDynamic("valueName")(v))
    max.foreach(v => o.updateDynamic("max")(v))
    separator.foreach(v => o.updateDynamic("separator")(v))
    o.asInstanceOf[JsTailPositional]
  }
}

@js.native
sealed trait JsPositionals extends js.Object {
  def fixed: js.Array[JsPositional]      = js.native
  def tail: js.UndefOr[JsTailPositional] = js.native
}
object JsPositionals {
  def apply(fixed: js.Array[JsPositional], tail: js.UndefOr[JsTailPositional]): JsPositionals = {
    val o = js.Dynamic.literal("fixed" -> fixed)
    tail.foreach(v => o.updateDynamic("tail")(v))
    o.asInstanceOf[JsPositionals]
  }
}

// === Options and flags ===

@js.native
sealed trait JsOptionSpec extends js.Object {
  def long: String                  = js.native
  def short: js.UndefOr[String]     = js.native
  def aliases: js.Array[String]     = js.native
  def doc: JsDoc                    = js.native
  def valueName: js.UndefOr[String] = js.native
  def shape: JsOptionShape          = js.native
  @JSName("default_")
  def default: js.UndefOr[JsSchemaValueTree] = js.native
  def required: Boolean                      = js.native
  def envVar: js.UndefOr[String]             = js.native
}
object JsOptionSpec {
  def apply(
    long: String,
    short: js.UndefOr[String],
    aliases: js.Array[String],
    doc: JsDoc,
    valueName: js.UndefOr[String],
    shape: JsOptionShape,
    default: js.UndefOr[JsSchemaValueTree],
    required: Boolean,
    envVar: js.UndefOr[String]
  ): JsOptionSpec = {
    val o = js.Dynamic.literal(
      "long"     -> long,
      "aliases"  -> aliases,
      "doc"      -> doc,
      "shape"    -> shape,
      "required" -> required
    )
    short.foreach(v => o.updateDynamic("short")(v))
    valueName.foreach(v => o.updateDynamic("valueName")(v))
    default.foreach(v => o.updateDynamic("default_")(v))
    envVar.foreach(v => o.updateDynamic("envVar")(v))
    o.asInstanceOf[JsOptionSpec]
  }
}

@js.native
sealed trait JsFlagSpec extends js.Object {
  def long: String               = js.native
  def short: js.UndefOr[String]  = js.native
  def aliases: js.Array[String]  = js.native
  def doc: JsDoc                 = js.native
  def shape: JsFlagShape         = js.native
  def envVar: js.UndefOr[String] = js.native
}
object JsFlagSpec {
  def apply(
    long: String,
    short: js.UndefOr[String],
    aliases: js.Array[String],
    doc: JsDoc,
    shape: JsFlagShape,
    envVar: js.UndefOr[String]
  ): JsFlagSpec = {
    val o = js.Dynamic.literal("long" -> long, "aliases" -> aliases, "doc" -> doc, "shape" -> shape)
    short.foreach(v => o.updateDynamic("short")(v))
    envVar.foreach(v => o.updateDynamic("envVar")(v))
    o.asInstanceOf[JsFlagSpec]
  }
}

@js.native
sealed trait JsGlobals extends js.Object {
  def options: js.Array[JsOptionSpec] = js.native
  def flags: js.Array[JsFlagSpec]     = js.native
}
object JsGlobals {
  def apply(options: js.Array[JsOptionSpec], flags: js.Array[JsFlagSpec]): JsGlobals =
    js.Dynamic.literal("options" -> options, "flags" -> flags).asInstanceOf[JsGlobals]
}

// === Streams, structured results, errors ===

@js.native
sealed trait JsStreamSpec extends js.Object {
  def doc: JsDoc             = js.native
  def mime: js.Array[String] = js.native
  def required: Boolean      = js.native
}
object JsStreamSpec {
  def apply(doc: JsDoc, mime: js.Array[String], required: Boolean): JsStreamSpec =
    js.Dynamic.literal("doc" -> doc, "mime" -> mime, "required" -> required).asInstanceOf[JsStreamSpec]
}

@js.native
sealed trait JsFormatter extends js.Object {
  def name: String = js.native
  def doc: JsDoc   = js.native
}
object JsFormatter {
  def apply(name: String, doc: JsDoc): JsFormatter =
    js.Dynamic.literal("name" -> name, "doc" -> doc).asInstanceOf[JsFormatter]
}

@js.native
sealed trait JsResultSpec extends js.Object {
  @JSName("type")
  def tpe: Int                          = js.native
  def doc: JsDoc                        = js.native
  def formatters: js.Array[JsFormatter] = js.native
  def defaultFormatter: String          = js.native
}
object JsResultSpec {
  def apply(tpe: Int, doc: JsDoc, formatters: js.Array[JsFormatter], defaultFormatter: String): JsResultSpec =
    js.Dynamic
      .literal("type" -> tpe, "doc" -> doc, "formatters" -> formatters, "defaultFormatter" -> defaultFormatter)
      .asInstanceOf[JsResultSpec]
}

/** `error-kind` is a plain string enum. */
object JsErrorKind {
  val usageError: String   = "usage-error"
  val runtimeError: String = "runtime-error"
}

@js.native
sealed trait JsErrorCase extends js.Object {
  def name: String             = js.native
  def doc: JsDoc               = js.native
  def kind: String             = js.native
  def exitCode: Int            = js.native
  def payload: js.UndefOr[Int] = js.native
}
object JsErrorCase {
  def apply(name: String, doc: JsDoc, kind: String, exitCode: Int, payload: js.UndefOr[Int]): JsErrorCase = {
    val o = js.Dynamic.literal("name" -> name, "doc" -> doc, "kind" -> kind, "exitCode" -> exitCode)
    payload.foreach(v => o.updateDynamic("payload")(v))
    o.asInstanceOf[JsErrorCase]
  }
}

// === Command tree ===

@js.native
sealed trait JsCommandBody extends js.Object {
  def positionals: JsPositionals                    = js.native
  def options: js.Array[JsOptionSpec]               = js.native
  def flags: js.Array[JsFlagSpec]                   = js.native
  def constraints: js.Array[JsConstraint]           = js.native
  def stdin: js.UndefOr[JsStreamSpec]               = js.native
  def stdout: js.UndefOr[JsStreamSpec]              = js.native
  def result: js.UndefOr[JsResultSpec]              = js.native
  def errors: js.Array[JsErrorCase]                 = js.native
  def annotations: js.UndefOr[JsCommandAnnotations] = js.native
}
object JsCommandBody {
  def apply(
    positionals: JsPositionals,
    options: js.Array[JsOptionSpec],
    flags: js.Array[JsFlagSpec],
    constraints: js.Array[JsConstraint],
    stdin: js.UndefOr[JsStreamSpec],
    stdout: js.UndefOr[JsStreamSpec],
    result: js.UndefOr[JsResultSpec],
    errors: js.Array[JsErrorCase],
    annotations: js.UndefOr[JsCommandAnnotations]
  ): JsCommandBody = {
    val o = js.Dynamic.literal(
      "positionals" -> positionals,
      "options"     -> options,
      "flags"       -> flags,
      "constraints" -> constraints,
      "errors"      -> errors
    )
    stdin.foreach(v => o.updateDynamic("stdin")(v))
    stdout.foreach(v => o.updateDynamic("stdout")(v))
    result.foreach(v => o.updateDynamic("result")(v))
    annotations.foreach(v => o.updateDynamic("annotations")(v))
    o.asInstanceOf[JsCommandBody]
  }
}

@js.native
sealed trait JsCommandNode extends js.Object {
  def name: String                    = js.native
  def aliases: js.Array[String]       = js.native
  def doc: JsDoc                      = js.native
  def globals: JsGlobals              = js.native
  def subcommands: js.Array[Int]      = js.native
  def body: js.UndefOr[JsCommandBody] = js.native
}
object JsCommandNode {
  def apply(
    name: String,
    aliases: js.Array[String],
    doc: JsDoc,
    globals: JsGlobals,
    subcommands: js.Array[Int],
    body: js.UndefOr[JsCommandBody]
  ): JsCommandNode = {
    val o = js.Dynamic.literal(
      "name"        -> name,
      "aliases"     -> aliases,
      "doc"         -> doc,
      "globals"     -> globals,
      "subcommands" -> subcommands
    )
    body.foreach(v => o.updateDynamic("body")(v))
    o.asInstanceOf[JsCommandNode]
  }
}

@js.native
sealed trait JsCommandTree extends js.Object {
  def nodes: js.Array[JsCommandNode] = js.native
}
object JsCommandTree {
  def apply(nodes: js.Array[JsCommandNode]): JsCommandTree =
    js.Dynamic.literal("nodes" -> nodes).asInstanceOf[JsCommandTree]
}

// === Top level ===

@js.native
sealed trait JsTool extends js.Object {
  def version: String         = js.native
  def commands: JsCommandTree = js.native
  def schema: JsSchemaGraph   = js.native
}
object JsTool {
  def apply(version: String, commands: JsCommandTree, schema: JsSchemaGraph): JsTool =
    js.Dynamic.literal("version" -> version, "commands" -> commands, "schema" -> schema).asInstanceOf[JsTool]
}

// === Invocation contract ===

@js.native
sealed trait JsToolError extends js.Object {
  def tag: String = js.native
}
object JsToolError {
  def invalidToolName(name: String): JsToolError =
    JsShape.tagged[JsToolError]("invalid-tool-name", name)
  def invalidCommandPath(path: js.Array[String]): JsToolError =
    JsShape.tagged[JsToolError]("invalid-command-path", path)
  def invalidInput(message: String): JsToolError =
    JsShape.tagged[JsToolError]("invalid-input", message)
  def constraintViolation(message: String): JsToolError =
    JsShape.tagged[JsToolError]("constraint-violation", message)
  def invalidResult(message: String): JsToolError =
    JsShape.tagged[JsToolError]("invalid-result", message)
  def customError(payload: JsTypedSchemaValue): JsToolError =
    JsShape.tagged[JsToolError]("custom-error", payload)
}

@js.native
sealed trait JsInvocationResult extends js.Object {
  def result: js.UndefOr[JsTypedSchemaValue] = js.native
  def stdout: js.UndefOr[JsWasiOutputStream] = js.native
}
object JsInvocationResult {
  def apply(
    result: js.UndefOr[JsTypedSchemaValue],
    stdout: js.UndefOr[JsWasiOutputStream]
  ): JsInvocationResult = {
    val o = js.Dynamic.literal()
    result.foreach(v => o.updateDynamic("result")(v))
    stdout.foreach(v => o.updateDynamic("stdout")(v))
    o.asInstanceOf[JsInvocationResult]
  }
}
