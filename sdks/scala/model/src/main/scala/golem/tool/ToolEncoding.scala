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

import golem.schema.wire.{GraphEncoder, SchemaWire, WitSchemaValueTree}
import golem.schema.{SchemaBuilder, SchemaConflictError, SchemaEncodeError, SchemaGraph, SchemaValue}
import golem.tool.wire._

import scala.util.control.NonFatal

/**
 * Encoding of a validated [[ExtendedToolType]] into the flat [[WitTool]] wire
 * carrier: the per-argument graphs are merged into one shared def registry,
 * every referenced type is flattened into a single type-node pool, and every
 * metadata-time value becomes a self-contained value tree.
 */
private[tool] object ToolEncoding {

  def tryToTool(tool: ExtendedToolType): Either[ToolBuildError, WitTool] =
    ToolValidation.validateTool(tool).flatMap { _ =>
      try {
        val defs =
          try SchemaBuilder.mergeGraphDefs(collectSchemaGraphs(tool))
          catch {
            case e: SchemaConflictError => throw ToolBuildException(ToolBuildError.EncodeError(e.getMessage))
          }
        val encoder = new GraphEncoder(defs)
        val nodes   = tool.commands.map(encodeNode(_, encoder))
        Right(WitTool(tool.version, WitCommandTree(nodes), encoder.finish()))
      } catch {
        case ToolBuildException(error) => Left(error)
        case e: SchemaEncodeError      => Left(ToolBuildError.EncodeError(e.getMessage))
      }
    }

  /**
   * Encodes a metadata-time literal (option/positional default, `value-is`
   * literal) into its self-contained `schema-value-tree` wire form. Type
   * conformance of these literals against their referenced type node is checked
   * separately by validation, which has the per-command field context needed to
   * resolve `value-is` references.
   */
  def encodeSchemaValueDefault(value: SchemaValue): Either[ToolBuildError, WitSchemaValueTree] =
    try Right(SchemaWire.schemaValueToWit(value))
    catch {
      case e: SchemaEncodeError => Left(ToolBuildError.EncodeError(e.getMessage))
      case NonFatal(e)          => Left(ToolBuildError.EncodeError(e.getMessage))
    }

  private def encodeValue(value: SchemaValue): WitSchemaValueTree =
    encodeSchemaValueDefault(value).fold(e => throw ToolBuildException(e), identity)

  private def encodeGraphRoot(graph: SchemaGraph, encoder: GraphEncoder): Int =
    try encoder.encodeType(graph.root)
    catch {
      case e: SchemaEncodeError => throw ToolBuildException(ToolBuildError.EncodeError(e.getMessage))
    }

  private def encodeNode(node: ExtendedCommandNode, encoder: GraphEncoder): WitCommandNode =
    WitCommandNode(
      name = node.name,
      aliases = node.aliases,
      doc = node.doc,
      globals = encodeGlobals(node.globals, encoder),
      subcommands = node.subcommands,
      body = node.body.map(encodeBody(_, encoder))
    )

  private def encodeGlobals(globals: ExtendedGlobals, encoder: GraphEncoder): WitGlobals =
    WitGlobals(
      options = globals.options.map(encodeOption(_, encoder)),
      flags = globals.flags
    )

  private def encodeBody(body: ExtendedCommandBody, encoder: GraphEncoder): WitCommandBody =
    WitCommandBody(
      positionals = WitPositionals(
        fixed = body.positionals.fixed.map(encodePositional(_, encoder)),
        tail = body.positionals.tail.map(encodeTail(_, encoder))
      ),
      options = body.options.map(encodeOption(_, encoder)),
      flags = body.flags,
      constraints = body.constraints.map(encodeConstraint),
      stdin = body.stdin,
      stdout = body.stdout,
      result = body.result.map(encodeResult(_, encoder)),
      errors = body.errors.map(encodeError(_, encoder)),
      annotations = body.annotations
    )

  private def encodePositional(p: ExtendedPositional, encoder: GraphEncoder): WitPositional =
    WitPositional(
      name = p.name,
      doc = p.doc,
      valueName = p.valueName,
      tpe = encodeGraphRoot(p.tpe, encoder),
      default = p.default.map(encodeValue),
      required = p.required,
      acceptsStdio = p.acceptsStdio
    )

  private def encodeTail(t: ExtendedTailPositional, encoder: GraphEncoder): WitTailPositional =
    WitTailPositional(
      name = t.name,
      doc = t.doc,
      valueName = t.valueName,
      itemType = encodeGraphRoot(t.itemType, encoder),
      min = t.min,
      max = t.max,
      separator = t.separator,
      verbatim = t.verbatim,
      acceptsStdio = t.acceptsStdio
    )

  private def encodeOption(o: ExtendedOptionSpec, encoder: GraphEncoder): WitOptionSpec =
    WitOptionSpec(
      long = o.long,
      short = o.short,
      aliases = o.aliases,
      doc = o.doc,
      valueName = o.valueName,
      shape = encodeOptionShape(o.shape, encoder),
      default = o.default.map(encodeValue),
      required = o.required,
      envVar = o.envVar
    )

  private def encodeOptionShape(shape: ExtendedOptionShape, encoder: GraphEncoder): WitOptionShape =
    shape match {
      case ExtendedOptionShape.Scalar(g) =>
        WitOptionShape.Scalar(encodeGraphRoot(g, encoder))
      case ExtendedOptionShape.OptionalScalar(g) =>
        WitOptionShape.OptionalScalar(encodeGraphRoot(g, encoder))
      case ExtendedOptionShape.RepeatableList(r) =>
        WitOptionShape.RepeatableList(
          WitRepeatableListShape(r.repetition, encodeGraphRoot(r.itemType, encoder))
        )
      case ExtendedOptionShape.RepeatableMap(r) =>
        WitOptionShape.RepeatableMap(
          WitRepeatableMapShape(r.repetition, encodeGraphRoot(r.mapType, encoder), r.duplicateKeyPolicy)
        )
    }

  private def encodeResult(r: ExtendedResultSpec, encoder: GraphEncoder): WitResultSpec =
    WitResultSpec(
      tpe = encodeGraphRoot(r.tpe, encoder),
      doc = r.doc,
      formatters = r.formatters,
      defaultFormatter = r.defaultFormatter
    )

  private def encodeError(err: ExtendedErrorCase, encoder: GraphEncoder): WitErrorCase =
    WitErrorCase(
      name = err.name,
      doc = err.doc,
      kind = err.kind,
      exitCode = err.exitCode,
      payload = err.payload.map(encodeGraphRoot(_, encoder))
    )

  private def encodeConstraint(c: ExtendedConstraint): WitConstraint =
    c match {
      case ExtendedConstraint.RequiresAll(v) => WitConstraint.RequiresAll(encodeRefs(v))
      case ExtendedConstraint.AllOrNone(v)   => WitConstraint.AllOrNone(encodeRefs(v))
      case ExtendedConstraint.RequiresAny(v) => WitConstraint.RequiresAny(encodeRefs(v))
      case ExtendedConstraint.MutexGroups(g) =>
        WitConstraint.MutexGroups(g.map(group => WitRefGroup(encodeRefs(group.refs))))
      case ExtendedConstraint.Implies(i) =>
        WitConstraint.Implies(WitImpliesC(i.lhsQuant, encodeRefs(i.lhs), i.rhsQuant, encodeRefs(i.rhs)))
      case ExtendedConstraint.Forbids(f) =>
        WitConstraint.Forbids(WitForbidsC(f.lhsQuant, encodeRefs(f.lhs), encodeRefs(f.rhs)))
    }

  private def encodeRefs(refs: List[ExtendedRef]): List[WitRef] =
    refs.map {
      case ExtendedRef.Present(name) => WitRef.Present(name)
      case ExtendedRef.ValueIs(v)    =>
        v.value match {
          case ExtendedValueIsLiteral.Resolved(sv) =>
            WitRef.ValueIs(WitValueIsRef(v.name, encodeValue(sv)))
          // A deferred literal reaching encoding means composition never
          // resolved it against a comparand type; the wire model only carries
          // resolved values.
          case _: ExtendedValueIsLiteral.Deferred =>
            throw ToolBuildException(ToolBuildError.UnresolvedValueIsLiteral(v.name))
        }
    }

  /**
   * Every self-contained per-argument graph referenced by the tool, in
   * traversal order, for merging into the single tool-level def registry.
   */
  def collectSchemaGraphs(tool: ExtendedToolType): List[SchemaGraph] = {
    val out = List.newBuilder[SchemaGraph]

    def collectOption(shape: ExtendedOptionShape): Unit =
      shape match {
        case ExtendedOptionShape.Scalar(g)         => out += g
        case ExtendedOptionShape.OptionalScalar(g) => out += g
        case ExtendedOptionShape.RepeatableList(r) => out += r.itemType
        case ExtendedOptionShape.RepeatableMap(r)  => out += r.mapType
      }

    tool.commands.foreach { c =>
      c.globals.options.foreach(o => collectOption(o.shape))
      c.body.foreach { b =>
        b.positionals.fixed.foreach(p => out += p.tpe)
        b.positionals.tail.foreach(t => out += t.itemType)
        b.options.foreach(o => collectOption(o.shape))
        b.result.foreach(r => out += r.tpe)
        b.errors.foreach(e => e.payload.foreach(p => out += p))
      }
    }
    out.result()
  }
}
