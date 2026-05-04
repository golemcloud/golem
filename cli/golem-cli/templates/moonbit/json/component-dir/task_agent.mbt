///|
/// A task record
#derive.golem_schema
pub(all) struct Task {
  id : Int
  title : String
  mut completed : Bool
  created_at : String
} derive(ToJson, @json.FromJson)

///|
/// Request to create a new task
#derive.golem_schema
pub(all) struct CreateTaskRequest {
  title : String
} derive(ToJson, @json.FromJson)

///|
/// Task agent in MoonBit
#derive.agent
#derive.mount("/task-agents/{name}")
struct TaskAgent {
  name : String
  mut next_id : Int
  tasks : Array[Task]
}

///|
/// Creates a new task agent with the given name
fn TaskAgent::new(name : String) -> TaskAgent {
  { name, next_id: 1, tasks: [] }
}

///|
/// Creates a task and returns the complete task object
#derive.prompt_hint("Create a new task with the given title")
#derive.endpoint(post="/tasks")
pub fn TaskAgent::create_task(self : Self, request : CreateTaskRequest) -> Task {
  let task : Task = {
    id: self.next_id,
    title: request.title,
    completed: false,
    created_at: "",
  }
  self.next_id += 1
  self.tasks.push(task)
  task
}

///|
/// Returns all tasks as a JSON array
#derive.prompt_hint("List all existing tasks")
#derive.endpoint(get="/tasks")
pub fn TaskAgent::get_tasks(self : Self) -> Array[Task] {
  self.tasks
}

///|
/// Marks a task as completed by its ID
#derive.endpoint(post="/tasks/{id}/complete")
pub fn TaskAgent::complete_task(self : Self, id : Int) -> Task? {
  for t in self.tasks {
    if t.id == id {
      t.completed = true
      return Some(t)
    }
  }
  None
}

///|
fn main {

}
