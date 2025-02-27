mod bindings;

use crate::bindings::exports::pack::name::api::*;
use lib::core;
use std::cell::RefCell;

struct AppState(usize);

thread_local! {
    static APP_STATE: RefCell<AppState> = RefCell::new(AppState(0));
}

struct Component;

impl Guest for Component {
    fn hello() -> String {
        APP_STATE.with_borrow_mut(|state| {
            let (n, message) = core::hello(state.0);

            state.0 = n;

            message
        })
    }
}

bindings::export!(Component with_types_in bindings);
