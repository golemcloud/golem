wit_bindgen::generate!({
    path: "wit",
    world: "todo-worker"
});

use crate::todo::personal::types::{
    CreateTaskInput, UpdateProfileInput, UpdateTaskInput, JsonResponse, Timestamp
};
use crate::exports::todo::personal::{profile, tasks};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Profile {
    name: String,
    email: String,
    created_at: Timestamp,
    updated_at: Timestamp,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct Task {
    id: u64,
    title: String,
    description: String,
    completed: bool,
    due_date: Option<Timestamp>,
    created_at: Timestamp,
    updated_at: Timestamp,
}

struct TodoWorker {
    tasks: Vec<Task>,
}

impl Default for TodoWorker {
    fn default() -> Self {
        let tasks = vec![
            Task {
                id: 1,
                title: "Complete Project Proposal".to_string(),
                description: "Write and submit the Q2 project proposal".to_string(),
                completed: false,
                due_date: Some(1683849600000), // May 12, 2023
                created_at: 1683676800000,     // May 10, 2023
                updated_at: 1683676800000,
            },
            Task {
                id: 2,
                title: "Review Code Changes".to_string(),
                description: "Review pending pull requests for the main branch".to_string(),
                completed: true,
                due_date: Some(1683763200000), // May 11, 2023
                created_at: 1683676800000,
                updated_at: 1683763200000,
            },
            Task {
                id: 3,
                title: "Update Documentation".to_string(),
                description: "Update API documentation with new endpoints".to_string(),
                completed: false,
                due_date: Some(1684108800000), // May 15, 2023
                created_at: 1683676800000,
                updated_at: 1683676800000,
            },
            Task {
                id: 4,
                title: "Team Meeting".to_string(),
                description: "Weekly team sync meeting".to_string(),
                completed: false,
                due_date: Some(1683936000000), // May 13, 2023
                created_at: 1683676800000,
                updated_at: 1683676800000,
            },
            Task {
                id: 5,
                title: "Deploy Updates".to_string(),
                description: "Deploy latest changes to production".to_string(),
                completed: true,
                due_date: Some(1684022400000), // May 14, 2023
                created_at: 1683676800000,
                updated_at: 1684022400000,
            },
        ]
        .into_iter()
        .collect();

        TodoWorker { tasks }
    }
}

impl profile::Guest for TodoWorker {
    fn get() -> JsonResponse {
        let profile = Profile {
            name: "John Doe".to_string(),
            email: "john@example.com".to_string(),
            created_at: 1234567890000,
            updated_at: 1234567890000,
        };

        JsonResponse {
            status: 200,
            body: serde_json::to_string(&profile).unwrap_or_default(),
        }
    }

    fn update(input: UpdateProfileInput) -> JsonResponse {
        let profile = Profile {
            name: input.name.unwrap_or_else(|| "John Doe".to_string()),
            email: input.email.unwrap_or_else(|| "john@example.com".to_string()),
            created_at: 1234567890000,
            updated_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
        };

        JsonResponse {
            status: 200,
            body: serde_json::to_string(&profile).unwrap_or_default(),
        }
    }
}

impl tasks::Guest for TodoWorker {
    fn create(input: CreateTaskInput) -> JsonResponse {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        let task = Task {
            id: 1,
            title: input.title,
            description: input.description,
            completed: false,
            due_date: input.due_date,
            created_at: now,
            updated_at: now,
        };

        JsonResponse {
            status: 201,
            body: serde_json::to_string(&task).unwrap_or_default(),
        }
    }

    fn get(id: u64) -> JsonResponse {
        if id == 1 {
            let task = Task {
                id,
                title: "Example Task".to_string(),
                description: "This is an example task".to_string(),
                completed: false,
                due_date: None,
                created_at: 1234567890000,
                updated_at: 1234567890000,
            };

            JsonResponse {
                status: 200,
                body: serde_json::to_string(&task).unwrap_or_default(),
            }
        } else {
            JsonResponse {
                status: 404,
                body: r#"{"error": "Task not found"}"#.to_string(),
            }
        }
    }

    fn update(id: u64, input: UpdateTaskInput) -> JsonResponse {
        let mut task = match Self::get(id) {
            response if response.status == 404 => return response,
            response => serde_json::from_str::<Task>(&response.body)
                .unwrap_or_else(|_| Task {
                    id,
                    title: "".to_string(),
                    description: "".to_string(),
                    completed: false,
                    due_date: None,
                    created_at: 0,
                    updated_at: 0,
                }),
        };

        if let Some(title) = input.title {
            task.title = title;
        }
        if let Some(description) = input.description {
            task.description = description;
        }
        if let Some(completed) = input.completed {
            task.completed = completed;
        }
        if let Some(due_date) = input.due_date {
            task.due_date = Some(due_date);
        }

        task.updated_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        JsonResponse {
            status: 200,
            body: serde_json::to_string(&task).unwrap_or_default(),
        }
    }

    fn delete(id: u64) -> JsonResponse {
        match Self::get(id) {
            response if response.status == 404 => response,
            _ => JsonResponse {
                status: 204,
                body: "".to_string(),
            },
        }
    }

    fn get_all() -> JsonResponse {
        let worker = TodoWorker::default();
        JsonResponse {
            status: 200,
            body: serde_json::to_string(&worker.tasks).unwrap_or_default(),
        }
    }

    fn list_due_before(before: u64) -> JsonResponse {
        let worker = TodoWorker::default();
        let filtered_tasks: Vec<_> = worker.tasks
            .into_iter()
            .filter(|task| task.due_date.map_or(false, |due| due < before))
            .collect();

        JsonResponse {
            status: 200,
            body: serde_json::to_string(&filtered_tasks).unwrap_or_default(),
        }
    }

    fn list_completed() -> JsonResponse {
        let worker = TodoWorker::default();
        let filtered_tasks: Vec<_> = worker.tasks
            .into_iter()
            .filter(|task| task.completed)
            .collect();

        JsonResponse {
            status: 200,
            body: serde_json::to_string(&filtered_tasks).unwrap_or_default(),
        }
    }

    fn list_incomplete() -> JsonResponse {
        let worker = TodoWorker::default();
        let filtered_tasks: Vec<_> = worker.tasks
            .into_iter()
            .filter(|task| !task.completed)
            .collect();

        JsonResponse {
            status: 200,
            body: serde_json::to_string(&filtered_tasks).unwrap_or_default(),
        }
    }
}

export!(TodoWorker); 