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
/// A task management agent
#derive.agent
struct TaskManager {
  tasks : Array[TaskInfo]
}

///|
/// Creates a new empty task manager
fn TaskManager::new() -> TaskManager {
  { tasks: [] }
}

///|
/// Adds a new task, returns the total task count
pub fn TaskManager::add_task(self : Self, task : TaskInfo) -> UInt64 {
  self.tasks.push(task)
  self.tasks.length().reinterpret_as_uint().to_uint64()
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


