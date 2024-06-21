mod bindings;

use crate::bindings::Guest;

struct Component;

impl Guest for Component {
    fn run() {
        eprintln!("Sample text written to the error output");
    }
}

bindings::export!(Component with_types_in bindings);
