use crate::bindings::exports::golem::itrpc_exports::rpc_api::Guest;
use crate::bindings::golem::it_client::child_component_client::{Api, Data};
use std::cell::RefCell;
use std::env;

#[allow(static_mut_refs)]
mod bindings;

struct Component;

struct State {
    api: Option<Api>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State { api: None });
}

fn with_api<T>(f: impl FnOnce(&Api) -> T) -> T {
    STATE.with_borrow_mut(|state| match &state.api {
        None => {
            let remote_component_id =
                env::var("CHILD_COMPONENT_ID").expect("CHILD_COMPONENT_ID not set");

            let uuid = bindings::golem::rpc::types::parse_uuid(&remote_component_id).unwrap();
            let worker_name = env::var("CHILD_WORKER_NAME").expect("CHILD_WORKER_NAME not set");

            let api = Api::custom(&bindings::golem::rpc::types::WorkerId {
                component_id: bindings::golem::rpc::types::ComponentId { uuid },
                worker_name,
            });
            let result = f(&api);
            state.api = Some(api);
            result
        }
        Some(api) => f(api),
    })
}

impl Guest for Component {
    fn echo(input: String) -> String {
        with_api(|api| api.blocking_echo(input.as_str()))
    }

    fn calculate(input: u64) -> u64 {
        with_api(|api| api.blocking_calculate(input))
    }

    fn process(input: Vec<Data>) -> Vec<Data> {
        with_api(|api| api.blocking_process(&input))
    }
}

bindings::export!(Component with_types_in bindings);
