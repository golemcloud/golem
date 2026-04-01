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

object ConfigHolder {
  private var _current: Option[Config[_]] = None

  private[golem] def set[T](config: Config[T]): Unit =
    _current = Some(config)

  private[golem] def clear(): Unit =
    _current = None

  def current[T]: Config[T] =
    _current match {
      case Some(c) => c.asInstanceOf[Config[T]]
      case None    =>
        throw new IllegalStateException(
          "No config is available. Ensure your agent trait extends AgentConfig[T] and an implicit Schema[T] is provided for your config type."
        )
    }
}
