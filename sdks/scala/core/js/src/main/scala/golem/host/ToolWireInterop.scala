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

package golem.host

import golem.host.js.tool._
import golem.tool._
import golem.tool.wire._

import scala.scalajs.js
import scala.scalajs.js.JSConverters._

/**
 * Mechanical mapping between the host-agnostic flat tool carrier
 * [[golem.tool.wire]] (`Wit*`) and the `golem:tool/common@0.1.0` JS facades
 * [[golem.host.js.tool]] (`Js*`).
 *
 * The extended-model -> wire encoding (validation, graph merging) is owned by
 * [[golem.tool.ToolEncoding]]; this layer is the pure, lossless `Wit* <-> Js*`
 * rename. Schema positions (the tool-level `schema-graph`, default
 * `schema-value-tree`s, custom-error `typed-schema-value`s) delegate to
 * [[SchemaWireInterop]].
 */
object ToolWireInterop {

  // ===========================================================================
  // Public entry points
  // ===========================================================================

  def toolToJs(t: WitTool): JsTool =
    JsTool(
      t.version,
      JsCommandTree(t.commands.nodes.map(commandNodeToJs).toJSArray),
      SchemaWireInterop.graphToJs(t.schema)
    )

  def toolFromJs(j: JsTool): WitTool =
    WitTool(
      j.version,
      WitCommandTree(j.commands.nodes.toList.toVector.map(commandNodeFromJs)),
      SchemaWireInterop.graphFromJs(j.schema)
    )

  def toolErrorToJs(e: WitToolError): JsToolError =
    e match {
      case WitToolError.InvalidToolName(name)    => JsToolError.invalidToolName(name)
      case WitToolError.InvalidCommandPath(path) => JsToolError.invalidCommandPath(path.toJSArray)
      case WitToolError.InvalidInput(message)    => JsToolError.invalidInput(message)
      case WitToolError.ConstraintViolation(msg) => JsToolError.constraintViolation(msg)
      case WitToolError.InvalidResult(message)   => JsToolError.invalidResult(message)
      case WitToolError.CustomError(payload)     => JsToolError.customError(SchemaWireInterop.typedToJs(payload))
    }

  def toolErrorFromJs(j: JsToolError): WitToolError =
    j.tag match {
      case "invalid-tool-name"    => WitToolError.InvalidToolName(valOf(j).asInstanceOf[String])
      case "invalid-command-path" =>
        WitToolError.InvalidCommandPath(valOf(j).asInstanceOf[js.Array[String]].toList)
      case "invalid-input"        => WitToolError.InvalidInput(valOf(j).asInstanceOf[String])
      case "constraint-violation" => WitToolError.ConstraintViolation(valOf(j).asInstanceOf[String])
      case "invalid-result"       => WitToolError.InvalidResult(valOf(j).asInstanceOf[String])
      case "custom-error"         =>
        WitToolError.CustomError(
          SchemaWireInterop.typedFromJs(valOf(j).asInstanceOf[golem.host.js.schema.JsTypedSchemaValue])
        )
      case other => throw new IllegalArgumentException(s"Unknown tool-error tag: $other")
    }

  // ===========================================================================
  // Low-level helpers
  // ===========================================================================

  /** Read the positional `val` payload of a `{ tag, val }` JS object. */
  private def valOf(o: js.Object): js.Dynamic =
    o.asInstanceOf[js.Dynamic].selectDynamic("val")

  // ===========================================================================
  // Documentation, annotations
  // ===========================================================================

  private def exampleToJs(e: Example): JsExample   = JsExample(e.title, e.body)
  private def exampleFromJs(j: JsExample): Example = Example(j.title, j.body)

  private def docToJs(d: Doc): JsDoc =
    JsDoc(d.summary, d.description, d.examples.map(exampleToJs).toJSArray)

  private def docFromJs(j: JsDoc): Doc =
    Doc(j.summary, j.description, j.examples.toList.map(exampleFromJs))

  private def annotationsToJs(a: CommandAnnotations): JsCommandAnnotations =
    JsCommandAnnotations(a.readOnly, a.destructive, a.idempotent, a.openWorld)

  private def annotationsFromJs(j: JsCommandAnnotations): CommandAnnotations =
    CommandAnnotations(j.readOnly, j.destructive, j.idempotent, j.openWorld)

  // ===========================================================================
  // String enums, chars
  // ===========================================================================

  private def quantifierToJs(q: Quantifier): String =
    q match {
      case Quantifier.All => JsQuantifier.all
      case Quantifier.Any => JsQuantifier.any
    }

  private def quantifierFromJs(s: String): Quantifier =
    s match {
      case "all" => Quantifier.All
      case "any" => Quantifier.Any
      case other => throw new IllegalArgumentException(s"Unknown quantifier: $other")
    }

  private def errorKindToJs(k: ErrorKind): String =
    k match {
      case ErrorKind.UsageError   => JsErrorKind.usageError
      case ErrorKind.RuntimeError => JsErrorKind.runtimeError
    }

  private def errorKindFromJs(s: String): ErrorKind =
    s match {
      case "usage-error"   => ErrorKind.UsageError
      case "runtime-error" => ErrorKind.RuntimeError
      case other           => throw new IllegalArgumentException(s"Unknown error-kind: $other")
    }

  private def duplicateKeyPolicyToJs(p: DuplicateKeyPolicy): String =
    p match {
      case DuplicateKeyPolicy.Reject   => JsDuplicateKeyPolicy.reject
      case DuplicateKeyPolicy.LastWins => JsDuplicateKeyPolicy.lastWins
    }

  private def duplicateKeyPolicyFromJs(s: String): DuplicateKeyPolicy =
    s match {
      case "reject"    => DuplicateKeyPolicy.Reject
      case "last-wins" => DuplicateKeyPolicy.LastWins
      case other       => throw new IllegalArgumentException(s"Unknown duplicate-key-policy: $other")
    }

  /**
   * WIT `char` positions (option/flag `short`, repetition delimiters) are
   * single-code-point JS strings. A WIT `char` is a Unicode scalar value, so a
   * surrogate `Char` is rejected on encode (it is not a valid wire value), and
   * a supplementary-plane code point is rejected on decode (the model carries a
   * UTF-16 `Char` and cannot represent it).
   */
  private def charToJs(c: Char): String =
    if (c >= 0xd800 && c <= 0xdfff)
      throw new IllegalArgumentException(
        f"char value is not a Unicode scalar value: 0x${c.toInt}%04x"
      )
    else c.toString

  private def charFromJs(s: String): Char =
    if (s.isEmpty || Character.codePointCount(s, 0, s.length) != 1)
      throw new IllegalArgumentException(s"Expected a single-code-point string, got: '$s'")
    else {
      val codePoint = s.codePointAt(0)
      if (codePoint >= 0xd800 && codePoint <= 0xdfff)
        throw new IllegalArgumentException(
          f"char value is not a Unicode scalar value: 0x$codePoint%04x"
        )
      else if (codePoint > 0xffff)
        throw new IllegalArgumentException(
          s"char value outside the Basic Multilingual Plane is not representable: '$s'"
        )
      else codePoint.toChar
    }

  // ===========================================================================
  // Option shapes
  // ===========================================================================

  private def repetitionToJs(r: Repetition): JsRepetition =
    r match {
      case Repetition.Repeated     => JsRepetition.repeated
      case Repetition.Delimited(d) => JsRepetition.delimited(charToJs(d))
      case Repetition.Either(d)    => JsRepetition.either(charToJs(d))
    }

  private def repetitionFromJs(j: JsRepetition): Repetition =
    j.tag match {
      case "repeated"  => Repetition.Repeated
      case "delimited" => Repetition.Delimited(charFromJs(valOf(j).asInstanceOf[String]))
      case "either"    => Repetition.Either(charFromJs(valOf(j).asInstanceOf[String]))
      case other       => throw new IllegalArgumentException(s"Unknown repetition tag: $other")
    }

  private def optionShapeToJs(s: WitOptionShape): JsOptionShape =
    s match {
      case WitOptionShape.Scalar(tpe)         => JsOptionShape.scalar(tpe)
      case WitOptionShape.OptionalScalar(tpe) => JsOptionShape.optionalScalar(tpe)
      case WitOptionShape.RepeatableList(sh)  =>
        JsOptionShape.repeatableList(JsRepeatableListShape(repetitionToJs(sh.repetition), sh.itemType))
      case WitOptionShape.RepeatableMap(sh) =>
        JsOptionShape.repeatableMap(
          JsRepeatableMapShape(
            repetitionToJs(sh.repetition),
            sh.mapType,
            duplicateKeyPolicyToJs(sh.duplicateKeyPolicy)
          )
        )
    }

  private def optionShapeFromJs(j: JsOptionShape): WitOptionShape =
    j.tag match {
      case "scalar"          => WitOptionShape.Scalar(valOf(j).asInstanceOf[Int])
      case "optional-scalar" => WitOptionShape.OptionalScalar(valOf(j).asInstanceOf[Int])
      case "repeatable-list" =>
        val sh = valOf(j).asInstanceOf[JsRepeatableListShape]
        WitOptionShape.RepeatableList(WitRepeatableListShape(repetitionFromJs(sh.repetition), sh.itemType))
      case "repeatable-map" =>
        val sh = valOf(j).asInstanceOf[JsRepeatableMapShape]
        WitOptionShape.RepeatableMap(
          WitRepeatableMapShape(
            repetitionFromJs(sh.repetition),
            sh.mapType,
            duplicateKeyPolicyFromJs(sh.duplicateKeyPolicy)
          )
        )
      case other => throw new IllegalArgumentException(s"Unknown option-shape tag: $other")
    }

  private def flagShapeToJs(s: FlagShape): JsFlagShape =
    s match {
      case FlagShape.BoolFlag(sh)   => JsFlagShape.boolFlag(JsBoolFlagShape(sh.default, sh.negatable))
      case FlagShape.CountFlag(max) => JsFlagShape.countFlag(max.orUndefined)
    }

  private def flagShapeFromJs(j: JsFlagShape): FlagShape =
    j.tag match {
      case "bool-flag" =>
        val sh = valOf(j).asInstanceOf[JsBoolFlagShape]
        FlagShape.BoolFlag(BoolFlagShape(sh.default, sh.negatable))
      case "count-flag" =>
        FlagShape.CountFlag(valOf(j).asInstanceOf[js.UndefOr[Int]].toOption)
      case other => throw new IllegalArgumentException(s"Unknown flag-shape tag: $other")
    }

  // ===========================================================================
  // Constraints
  // ===========================================================================

  private def refToJs(r: WitRef): JsRef =
    r match {
      case WitRef.Present(name) => JsRef.present(name)
      case WitRef.ValueIs(v)    =>
        JsRef.valueIs(JsValueIsRef(v.name, SchemaWireInterop.valueTreeToJs(v.value)))
    }

  private def refFromJs(j: JsRef): WitRef =
    j.tag match {
      case "present"  => WitRef.Present(valOf(j).asInstanceOf[String])
      case "value-is" =>
        val v = valOf(j).asInstanceOf[JsValueIsRef]
        WitRef.ValueIs(WitValueIsRef(v.name, SchemaWireInterop.valueTreeFromJs(v.value)))
      case other => throw new IllegalArgumentException(s"Unknown ref tag: $other")
    }

  private def refsToJs(refs: List[WitRef]): js.Array[JsRef] = refs.map(refToJs).toJSArray
  private def refsFromJs(j: js.Array[JsRef]): List[WitRef]  = j.toList.map(refFromJs)

  private def constraintToJs(c: WitConstraint): JsConstraint =
    c match {
      case WitConstraint.RequiresAll(refs)   => JsConstraint.requiresAll(refsToJs(refs))
      case WitConstraint.AllOrNone(refs)     => JsConstraint.allOrNone(refsToJs(refs))
      case WitConstraint.RequiresAny(refs)   => JsConstraint.requiresAny(refsToJs(refs))
      case WitConstraint.MutexGroups(groups) =>
        JsConstraint.mutexGroups(groups.map(g => JsRefGroup(refsToJs(g.refs))).toJSArray)
      case WitConstraint.Implies(i) =>
        JsConstraint.implies(
          JsImpliesC(quantifierToJs(i.lhsQuant), refsToJs(i.lhs), quantifierToJs(i.rhsQuant), refsToJs(i.rhs))
        )
      case WitConstraint.Forbids(f) =>
        JsConstraint.forbids(JsForbidsC(quantifierToJs(f.lhsQuant), refsToJs(f.lhs), refsToJs(f.rhs)))
    }

  private def constraintFromJs(j: JsConstraint): WitConstraint =
    j.tag match {
      case "requires-all" => WitConstraint.RequiresAll(refsFromJs(valOf(j).asInstanceOf[js.Array[JsRef]]))
      case "all-or-none"  => WitConstraint.AllOrNone(refsFromJs(valOf(j).asInstanceOf[js.Array[JsRef]]))
      case "requires-any" => WitConstraint.RequiresAny(refsFromJs(valOf(j).asInstanceOf[js.Array[JsRef]]))
      case "mutex-groups" =>
        WitConstraint.MutexGroups(
          valOf(j).asInstanceOf[js.Array[JsRefGroup]].toList.map(g => WitRefGroup(refsFromJs(g.refs)))
        )
      case "implies" =>
        val i = valOf(j).asInstanceOf[JsImpliesC]
        WitConstraint.Implies(
          WitImpliesC(
            quantifierFromJs(i.lhsQuant),
            refsFromJs(i.lhs),
            quantifierFromJs(i.rhsQuant),
            refsFromJs(i.rhs)
          )
        )
      case "forbids" =>
        val f = valOf(j).asInstanceOf[JsForbidsC]
        WitConstraint.Forbids(WitForbidsC(quantifierFromJs(f.lhsQuant), refsFromJs(f.lhs), refsFromJs(f.rhs)))
      case other => throw new IllegalArgumentException(s"Unknown constraint tag: $other")
    }

  // ===========================================================================
  // Positionals, options, flags
  // ===========================================================================

  private def positionalToJs(p: WitPositional): JsPositional =
    JsPositional(
      p.name,
      docToJs(p.doc),
      p.valueName.orUndefined,
      p.tpe,
      p.default.map(SchemaWireInterop.valueTreeToJs).orUndefined,
      p.required,
      p.acceptsStdio
    )

  private def positionalFromJs(j: JsPositional): WitPositional =
    WitPositional(
      j.name,
      docFromJs(j.doc),
      j.valueName.toOption,
      j.tpe,
      j.default.toOption.map(SchemaWireInterop.valueTreeFromJs),
      j.required,
      j.acceptsStdio
    )

  private def tailPositionalToJs(t: WitTailPositional): JsTailPositional =
    JsTailPositional(
      t.name,
      docToJs(t.doc),
      t.valueName.orUndefined,
      t.itemType,
      t.min,
      t.max.orUndefined,
      t.separator.orUndefined,
      t.verbatim,
      t.acceptsStdio
    )

  private def tailPositionalFromJs(j: JsTailPositional): WitTailPositional =
    WitTailPositional(
      j.name,
      docFromJs(j.doc),
      j.valueName.toOption,
      j.itemType,
      j.min,
      j.max.toOption,
      j.separator.toOption,
      j.verbatim,
      j.acceptsStdio
    )

  private def positionalsToJs(p: WitPositionals): JsPositionals =
    JsPositionals(p.fixed.map(positionalToJs).toJSArray, p.tail.map(tailPositionalToJs).orUndefined)

  private def positionalsFromJs(j: JsPositionals): WitPositionals =
    WitPositionals(j.fixed.toList.map(positionalFromJs), j.tail.toOption.map(tailPositionalFromJs))

  private def optionSpecToJs(o: WitOptionSpec): JsOptionSpec =
    JsOptionSpec(
      o.long,
      o.short.map(charToJs).orUndefined,
      o.aliases.toJSArray,
      docToJs(o.doc),
      o.valueName.orUndefined,
      optionShapeToJs(o.shape),
      o.default.map(SchemaWireInterop.valueTreeToJs).orUndefined,
      o.required,
      o.envVar.orUndefined
    )

  private def optionSpecFromJs(j: JsOptionSpec): WitOptionSpec =
    WitOptionSpec(
      j.long,
      j.short.toOption.map(charFromJs),
      j.aliases.toList,
      docFromJs(j.doc),
      j.valueName.toOption,
      optionShapeFromJs(j.shape),
      j.default.toOption.map(SchemaWireInterop.valueTreeFromJs),
      j.required,
      j.envVar.toOption
    )

  private def flagSpecToJs(f: FlagSpec): JsFlagSpec =
    JsFlagSpec(
      f.long,
      f.short.map(charToJs).orUndefined,
      f.aliases.toJSArray,
      docToJs(f.doc),
      flagShapeToJs(f.shape),
      f.envVar.orUndefined
    )

  private def flagSpecFromJs(j: JsFlagSpec): FlagSpec =
    FlagSpec(
      j.long,
      j.short.toOption.map(charFromJs),
      j.aliases.toList,
      docFromJs(j.doc),
      flagShapeFromJs(j.shape),
      j.envVar.toOption
    )

  private def globalsToJs(g: WitGlobals): JsGlobals =
    JsGlobals(g.options.map(optionSpecToJs).toJSArray, g.flags.map(flagSpecToJs).toJSArray)

  private def globalsFromJs(j: JsGlobals): WitGlobals =
    WitGlobals(j.options.toList.map(optionSpecFromJs), j.flags.toList.map(flagSpecFromJs))

  // ===========================================================================
  // Streams, results, errors
  // ===========================================================================

  private def streamSpecToJs(s: StreamSpec): JsStreamSpec =
    JsStreamSpec(docToJs(s.doc), s.mime.toJSArray, s.required)

  private def streamSpecFromJs(j: JsStreamSpec): StreamSpec =
    StreamSpec(docFromJs(j.doc), j.mime.toList, j.required)

  private def formatterToJs(f: Formatter): JsFormatter   = JsFormatter(f.name, docToJs(f.doc))
  private def formatterFromJs(j: JsFormatter): Formatter = Formatter(j.name, docFromJs(j.doc))

  private def resultSpecToJs(r: WitResultSpec): JsResultSpec =
    JsResultSpec(r.tpe, docToJs(r.doc), r.formatters.map(formatterToJs).toJSArray, r.defaultFormatter)

  private def resultSpecFromJs(j: JsResultSpec): WitResultSpec =
    WitResultSpec(j.tpe, docFromJs(j.doc), j.formatters.toList.map(formatterFromJs), j.defaultFormatter)

  private def errorCaseToJs(e: WitErrorCase): JsErrorCase =
    JsErrorCase(e.name, docToJs(e.doc), errorKindToJs(e.kind), e.exitCode, e.payload.orUndefined)

  private def errorCaseFromJs(j: JsErrorCase): WitErrorCase =
    WitErrorCase(j.name, docFromJs(j.doc), errorKindFromJs(j.kind), j.exitCode, j.payload.toOption)

  // ===========================================================================
  // Command tree
  // ===========================================================================

  private def commandBodyToJs(b: WitCommandBody): JsCommandBody =
    JsCommandBody(
      positionalsToJs(b.positionals),
      b.options.map(optionSpecToJs).toJSArray,
      b.flags.map(flagSpecToJs).toJSArray,
      b.constraints.map(constraintToJs).toJSArray,
      b.stdin.map(streamSpecToJs).orUndefined,
      b.stdout.map(streamSpecToJs).orUndefined,
      b.result.map(resultSpecToJs).orUndefined,
      b.errors.map(errorCaseToJs).toJSArray,
      b.annotations.map(annotationsToJs).orUndefined
    )

  private def commandBodyFromJs(j: JsCommandBody): WitCommandBody =
    WitCommandBody(
      positionalsFromJs(j.positionals),
      j.options.toList.map(optionSpecFromJs),
      j.flags.toList.map(flagSpecFromJs),
      j.constraints.toList.map(constraintFromJs),
      j.stdin.toOption.map(streamSpecFromJs),
      j.stdout.toOption.map(streamSpecFromJs),
      j.result.toOption.map(resultSpecFromJs),
      j.errors.toList.map(errorCaseFromJs),
      j.annotations.toOption.map(annotationsFromJs)
    )

  private def commandNodeToJs(n: WitCommandNode): JsCommandNode =
    JsCommandNode(
      n.name,
      n.aliases.toJSArray,
      docToJs(n.doc),
      globalsToJs(n.globals),
      n.subcommands.toJSArray,
      n.body.map(commandBodyToJs).orUndefined
    )

  private def commandNodeFromJs(j: JsCommandNode): WitCommandNode =
    WitCommandNode(
      j.name,
      j.aliases.toList,
      docFromJs(j.doc),
      globalsFromJs(j.globals),
      j.subcommands.toList,
      j.body.toOption.map(commandBodyFromJs)
    )
}
