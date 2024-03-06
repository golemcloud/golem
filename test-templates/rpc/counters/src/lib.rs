mod bindings;

use std::cell::RefCell;
use bindings::*;
use crate::bindings::exports::rpc::counters::api::{Guest, GuestCounter};

pub struct Component;

struct State {
    dropped_counters: Vec<(String, u64)>,
    global: u64
}

static mut STATE: State = State {
    dropped_counters: vec![],
    global: 0
};

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let result = unsafe { f(&mut STATE) };

    return result;
}

impl Guest for Component {
    fn get_all_dropped() -> Vec<(String, u64)> {
        with_state(|state| {
            state.dropped_counters.clone()
        })
    }

    fn inc_global_by(value: u64) {
        with_state(|state| {
            state.global += value;
        });
    }

    fn get_global_value() -> u64 {
        with_state(|state| {
            state.global
        })
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
}

impl Drop for Counter {
    fn drop(&mut self) {
        println!("Dropping counter {}", self.name);
        with_state(|state| {
            state.dropped_counters.push((self.name.clone(), *self.value.borrow()));
        });
    }
}
