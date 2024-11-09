mod bindings;

use crate::bindings::Guest;

use std::fs::write;

struct Component;

impl Guest for Component {
    fn run() -> bool {
        write("/files/bar.txt", "hello world").unwrap();
        true
    }
}

bindings::export!(Component with_types_in bindings);
