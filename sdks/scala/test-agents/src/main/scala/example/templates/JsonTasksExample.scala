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
import zio.blocks.schema.Schema

import scala.concurrent.Future

final case class Task(id: Int, title: String, completed: Boolean, createdAt: String)
object Task {
  implicit val schema: Schema[Task] = Schema.derived
}

final case class CreateTaskRequest(title: String)
object CreateTaskRequest {
  implicit val schema: Schema[CreateTaskRequest] = Schema.derived
}

@agentDefinition()
@description("A simple agent demonstrating JSON API support (Scala equivalent of the Rust/TS JSON template).")
trait Tasks extends BaseAgent {

  class Id(val value: String)

  @prompt("Create a new task with the given title")
  @description("Creates a task and returns the complete task object")
  def createTask(request: CreateTaskRequest): Future[Task]

  @prompt("List all existing tasks")
  @description("Returns all tasks as a JSON array")
  def getTasks(): Future[List[Task]]

  @description("Marks a task as completed by its ID")
  def completeTask(id: Int): Future[Option[Task]]
}
