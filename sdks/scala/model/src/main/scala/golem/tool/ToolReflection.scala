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

import golem.schema.wire.SchemaWire
import golem.tool.wire.*

/**
 * Owned descriptor models are reconstructed only when a dynamic caller requests
 * one.
 */
object ToolReflection {
  def fromWire(tool: WitTool): ExtendedToolType = {
    def graph(index: Int)                            = SchemaWire.schemaGraphFromWit(tool.schema.copy(root = index))
    def option(o: WitOptionSpec): ExtendedOptionSpec = {
      val shape = o.shape match {
        case WitOptionShape.Scalar(tpe)         => ExtendedOptionShape.Scalar(graph(tpe))
        case WitOptionShape.OptionalScalar(tpe) => ExtendedOptionShape.OptionalScalar(graph(tpe))
        case WitOptionShape.RepeatableList(s)   =>
          ExtendedOptionShape.RepeatableList(ExtendedRepeatableListShape(s.repetition, graph(s.itemType)))
        case WitOptionShape.RepeatableMap(s) =>
          ExtendedOptionShape.RepeatableMap(
            ExtendedRepeatableMapShape(s.repetition, graph(s.mapType), s.duplicateKeyPolicy)
          )
      }
      ExtendedOptionSpec(
        o.long,
        o.short,
        o.aliases,
        o.doc,
        o.valueName,
        shape,
        o.default.map(SchemaWire.schemaValueFromWit),
        o.required,
        o.envVar
      )
    }
    def ref(value: WitRef): ExtendedRef = value match {
      case WitRef.Present(name) => ExtendedRef.Present(name)
      case WitRef.ValueIs(v)    =>
        ExtendedRef.ValueIs(
          ExtendedValueIsRef(v.name, ExtendedValueIsLiteral.Resolved(SchemaWire.schemaValueFromWit(v.value)))
        )
    }
    def constraint(value: WitConstraint): ExtendedConstraint = value match {
      case WitConstraint.RequiresAll(refs)   => ExtendedConstraint.RequiresAll(refs.map(ref))
      case WitConstraint.AllOrNone(refs)     => ExtendedConstraint.AllOrNone(refs.map(ref))
      case WitConstraint.RequiresAny(refs)   => ExtendedConstraint.RequiresAny(refs.map(ref))
      case WitConstraint.MutexGroups(groups) =>
        ExtendedConstraint.MutexGroups(groups.map(g => ExtendedRefGroup(g.refs.map(ref))))
      case WitConstraint.Implies(c) =>
        ExtendedConstraint.Implies(ExtendedImpliesC(c.lhsQuant, c.lhs.map(ref), c.rhsQuant, c.rhs.map(ref)))
      case WitConstraint.Forbids(c) =>
        ExtendedConstraint.Forbids(ExtendedForbidsC(c.lhsQuant, c.lhs.map(ref), c.rhs.map(ref)))
    }
    val commands = tool.commands.nodes.map { node =>
      val body = node.body.map { b =>
        ExtendedCommandBody(
          ExtendedPositionals(
            b.positionals.fixed.map { p =>
              ExtendedPositional(
                p.name,
                p.doc,
                p.valueName,
                graph(p.tpe),
                p.default.map(SchemaWire.schemaValueFromWit),
                p.required,
                p.acceptsStdio
              )
            },
            b.positionals.tail.map { p =>
              ExtendedTailPositional(
                p.name,
                p.doc,
                p.valueName,
                graph(p.itemType),
                p.min,
                p.max,
                p.separator,
                p.verbatim,
                p.acceptsStdio
              )
            }
          ),
          b.options.map(option),
          b.flags,
          b.constraints.map(constraint),
          b.stdin,
          b.stdout,
          b.result.map(r => ExtendedResultSpec(graph(r.tpe), r.doc, r.formatters, r.defaultFormatter)),
          b.errors.map(e => ExtendedErrorCase(e.name, e.doc, e.kind, e.exitCode, e.payload.map(graph))),
          b.annotations
        )
      }
      ExtendedCommandNode(
        node.name,
        node.aliases,
        node.doc,
        ExtendedGlobals(node.globals.options.map(option), node.globals.flags),
        node.subcommands,
        body
      )
    }
    ExtendedToolType(tool.version, commands)
  }
}
