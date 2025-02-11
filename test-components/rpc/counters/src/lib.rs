mod bindings;

use crate::bindings::exports::rpc::counters_exports::api::{
    Guest, GuestCounter, PromiseId, TimelineNode,
};
use std::cell::RefCell;
use std::env::{args, vars};

pub struct Component;

struct State {
    dropped_counters: Vec<(String, u64)>,
    global: u64,
}

static mut STATE: State = State {
    dropped_counters: vec![],
    global: 0,
};

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let result = unsafe { f(&mut STATE) };

    return result;
}

impl Guest for Component {
    type Counter = crate::Counter;

    fn bug_golem1265(s: String) -> Result<(), String> {
        eprintln!("Got {s}");
        Ok(())
    }

    fn bug_wasm_rpc_i32(in_: TimelineNode) -> TimelineNode {
        in_
    }

    fn get_all_dropped() -> Vec<(String, u64)> {
        with_state(|state| state.dropped_counters.clone())
    }

    fn get_global_value() -> u64 {
        with_state(|state| state.global)
    }

    fn inc_global_by(value: u64) {
        with_state(|state| {
            state.global += value;
        });
    }
}

pub struct Counter {
    name: String,
    value: RefCell<u64>,
}

impl GuestCounter for Counter {
    fn new(name: String) -> Self {
        println!("Creating counter {}", name);
        Self {
            name,
            value: RefCell::new(0),
        }
    }

    fn inc_by(&self, value: u64) {
        println!("Incrementing counter {} by {}", self.name, value);
        *self.value.borrow_mut() += value;
    }

    fn get_value(&self) -> u64 {
        println!("Getting value of counter {}", self.name);
        *self.value.borrow()
    }

    fn create_promise(&self) -> PromiseId {
        bindings::golem::api::host::create_promise()
    }

    fn block_on_promise(&self, promise: PromiseId) {
        println!("Awaiting promise ${promise:?}");
        bindings::golem::api::host::await_promise(&promise);
        println!("Promise ${promise:?} completed");
    }

    fn get_args(&self) -> Vec<String> {
        args().collect::<Vec<_>>()
    }

    fn get_env(&self) -> Vec<(String, String)> {
        vars().collect::<Vec<_>>()
    }
}

impl Drop for Counter {
    fn drop(&mut self) {
        println!("Dropping counter {}", self.name);
        with_state(|state| {
            state
                .dropped_counters
                .push((self.name.clone(), *self.value.borrow()));
        });
    }
}

bindings::export!(Component with_types_in bindings);
