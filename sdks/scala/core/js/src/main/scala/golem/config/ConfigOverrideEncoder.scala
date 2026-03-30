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

import golem.data.ElementSchema
import golem.host.js.{JsTypedAgentConfigValue, JsValueAndType}
import golem.runtime.autowire.{WitTypeBuilder, WitValueBuilder}

import scala.scalajs.js

private[golem] object ConfigOverrideEncoder {
  def encode(overrides: List[ConfigOverride]): js.Array[JsTypedAgentConfigValue] = {
    val result = new js.Array[JsTypedAgentConfigValue]()
    overrides.foreach { co =>
      co.valueType match {
        case ElementSchema.Component(dataType) =>
          val witValue = WitValueBuilder.build(dataType, co.value) match {
            case Right(v)  => v
            case Left(err) =>
              throw new IllegalArgumentException(
                s"Failed to encode config override at ${co.path.mkString(".")}: $err"
              )
          }
          val witType = WitTypeBuilder.build(dataType)
          result.push(JsTypedAgentConfigValue(js.Array(co.path: _*), JsValueAndType(witValue, witType)))
        case other =>
          throw new IllegalArgumentException(
            s"Config overrides only support component types, found: $other at ${co.path.mkString(".")}"
          )
      }
    }
    result
  }
}
