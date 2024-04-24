mod bindings;

use std::cell::RefCell;
use std::thread::sleep;
use std::time::Duration;
use crate::bindings::exports::golem::component::api::*;

struct State {
    last: u64,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State { last: 0 });
}

struct Component;

impl Guest for Component {
    fn f1(speed_ms: u64) -> u64 {
        STATE.with_borrow_mut(|state| {
            let mut current = state.last;

            println!("Starting to count..."); // newly added log line

            for _ in 0..30 {
                current += 10;
                println!("Current count: {}", current); // newly added log line
                sleep(Duration::from_millis(speed_ms));
            }

            println!("Finished to count..."); // newly added log line

            state.last = current / 2; // Changed expression
            state.last
        })
    }

    fn f2() -> u64 {
        STATE.with_borrow(|state| {
            state.last // Not using random anymore
        })
    }

    fn f3() -> u64 {
        std::env::args().collect::<Vec<_>>().len() as u64 +
            std::env::vars().collect::<Vec<_>>().len() as u64
    }

    // New function added
    fn f4() -> u64 {
        11
    }
}
