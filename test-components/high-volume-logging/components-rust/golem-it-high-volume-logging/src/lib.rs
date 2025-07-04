#[allow(static_mut_refs)]
mod bindings;

use crate::bindings::exports::golem_it::high_volume_logging_exports::golem_it_high_volume_logging_api::*;

struct Component;

impl Guest for Component {
    fn run() {
        for n in 1..=100 {
            println!("Iteration {n}");
        }
    }
}

bindings::export!(Component with_types_in bindings);
