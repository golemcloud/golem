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

package golem.runtime

import golem.data.GolemSchema

/**
 * Reflected structure of an agent trait for client-side calling: schemas, WIT
 * function names, and invocation kind.
 */
final case class AgentType[Trait, Constructor](
  traitClassName: String,
  typeName: String,
  constructor: ConstructorType[Constructor],
  methods: List[AgentType.AnyMethod[Trait]]
)

final case class ConstructorType[Input](schema: GolemSchema[Input])

final case class AgentMethod[Trait, Input, Output](
  metadata: MethodMetadata,
  functionName: String,
  inputSchema: GolemSchema[Input],
  outputSchema: GolemSchema[Output],
  invocation: MethodInvocation
)

object AgentType {
  type AnyMethod[Trait] = AgentMethod[Trait, _, _]
}

sealed trait MethodInvocation

object MethodInvocation {
  case object Awaitable     extends MethodInvocation
  case object FireAndForget extends MethodInvocation
}
