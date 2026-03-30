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

import golem.data.{DataValue, ElementSchema}

/**
 * Internal — used by generated RPC client code to build config overrides. Not
 * intended for direct use.
 */
final case class ConfigOverride(
  path: List[String],
  value: DataValue,
  valueType: ElementSchema
)

/**
 * Internal — used by generated RPC client code. Not intended for direct use.
 */
object ConfigOverride {
  def apply[A](path: List[String], value: A)(implicit gs: golem.data.GolemSchema[A]): ConfigOverride = {
    val encoded = gs.encodeElement(value) match {
      case Right(golem.data.ElementValue.Component(dv)) => dv
      case Right(_)                                     =>
        throw new IllegalArgumentException(
          s"Expected component value for config override at ${path.mkString(".")}"
        )
      case Left(err) =>
        throw new IllegalArgumentException(
          s"Failed to encode config override at ${path.mkString(".")}: $err"
        )
    }
    new ConfigOverride(path, encoded, gs.elementSchema)
  }
}
