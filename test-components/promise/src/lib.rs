#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::exports::golem::it::api::{Guest, PromiseId};
use crate::bindings::golem::api::host::*;

struct Component;

impl Guest for Component {
    fn create() -> PromiseId {
        create_promise()
    }

    fn await_(id: PromiseId) -> Vec<u8> {
        let promise = get_promise(&id);
        promise.subscribe().block();
        promise.get().unwrap()
    }

    fn poll(id: PromiseId) -> Option<Vec<u8>> {
        get_promise(&id).get()
    }
}

bindings::export!(Component with_types_in bindings);
