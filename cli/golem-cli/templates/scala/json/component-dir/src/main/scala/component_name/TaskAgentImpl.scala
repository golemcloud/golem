package component_name

import golem.runtime.annotations.agentImplementation

import scala.annotation.unused
import scala.concurrent.Future
import scala.scalajs.js

@agentImplementation()
final class TaskAgentImpl(@unused private val name: String) extends TaskAgent {
  private var nextId: Int = 1
  private var tasks: List[Task] = Nil

  override def createTask(request: CreateTaskRequest): Future[Task] =
    Future.successful {
      val task = Task(
        id = nextId,
        title = request.title,
        completed = false,
        createdAt = new js.Date().toISOString()
      )
      nextId += 1
      tasks = tasks :+ task
      task
    }

  override def getTasks(): Future[List[Task]] =
    Future.successful(tasks)

  override def completeTask(id: Int): Future[Option[Task]] =
    Future.successful {
      tasks.find(_.id == id).map { t =>
        val updated = t.copy(completed = true)
        tasks = tasks.map(curr => if (curr.id == id) updated else curr)
        updated
      }
    }
}
