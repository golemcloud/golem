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

package golem

import scala.annotation.unused
import scala.scalajs.js

final class AgentMethodProxy extends Selectable {
  private val fns: js.Dictionary[js.Any] = js.Dictionary.empty

  def register(name: String, fn: js.Any): Unit = fns(name) = fn

  def selectDynamic(name: String): Any =
    fns.getOrElse(name, throw new NoSuchMethodError(name))

  def applyDynamic(name: String, @unused paramTypes: Class[?]*)(args: Any*): Any = {
    val fn     = fns.getOrElse(name, throw new NoSuchMethodError(s"No agent method: $name"))
    val jsArgs = args.map(_.asInstanceOf[js.Any])
    jsArgs.length match {
      case 0 => fn.asInstanceOf[js.Function0[Any]].apply()
      case 1 => fn.asInstanceOf[js.Function1[js.Any, Any]].apply(jsArgs(0))
      case 2 => fn.asInstanceOf[js.Function2[js.Any, js.Any, Any]].apply(jsArgs(0), jsArgs(1))
      case 3 =>
        fn.asInstanceOf[js.Function3[js.Any, js.Any, js.Any, Any]].apply(jsArgs(0), jsArgs(1), jsArgs(2))
      case 4 =>
        fn.asInstanceOf[js.Function4[js.Any, js.Any, js.Any, js.Any, Any]]
          .apply(jsArgs(0), jsArgs(1), jsArgs(2), jsArgs(3))
      case n => throw new UnsupportedOperationException(s"Agent method $name: unsupported arity $n (max 4)")
    }
  }
}
