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
 * Scala.js facade for WASI config store (`wasi:config/store@0.2.0-draft`).
 *
 * WIT interface:
 * {{{
 *   variant error { upstream(string), io(string) }
 *   get: func(key: string) -> result<option<string>, error>
 *   get-all: func() -> result<list<tuple<string, string>>, error>
 * }}}
 */
object Config {

  sealed trait ConfigError extends Product with Serializable
  object ConfigError {
    final case class Upstream(message: String) extends ConfigError
    final case class Io(message: String)       extends ConfigError
  }

  @js.native
  @JSImport("wasi:config/store@0.2.0-draft", JSImport.Namespace)
  private object StoreModule extends js.Object {
    def get(key: String): js.Any                      = js.native
    def getAll(): js.Array[js.Tuple2[String, String]] = js.native
  }

  def get(key: String): Either[ConfigError, Option[String]] =
    try {
      val result = StoreModule.get(key)
      if (js.isUndefined(result) || result == null) Right(None)
      else Right(Some(result.asInstanceOf[String]))
    } catch { case t: Throwable => Left(toConfigError(t)) }

  def getAll(): Either[ConfigError, Map[String, String]] =
    try {
      val arr = StoreModule.getAll()
      Right(arr.toSeq.map(kv => kv._1 -> kv._2).toMap)
    } catch { case t: Throwable => Left(toConfigError(t)) }

  private def toConfigError(t: Throwable): ConfigError = {
    val msg = if (t.getMessage != null) t.getMessage else t.toString
    ConfigError.Upstream(msg)
  }

  def raw: Any = StoreModule
}
