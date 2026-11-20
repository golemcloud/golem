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

import scala.language.dynamics
import scala.quoted.*

final class Config[T] private[golem] (private val loadFn: () => T) extends Dynamic {
  def value: T = loadFn()

  transparent inline def selectDynamic(inline name: String): Any = ${ ConfigAccess.access[T]('this, 'name) }
}

object Config {
  private[golem] def apply[T](loadFn: () => T): Config[T] = new Config[T](loadFn)
  private[golem] def eager[T](value: T): Config[T]        = new Config[T](() => value)
}

private[golem] object ConfigAccess {
  def access[T: Type](config: Expr[Config[T]], name: Expr[String])(using Quotes): Expr[Any] = {
    import quotes.reflect.*
    val field = name.valueOrAbort
    if (!TypeRepr.of[T].typeSymbol.caseFields.exists(_.name == field))
      report.errorAndAbort(s"No config field '$field' in ${Type.show[T]}")
    Select.unique('{ $config.value }.asTerm, field).asExpr
  }
}
