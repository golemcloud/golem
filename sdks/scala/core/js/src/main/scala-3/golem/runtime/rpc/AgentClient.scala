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

package golem.runtime.rpc

import golem.runtime.macros.AgentClientMacro
import golem.runtime.AgentType

object AgentClient {
  transparent inline def agentType[Trait]: AgentType[Trait, ?] =
    AgentClientMacro.agentType[Trait]

  /**
   * Typed agent-type accessor (no user-land casts).
   *
   * This exists because Scala.js cannot safely cast a plain JS object to a
   * Scala trait at runtime. When you need to operate at the "agent type +
   * resolved client" level (e.g. in internal wiring), use this API to keep
   * examples cast-free. Constructor type is always Unit (temporary).
   */
  transparent inline def agentTypeWithCtor[Trait, Constructor]: AgentType[Trait, Constructor] =
    ${ AgentTypeMacro.agentTypeWithCtorImpl[Trait, Constructor] }
}

private object AgentTypeMacro {
  import scala.quoted.*

  def agentTypeWithCtorImpl[Trait: Type, Constructor: Type](using Quotes): Expr[AgentType[Trait, Constructor]] = {
    import quotes.reflect.*

    val traitRepr   = TypeRepr.of[Trait]
    val traitSymbol = traitRepr.typeSymbol

    if !traitSymbol.flags.is(Flags.Trait) then
      report.errorAndAbort(s"Agent client target must be a trait, found: ${traitSymbol.fullName}")

    '{ AgentClientMacro.agentType[Trait].asInstanceOf[AgentType[Trait, Constructor]] }
  }
}
