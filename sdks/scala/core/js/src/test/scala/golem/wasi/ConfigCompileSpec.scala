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

import zio.test._

object ConfigCompileSpec extends ZIOSpecDefault {
  import Config._

  private val errors: List[ConfigError] = List(
    ConfigError.Upstream("upstream error"),
    ConfigError.Io("io error")
  )

  private def describeError(e: ConfigError): String = e match {
    case ConfigError.Upstream(msg) => s"upstream($msg)"
    case ConfigError.Io(msg)       => s"io($msg)"
  }

  def spec = suite("ConfigCompileSpec")(
    test("ConfigError exhaustive match") {
      errors.foreach(e => assertTrue(describeError(e).nonEmpty))
      assertTrue(true)
    },
    test("ConfigError field access") {
      assertTrue(
        errors.head.asInstanceOf[ConfigError.Upstream].message == "upstream error",
        errors(1).asInstanceOf[ConfigError.Io].message == "io error"
      )
    },
    test("Either result type usage") {
      val result: Either[ConfigError, Option[String]]         = Right(Some("value"))
      val allResult: Either[ConfigError, Map[String, String]] = Right(Map("k" -> "v"))

      result match {
        case Right(Some(v)) =>
          allResult match {
            case Right(m) =>
              assertTrue(
                v == "value",
                m.size == 1
              )
            case Left(_) => assertTrue(false)
          }
        case Right(None)                     => assertTrue(false)
        case Left(ConfigError.Upstream(msg)) => assertTrue(false)
        case Left(ConfigError.Io(msg))       => assertTrue(false)
      }
    }
  )
}
