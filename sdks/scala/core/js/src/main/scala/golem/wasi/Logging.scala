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
 * Scala.js facade for WASI logging (`wasi:logging/logging`).
 *
 * WIT interface:
 * {{{
 *   enum level { trace, debug, info, warn, error, critical }
 *   log: func(level: level, context: string, message: string)
 * }}}
 */
object Logging {

  sealed abstract class Level(val value: String)
  object Level {
    case object Trace    extends Level("trace")
    case object Debug    extends Level("debug")
    case object Info     extends Level("info")
    case object Warn     extends Level("warn")
    case object Error    extends Level("error")
    case object Critical extends Level("critical")
  }

  @js.native
  @JSImport("wasi:logging/logging", JSImport.Namespace)
  private object LoggingModule extends js.Object {
    def log(level: String, context: String, message: String): Unit = js.native
  }

  def log(level: Level, context: String, message: String): Unit =
    LoggingModule.log(level.value, context, message)

  def trace(message: String, context: String = ""): Unit =
    log(Level.Trace, context, message)

  def debug(message: String, context: String = ""): Unit =
    log(Level.Debug, context, message)

  def info(message: String, context: String = ""): Unit =
    log(Level.Info, context, message)

  def warn(message: String, context: String = ""): Unit =
    log(Level.Warn, context, message)

  def error(message: String, context: String = ""): Unit =
    log(Level.Error, context, message)

  def critical(message: String, context: String = ""): Unit =
    log(Level.Critical, context, message)

  def raw: Any = LoggingModule
}
