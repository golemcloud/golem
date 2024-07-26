mod bindings;

use crate::bindings::golem::api::host::*;
use crate::bindings::Guest;

struct Component;

impl Guest for Component {
    fn run() -> Vec<u8> {
        let promise_id = create_promise();
        await_promise(&promise_id)
    }
}

bindings::export!(Component with_types_in bindings);
