mod bindings;

use crate::bindings::exports::pa_ck::na_me_exports::component_name_api::*;
// Import for using common lib:
// use common_lib::example_common_function;
use std::sync::LazyLock;
use tokio::runtime;
use tokio::sync::Mutex;

/// This is one of any number of data types that our application
/// uses. Golem will take care to persist all application state,
/// whether that state is local to a function being executed or
/// global across the entire program.
struct State {
    total: u64,
}

impl Default for State {
    fn default() -> Self {
        Self { total: 0 }
    }
}

/// This holds the state of our application.
static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

struct Component;

impl Guest for Component {
    /// Updates the component's state by adding the given value to the total.
    fn add(value: u64) {
        runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async_add(value));
    }

    /// Returns the current total.
    fn get() -> u64 {
        runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async_get())
    }
}

async fn async_add(value: u64) {
    let mut state = STATE.lock().await;
    state.total += value;
}

async fn async_get() -> u64 {
    let state = STATE.lock().await;

    // Call code from shared lib
    // println!("{}", example_common_function());

    state.total
}

bindings::export!(Component with_types_in bindings);
