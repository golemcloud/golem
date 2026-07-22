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

import golem.host.SchemaWireInterop
import golem.host.js.schema.{JsTypedAgentConfigValue, JsTypedSchemaValue}
import golem.schema.wire.SchemaWire

import scala.scalajs.js
import scala.scalajs.js.JSConverters._

private[golem] object ConfigOverrideEncoder {

  /**
   * Convert each [[ConfigOverride]] (which already carries a self-contained
   * `TypedSchemaValue`) into the `golem:agent@2.0.0` `typed-agent-config-value`
   * JS facade.
   */
  def encode(overrides: List[ConfigOverride]): js.Array[JsTypedAgentConfigValue] =
    overrides.map { co =>
      val typed: JsTypedSchemaValue =
        SchemaWireInterop.typedToJs(SchemaWire.typedSchemaValueToWit(co.value))
      JsTypedAgentConfigValue(co.path.toJSArray, typed)
    }.toJSArray
}
