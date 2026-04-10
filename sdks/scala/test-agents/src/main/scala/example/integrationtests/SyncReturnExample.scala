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

package example.integrationtests

import golem.runtime.annotations.{agentDefinition, description}
import golem.BaseAgent

@agentDefinition()
@description("Demonstrates agent methods with synchronous return types.")
trait SyncReturnAgent extends BaseAgent {
  class Id()

  @description("Returns a greeting synchronously.")
  def greet(name: String): String

  @description("Adds two numbers synchronously.")
  def add(a: Int, b: Int): Int

  @description("Stores a tag without returning a value.")
  def touch(tag: String): Unit

  @description("Returns the last stored tag.")
  def lastTag(): String
}
