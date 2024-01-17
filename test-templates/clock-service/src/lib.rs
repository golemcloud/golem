cargo_component_bindings::generate!();

use crate::bindings::exports::golem::it::api::Guest;

use std::time::Duration;
use std::thread;

struct Component;

impl Guest for Component {

    fn sleep(secs: u64) -> Result<(), String> {
        thread::sleep(Duration::from_secs(secs));
        Ok(())
    }
}
