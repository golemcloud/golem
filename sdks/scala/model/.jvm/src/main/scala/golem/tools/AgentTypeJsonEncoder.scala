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

package golem.tools

import golem.runtime.AgentMetadata
import ujson._

object AgentTypeJsonEncoder {
  def encode(agentName: String, metadata: AgentMetadata): Value = {
    val methods = Arr(
      metadata.methods.map { method =>
        Obj(
          "name"        -> Str(method.name),
          "description" -> Str(method.description.getOrElse("")),
          "prompt"      -> method.prompt.map(Str(_)).getOrElse(Null),
          "input"       -> SchemaJsonEncoder.encode(method.input),
          "output"      -> SchemaJsonEncoder.encode(method.output)
        )
      }: _*
    )

    Obj(
      "name"        -> Str(agentName),
      "description" -> Str(metadata.description.getOrElse("")),
      "constructor" -> SchemaJsonEncoder.encode(metadata.constructor),
      "methods"     -> methods
    )
  }
}
