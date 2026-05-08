package component_name

import golem.runtime.annotations.{agentDefinition, description, endpoint, prompt}
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

@agentDefinition(mount = "/task-agents/{name}")
trait TaskAgent extends BaseAgent {

  class Id(val name: String)

  @prompt("Create a new task with the given title")
  @description("Creates a task and returns the complete task object")
  @endpoint(method = "POST", path = "/tasks")
  def createTask(request: CreateTaskRequest): Future[Task]

  @prompt("List all existing tasks")
  @description("Returns all tasks as a JSON array")
  @endpoint(method = "GET", path = "/tasks")
  def getTasks(): Future[List[Task]]

  @description("Marks a task as completed by its ID")
  @endpoint(method = "POST", path = "/tasks/{id}/complete")
  def completeTask(id: Int): Future[Option[Task]]
}
