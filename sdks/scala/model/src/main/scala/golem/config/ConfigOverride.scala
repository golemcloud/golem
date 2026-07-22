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

import golem.schema.{IntoSchema, TypedSchemaValue}

/**
 * Internal — used by generated RPC client code to build config overrides. Not
 * intended for direct use.
 *
 * Carries a self-contained [[TypedSchemaValue]] (graph + value) so it maps
 * directly to a `golem:agent@2.0.0` `typed-agent-config-value`.
 */
final case class ConfigOverride(
  path: List[String],
  value: TypedSchemaValue
)

/**
 * Internal — used by generated RPC client code. Not intended for direct use.
 */
object ConfigOverride {
  def apply[A](path: List[String], value: A)(implicit into: IntoSchema[A]): ConfigOverride =
    new ConfigOverride(path, into.toTyped(value))
}
