mod bindings;

use std::cell::RefCell;
use std::thread::sleep;
use std::time::Duration;
use rand::RngCore;
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
                sleep(Duration::from_millis(speed_ms));
            }

            state.last = current; // In v2, we replace this to current/2
            current
        })
    }

    fn f2() -> u64 {
        let mut rng = rand::thread_rng();
        rng.next_u64() // In v2, we replace this with returning the current state
    }

    fn f3() -> u64 {
        std::env::args().collect::<Vec<_>>().len() as u64 +
            std::env::vars().collect::<Vec<_>>().len() as u64
    }
}

bindings::export!(Component with_types_in bindings);
