cargo_component_bindings::generate!();

use crate::bindings::Guest;

struct Component;

impl Guest for Component {
    fn run() {
        eprintln!("Sample text written to the error output");
    }
}
