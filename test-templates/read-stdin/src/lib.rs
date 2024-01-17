cargo_component_bindings::generate!();

use crate::bindings::Guest;

use std::io;

struct Component;

impl Guest for Component {
    fn run() -> Result<String, String> {
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .map_err(|e| format!("{:?}", e))
            .map(|_| line)
    }
}
