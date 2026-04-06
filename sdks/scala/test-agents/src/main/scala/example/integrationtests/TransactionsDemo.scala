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

import scala.concurrent.Future

@agentDefinition()
@description("Demonstrates the Transactions API: infallible and fallible saga-style transactions.")
trait TransactionsDemo extends BaseAgent {

  class Id(val value: String)

  @description("Runs an infallible transaction with multiple operations (success path).")
  def infallibleDemo(): Future[String]

  @description("Runs a fallible transaction where all operations succeed.")
  def fallibleSuccessDemo(): Future[String]

  @description("Runs a fallible transaction where the last operation fails, triggering compensations.")
  def fallibleFailureDemo(): Future[String]
}
