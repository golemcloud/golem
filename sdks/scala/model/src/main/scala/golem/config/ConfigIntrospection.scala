/*
 * Copyright 2024-2026 John A. De Goes and the ZIO Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.config

import golem.data.{DataInterop, ElementSchema}
import zio.blocks.schema.{Reflect, Schema}
import zio.blocks.schema.binding.Binding
import zio.blocks.typeid.TypeId

private[golem] object ConfigIntrospection {

  private val secretFullName: String =
    TypeId.normalize(TypeId.of[Secret[Unit]]).fullName

  def declarations[A](prefix: List[String] = Nil)(implicit schema: Schema[A]): List[AgentConfigDeclaration] =
    walk(prefix, schema.reflect)

  private def isSecret(reflect: Reflect.Bound[_]): Boolean =
    TypeId.normalize(reflect.typeId).fullName == secretFullName

  private def isTupleRecord(rec: Reflect.Record[Binding, _]): Boolean = {
    val names = rec.fields.map(_.name).toSet
    (rec.fields.length == 2 && names == Set("_1", "_2")) ||
    (rec.fields.length == 3 && names == Set("_1", "_2", "_3"))
  }

  private def walk[A](path: List[String], reflect: Reflect.Bound[A]): List[AgentConfigDeclaration] =
    if (isSecret(reflect)) {
      val inner = reflect.asWrapperUnknown match {
        case Some(u) => u.wrapper.wrapped.asInstanceOf[Reflect.Bound[Any]]
        case None    => reflect.asInstanceOf[Reflect.Bound[Any]]
      }
      List(
        AgentConfigDeclaration(
          AgentConfigSource.Secret,
          path,
          ElementSchema.Component(DataInterop.reflectToDataType(inner))
        )
      )
    } else {
      reflect.asWrapperUnknown match {
        case Some(u) =>
          walk(path, u.wrapper.wrapped.asInstanceOf[Reflect.Bound[Any]])
        case None =>
          reflect.asRecord match {
            case Some(rec) if !isTupleRecord(rec) =>
              rec.fields.toList.flatMap { field =>
                walk(path :+ field.name, field.value.asInstanceOf[Reflect.Bound[Any]])
              }
            case _ =>
              List(
                AgentConfigDeclaration(
                  AgentConfigSource.Local,
                  path,
                  ElementSchema.Component(DataInterop.reflectToDataType(reflect))
                )
              )
          }
      }
    }
}
