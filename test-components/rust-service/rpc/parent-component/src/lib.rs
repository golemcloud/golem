use crate::bindings::exports::golem::itrpc::rpc_api::Guest;
use crate::bindings::golem::it_stub::stub_child_component::{Api, Data};
use crate::bindings::golem::rpc::types::Uri;
use std::env;

mod bindings;

struct Component;

impl Guest for Component {
    fn echo(input: String) -> String {
        let remote_component_id =
            env::var("CHILD_COMPONENT_ID").expect("PARENT_COMPONENT_ID not set");

        let remote_worker_name = env::var("CHILD_WORKER_NAME").expect("CHILD_WORKER_NAME not set");

        let uri = Uri {
            value: format!("worker://{remote_component_id}/{remote_worker_name}"),
        };

        let api = Api::new(&uri);

        api.blocking_echo(input.as_str())
    }

    fn calculate(input: u64) -> u64 {
        let remote_component_id =
            env::var("CHILD_COMPONENT_ID").expect("PARENT_COMPONENT_ID not set");

        let remote_worker_name = env::var("CHILD_WORKER_NAME").expect("CHILD_WORKER_NAME not set");

        let uri = Uri {
            value: format!("worker://{remote_component_id}/{remote_worker_name}"),
        };

        let api = Api::new(&uri);

        api.blocking_calculate(input)
    }

    fn process(input: Vec<Data>) -> Vec<Data> {
        let remote_component_id =
            env::var("CHILD_COMPONENT_ID").expect("PARENT_COMPONENT_ID not set");

        let remote_worker_name = env::var("CHILD_WORKER_NAME").expect("CHILD_WORKER_NAME not set");

        let uri = Uri {
            value: format!("worker://{remote_component_id}/{remote_worker_name}"),
        };

        let api = Api::new(&uri);

        api.blocking_process(&input)
    }
}

bindings::export!(Component with_types_in bindings);
