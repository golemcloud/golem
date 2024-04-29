mod bindings;

use crate::bindings::exports::golem::component::api::*;
use golem_rust::*;

struct Component;

impl Guest for Component {
    fn add(value: u64) {
    }

    fn get() -> u64 {
        0
    }
}
