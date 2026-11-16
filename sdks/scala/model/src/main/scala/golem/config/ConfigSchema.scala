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

package golem.config

import golem.reflection.SchemaRef
import golem.schema.{DiscriminatorRule, SchemaGraph, SchemaType, SchemaTypeBody}
import scala.collection.immutable.ListMap
import zio.blocks.schema.Schema
import zio.blocks.schema.json.Json

trait ConfigSchema[T] {
  def describe(path: List[String]): List[AgentConfigDeclaration]
}

object ConfigSchema {

  /**
   * Schema modifier key for a canonical JSON default on a static config field.
   */
  final val DefaultKey = "golem.config.default"

  private[golem] def canonicalName(name: String): String = {
    require(!name.startsWith("$") && !name.contains("."), s"Invalid config field $name")
    val joined = "[_-]+([a-zA-Z0-9])".r.replaceAllIn(name, m => m.group(1).toUpperCase)
    if (joined.isEmpty) joined else joined.head.toLower + joined.tail
  }

  /**
   * Project config names without changing the positional codec or ordinary
   * method schemas.
   */
  private[golem] def canonicalGraph(graph: SchemaGraph): SchemaGraph = {
    import SchemaTypeBody.*
    def visit(node: SchemaType, namespace: Boolean): SchemaType = {
      def child(value: SchemaType): SchemaType = visit(value, namespace)
      val body                                 = node.body match {
        case RefType(id)        => RefType(if (namespace) s"$id/config" else id)
        case RecordType(fields) =>
          val names = fields.map(field => canonicalName(field.name))
          require(names.distinct.size == names.size, "Duplicate canonical config fields")
          RecordType(fields.zip(names).map { case (field, name) => field.copy(name = name, body = child(field.body)) })
        case VariantType(cases)  => VariantType(cases.map(c => c.copy(payload = c.payload.map(child))))
        case UnionType(branches) =>
          UnionType(branches.map { branch =>
            val discriminator = branch.discriminator match {
              case DiscriminatorRule.FieldEquals(field) =>
                DiscriminatorRule.FieldEquals(field.copy(fieldName = canonicalName(field.fieldName)))
              case DiscriminatorRule.FieldAbsent(name) => DiscriminatorRule.FieldAbsent(canonicalName(name))
              case other                               => other
            }
            branch.copy(body = child(branch.body), discriminator = discriminator)
          })
        case TupleType(elements)            => TupleType(elements.map(child))
        case ListType(element)              => ListType(child(element))
        case FixedListType(element, length) => FixedListType(child(element), length)
        case MapType(key, value)            => MapType(child(key), child(value))
        case OptionType(element)            => OptionType(child(element))
        case ResultType(ok, err)            => ResultType(ok.map(child), err.map(child))
        case SecretType(spec)               => SecretType(spec.copy(inner = child(spec.inner)))
        case FutureType(element)            => FutureType(element.map(child))
        case StreamType(element)            => StreamType(element.map(child))
        case other                          => other
      }
      node.copy(body = body)
    }
    val projected = SchemaGraph(
      ListMap.from(graph.defs.map { case (id, definition) =>
        id -> definition.copy(body = visit(definition.body, false))
      }),
      visit(graph.root, false)
    )
    if (projected == graph) graph
    else
      SchemaGraph(
        ListMap.from(graph.defs.map { case (id, definition) =>
          s"$id/config" -> definition.copy(body = visit(definition.body, true))
        }),
        visit(graph.root, true)
      )
  }

  private[golem] def defaultJson(graph: SchemaGraph, json: String): String = {
    val ref    = SchemaRef(graph)
    val result = for {
      parsed <- Json.parse(json).left.map(_.toString)
      value  <- ref.validateJson(parsed).left.map(_.map(_.message).mkString("; "))
      output <- ref.unpackJson(value).left.map(_.message)
    } yield output.print
    result.fold(error => throw new IllegalArgumentException(s"Invalid config default: $error"), identity)
  }

  def apply[T](implicit cs: ConfigSchema[T]): ConfigSchema[T] = cs

  implicit def fromSchema[A](implicit schema: Schema[A]): ConfigSchema[A] =
    new ConfigSchema[A] {
      def describe(path: List[String]): List[AgentConfigDeclaration] =
        ConfigIntrospection.declarations[A](path)
    }
}
