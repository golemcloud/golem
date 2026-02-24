use chrono::{DateTime, Utc};
use golem_rust::{agent_definition, agent_implementation, description, prompt, Schema, endpoint};

#[derive(Clone, Schema)]
pub struct Task {
    id: usize,
    title: String,
    completed: bool,
    created_at: DateTime<Utc>,
}

#[derive(Schema)]
pub struct CreateTaskRequest {
    title: String,
}

#[agent_definition(
    mount = "/task-agents/{name}",
    cors = [ "*" ]
)]
#[description("An agent managing a named set of tasks")]
pub trait Tasks {
    fn new(name: String) -> Self;

    #[prompt("Create a new task with the given title")]
    #[description("Creates a task and returns the complete task object")]
    #[endpoint(post = "/tasks")]
    fn create_task(&mut self, request: CreateTaskRequest) -> Task;

    #[prompt("List all existing tasks")]
    #[description("Returns all tasks as a JSON array")]
    #[endpoint(get = "/tasks")]
    fn get_tasks(&self) -> Vec<Task>;

    #[description("Marks a task as completed by its ID")]
    #[endpoint(get = "/tasks/{id}/complete")]
    fn complete_task(&mut self, id: usize) -> Option<Task>;
}

struct TasksImpl {
    _name: String,
    tasks: Vec<Task>,
}

#[agent_implementation]
impl Tasks for TasksImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            tasks: vec![],
        }
    }

    fn create_task(&mut self, request: CreateTaskRequest) -> Task {
        let id = self.tasks.len() + 1;
        let task = Task {
            id,
            title: request.title,
            completed: false,
            created_at: Utc::now(),
        };
        self.tasks.insert(id, task.clone());
        task
    }

    fn get_tasks(&self) -> Vec<Task> {
        self.tasks.clone()
    }

    fn complete_task(&mut self, id: usize) -> Option<Task> {
        self.tasks.iter_mut().find(|t| t.id == id).map(|t| {
            t.completed = true;
            t.clone()
        })
    }
}
