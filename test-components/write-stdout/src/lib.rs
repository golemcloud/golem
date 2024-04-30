mod bindings;

use crate::bindings::Guest;

struct Component;

impl Guest for Component {
    fn run() {
        println!("Sample text written to the output");
    }
}
