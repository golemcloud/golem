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
import zio.blocks.schema.binding.{Binding, Registers}
import zio.blocks.typeid.TypeId

trait ConfigBuilder[T] {
  def build(path: List[String], loader: ConfigFieldLoader): T
}

object ConfigBuilder {
  def apply[T](implicit cb: ConfigBuilder[T]): ConfigBuilder[T] = cb

  implicit def fromSchema[A](implicit schema: Schema[A]): ConfigBuilder[A] =
    new ConfigBuilder[A] {
      override def build(path: List[String], loader: ConfigFieldLoader): A =
        buildFromReflect(path, schema.reflect, loader)
    }

  private val secretFullName: String =
    TypeId.normalize(TypeId.of[Secret[Unit]]).fullName

  private def isSecret(reflect: Reflect.Bound[_]): Boolean =
    TypeId.normalize(reflect.typeId).fullName == secretFullName

  private def isTupleRecord(rec: Reflect.Record[Binding, _]): Boolean = {
    val names = rec.fields.map(_.name).toSet
    (rec.fields.length == 2 && names == Set("_1", "_2")) ||
    (rec.fields.length == 3 && names == Set("_1", "_2", "_3"))
  }

  private def buildFromReflect[A](
    path: List[String],
    reflect: Reflect.Bound[A],
    loader: ConfigFieldLoader
  ): A =
    if (isSecret(reflect)) {
      val inner = reflect.asWrapperUnknown match {
        case Some(u) => u.wrapper.wrapped.asInstanceOf[Reflect.Bound[Any]]
        case None    => reflect.asInstanceOf[Reflect.Bound[Any]]
      }
      buildSecret(path, inner, loader).asInstanceOf[A]
    } else {
      reflect.asRecord match {
        case Some(record) if !isTupleRecord(record) =>
          buildRecord(path, record, loader)
        case _ =>
          reflect.asWrapperUnknown match {
            case Some(u) =>
              buildWrapper(path, u, loader).asInstanceOf[A]
            case None =>
              buildLeaf(path, reflect, loader)
          }
      }
    }

  private def buildRecord[A](
    path: List[String],
    record: Reflect.Record.Bound[A],
    loader: ConfigFieldLoader
  ): A = {
    val constructor = record.constructor
    val registers   = Registers(constructor.usedRegisters)
    var idx         = 0
    while (idx < record.fields.length) {
      val field = record.fields(idx)
      val value = buildFromReflect(
        path :+ field.name,
        field.value.asInstanceOf[Reflect.Bound[Any]],
        loader
      )
      record.registers(idx).set(registers, 0, value)
      idx += 1
    }
    constructor.construct(registers, 0)
  }

  private def buildLeaf[A](
    path: List[String],
    reflect: Reflect.Bound[A],
    loader: ConfigFieldLoader
  ): A = {
    implicit val schemaA: Schema[A] = new Schema(reflect)
    val elem                        = ElementSchema.Component(DataInterop.reflectToDataType(reflect))
    loader.loadLocal[A](path, elem)
  }

  private def buildSecret[A](
    path: List[String],
    innerReflect: Reflect.Bound[A],
    loader: ConfigFieldLoader
  ): Secret[A] = {
    implicit val schemaA: Schema[A] = new Schema(innerReflect)
    val elem                        = ElementSchema.Component(DataInterop.reflectToDataType(innerReflect))
    loader.loadSecret[A](path, elem)
  }

  private def buildWrapper(
    path: List[String],
    u: Reflect.Wrapper.Unknown[Binding],
    loader: ConfigFieldLoader
  ): Any = {
    val wrapper = u.wrapper
    val wrapped = wrapper.wrapped.asInstanceOf[Reflect.Bound[Any]]
    val value   = buildFromReflect(path, wrapped, loader)
    wrapper.binding.wrap(value.asInstanceOf[u.Wrapped])
  }
}
