mod bindings;

use crate::bindings::exports::golem::it::api::{Guest, PromiseId};
use crate::bindings::golem::api::host::*;

struct Component;

impl Guest for Component {
    fn create() -> PromiseId {
        create_promise()
    }

    fn await_(id: PromiseId) -> Vec<u8> {
        await_promise(&id)
    }

    fn poll(id: PromiseId) -> Option<Vec<u8>> {
        poll_promise(&id)
    }
}

bindings::export!(Component with_types_in bindings);
