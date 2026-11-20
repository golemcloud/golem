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

import golem.schema.{Derivation, SchemaGraph, SchemaType, SchemaTypeBody, SecretSpec}
import zio.blocks.schema.{Modifier, Reflect, Schema}
import zio.blocks.schema.binding.Binding
import zio.blocks.typeid.TypeId

private[golem] object ConfigIntrospection {

  private val secretFullName: String =
    TypeId.normalize(TypeId.of[Secret[Unit]]).fullName

  def declarations[A](prefix: List[String] = Nil)(implicit schema: Schema[A]): List[AgentConfigDeclaration] =
    walk(prefix, schema.reflect, root = true)

  private[golem] def containsSecret(reflect: Reflect.Bound[_], seen: Set[String] = Set.empty): Boolean = {
    val id = reflect.typeId.toString
    if (isSecret(reflect)) true
    else if (seen(id)) false
    else {
      val next = seen + id
      reflect.asWrapperUnknown.exists(u => containsSecret(u.wrapper.wrapped, next)) ||
      reflect.asRecord.exists(_.fields.exists(f => containsSecret(f.value, next))) ||
      reflect.asVariant.exists(_.cases.exists(c => containsSecret(c.value, next))) ||
      reflect.asSequenceUnknown.exists(s => containsSecret(s.sequence.element, next)) ||
      reflect.asMapUnknown.exists(m => containsSecret(m.map.key, next) || containsSecret(m.map.value, next))
    }
  }

  private def isSecret(reflect: Reflect.Bound[_]): Boolean =
    TypeId.normalize(reflect.typeId).fullName == secretFullName

  private def isTupleRecord(rec: Reflect.Record[Binding, _]): Boolean = {
    val names = rec.fields.map(_.name).toSet
    (rec.fields.length == 2 && names == Set("_1", "_2")) ||
    (rec.fields.length == 3 && names == Set("_1", "_2", "_3"))
  }

  private def walk[A](
    path: List[String],
    reflect: Reflect.Bound[A],
    root: Boolean = false,
    default: Option[String] = None
  ): List[AgentConfigDeclaration] =
    if (isSecret(reflect)) {
      require(default.isEmpty, s"Code default on managed config ${path.mkString(".")}")
      val inner = reflect.asWrapperUnknown match {
        case Some(u) => u.wrapper.wrapped.asInstanceOf[Reflect.Bound[Any]]
        case None    => reflect.asInstanceOf[Reflect.Bound[Any]]
      }
      List(
        AgentConfigDeclaration(
          AgentConfigSource.Secret,
          path,
          secretGraph(ConfigSchema.canonicalGraph(Derivation.graphOf(new Schema(inner))))
        )
      )
    } else {
      reflect.asWrapperUnknown match {
        case Some(u) =>
          walk(path, u.wrapper.wrapped.asInstanceOf[Reflect.Bound[Any]], root, default)
        case None =>
          reflect.asRecord match {
            case Some(rec) if !isTupleRecord(rec) && (root || containsSecret(reflect)) =>
              require(default.isEmpty, s"Code default on config group ${path.mkString(".")}; default its static fields")
              val names = rec.fields.map(field => ConfigSchema.canonicalName(field.name))
              require(names.distinct.size == names.size, "Duplicate canonical config fields")
              rec.fields.toList.flatMap { field =>
                val defaults = field.modifiers.collect { case Modifier.config(ConfigSchema.DefaultKey, json) => json }
                require(defaults.size <= 1, s"Duplicate config default on ${field.name}")
                require(!field.name.startsWith("$") && !field.name.contains("."), s"Invalid config field ${field.name}")
                walk(
                  path :+ ConfigSchema.canonicalName(field.name),
                  field.value.asInstanceOf[Reflect.Bound[Any]],
                  default = defaults.headOption
                )
              }
            case _ =>
              checkStaticFields(reflect, path, Set.empty)
              val graph = ConfigSchema.canonicalGraph(Derivation.graphOf(new Schema(reflect)))
              List(
                AgentConfigDeclaration(
                  AgentConfigSource.Local,
                  path,
                  graph,
                  default.map(ConfigSchema.defaultValue(graph, _))
                )
              )
          }
      }
    }

  private def checkStaticFields(reflect: Reflect.Bound[_], path: List[String], seen: Set[String]): Unit = {
    val id = reflect.typeId.toString
    if (!seen(id)) {
      val next = seen + id
      reflect.asRecord.foreach(_.fields.foreach { field =>
        require(!field.name.startsWith("$") && !field.name.contains("."), s"Invalid config field ${field.name}")
        require(
          !field.modifiers.exists {
            case Modifier.config(ConfigSchema.DefaultKey, _) => true
            case _                                           => false
          },
          s"Place the config default on the owning static field ${path.mkString(".")}, not ${field.name}"
        )
        checkStaticFields(field.value, path :+ field.name, next)
      })
      reflect.asWrapperUnknown.foreach(u => checkStaticFields(u.wrapper.wrapped, path, next))
      reflect.asVariant.foreach(_.cases.foreach(c => checkStaticFields(c.value, path, next)))
      reflect.asSequenceUnknown.foreach(s => checkStaticFields(s.sequence.element, path, next))
      reflect.asMapUnknown.foreach { m =>
        checkStaticFields(m.map.key, path, next); checkStaticFields(m.map.value, path, next)
      }
    }
  }

  private def secretGraph(inner: SchemaGraph): SchemaGraph =
    SchemaGraph(inner.defs, SchemaType(SchemaTypeBody.SecretType(SecretSpec(inner.root))))
}
