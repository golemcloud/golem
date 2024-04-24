mod bindings;

use crate::bindings::exports::golem::it::api::*;

struct Component;

impl Guest for Component {
    fn create_task(input: Task) -> Task {
        let permissions = if (input.permissions & Permissions::EXEC) == Permissions::empty() {
            input.permissions | Permissions::EXEC
        } else {
            input.permissions
        };

        Task {
            name: input.name,
            permissions,
        }
    }

    fn get_tasks() -> Vec<Task> {
        vec![
            Task {
                name: "t1".to_string(),
                permissions: Permissions::READ,
            },
            Task {
                name: "t2".to_string(),
                permissions: Permissions::READ | Permissions::EXEC | Permissions::CLOSE,
            },
        ]
    }
}
