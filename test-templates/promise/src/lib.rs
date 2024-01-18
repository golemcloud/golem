cargo_component_bindings::generate!();

use crate::bindings::golem::api::host::*;
use crate::bindings::Guest;

struct Component;

impl Guest for Component {
    fn run() -> Vec<u8> {
        let promise_id = golem_create_promise();
        golem_await_promise(&promise_id)
    }
}
