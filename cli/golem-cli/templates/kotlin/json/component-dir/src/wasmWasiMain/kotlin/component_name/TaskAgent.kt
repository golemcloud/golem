package component_name

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Description
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.Prompt
import kotlin.time.Clock

/** Data classes are mapped to JSON objects by the agent's HTTP API (and to schema records for RPC). */
data class Task(val id: Int, val title: String, val completed: Boolean, val createdAt: String)

data class CreateTaskRequest(val title: String)

/**
 * A task list that speaks JSON: request bodies and results are data classes, lists and nullable values.
 *
 * Mounted at /task-agents/{name}: each unique {name} is a separate agent instance with its own tasks.
 */
@Agent(mount = "/task-agents/{name}", description = "A task list with a JSON HTTP API")
class TaskAgent(val name: String) : BaseAgent() {
    private var nextId = 1
    private var tasks = listOf<Task>()

    @Prompt("Create a new task with the given title")
    @Description("Creates a task and returns the complete task object")
    @Endpoint(post = "/tasks")
    fun createTask(request: CreateTaskRequest): Task {
        val task = Task(nextId, request.title, false, Clock.System.now().toString())
        nextId += 1
        tasks = tasks + task
        return task
    }

    @Prompt("List all existing tasks")
    @Description("Returns all tasks as a JSON array")
    @Endpoint(get = "/tasks")
    fun getTasks(): List<Task> = tasks

    @Description("Marks a task as completed by its ID")
    @Endpoint(post = "/tasks/{id}/complete")
    fun completeTask(id: Int): Task? {
        val task = tasks.find { it.id == id } ?: return null
        val updated = task.copy(completed = true)
        tasks = tasks.map { if (it.id == id) updated else it }
        return updated
    }
}
