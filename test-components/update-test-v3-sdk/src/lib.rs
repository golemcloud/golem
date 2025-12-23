#[allow(static_mut_refs)]
#[allow(unused_imports)]
mod bindings;

use std::cell::RefCell;
use bytes::{Buf, BufMut, Bytes};
use crate::bindings::exports::golem::component::api::Guest;

struct State {
    last: u64,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State { last: 0 });
}

struct Component;

impl Guest for Component {
    fn get() -> u64 {
        STATE.with_borrow(|state| state.last)
    }

    fn set(value: u64) -> u64 {
        STATE.with_borrow_mut(|state| {
            state.last = value;
            state.last
        })
    }
}

impl golem_rust::save_snapshot::exports::golem::api::save_snapshot::Guest for Component {
    fn save() -> Vec<u8> {
        let mut result = Vec::new();
        result.put_u64(Component::get());
        result
    }
}

impl golem_rust::load_snapshot::exports::golem::api::load_snapshot::Guest for Component {
    fn load(bytes: Vec<u8>) -> Result<(), String> {
        if bytes.len() >= 8 {
            Component::set(Bytes::from(bytes).get_u64());
            Ok(())
        } else {
            Err("Invalid snapshot - not enough bytes to read u64".to_string())
        }
    }
}

bindings::export!(Component with_types_in bindings);
golem_rust::save_snapshot::export_save_snapshot!(Component with_types_in golem_rust::save_snapshot);
golem_rust::load_snapshot::export_load_snapshot!(Component with_types_in golem_rust::load_snapshot);
