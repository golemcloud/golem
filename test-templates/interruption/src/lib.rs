cargo_component_bindings::generate!();

use crate::bindings::Guest;

use std::thread::sleep;
use std::time::Duration;

struct Component;

impl Guest for Component {
    fn run() -> String {
        println!("Starting interruption test");
        for _ in 0..100 {
            sleep(Duration::from_millis(100));
        }

        "done".to_string()
    }
}
