#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::exports::pa_ck::na_me_exports::component_name_api::*;
// Import for using common lib (also see Cargo.toml for adding the dependency):
// use common_lib::example_common_function;
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
    /// Updates the component's state by adding the given value to the total.
    fn add(value: u64) {
        STATE.with_borrow_mut(|state| state.total += value);
    }

    /// Returns the current total.
    fn get() -> u64 {
        // Call code from shared lib
        // println!("{}", example_common_function());

        STATE.with_borrow(|state| state.total)
    }
}

bindings::export!(Component with_types_in bindings);
