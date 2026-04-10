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

package example.templates

import golem.runtime.annotations.{agentDefinition, description, prompt}
import golem.BaseAgent

import scala.concurrent.Future

@agentDefinition()
@description("Human-in-the-loop workflow using Golem promises (Scala equivalent of the Rust/TS HITL template).")
trait ApprovalWorkflow extends BaseAgent {

  class Id(val value: String)

  @prompt("Start approval process")
  @description("Starts a workflow that requires human approval before continuing")
  def begin(): Future[String]

  @description("Wait until the approval decision is provided, then return it")
  def awaitOutcome(): Future[String]

  @description("Complete the workflow decision")
  def complete(decision: String): Future[Boolean]
}

@agentDefinition(typeName = "Human")
@description("A minimal 'human' agent that can approve/reject workflows (used by ApprovalWorkflow examples).")
trait HumanAgent extends BaseAgent {

  class Id(val value: String)

  @prompt("Approve or reject a workflow")
  @description("Makes a decision on a workflow approval request")
  def decide(workflowId: String, decision: String): Future[String]
}
