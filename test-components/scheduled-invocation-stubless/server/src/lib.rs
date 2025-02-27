mod bindings;

use std::borrow::BorrowMut;

use once_cell::sync::Lazy;

use crate::bindings::exports::it::scheduled_invocation_stubless_server_exports::server_api::Guest;

pub struct Component;

struct State {
    global: u64,
}

static mut STATE: Lazy<State> = Lazy::new(|| State { global: 0 } );

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut state = unsafe { STATE.borrow_mut() };
    f(&mut state)
}

impl Guest for Component {
    fn get_global_value() -> u64 {
        with_state(|state| state.global)
    }

    fn inc_global_by(value: u64) {
        with_state(|state| {
            state.global += value;
        });
    }
}

bindings::export!(Component with_types_in bindings);
