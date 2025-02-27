mod bindings;

use crate::bindings::exports::pack::name::api::*;
use std::cell::RefCell;

/// This is one of any number of data types that our application
/// uses. Golem will take care to persist all application state,
/// whether that state is local to a function being executed or
/// global across the entire program.
struct State {
    total: u64,
}

thread_local! {
    /// This holds the state of our application.
    static STATE: RefCell<State> = RefCell::new(State {
        total: 0,
    });
}

struct Component;

impl Guest for Component {
    fn add(value: u64) {
        STATE.with_borrow_mut(|state| state.total += value);
    }

    fn get() -> u64 {
        STATE.with_borrow(|state| state.total)
    }
}

bindings::export!(Component with_types_in bindings);
