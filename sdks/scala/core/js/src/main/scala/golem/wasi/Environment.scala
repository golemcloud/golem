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

package golem.wasi

import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport

/**
 * Scala.js facade for `wasi:cli/environment@0.2.3`.
 */
object Environment {
  @js.native
  @JSImport("wasi:cli/environment@0.2.3", JSImport.Namespace)
  private object EnvModule extends js.Object {
    def getEnvironment(): js.Array[js.Tuple2[String, String]] = js.native
  }

  def raw: Any =
    EnvModule

  def getEnvironment(): Map[String, String] =
    EnvModule
      .getEnvironment()
      .toSeq
      .map { kv =>
        kv._1 -> kv._2
      }
      .toMap
}
