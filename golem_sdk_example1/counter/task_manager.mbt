///|
/// Priority level for tasks
#derive.golem_schema
pub(all) enum Priority {
  Low
  Medium
  High
} derive(Eq)

///|
/// Information about a task
#derive.golem_schema
pub(all) struct TaskInfo {
  title : String
  priority : Priority
  description : String?
}

///|
fn priority_to_string(p : Priority) -> String {
  match p {
    Low => "low"
    Medium => "medium"
    High => "high"
  }
}

///|
/// A task management agent
#derive.agent
struct TaskManager {
  tasks : Array[TaskInfo]
}

///|
let task_logger : @logging.Logger = @logging.with_name("task-manager")

///|
/// Creates a new empty task manager
fn TaskManager::new() -> TaskManager {
  task_logger.info("Creating new task manager")
  { tasks: [] }
}

///|
/// Adds a new task, returns the total task count
pub fn TaskManager::add_task(self : Self, task : TaskInfo) -> UInt64 {
  @context.with_span(
    "task_manager.add_task",
    attributes=[
      ("task.title", task.title),
      ("task.priority", priority_to_string(task.priority)),
    ],
    fn(_span) {
      task_logger.info("Adding task: " + task.title)
      self.tasks.push(task)
      self.tasks.length().reinterpret_as_uint().to_uint64()
    },
  )
}

///|
/// Gets all tasks
pub fn TaskManager::get_tasks(self : Self) -> Array[TaskInfo] {
  self.tasks
}

///|
/// Gets tasks filtered by priority
pub fn TaskManager::get_by_priority(
  self : Self,
  priority : Priority,
) -> Array[TaskInfo] {
  let result : Array[TaskInfo] = []
  for t in self.tasks {
    if t.priority == priority {
      result.push(t)
    }
  }
  result
}
